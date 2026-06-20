//! Database path resolution and initialization.
//!
//! The on-disk index location is resolved through a precedence chain so the
//! same persistent index is shared by the CLI and the MCP server without
//! polluting the working tree:
//!
//! 1. `SYMGRAPH_DB` — explicit path override (wins over everything).
//! 2. `SYMGRAPH_STORAGE` = `git` | `cache` | `local` — pick a strategy.
//! 3. Auto (default): reuse an existing `.symgraph/` if present (back-compat),
//!    else store under the git dir when in a repo, else an OS cache dir.
//!
//! In-memory mode (ephemeral, no filesystem writes) is selected separately by
//! the caller via `--in-memory` / `SYMGRAPH_IN_MEMORY`.

use anyhow::{Context, Result};
use std::{
    path::{Path, PathBuf},
    process::{self, Command},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::db::Database;
use crate::{build_full_index, IndexConfig, IndexingStats};

const DB_DIR: &str = ".symgraph";
const DB_FILE: &str = "index.db";
const SHADOW_DB_PREFIX: &str = "index.shadow";

/// On-disk storage strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Storage {
    /// Under the repository's git dir: `<git-common-dir>/symgraph/index.db`.
    Git,
    /// In an OS cache dir, keyed by a hash of the canonical project root.
    Cache,
    /// In-tree: `<root>/.symgraph/index.db`.
    Local,
}

impl Storage {
    fn from_env() -> Option<Storage> {
        match std::env::var("SYMGRAPH_STORAGE")
            .ok()?
            .to_ascii_lowercase()
            .as_str()
        {
            "git" => Some(Storage::Git),
            "cache" => Some(Storage::Cache),
            "local" => Some(Storage::Local),
            _ => None,
        }
    }
}

/// A resolved database location, with a human-readable label of how it was
/// chosen (for `symgraph where` / `status`).
pub struct ResolvedDb {
    pub path: PathBuf,
    pub label: &'static str,
}

/// Resolve the on-disk index path for a project root, honoring overrides.
pub fn resolve_db(project_root: &str) -> Result<ResolvedDb> {
    // 1. Explicit override.
    if let Ok(p) = std::env::var("SYMGRAPH_DB") {
        if !p.is_empty() {
            return Ok(ResolvedDb {
                path: PathBuf::from(p),
                label: "explicit (SYMGRAPH_DB)",
            });
        }
    }

    // 2. Strategy from env, else auto-detect.
    let strategy = Storage::from_env().unwrap_or_else(|| auto_strategy(project_root));

    match strategy {
        Storage::Local => Ok(ResolvedDb {
            path: local_db_path(project_root),
            label: "local (.symgraph)",
        }),
        Storage::Cache => Ok(ResolvedDb {
            path: cache_db_path(project_root)?,
            label: "cache",
        }),
        Storage::Git => match git_db_path(project_root) {
            Some(path) => Ok(ResolvedDb {
                path,
                label: "git-dir",
            }),
            // Not a git repo — fall back to the cache rather than failing.
            None => Ok(ResolvedDb {
                path: cache_db_path(project_root)?,
                label: "cache (git fallback)",
            }),
        },
    }
}

/// Auto strategy: prefer an existing in-tree index (don't orphan it), then the
/// git dir, then the OS cache.
fn auto_strategy(project_root: &str) -> Storage {
    if local_db_path(project_root).exists() {
        return Storage::Local;
    }
    if git_common_dir(project_root).is_some() {
        return Storage::Git;
    }
    Storage::Cache
}

fn local_db_path(project_root: &str) -> PathBuf {
    PathBuf::from(project_root).join(DB_DIR).join(DB_FILE)
}

/// Resolve the repository's common git dir (shared by all linked worktrees),
/// handling the case where `.git` is a file (worktrees/submodules). Returns
/// `None` when `project_root` is not inside a git repository.
fn git_common_dir(project_root: &str) -> Option<PathBuf> {
    let out = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(project_root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }
    let p = PathBuf::from(&raw);
    let abs = if p.is_absolute() {
        p
    } else {
        PathBuf::from(project_root).join(p)
    };
    Some(abs)
}

