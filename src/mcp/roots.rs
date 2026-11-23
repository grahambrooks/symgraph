//! Per-call project roots for the MCP server.
//!
//! A server is started in one directory, but a client may be working in
//! another: a session opened over a workspace of many repositories
//! (`~/dev/projects`) has no single index to answer from. Every tool takes an
//! optional `root`, and the server keeps one index per repository root,
//! opened the first time a call names it.
//!
//! The first call against a root builds its index when it is empty or from
//! an older extraction version. Before this, a query against a missing or
//! stale index answered "No symbols found" or a partial result, with nothing
//! to say why.
//!
//! Every call then refreshes the index first: files deleted from disk are
//! dropped, changed and new ones re-indexed (a file whose size and mtime
//! match is not read), and references are re-resolved only when something
//! changed. A no-change refresh is a walk and a stat per file. Calls within
//! [`REFRESH_INTERVAL`] of the last refresh share it, since a client often
//! issues several tools at once. `SYMGRAPH_AUTO_INDEX=0` turns both off.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::cli::{canonicalize_path, open_project_database, rebuild_project_database};
use crate::db::Database;
use crate::{IndexConfig, build_full_index, index_codebase};

use super::SharedDatabase;

/// Calls this soon after a refresh don't refresh again.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(1);

/// One repository's index.
pub struct RootIndex {
    /// The canonical project root.
    pub root: String,
    pub db: SharedDatabase,
    /// Guards background rebuilds of this root's index (`symgraph-reindex`).
    pub is_reindexing: Arc<AtomicBool>,
    /// What the first-use index pass did, once it has run. Held while it
    /// runs, so concurrent first calls wait for one pass rather than each
    /// starting their own.
    prepared: Mutex<Option<String>>,
    /// When the index was last brought up to date. Held while a refresh
    /// runs, so concurrent calls share it.
    refreshed: Mutex<Option<Instant>>,
}

impl RootIndex {
    fn new(root: String, db: SharedDatabase, is_reindexing: Arc<AtomicBool>) -> Self {
        Self {
            root,
            db,
            is_reindexing,
            prepared: Mutex::new(None),
            refreshed: Mutex::new(None),
        }
    }

    /// What the first-use pass did, if it has run.
    pub fn prepared(&self) -> Option<String> {
        self.prepared.lock().ok().and_then(|p| p.clone())
    }
}

/// The roots a server answers for.
pub struct Roots {
    default_root: String,
    in_memory: bool,
    auto_index: bool,
    /// Refuse roots outside the default root (the HTTP transport, which may
    /// face other users; a stdio client can read the disk anyway).
    confine: bool,
    entries: Mutex<HashMap<String, Arc<RootIndex>>>,
}

impl Roots {
    /// The default root and its already-open database.
    pub fn new(default_root: String, db: SharedDatabase, is_reindexing: Arc<AtomicBool>) -> Self {
        let in_memory = db.lock().map(|d| d.path().is_none()).unwrap_or(false);
        let mut entries = HashMap::new();
        entries.insert(
            default_root.clone(),
            Arc::new(RootIndex::new(default_root.clone(), db, is_reindexing)),
        );
        Self {
            default_root,
            in_memory,
            auto_index: std::env::var("SYMGRAPH_AUTO_INDEX").map_or(true, |v| v != "0"),
            confine: false,
            entries: Mutex::new(entries),
        }
    }

    /// Only roots inside the default root are served.
    pub fn confined(mut self) -> Self {
        self.confine = true;
        self
    }

    #[cfg(test)]
    fn with_auto_index(mut self, on: bool) -> Self {
        self.auto_index = on;
        self
    }

    pub fn default_root(&self) -> &str {
        &self.default_root
    }

    /// The index for `requested` (or the default root), brought up to date
    /// on first use. Blocking: call it on the blocking pool.
    pub fn open(&self, requested: Option<&str>) -> Result<Arc<RootIndex>, String> {
        let entry = self.entry(requested)?;
        if self.auto_index {
            prepare(&entry);
            refresh(&entry, REFRESH_INTERVAL);
        }
        Ok(entry)
    }

    /// The index for `requested`, opened if need be, without indexing it.
    pub fn entry(&self, requested: Option<&str>) -> Result<Arc<RootIndex>, String> {
        let root = self.resolve(requested)?;
        let mut entries = self
            .entries
            .lock()
            .map_err(|e| format!("roots lock poisoned: {e}"))?;
        if let Some(e) = entries.get(&root) {
            return Ok(Arc::clone(e));
        }
        let db = if self.in_memory {
            Database::in_memory()
        } else {
            open_project_database(&root)
        }
        .map_err(|e| format!("could not open the index for {root}: {e}"))?;
        let entry = Arc::new(RootIndex::new(
            root.clone(),
            Arc::new(Mutex::new(db)),
            Arc::new(AtomicBool::new(false)),
        ));
        entries.insert(root, Arc::clone(&entry));
        Ok(entry)
    }

