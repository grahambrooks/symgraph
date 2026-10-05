//! `symgraph export`: code-structure records for software-analytics.
//!
//! Writes delivery-schema records (software-analytics `delivery-schema.md`,
//! "Changeability") as JSON Lines on stdout, one repository at a time, for
//! every git mirror under a directory laid out `<owner>/<name>`, as
//! insight-collect keeps them. insight-collect runs it after each sync and
//! streams the records into insight (roadmap Phase 5a).
//!
//! Per repository and checked-out commit:
//! - `coupling_snapshot`: the directory graph's size and cycles, god-structs,
//!   dead code and parse coverage;
//! - `directory_coupling`: fan-in, fan-out, instability, cycle membership and
//!   churn for each directory;
//! - `coupling_edge`: the `top` directory pairs by strength × distance ×
//!   volatility;
//! - `god_struct`: the `top` structs by public fields × inbound references ×
//!   churn.
//!
//! **Cursor.** Each exported commit gets a sequence number (`source_seq`),
//! kept in `<state>/export.json`. A repository is exported again only when its
//! commit changed, or when its last export is newer than the `--since` cursor
//! (sent, but not confirmed by the caller). Record ids name the commit, so a
//! re-sent export is idempotent.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::coupling::{boundary_of, build_module_graph, score_coupling, Granularity};
use crate::db::Database;
use crate::mcp::handlers::churn::file_churn;
use crate::mcp::handlers::god_struct::rank_god_structs;
use crate::{index_codebase, IndexConfig};

use super::db_utils::{open_project_database, rebuild_project_database};

/// Delivery-schema version these records follow.
pub const SCHEMA_VERSION: &str = "1.0";

/// A god-struct counts towards `coupling_snapshot.god_structs` at this score.
/// With churn, it means e.g. 5 public fields, referenced from 10 files and
/// changed twice in the window.
pub const GOD_STRUCT_MIN_SCORE: u64 = 100;

#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// Directory of mirrors, `<owner>/<name>` each.
    pub mirrors: PathBuf,
    /// Directory for the export's own state (sequence numbers per commit).
    pub state: PathBuf,
    /// The caller's cursor: the highest `source_seq` it has stored.
    pub since: u64,
    /// How many coupling pairs and god-structs to send per repository.
    pub top: usize,
    /// Churn window, in days, for volatility.
    pub churn_days: u32,
}

/// What one export did, for the summary on stderr.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ExportSummary {
    pub repositories: usize,
    pub exported: usize,
    pub unchanged: usize,
    pub failed: usize,
    pub records: usize,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    next_seq: u64,
    repositories: BTreeMap<String, Exported>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Exported {
    commit: String,
    seq: u64,
    /// The commit exported before this one: where this export's
    /// `change_impact` range starts, so a re-send covers the same commits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous: Option<String>,
}

/// Export every mirror's records to `out`. A repository that fails is
/// reported on stderr and left for the next run; the others still export.
pub fn export(opts: &ExportOptions, out: &mut impl Write) -> Result<ExportSummary> {
    let state_path = opts.state.join("export.json");
    let mut state: State = match std::fs::read_to_string(&state_path) {
        Ok(text) => serde_json::from_str(&text)
            .with_context(|| format!("reading {}", state_path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", state_path.display())),
    };

    let mut summary = ExportSummary::default();
    for (repo, path) in discover(&opts.mirrors)? {
        summary.repositories += 1;
        let commit = match head_commit(&path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("symgraph export: {repo}: {e:#}");
                summary.failed += 1;
                continue;
            }
        };
        let (seq, previous) = match state.repositories.get(&repo) {
            Some(prev) if prev.commit == commit && prev.seq <= opts.since => {
                summary.unchanged += 1;
                continue;
            }
            // Sent before but not confirmed: the same records again.
            Some(prev) if prev.commit == commit => (prev.seq, prev.previous.clone()),
            prev => {
                state.next_seq += 1;
                (state.next_seq, prev.map(|p| p.commit.clone()))
            }
        };
        match repository_records(&repo, &path, &commit, previous.as_deref(), seq, opts) {
            Ok(records) => {
                for r in &records {
                    serde_json::to_writer(&mut *out, r)?;
                    out.write_all(b"\n")?;
                }
                summary.records += records.len();
                summary.exported += 1;
                state.repositories.insert(
                    repo,
                    Exported {
                        commit,
                        seq,
                        previous,
                    },
                );
            }
            Err(e) => {
                eprintln!("symgraph export: {repo}: {e:#}");
                summary.failed += 1;
            }
        }
    }
    out.flush()?;

    std::fs::create_dir_all(&opts.state)
        .with_context(|| format!("creating {}", opts.state.display()))?;
    let tmp = state_path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&state)?)?;
    std::fs::rename(&tmp, &state_path)?;
    Ok(summary)
}