fn git_db_path(project_root: &str) -> Option<PathBuf> {
    Some(git_common_dir(project_root)?.join("symgraph").join(DB_FILE))
}

fn cache_db_path(project_root: &str) -> Result<PathBuf> {
    let base = cache_base()
        .context("could not determine an OS cache directory (set SYMGRAPH_DB or XDG_CACHE_HOME)")?;
    Ok(base
        .join("symgraph")
        .join(repo_key(project_root))
        .join(DB_FILE))
}

/// Stable per-repo cache key: first 16 hex chars of sha256(canonical root).
fn repo_key(project_root: &str) -> String {
    use sha2::{Digest, Sha256};
    let canonical = std::fs::canonicalize(project_root)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| project_root.to_string());
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    hex::encode(&hasher.finalize()[..8])
}

/// Cross-platform cache base dir, honoring `XDG_CACHE_HOME` when set.
fn cache_base() -> Option<PathBuf> {
    if let Ok(x) = std::env::var("XDG_CACHE_HOME") {
        if !x.is_empty() {
            return Some(PathBuf::from(x));
        }
    }
    // Exactly one of these cfg'd bindings compiles per target.
    #[cfg(target_os = "macos")]
    let base = std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join("Library").join("Caches"));
    #[cfg(target_os = "windows")]
    let base = std::env::var("LOCALAPPDATA").ok().map(PathBuf::from);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let base = std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".cache"));
    base
}

/// Ensure the directory for the resolved index exists, and drop bookkeeping
/// files (a `source` marker for cache pruning; a self-`.gitignore` for local
/// storage so it needs no working-tree `.gitignore` entry).
fn prepare_location(project_root: &str, resolved: &ResolvedDb) -> Result<()> {
    if let Some(parent) = resolved.path.parent() {
        std::fs::create_dir_all(parent)?;
        if resolved.label.starts_with("cache") {
            let canonical = std::fs::canonicalize(project_root)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| project_root.to_string());
            let _ = std::fs::write(parent.join("source"), canonical);
        }
        if resolved.label.starts_with("local") {
            let gi = parent.join(".gitignore");
            if !gi.exists() {
                let _ = std::fs::write(gi, "*\n");
            }
        }
    }
    Ok(())
}

/// Path to the index progress log, co-located with the resolved index DB so it
/// lives wherever the index lives (git-dir / OS cache / `.symgraph`) instead of
/// the working tree. Ensures the parent directory exists. Callers that index in
/// the background point the process log here so it never pollutes the repo.
pub fn index_log_path(project_root: &str) -> Result<PathBuf> {
    let root = canonicalize_path(project_root)?;
    let resolved = resolve_db(&root)?;
    let dir = resolved
        .path
        .parent()
        .context("resolved database path has no parent directory")?;
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating index directory {}", dir.display()))?;
    Ok(dir.join("index.log"))
}

/// Open or initialize the database for a project (creating the location).
pub fn open_project_database(project_root: &str) -> Result<Database> {
    let resolved = resolve_db(project_root)?;
    prepare_location(project_root, &resolved)?;
    Database::open(&resolved.path)
}

/// Open a fresh shadow database in the project index directory.
pub fn open_shadow_database(project_root: &str) -> Result<Database> {
    let shadow_path = shadow_database_path(project_root)?;
    Database::cleanup_on_disk_path(&shadow_path)?;
    Database::open(&shadow_path)
}

/// Best-effort cleanup for a shadow database path and its SQLite sidecars.
pub fn cleanup_shadow_database_path(path: &Path) -> Result<()> {
    Database::cleanup_on_disk_path(path)
}

/// Best-effort cleanup for a shadow database handle and its SQLite sidecars.
pub fn cleanup_shadow_database(db: &Database) -> Result<()> {
    match db.path() {
        Some(path) => cleanup_shadow_database_path(path),
        None => Ok(()),
    }
}

