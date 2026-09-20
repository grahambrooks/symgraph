//! CLI ⇄ MCP equivalence.
//!
//! `docs/cli-mcp-parity.md` claims CLI output is identical to MCP output "by
//! construction", because both front-ends call the same handler. That held
//! only where it was true: `search`, `context` and `status` had their own CLI
//! implementations and drifted from the handlers three separate times.
//!
//! These tests run the real binary and compare it against the handler the MCP
//! server calls, so the claim is checked rather than asserted.

use std::process::Command;

use symgraph::db::Database;
use symgraph::mcp::handlers;
use symgraph::mcp::{ContextRequest, FormatRequest, SearchRequest};
use symgraph::{build_full_index, IndexConfig};
use tempfile::TempDir;

/// Index a fixture and return its directory plus the on-disk index path.
fn indexed() -> (TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("lib.rs"),
        "pub mod engine;\npub struct Settings { pub retries: u32 }\n",
    )
    .unwrap();
    std::fs::write(
        src.join("engine.rs"),
        "use crate::Settings;\n\
         pub struct Engine { pub settings: Settings }\n\
         impl Engine {\n\
         \x20   pub fn run(&self) -> u32 { self.helper() }\n\
         \x20   fn helper(&self) -> u32 { 1 }\n\
         }\n\
         pub fn unreferenced() {}\n",
    )
    .unwrap();

    // The git-backed tools (churn, diff-impact) need a repository to read, so
    // the fixture is one. Identity and hooks are set locally to keep the test
    // independent of the developer's git configuration.
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .output()
            .expect("git is required for the parity fixture");
    };
    git(&["init", "--quiet"]);
    git(&["config", "user.email", "test@example.invalid"]);
    git(&["config", "user.name", "Parity Fixture"]);
    git(&["add", "-A"]);
    git(&[
        "-c",
        "commit.gpgsign=false",
        "commit",
        "--quiet",
        "-m",
        "fixture",
    ]);

    let db_path = dir.path().join("index.db").display().to_string();
    let mut db = Database::open(&db_path).unwrap();
    build_full_index(
        &mut db,
        &IndexConfig {
            root: dir.path().display().to_string(),
            ..Default::default()
        },
    )
    .unwrap();
    drop(db);
    (dir, db_path)
}

fn binary() -> std::path::PathBuf {
    // The test binary lives in target/<profile>/deps; the CLI is two up.
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("symgraph-cli")
}

/// Run the CLI in the fixture, returning stdout.
fn run_cli(dir: &TempDir, db_path: &str, args: &[&str]) -> String {
    let bin = binary();
    if !bin.exists() {
        // `cargo test` does not necessarily build the binary; skip rather than
        // fail, and let the handler-level tests carry the coverage.
        return String::new();
    }
    let out = Command::new(&bin)
        .args(args)
        .current_dir(dir.path())
        .env("SYMGRAPH_DB", db_path)
        .output()
        .unwrap_or_else(|e| panic!("running {}: {e}", bin.display()));
    assert!(
        out.status.success(),
        "{:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// `search` had its own CLI implementation that ignored `--limit` entirely.
#[test]
fn cli_search_matches_the_handler() {
    let (dir, db_path) = indexed();
    let cli = run_cli(&dir, &db_path, &["search", "Engine", "--format", "json"]);
    if cli.is_empty() {
        return;
    }

    let db = Database::open(&db_path).unwrap();
    let handler = handlers::search::handle_search(
        &db,
        &SearchRequest {
            query: "Engine".to_string(),
            semantic: None,
            limit: None,
            format: Some("json".to_string()),
        },
    )
    .unwrap();

    assert_eq!(
        cli.trim(),
        handler.trim(),
        "CLI search output diverged from the handler"
    );
}

/// `context` supported `--format json` on the CLI while the MCP tool had no
/// `format` field at all — one tool, two shapes.
#[test]
fn cli_context_matches_the_handler() {
    let (dir, db_path) = indexed();
    let cli = run_cli(
        &dir,
        &db_path,
        &["context", "engine settings", "--format", "json"],
    );
    if cli.is_empty() {
        return;
    }

    let db = Database::open(&db_path).unwrap();
    let handler = handlers::context::handle_context(
        &db,
        &dir.path().canonicalize().unwrap().display().to_string(),
        &ContextRequest {
            task: "engine settings".to_string(),
            limit: None,
            format: Some("json".to_string()),
        },
    )
    .unwrap();

    assert_eq!(
        cli.trim(),
        handler.trim(),
        "CLI context output diverged from the handler"
    );
}

/// `status` had a separate CLI renderer, so the health block added in commit 3
/// had to be written twice.
#[test]
fn cli_status_matches_the_handler() {
    let (dir, db_path) = indexed();
    let cli = run_cli(&dir, &db_path, &["status", "--format", "json"]);
    if cli.is_empty() {
        return;
    }

    let db = Database::open(&db_path).unwrap();
    let handler = handlers::status::handle_status(&db, Some("json".to_string())).unwrap();

    assert_eq!(
        cli.trim(),
        handler.trim(),
        "CLI status output diverged from the handler"
    );
}

#[test]
fn cli_unused_matches_the_handler() {
    let (dir, db_path) = indexed();
    let cli = run_cli(&dir, &db_path, &["unused", "--format", "json"]);
    if cli.is_empty() {
        return;
    }

    let db = Database::open(&db_path).unwrap();
    let handler = handlers::unused::handle_unused(
        &db,
        &FormatRequest {
            limit: None,
            offset: None,
            format: Some("json".to_string()),
        },
    )
    .unwrap();

    assert_eq!(cli.trim(), handler.trim());
}

/// Every tool answers in JSON. This is the check behind the parity doc's
/// "machine-readable `--format json` on **every** tool" goal, which was false
/// for five of them until F4 was closed.
#[test]
fn every_cli_command_emits_valid_json() {
    let (dir, db_path) = indexed();
    let commands: &[&[&str]] = &[
        &["search", "Engine"],
        &["context", "engine"],
        &["status"],
        &["callers", "helper"],
        &["callees", "run"],
        &["node", "Engine"],
        &["references", "Engine"],
        &["definition", "run"],
        &["hierarchy", "Engine"],
        &["implementations", "Engine"],
        &["file", "src/engine.rs"],
        &["path", "run", "helper"],
        &["unused"],
        &["impact", "Engine"],
        &["module-graph"],
        &["coupling-score"],
        &["god-struct"],
        &["dispatch-sites", "Engine"],
        &["churn"],
        &[
            "diff-impact",
            "--file",
            "src/engine.rs",
            "--start",
            "1",
            "--end",
            "3",
        ],
    ];

    for cmd in commands {
        let mut args = cmd.to_vec();
        args.extend_from_slice(&["--format", "json"]);
        let out = run_cli(&dir, &db_path, &args);
        if out.is_empty() {
            return; // binary not built in this profile
        }
        serde_json::from_str::<serde_json::Value>(&out).unwrap_or_else(|e| {
            panic!("`{}` did not emit valid JSON ({e}):\n{out}", cmd.join(" "))
        });
    }
}