    /// The project root a call means.
    ///
    /// - No `root`: the default root, unless that is a workspace of
    ///   repositories (not itself in one), which has no index worth
    ///   answering from: the error names the repositories to pass instead.
    /// - A `root`: absolute, or relative to the default root; a file or a
    ///   directory inside a repository means that repository's git root.
    pub fn resolve(&self, requested: Option<&str>) -> Result<String, String> {
        let Some(requested) = requested.map(str::trim).filter(|r| !r.is_empty()) else {
            let default = Path::new(&self.default_root);
            let repos = child_repositories(default);
            if git_root(default).is_none() && repos.len() >= 2 {
                return Err(workspace_message(&self.default_root, &repos));
            }
            return Ok(self.default_root.clone());
        };
        let path = Path::new(requested);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            Path::new(&self.default_root).join(path)
        };
        let canonical = canonicalize_path(&path.to_string_lossy())
            .map_err(|_| format!("root not found: {requested}"))?;
        let canonical = PathBuf::from(canonical);
        let dir = if canonical.is_file() {
            canonical
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or(canonical)
        } else {
            canonical
        };
        let root = git_root(&dir).unwrap_or(dir);
        if self.confine && !root.starts_with(&self.default_root) {
            return Err(format!(
                "root {} is outside this server's root {}",
                root.display(),
                self.default_root
            ));
        }
        Ok(root.to_string_lossy().into_owned())
    }
}

/// Bring a root's index up to date, once per server: a full build when it
/// is empty or from an older extraction version, else an incremental pass.
/// A failure is logged and remembered, never fatal: the existing index still
/// answers.
fn prepare(entry: &RootIndex) {
    let Ok(mut prepared) = entry.prepared.lock() else {
        return;
    };
    if prepared.is_some() {
        return;
    }
    let started = Instant::now();
    let outcome = (|| -> anyhow::Result<String> {
        let mut db = entry
            .db
            .lock()
            .map_err(|e| anyhow::anyhow!("database lock poisoned: {e}"))?;
        let config = IndexConfig {
            root: entry.root.clone(),
            show_progress: false,
            ..Default::default()
        };
        // An empty index is built in place: the lock is held, so no reader
        // sees it half-built. A stale one is rebuilt in a shadow and swapped.
        let (mode, files) = if db.get_stats()?.total_files == 0 {
            ("built", build_full_index(&mut db, &config)?.files)
        } else if db.staleness()?.is_some() {
            ("rebuilt", rebuild_project_database(&mut db, &config)?.files)
        } else {
            let changed = refresh_index(&mut db, &entry.root)?;
            ("updated", changed.indexed + changed.removed)
        };
        if let Ok(mut at) = entry.refreshed.lock() {
            *at = Some(Instant::now());
        }
        let total = db.get_stats()?.total_files;
        Ok(format!(
            "{mode} on first use: {files} file(s) indexed, {total} in the index ({:.1}s)",
            started.elapsed().as_secs_f64()
        ))
    })();
    let note = match outcome {
        Ok(note) => {
            tracing::info!("index for {}: {note}", entry.root);
            note
        }
        Err(e) => {
            tracing::warn!("first-use index of {} failed: {e}", entry.root);
            format!("first-use index failed: {e}")
        }
    };
    *prepared = Some(note);
}

/// Bring the index up to date with the files on disk before a call, unless
/// it was within `interval`, or a rebuild of this root is running (the call
/// then waits on the database lock and sees the rebuilt index). Failures are
/// logged, never fatal: the index as it is still answers.
fn refresh(entry: &RootIndex, interval: Duration) {
    use std::sync::atomic::Ordering;
    if entry.is_reindexing.load(Ordering::SeqCst) {
        return;
    }
    let Ok(mut refreshed) = entry.refreshed.lock() else {
        return;
    };
    if refreshed.is_some_and(|at| at.elapsed() < interval) {
        return;
    }
    let outcome = entry
        .db
        .lock()
        .map_err(|e| anyhow::anyhow!("database lock poisoned: {e}"))
        .and_then(|mut db| {
            // An index from an older extraction version can't be updated
            // incrementally; symgraph-reindex (or a restart) rebuilds it.
            if db.staleness()?.is_some() {
                return Ok(Changed::default());
            }
            refresh_index(&mut db, &entry.root)
        });
    match outcome {
        Ok(c) if c.indexed + c.removed > 0 => tracing::info!(
            "refreshed {}: {} re-indexed, {} removed",
            entry.root,
            c.indexed,
            c.removed
        ),
        Ok(_) => {}
        Err(e) => tracing::warn!("refresh of {} failed: {e}", entry.root),
    }
    *refreshed = Some(Instant::now());
}