/// Flush, close, and atomically swap a prepared shadow database into place.
pub fn swap_shadow_database(live_db: &mut Database, shadow_db: Database) -> Result<()> {
    let shadow_path = shadow_db.prepare_for_swap()?;
    live_db.replace_with_shadow(&shadow_path)
}

/// Build a full shadow index and atomically swap it into the live database handle.
pub fn rebuild_project_database(
    live_db: &mut Database,
    config: &IndexConfig,
) -> Result<IndexingStats> {
    let mut shadow_db = open_shadow_database(&config.root)?;
    let shadow_path = shadow_db
        .path()
        .map(Path::to_path_buf)
        .context("shadow database is not file-backed")?;

    match build_full_index(&mut shadow_db, config) {
        Ok(stats) => {
            if let Err(err) = swap_shadow_database(live_db, shadow_db) {
                let _ = cleanup_shadow_database_path(&shadow_path);
                Err(err)
            } else {
                Ok(stats)
            }
        }
        Err(err) => {
            let _ = cleanup_shadow_database_path(&shadow_path);
            Err(err)
        }
    }
}

fn shadow_database_path(project_root: &str) -> Result<PathBuf> {
    // Co-locate the shadow with the resolved live DB so the atomic rename in
    // `replace_with_shadow` stays within one directory (and filesystem). The
    // live location follows storage resolution (git-dir / cache / .symgraph),
    // so we can't assume `.symgraph/` here.
    let live = resolve_db(project_root)?.path;
    let dir = live
        .parent()
        .context("resolved database path has no parent directory")?;
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating index directory {}", dir.display()))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("System clock is before Unix epoch")?
        .as_nanos();
    Ok(dir.join(format!("{SHADOW_DB_PREFIX}.{}.{}.db", process::id(), nonce)))
}

/// Canonicalize and validate a path
pub fn canonicalize_path(path: &str) -> Result<String> {
    let canonical = std::path::Path::new(path)
        .canonicalize()
        .context("Invalid path")?;
    Ok(canonical.display().to_string())
}

/// Summary of a cache-prune pass.
#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct PruneStats {
    pub removed: usize,
    pub bytes_freed: u64,
}

/// Remove OS-cache index entries that are no longer useful and report how much
/// space was reclaimed. An entry is pruned when any of these hold:
///   * its `source` marker is missing, or points at a path that no longer
///     exists;
///   * the source repository would not, by default, store its index in the
///     cache — e.g. it is a git repo (index belongs under the git dir) or has
///     an in-tree `.symgraph/`. An explicit `SYMGRAPH_STORAGE=cache` keeps such
///     entries (the cache is then the active location);
///   * `max_age_days` is set and the index has not been modified within that
///     window.
///
/// Only the OS cache is touched. In-tree (`.symgraph/`) and git-dir indexes are
/// left alone — they are removed together with their repositories.
pub fn prune_cache(max_age_days: Option<u64>) -> Result<PruneStats> {
    let Some(base) = cache_base() else {
        return Ok(PruneStats::default());
    };
    let root = base.join("symgraph");
    if !root.exists() {
        return Ok(PruneStats::default());
    }

    let max_age = max_age_days.map(|d| std::time::Duration::from_secs(d * 24 * 60 * 60));
    let now = SystemTime::now();

    let mut stats = PruneStats::default();
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        if cache_entry_is_stale(&dir, max_age, now) {
            let freed = dir_size(&dir).unwrap_or(0);
            std::fs::remove_dir_all(&dir)?;
            stats.removed += 1;
            stats.bytes_freed += freed;
        }
    }
    Ok(stats)
}