/// Git checkouts under `mirrors`, as (`owner/name`, path), in name order.
fn discover(mirrors: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut found = Vec::new();
    let entries = |dir: &Path| -> Result<Vec<PathBuf>> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
            .with_context(|| format!("reading {}", dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.is_dir()
                    && !p
                        .file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with('.'))
            })
            .collect();
        v.sort();
        Ok(v)
    };
    for owner in entries(mirrors)? {
        for repo in entries(&owner)? {
            if repo.join(".git").exists() {
                let name = |p: &Path| {
                    p.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string()
                };
                found.push((format!("{}/{}", name(&owner), name(&repo)), repo));
            }
        }
    }
    Ok(found)
}

fn git(path: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .context("running git")?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn head_commit(path: &Path) -> Result<String> {
    git(path, &["rev-parse", "HEAD"])
}

/// Index the checkout and build its records.
fn repository_records(
    repo: &str,
    path: &Path,
    commit: &str,
    previous: Option<&str>,
    seq: u64,
    opts: &ExportOptions,
) -> Result<Vec<Value>> {
    let root = path
        .canonicalize()
        .with_context(|| format!("resolving {}", path.display()))?
        .to_string_lossy()
        .to_string();
    let mut db = open_project_database(&root)?;
    let config = IndexConfig {
        root: root.clone(),
        show_progress: false,
        ..Default::default()
    };
    // Incremental unless the index was built by another version of symgraph.
    let stats = if db.staleness()?.is_some() {
        rebuild_project_database(&mut db, &config)?
    } else {
        index_codebase(&mut db, &config)?
    };

    let churn = file_churn(&root, opts.churn_days, None).ok();
    let changed_at = git(path, &["log", "-1", "--format=%cI", "HEAD"])?;
    let common = Common {
        repo,
        commit,
        seq,
        changed_at: &changed_at,
        observed_at: &rfc3339_now(),
    };

    let endpoints = db.get_edge_endpoints(false)?;
    let graph = build_module_graph(&endpoints, Granularity::Dir, churn.as_ref());
    let scores = score_coupling(&graph, Granularity::Dir, churn.as_ref());
    let gods = rank_god_structs(&db, churn.as_ref(), false).map_err(anyhow::Error::msg)?;
    let in_cycle: std::collections::HashSet<&str> =
        graph.cycles.iter().flatten().map(String::as_str).collect();

    // Who depends on each directory (the graph's edges reversed).
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in &graph.edges {
        dependents
            .entry(e.to.as_str())
            .or_default()
            .push(e.from.as_str());
    }
    let nodes: std::collections::HashSet<&str> =
        graph.nodes.iter().map(|n| n.id.as_str()).collect();

    let mut records = Vec::new();
    records.push(snapshot(&common, &db, &stats, &graph, &gods, &in_cycle)?);
    for n in &graph.nodes {
        let total = n.fan_in + n.fan_out;
        records.push(common.record(
            "directory_coupling",
            &format!("dir:{}", n.id),
            json!({
                "directory": n.id,
                "fan_in": n.fan_in,
                "fan_out": n.fan_out,
                // Fan-out over all dependencies: 0 is depended on and
                // depends on nothing (stable), 1 the opposite.
                "instability": (total > 0).then(|| f64::from(n.fan_out) / f64::from(total)),
                "in_cycle": in_cycle.contains(n.id.as_str()),
                "churn": n.churn,
            }),
        ));
    }
    for (rank, s) in scores.iter().take(opts.top).enumerate() {
        records.push(common.record(
            "coupling_edge",
            &format!("edge:{}->{}", s.from, s.to),
            json!({
                "rank": rank + 1,
                "from_directory": s.from,
                "to_directory": s.to,
                "strength": s.strength,
                "distance": s.distance,
                "volatility": s.volatility,
                "impact": s.impact,
                "edge_count": s.edge_count,
                "by_kind": s.by_kind,
            }),
        ));
    }
    for change in changes(path, previous, opts.churn_days)? {
        let touched: BTreeSet<String> = change
            .files
            .iter()
            .map(|f| boundary_of(f, Granularity::Dir))
            .filter(|d| dependents.contains_key(d.as_str()) || nodes.contains(d.as_str()))
            .collect();
        let (direct, transitive) = reach(&touched, &dependents);
        records.push(Value::Object({
            let mut r = common
                .record(
                    "change_impact",
                    &format!("change:{}", change.sha),
                    json!({
                        "sha": change.sha,
                        "files_changed": change.files.len(),
                        "directories_touched": touched.len(),
                        "direct_dependents": direct,
                        "transitive_dependents": transitive,
                        // Share of the repository's directories a change can
                        // reach: 1.0 means everything depends on it.
                        "reach_share": (!nodes.is_empty())
                            .then(|| transitive as f64 / nodes.len() as f64),
                        "touches_cycle": touched.iter().any(|d| in_cycle.contains(d.as_str())),
                        // Impact is read off the graph at the exported commit,
                        // not at each change's own commit.
                        "graph_commit": commit,
                    }),
                )
                .as_object()
                .cloned()
                .unwrap_or_default();
            // A change is dated, and identified, by its own commit.
            r.insert(
                "id".into(),
                json!(format!("symgraph:{repo}@{}:change", change.sha)),
            );
            r.insert("commit".into(), json!(change.sha));
            r.insert("changed_at".into(), json!(change.committed_at));
            r
        }));
    }
    for (rank, g) in gods.iter().take(opts.top).enumerate() {
        records.push(common.record(
            "god_struct",
            &format!("struct:{}#{}", g.file, g.name),
            json!({
                "rank": rank + 1,
                "name": g.name,
                "file": g.file,
                "public_fields": g.pub_fields,
                "total_fields": g.total_fields,
                "inbound_refs": g.inbound_refs,
                "churn": g.churn,
                "score": g.score,
            }),
        ));
    }
    Ok(records)
}

/// One commit's changed files.
struct Change {
    sha: String,
    committed_at: String,
    files: Vec<String>,
}

/// Most commits one export reads per repository.
const MAX_CHANGES: usize = 1_000;

/// The non-merge commits since `previous` (exclusive), or within the last
/// `days` on a first export, with their changed files, oldest first.
fn changes(path: &Path, previous: Option<&str>, days: u32) -> Result<Vec<Change>> {
    let max = format!("--max-count={MAX_CHANGES}");
    let since = format!("--since={days}.days");
    let range = previous.map(|p| format!("{p}..HEAD"));
    let mut args = vec![
        "log",
        "--no-merges",
        "--reverse",
        "--format=%x00%H%x09%cI",
        "--name-only",
        &max,
    ];
    match &range {
        Some(r) => args.push(r),
        None => {
            args.push(&since);
            args.push("HEAD");
        }
    }
    let text = match git(path, &args) {
        Ok(t) => t,
        // The previous commit is gone (a force-push): fall back to the window.
        Err(_) if previous.is_some() => return changes(path, None, days),
        Err(e) => return Err(e),
    };
    Ok(text
        .split('\0')
        .filter_map(|block| {
            let mut lines = block.lines();
            let (sha, at) = lines.next()?.split_once('\t')?;
            Some(Change {
                sha: sha.to_string(),
                committed_at: at.to_string(),
                files: lines
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect(),
            })
        })
        .collect())
}

/// How many directories depend on `touched` directly, and how many can
/// reach it through any chain of dependencies, not counting `touched`.
fn reach(touched: &BTreeSet<String>, dependents: &HashMap<&str, Vec<&str>>) -> (usize, usize) {
    let direct: BTreeSet<&str> = touched
        .iter()
        .flat_map(|d| dependents.get(d.as_str()).into_iter().flatten().copied())
        .filter(|d| !touched.contains(*d))
        .collect();
    let mut seen: BTreeSet<&str> = touched.iter().map(String::as_str).collect();
    let mut queue: Vec<&str> = seen.iter().copied().collect();
    while let Some(d) = queue.pop() {
        for &up in dependents.get(d).into_iter().flatten() {
            if seen.insert(up) {
                queue.push(up);
            }
        }
    }
    (direct.len(), seen.len() - touched.len())
}

fn snapshot(
    common: &Common,
    db: &Database,
    stats: &crate::IndexingStats,
    graph: &crate::coupling::ModuleGraph,
    gods: &[crate::mcp::handlers::god_struct::GodStruct],
    in_cycle: &std::collections::HashSet<&str>,
) -> Result<Value> {
    let directories = graph.nodes.len();
    let largest = graph.cycles.iter().map(Vec::len).max().unwrap_or(0);
    let share = |n: usize| (directories > 0).then(|| n as f64 / directories as f64);
    let indexed = db.get_stats()?.total_files;
    let unsupported: u64 = stats.unsupported_types.values().sum();
    Ok(common.record(
        "coupling_snapshot",
        "snapshot",
        json!({
            "granularity": "dir",
            "directories": directories,
            "dependencies": graph.edges.len(),
            "cycles": graph.cycles.len(),
            "largest_cycle": largest,
            "largest_cycle_share": share(largest),
            "directories_in_cycles": in_cycle.len(),
            "share_in_cycles": share(in_cycle.len()),
            "god_structs": gods.iter().filter(|g| g.score >= GOD_STRUCT_MIN_SCORE).count(),
            "god_struct_max_score": gods.first().map(|g| g.score),
            "unused_internal_symbols": db.count_unused_internal()?,
            "files_indexed": indexed,
            "files_unsupported": unsupported,
            "files_with_parse_errors": stats.parse_failures,
            // Share of source files symgraph could read: how much of the
            // repository these numbers describe.
            "parse_coverage": (indexed + unsupported > 0)
                .then(|| indexed as f64 / (indexed + unsupported) as f64),
        }),
    ))
}

/// Fields every record of one repository's export shares.
struct Common<'a> {
    repo: &'a str,
    commit: &'a str,
    seq: u64,
    changed_at: &'a str,
    observed_at: &'a str,
}

