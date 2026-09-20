//! Command implementations for CLI operations

use anyhow::{Context, Result};
use serde::Serialize;
use tracing::{info, warn};

use crate::context::{format_context_markdown, ContextBuilder, ContextOptions};
use crate::db::{Database, IndexHealth};
use crate::ops::constants::effective_limit;
use crate::types::{IndexStats, Node};

/// Default result count for `symgraph search`. Matches what the command has
/// always shown; `--limit` raises it.
const CLI_SEARCH_LIMIT: u32 = 20;
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
struct SearchReport {
    query: String,
    count: usize,
    /// True when more symbols matched than the limit allowed through.
    truncated: bool,
    results: Vec<Node>,
}

#[derive(Serialize)]
struct StatusReport {
    indexed: bool,
    database: String,
    strategy: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    stats: Option<IndexStats>,
    /// How far the index can be trusted, as opposed to how large it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    health: Option<IndexHealth>,
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

    if fmt.is_json() {
        return print_json(&stats);
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

/// Show index statistics for a project
pub fn status_command(path: &str, fmt: OutputFormat) -> Result<()> {
    let project_root = canonicalize_path(path)?;
    let resolved = resolve_db(&project_root)?;
    let db_path = resolved.path;

    if !db_path.exists() {
        if fmt.is_json() {
            return print_json(&StatusReport {
                indexed: false,
                database: db_path.display().to_string(),
                strategy: resolved.label.to_string(),
                stats: None,
                health: None,
            });
        }
        println!(
            "No index found at {} [{}]",
            db_path.display(),
            resolved.label
        );
        println!("Run 'symgraph index {}' first.", path);
        return Ok(());
    }

    let db = Database::open(&db_path)?;
    let stats = db.get_stats()?;
    let health = db.health()?;

    if fmt.is_json() {
        return print_json(&StatusReport {
            indexed: true,
            database: db_path.display().to_string(),
            strategy: resolved.label.to_string(),
            stats: Some(stats),
            health: Some(health),
        });
    }

    println!("symgraph Index Status");
    println!("=====================");
    println!("Database: {} [{}]", db_path.display(), resolved.label);
    println!("Files: {}", stats.total_files);
    println!("Symbols: {}", stats.total_nodes);
    println!("Relationships: {}", stats.total_edges);
    println!("Size: {:.2} KB", stats.db_size_bytes as f64 / 1024.0);

    print_health(&health);

    if !stats.languages.is_empty() {
        println!("\nLanguages:");
        for (lang, count) in &stats.languages {
            println!("  {}: {} symbols", lang.as_str(), count);
        }
    }

    if !stats.node_kinds.is_empty() {
        println!("\nSymbol Types:");
        for (kind, count) in &stats.node_kinds {
            println!("  {}: {}", kind.as_str(), count);
        }
    }

    Ok(())
}

/// Print the trust signals alongside the size ones: whether the index is
/// current, whether search is intact, and how much of the graph rests on
/// name resolution that had to guess.
fn print_health(health: &IndexHealth) {
    if let Some(stale) = &health.staleness {
        println!("\nWARNING: index is out of date ({stale}).");
        println!(
            "         Results may reflect older extraction semantics — run 'symgraph reindex'."
        );
    }
    if !health.fts_ok {
        println!("\nWARNING: the full-text search index disagrees with the symbol table.");
        println!("         Search results may be wrong; a full reindex rebuilds it.");
    }

    println!("\nIndex health:");
    match &health.version {
        Some(v) => println!(
            "  Built by: symgraph {} (schema v{}, extractor v{})",
            if v.symgraph.is_empty() {
                "unknown"
            } else {
                &v.symgraph
            },
            v.schema,
            v.extractor
        ),
        None => println!("  Built by: a symgraph that predates index provenance"),
    }
    println!(
        "  Ambiguous names: {} of {} ({:.1}%)",
        health.ambiguous_names,
        health.distinct_names,
        health.ambiguous_percent()
    );
    println!("  Unresolved references: {}", health.unresolved_refs);
}

/// Search for symbols by name
pub fn search_command(
    path: &str,
    query: &str,
    limit: Option<u32>,
    fmt: OutputFormat,
) -> Result<()> {
    let project_root = canonicalize_path(path)?;
    let db_path = resolve_db(&project_root)?.path;

    if !db_path.exists() {
        if fmt.is_json() {
            return print_json(&SearchReport {
                query: query.to_string(),
                count: 0,
                truncated: false,
                results: Vec::new(),
            });
        }
        println!("No index found. Run 'symgraph index' first.");
        return Ok(());
    }

    let db = Database::open(&db_path)?;
    // Ask for one more than we will show, so a full page can be distinguished
    // from a page that happens to be exactly the limit.
    let limit = effective_limit(limit, CLI_SEARCH_LIMIT);
    let mut results = db.search_nodes(query, None, limit + 1)?;
    let truncated = results.len() > limit as usize;
    results.truncate(limit as usize);

    if fmt.is_json() {
        return print_json(&SearchReport {
            query: query.to_string(),
            count: results.len(),
            truncated,
            results,
        });
    }

    if results.is_empty() {
        println!("No symbols found matching '{}'", query);
        return Ok(());
    }

    println!("Found {} symbols matching '{}':\n", results.len(), query);

    for node in results {
        println!(
            "  {} {} - {}:{}",
            node.kind.as_str(),
            node.name,
            node.file_path,
            node.start_line
        );
        if let Some(ref sig) = node.signature {
            let sig = sig.lines().next().unwrap_or(sig);
            if sig.len() > 80 {
                println!("    {}...", &sig[..80]);
            } else {
                println!("    {}", sig);
            }
        }
    }

    if truncated {
        println!(
            "\nMore than {} symbols match. Raise --limit or narrow the query.",
            limit
        );
    }

    Ok(())
}

/// Build AI context for a task
pub fn context_command(path: &str, task: &str, fmt: OutputFormat) -> Result<()> {
    let project_root = canonicalize_path(path)?;
    let db_path = resolve_db(&project_root)?.path;

    if !db_path.exists() {
        if fmt.is_json() {
            return print_json(&serde_json::json!({ "error": "no index", "task": task }));
        }
        println!("No index found. Run 'symgraph index' first.");
        return Ok(());
    }

    let db = Database::open(&db_path)?;
    let builder = ContextBuilder::new(&db, project_root);

    let options = ContextOptions {
        max_nodes: 20,
        include_code: true,
        max_code_blocks: 5,
        ..Default::default()
    };

    let context = builder.build_context(task, &options)?;

    if fmt.is_json() {
        return print_json(&context);
    }

    println!("{}", format_context_markdown(&context));

    Ok(())
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