/// What a refresh changed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Changed {
    indexed: u64,
    removed: u64,
}

/// Drop files deleted from disk, re-index changed and new ones, and resolve
/// references again only if either happened. The incremental index alone
/// keeps deleted files and resolves every reference even when nothing
/// changed.
fn refresh_index(db: &mut Database, root: &str) -> anyhow::Result<Changed> {
    let mut changed = Changed::default();
    for path in db.indexed_file_paths()? {
        if !Path::new(root).join(&path).exists() {
            db.delete_file(&path)?;
            changed.removed += 1;
        }
    }
    let config = IndexConfig {
        root: root.to_string(),
        show_progress: false,
        skip_resolve: true,
        ..Default::default()
    };
    changed.indexed = index_codebase(db, &config)?.files;
    if changed.indexed + changed.removed > 0 {
        db.resolve_references()?;
    }
    Ok(changed)
}

/// The nearest directory at or above `dir` holding `.git` (a directory, or
/// the file a worktree or submodule has).
pub fn git_root(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|d| d.join(".git").exists())
        .map(Path::to_path_buf)
}

/// The immediate subdirectories of `dir` that are repositories, by name.
fn child_repositories(dir: &Path) -> Vec<String> {
    let mut repos: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join(".git").exists())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    repos.sort();
    repos
}