impl Common<'_> {
    fn record(&self, record_type: &str, key: &str, fields: Value) -> Value {
        let mut r = Map::new();
        r.insert("record_type".into(), json!(record_type));
        r.insert(
            "id".into(),
            json!(format!("symgraph:{}@{}:{key}", self.repo, self.commit)),
        );
        r.insert("schema_version".into(), json!(SCHEMA_VERSION));
        r.insert("source".into(), json!("symgraph"));
        r.insert("repo".into(), json!(self.repo));
        r.insert("commit".into(), json!(self.commit));
        r.insert("changed_at".into(), json!(self.changed_at));
        r.insert("observed_at".into(), json!(self.observed_at));
        r.insert("collected_at".into(), json!(self.observed_at));
        r.insert("source_seq".into(), json!(self.seq));
        if let Value::Object(f) = fields {
            r.extend(f);
        }
        Value::Object(r)
    }
}

/// The current UTC time as RFC 3339, without a date crate.
fn rfc3339_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    rfc3339(secs)
}

fn rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reach_counts_direct_and_transitive_dependents() {
        // api -> core -> util, and cli -> util.
        let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
        dependents.insert("core", vec!["api"]);
        dependents.insert("util", vec!["core", "cli"]);
        let touched = |d: &[&str]| d.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
        assert_eq!(reach(&touched(&["util"]), &dependents), (2, 3));
        assert_eq!(reach(&touched(&["core"]), &dependents), (1, 1));
        assert_eq!(reach(&touched(&["api"]), &dependents), (0, 0));
        assert_eq!(reach(&touched(&["core", "util"]), &dependents), (2, 2));
    }

    #[test]
    fn formats_utc_timestamps() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_791_158_400), "2026-10-05T00:00:00Z");
    }

    /// A one-file repository under `<dir>/acme/<name>`, committed.
    fn mirror(dir: &Path, name: &str, source: &str) -> PathBuf {
        let path = dir.join("acme").join(name);
        std::fs::create_dir_all(path.join("src/a")).unwrap();
        std::fs::create_dir_all(path.join("src/b")).unwrap();
        std::fs::write(path.join("src/a/mod.rs"), source).unwrap();
        std::fs::write(
            path.join("src/b/mod.rs"),
            "pub struct Shared { pub x: u32, pub y: u32 }\npub fn helper() -> u32 { 1 }\n",
        )
        .unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["add", "."],
            vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.invalid",
                "commit",
                "-qm",
                "init",
            ],
        ] {
            git(&path, &args).unwrap();
        }
        path
    }

    fn run(mirrors: &Path, state: &Path, since: u64) -> (ExportSummary, Vec<Value>) {
        let opts = ExportOptions {
            mirrors: mirrors.to_path_buf(),
            state: state.to_path_buf(),
            since,
            top: 10,
            churn_days: 90,
        };
        let mut out = Vec::new();
        let summary = export(&opts, &mut out).unwrap();
        let records = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        (summary, records)
    }

    #[test]
    fn exports_each_commit_once_the_caller_confirms_it() {
        let dir = tempfile::tempdir().unwrap();
        let mirrors = dir.path().join("workspace");
        let state = dir.path().join("state");
        let repo = mirror(
            &mirrors,
            "app",
            "use crate::b::{helper, Shared};\npub fn run(s: &Shared) -> u32 { helper() + s.x }\n",
        );

        let (first, records) = run(&mirrors, &state, 0);
        assert_eq!((first.exported, first.unchanged), (1, 0));
        let snapshot = records
            .iter()
            .find(|r| r["record_type"] == "coupling_snapshot")
            .expect("a snapshot");
        assert_eq!(snapshot["repo"], "acme/app");
        assert_eq!(snapshot["source"], "symgraph");
        assert_eq!(snapshot["granularity"], "dir");
        assert_eq!(snapshot["source_seq"], 1);
        let commit = head_commit(&repo).unwrap();
        assert_eq!(snapshot["commit"], commit.as_str());
        assert_eq!(
            snapshot["id"],
            format!("symgraph:acme/app@{commit}:snapshot").as_str()
        );
        assert!(records
            .iter()
            .any(|r| r["record_type"] == "directory_coupling"
                && r["directory"] == "src/a"
                && r["fan_out"] == 1));

        // Not confirmed yet (cursor still 0): the same records again.
        let (again, resent) = run(&mirrors, &state, 0);
        assert_eq!(again.exported, 1);
        assert_eq!(
            resent.iter().map(|r| r["id"].clone()).collect::<Vec<_>>(),
            records.iter().map(|r| r["id"].clone()).collect::<Vec<_>>(),
            "the same ids, so re-sending is idempotent"
        );

        // Confirmed (cursor 1) and unchanged: nothing.
        let (confirmed, none) = run(&mirrors, &state, 1);
        assert_eq!((confirmed.exported, confirmed.unchanged), (0, 1));
        assert!(none.is_empty());

        // A new commit: a new sequence number.
        std::fs::write(repo.join("src/a/extra.rs"), "pub fn more() {}\n").unwrap();
        git(&repo, &["add", "."]).unwrap();
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.invalid",
                "commit",
                "-qm",
                "more",
            ],
        )
        .unwrap();
        let (changed, newer) = run(&mirrors, &state, 1);
        assert_eq!(changed.exported, 1);
        assert!(newer.iter().all(|r| r["source_seq"] == 2));
        let head = head_commit(&repo).unwrap();
        let impacts: Vec<&Value> = newer
            .iter()
            .filter(|r| r["record_type"] == "change_impact")
            .collect();
        assert_eq!(impacts.len(), 1, "only the commit since the last export");
        assert_eq!(impacts[0]["sha"], head.as_str());
        assert_eq!(
            impacts[0]["id"],
            format!("symgraph:acme/app@{head}:change").as_str()
        );
        assert_eq!(impacts[0]["directories_touched"], 1);
    }
}