/// Decide whether a single cache entry directory is no longer useful.
fn cache_entry_is_stale(dir: &Path, max_age: Option<std::time::Duration>, now: SystemTime) -> bool {
    let source = match std::fs::read_to_string(dir.join("source")) {
        Ok(s) => s.trim().to_string(),
        // No marker (partial/legacy dir) — can't attribute it; treat as stale.
        Err(_) => return true,
    };

    // Source repository is gone.
    if !Path::new(&source).exists() {
        return true;
    }

    // The repo now resolves its index elsewhere (git dir / in-tree), so this
    // cache copy is dead weight. Honor an explicit `SYMGRAPH_STORAGE=cache`,
    // which keeps the cache as the active location; `SYMGRAPH_DB` is ignored on
    // purpose (it would otherwise make every entry look redundant).
    let strategy = Storage::from_env().unwrap_or_else(|| auto_strategy(&source));
    if strategy != Storage::Cache {
        return true;
    }

    // Age-based eviction for entries that do legitimately belong in the cache.
    if let Some(max_age) = max_age {
        if let Ok(modified) = std::fs::metadata(dir.join(DB_FILE)).and_then(|m| m.modified()) {
            if now
                .duration_since(modified)
                .map(|age| age > max_age)
                .unwrap_or(false)
            {
                return true;
            }
        }
    }

    false
}

/// Recursively sum the byte sizes of regular files under `dir`.
fn dir_size(dir: &Path) -> Result<u64> {
    let mut total = 0;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_dir() {
            total += dir_size(&entry.path()).unwrap_or(0);
        } else {
            total += meta.len();
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // resolve_db reads process-wide env vars; serialize the env-mutating tests.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn repo_key_is_stable_and_short() {
        let a = repo_key("/tmp/does-not-exist-xyz");
        let b = repo_key("/tmp/does-not-exist-xyz");
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn explicit_override_wins() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        temp_env("SYMGRAPH_STORAGE", None, || {
            temp_env("SYMGRAPH_DB", Some("/tmp/custom.db"), || {
                let r = resolve_db("/tmp").unwrap();
                assert_eq!(r.path, PathBuf::from("/tmp/custom.db"));
                assert!(r.label.starts_with("explicit"));
            });
        });
    }

    #[test]
    fn local_strategy_path() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        temp_env("SYMGRAPH_DB", None, || {
            temp_env("SYMGRAPH_STORAGE", Some("local"), || {
                let r = resolve_db("/tmp/proj").unwrap();
                assert!(r.path.ends_with(".symgraph/index.db"));
                assert!(r.label.starts_with("local"));
            });
        });
    }

    #[test]
    fn prune_removes_missing_source_keeps_live_cache() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let cache = tempfile::tempdir().unwrap();
        // A real, non-git directory that resolves to cache storage by default.
        let live_repo = tempfile::tempdir().unwrap();

        let sg = cache.path().join("symgraph");

        // Entry 1: source points at a path that no longer exists -> pruned.
        let gone = sg.join("gone");
        std::fs::create_dir_all(&gone).unwrap();
        std::fs::write(gone.join(DB_FILE), vec![0u8; 4096]).unwrap();
        std::fs::write(gone.join("source"), "/no/such/path/xyz").unwrap();

        // Entry 2: source exists and is not a git repo -> kept (active cache).
        let live = sg.join("live");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::write(live.join(DB_FILE), vec![0u8; 4096]).unwrap();
        std::fs::write(
            live.join("source"),
            live_repo.path().canonicalize().unwrap().to_str().unwrap(),
        )
        .unwrap();

        temp_env("SYMGRAPH_DB", None, || {
            temp_env("SYMGRAPH_STORAGE", None, || {
                temp_env(
                    "XDG_CACHE_HOME",
                    Some(cache.path().to_str().unwrap()),
                    || {
                        let stats = prune_cache(None).unwrap();
                        assert_eq!(stats.removed, 1);
                        assert!(stats.bytes_freed >= 4096);
                    },
                );
            });
        });

        assert!(!gone.exists());
        assert!(live.exists());
    }

    /// Minimal scoped env setter/restorer for tests.
    fn temp_env<F: FnOnce()>(key: &str, val: Option<&str>, f: F) {
        let prev = std::env::var(key).ok();
        match val {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        f();
        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
}
