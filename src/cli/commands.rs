//! Command implementations for CLI operations

use anyhow::{Context, Result};
use tracing::info;

use crate::context::{format_context_markdown, ContextBuilder, ContextOptions};
use crate::db::Database;
use crate::{index_codebase, IndexConfig};

use super::db_utils::{canonicalize_path, database_path, open_project_database};

/// Index a codebase at the given path
pub fn index_command(path: &str) -> Result<()> {
    let project_root = canonicalize_path(path)?;
    let mut db = open_project_database(&project_root)?;

    let config = IndexConfig {
        root: project_root.clone(),
        ..Default::default()
    };

    let stats = index_codebase(&mut db, &config)?;

    println!("\nIndexing complete!");
    println!("  Files indexed: {}", stats.files);
    println!("  Symbols found: {}", stats.nodes);
    println!("  Relationships: {}", stats.edges);
    println!("  Files skipped: {}", stats.skipped);
    println!("  Refs resolved: {}", stats.resolved_refs);
    if stats.errors > 0 {
        println!("  Errors: {}", stats.errors);
    }

    Ok(())
}

/// Show index statistics for a project
pub fn status_command(path: &str) -> Result<()> {
    let project_root = canonicalize_path(path)?;
    let db_path = database_path(&project_root);

    if !db_path.exists() {
        println!("No index found at {}", db_path.display());
        println!("Run 'symgraph index {}' first.", path);
        return Ok(());
    }

    let db = Database::open(&db_path)?;
    let stats = db.get_stats()?;

    println!("symgraph Index Status");
    println!("=====================");
    println!("Database: {}", db_path.display());
    println!("Files: {}", stats.total_files);
    println!("Symbols: {}", stats.total_nodes);
    println!("Relationships: {}", stats.total_edges);
    println!("Size: {:.2} KB", stats.db_size_bytes as f64 / 1024.0);

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

/// Search for symbols by name
pub fn search_command(path: &str, query: &str) -> Result<()> {
    let project_root = canonicalize_path(path)?;
    let db_path = database_path(&project_root);

    if !db_path.exists() {
        println!("No index found. Run 'symgraph index' first.");
        return Ok(());
    }

    let db = Database::open(&db_path)?;
    let results = db.search_nodes(query, None, 20)?;

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

    Ok(())
}

/// Build AI context for a task
pub fn context_command(path: &str, task: &str) -> Result<()> {
    let project_root = canonicalize_path(path)?;
    let db_path = database_path(&project_root);

    if !db_path.exists() {
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
    let markdown = format_context_markdown(&context);

    println!("{}", markdown);

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

    Ok((project_root, db))
}