fn workspace_message(root: &str, repos: &[String]) -> String {
    const SHOWN: usize = 12;
    let mut names = repos
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if repos.len() > SHOWN {
        names.push_str(&format!(", … ({} more)", repos.len() - SHOWN));
    }
    format!(
        "{root} is a workspace of {} repositories, not one project, so there is no index \
         to answer from. Pass `root` naming the repository, relative to it (e.g. \"{}\") or \
         as an absolute path. Repositories: {names}.",
        repos.len(),
        repos
            .iter()
            .find(|r| !r.starts_with('.'))
            .or(repos.first())
            .map(String::as_str)
            .unwrap_or("my-repo"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn repo(dir: &Path, name: &str, source: &str) -> PathBuf {
        let r = dir.join(name);
        fs::create_dir_all(r.join(".git")).unwrap();
        fs::create_dir_all(r.join("src")).unwrap();
        fs::write(r.join("src/lib.rs"), source).unwrap();
        r
    }

    fn roots_at(dir: &Path) -> Roots {
        let root = canonicalize_path(&dir.to_string_lossy()).unwrap();
        Roots::new(
            root,
            Arc::new(Mutex::new(Database::in_memory().unwrap())),
            Arc::new(AtomicBool::new(false)),
        )
    }

    #[test]
    fn a_workspace_root_asks_for_a_repository() {
        let ws = tempdir().unwrap();
        repo(ws.path(), "alpha", "pub fn a() {}\n");
        repo(ws.path(), "beta", "pub fn b() {}\n");
        let roots = roots_at(ws.path());
        let err = roots.resolve(None).unwrap_err();
        assert!(err.contains("workspace of 2 repositories"), "{err}");
        assert!(err.contains("alpha, beta"), "{err}");
    }

    #[test]
    fn roots_resolve_relative_absolute_and_inside_a_repository() {
        let ws = tempdir().unwrap();
        let alpha = repo(ws.path(), "alpha", "pub fn a() {}\n");
        repo(ws.path(), "beta", "pub fn b() {}\n");
        let roots = roots_at(ws.path());
        let alpha = canonicalize_path(&alpha.to_string_lossy()).unwrap();
        assert_eq!(roots.resolve(Some("alpha")).unwrap(), alpha);
        assert_eq!(roots.resolve(Some(&alpha)).unwrap(), alpha);
        assert_eq!(roots.resolve(Some("alpha/src/lib.rs")).unwrap(), alpha);
        assert_eq!(roots.resolve(Some("alpha/src")).unwrap(), alpha);
        assert!(
            roots
                .resolve(Some("gamma"))
                .unwrap_err()
                .contains("root not found")
        );
    }

    #[test]
    fn a_confined_server_refuses_roots_outside_it() {
        let ws = tempdir().unwrap();
        let other = tempdir().unwrap();
        repo(ws.path(), "alpha", "pub fn a() {}\n");
        let outside = repo(other.path(), "elsewhere", "pub fn e() {}\n");
        let roots = roots_at(ws.path()).confined();
        assert!(roots.resolve(Some("alpha")).is_ok());
        let err = roots.resolve(Some(&outside.to_string_lossy())).unwrap_err();
        assert!(err.contains("outside"), "{err}");
    }

    #[test]
    fn a_repository_default_root_is_used_as_is() {
        let ws = tempdir().unwrap();
        let alpha = repo(ws.path(), "alpha", "pub fn a() {}\n");
        let roots = roots_at(&alpha);
        assert_eq!(roots.resolve(None).unwrap(), roots.default_root());
    }

    #[test]
    fn the_first_call_indexes_the_root_once() {
        let ws = tempdir().unwrap();
        repo(ws.path(), "alpha", "pub fn first_symbol() {}\n");
        repo(ws.path(), "beta", "pub fn other() {}\n");
        let roots = roots_at(ws.path());

        let entry = roots.open(Some("alpha")).unwrap();
        let note = entry.prepared().expect("indexed on first use");
        assert!(note.starts_with("built on first use: 1 file(s)"), "{note}");
        assert!(
            entry
                .db
                .lock()
                .unwrap()
                .find_node_by_name("first_symbol")
                .unwrap()
                .is_some()
        );

        // Later calls reuse the entry and don't build it again.
        let again = roots.open(Some("alpha/src")).unwrap();
        assert!(Arc::ptr_eq(&entry, &again));
        assert_eq!(again.prepared(), Some(note));
    }

    fn has(entry: &RootIndex, name: &str) -> bool {
        entry
            .db
            .lock()
            .unwrap()
            .find_node_by_name(name)
            .unwrap()
            .is_some()
    }

    #[test]
    fn every_call_refreshes_changed_new_and_deleted_files() {
        let ws = tempdir().unwrap();
        let alpha = repo(ws.path(), "alpha", "pub fn before() {}\n");
        let roots = roots_at(ws.path());
        let entry = roots.open(Some("alpha")).unwrap();
        assert!(has(&entry, "before"));

        // A different size, so the stat pre-filter sees the change even
        // within the filesystem's mtime resolution.
        fs::write(alpha.join("src/lib.rs"), "pub fn after_the_edit() {}\n").unwrap();
        fs::write(alpha.join("src/new.rs"), "pub fn brand_new() {}\n").unwrap();
        refresh(&entry, Duration::ZERO);
        assert!(has(&entry, "after_the_edit") && has(&entry, "brand_new"));
        assert!(!has(&entry, "before"));

        fs::remove_file(alpha.join("src/new.rs")).unwrap();
        refresh(&entry, Duration::ZERO);
        assert!(!has(&entry, "brand_new"), "a deleted file's symbols go");
        assert_eq!(entry.db.lock().unwrap().get_stats().unwrap().total_files, 1);
    }

    #[test]
    fn a_refresh_resolves_references_into_new_files() {
        let ws = tempdir().unwrap();
        let alpha = repo(ws.path(), "alpha", "pub fn caller() { callee(); }\n");
        let roots = roots_at(ws.path());
        let entry = roots.open(Some("alpha")).unwrap();
        fs::write(alpha.join("src/b.rs"), "pub fn callee() {}\n").unwrap();
        refresh(&entry, Duration::ZERO);
        let db = entry.db.lock().unwrap();
        let callee = db.find_node_by_name("callee").unwrap().unwrap();
        let callers = db.get_callers(callee.id, 10, 0).unwrap();
        assert!(callers.iter().any(|n| n.name == "caller"), "{callers:?}");
    }

    #[test]
    fn calls_within_the_interval_share_a_refresh() {
        let ws = tempdir().unwrap();
        let alpha = repo(ws.path(), "alpha", "pub fn one() {}\n");
        let roots = roots_at(ws.path());
        let entry = roots.open(Some("alpha")).unwrap();
        fs::write(alpha.join("src/two.rs"), "pub fn two() {}\n").unwrap();
        refresh(&entry, Duration::from_secs(3600));
        assert!(!has(&entry, "two"), "refreshed moments ago: skipped");
        refresh(&entry, Duration::ZERO);
        assert!(has(&entry, "two"));
    }

    #[test]
    fn auto_index_off_leaves_the_index_alone() {
        let ws = tempdir().unwrap();
        repo(ws.path(), "alpha", "pub fn a() {}\n");
        let roots = roots_at(ws.path()).with_auto_index(false);
        let entry = roots.open(Some("alpha")).unwrap();
        assert!(entry.prepared().is_none());
        assert_eq!(entry.db.lock().unwrap().get_stats().unwrap().total_files, 0);
    }
}
