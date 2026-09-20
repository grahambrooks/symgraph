//! Command implementations for CLI operations

use anyhow::{Context, Result};
use serde::Serialize;
use tracing::{info, warn};

use crate::db::Database;

use crate::{IndexConfig, IndexingStats};

use super::db_utils::{
    canonicalize_path, open_project_database, prune_cache, rebuild_project_database, resolve_db,
};

/// Output format selected on the command line (`--format text|json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
}

impl OutputFormat {
    /// Parse a `--format` value; `None` if unrecognized.
    pub fn parse(s: &str) -> Option<OutputFormat> {
        match s.to_ascii_lowercase().as_str() {
            "text" | "txt" => Some(OutputFormat::Text),
            "json" => Some(OutputFormat::Json),
            _ => None,
        }
    }

    fn is_json(self) -> bool {
        matches!(self, OutputFormat::Json)
    }

    /// The value to put in a handler request's `format` field
    /// (`Some("json")` for JSON, `None` for the handler's default markdown).
    pub fn request_format(self) -> Option<String> {
        if self.is_json() {
            Some("json".to_string())
        } else {
            None
        }
    }
}

/// Print a value as pretty JSON to stdout.
fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

#[derive(Serialize)]
struct WhereReport {
    project_root: String,
    index_path: String,
    strategy: String,
    present: bool,
}

/// Index a codebase at the given path
pub fn index_command(path: &str, fmt: OutputFormat) -> Result<()> {
    let project_root = canonicalize_path(path)?;
    let mut db = open_project_database(&project_root)?;

    let config = IndexConfig {
        root: project_root.clone(),
        // Keep stdout clean for JSON; show the progress bar only in text mode.
        show_progress: !fmt.is_json(),
        ..Default::default()
    };

    let stats = rebuild_project_database(&mut db, &config)?;

    // Same JSON shape as the `symgraph-reindex` tool, so an agent gets one
    // answer whether it shells out or calls the server. The text rendering
    // stays as it is: this command runs in the foreground with a progress
    // bar, which the tool does not.
    if fmt.is_json() {
        return print_json(&crate::ops::ReindexResult::from_stats(
            "full rebuild",
            &stats,
        ));
    }

    println!("\nIndexing complete!");
    println!("  Files indexed: {}", stats.files);
    println!("  Symbols found: {}", stats.nodes);
    println!("  Relationships: {}", stats.edges);
    println!("  Files skipped: {}", stats.skipped);
    println!("  Refs resolved: {}", stats.resolved_refs);
    if stats.errors > 0 {
        println!("  Errors: {}", stats.errors);
    }
    print_index_gaps(&stats);
    print_unsupported_types(&stats.unsupported_types);

    Ok(())
}

/// Print what indexing could not fully cover: files whose syntax defeated the
/// parser, and files too large to attempt. Silent zeros here previously made a
/// partial index look complete.
pub fn print_index_gaps(stats: &IndexingStats) {
    if stats.parse_failures > 0 {
        println!(
            "  Files with syntax errors: {} (symbols from these are incomplete)",
            stats.parse_failures
        );
    }
    if stats.skipped_too_large > 0 {
        println!(
            "  Files skipped as too large: {} (raise max_file_bytes to include them)",
            stats.skipped_too_large
        );
    }
}

/// Print the source file types that were found during indexing but left out of
/// the index because symgraph does not index them (no parser for the language,
/// or the extension isn't in the active set). Non-source files (docs, config,
/// images) are not counted. No-op when every source file found was indexed.
pub fn print_unsupported_types(unsupported: &std::collections::BTreeMap<String, u64>) {
    if unsupported.is_empty() {
        return;
    }
    let total: u64 = unsupported.values().sum();
    println!(
        "  Unsupported source file types skipped: {} file(s) across {} type(s)",
        total,
        unsupported.len()
    );
    // Show the most common types first.
    let mut types: Vec<(&String, &u64)> = unsupported.iter().collect();
    types.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    for (ext, count) in types {
        println!("    .{}: {}", ext, count);
    }
}

/// Print the resolved index location (and whether it exists) for a project.
pub fn where_command(path: &str, fmt: OutputFormat) -> Result<()> {
    let project_root = canonicalize_path(path)?;
    let resolved = resolve_db(&project_root)?;
    let present = resolved.path.exists();

    if fmt.is_json() {
        return print_json(&WhereReport {
            project_root,
            index_path: resolved.path.display().to_string(),
            strategy: resolved.label.to_string(),
            present,
        });
    }

    println!("Project root: {}", project_root);
    println!("Index path:   {}", resolved.path.display());
    println!("Strategy:     {}", resolved.label);
    println!(
        "Status:       {}",
        if present {
            "present"
        } else {
            "not indexed (run 'symgraph index')"
        }
    );
    Ok(())
}

/// Remove cache-stored indexes that are no longer useful (source repo gone, or
/// now indexed under its git dir / in-tree, or — with `max_age_days` — stale).
pub fn prune_command(max_age_days: Option<u64>, fmt: OutputFormat) -> Result<()> {
    let stats = prune_cache(max_age_days)?;
    if fmt.is_json() {
        return print_json(&stats);
    }
    println!(
        "Pruned {} stale cache index(es), reclaiming {:.1} KB.",
        stats.removed,
        stats.bytes_freed as f64 / 1024.0
    );
    Ok(())
}

/// Initialize database for MCP server mode
pub fn initialize_server_database(in_memory: bool) -> Result<(String, Database)> {
    use std::env;

    let in_memory = in_memory || env::var("SYMGRAPH_IN_MEMORY").is_ok_and(|v| v == "1");

    // Get project root from environment or current directory
    let project_root = env::var("SYMGRAPH_ROOT")
        .or_else(|_| env::current_dir().map(|p| p.display().to_string()))
        .context("Could not determine project root")?;

    let project_root = canonicalize_path(&project_root)?;
    let db = if in_memory {
        info!("Using in-memory database (no filesystem writes)");
        Database::in_memory()?
    } else {
        open_project_database(&project_root)?
    };

    // Log database status
    let stats = db.get_stats()?;
    if stats.total_files == 0 {
        info!("No index found, consider running 'symgraph index' first");
    } else {
        info!(
            "Index loaded: {} files, {} symbols",
            stats.total_files, stats.total_nodes
        );
    }
    if let Some(stale) = db.staleness()? {
        warn!(
            "index is out of date ({}); answers may reflect older extraction \
             semantics. Run `symgraph reindex` or call the symgraph-reindex tool.",
            stale
        );
    }

    Ok((project_root, db))
}
