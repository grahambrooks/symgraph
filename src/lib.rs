//! codemap: Semantic code intelligence MCP server
//!
//! A Rust-based MCP server that builds a knowledge graph of codebases
//! to enhance AI-assisted code exploration. Uses tree-sitter for parsing
//! and SQLite for storage.
//!
//! ## Features
//!
//! - Multi-language support: Rust, TypeScript, JavaScript, Python, Go, Java, C, C++
//! - Symbol extraction: functions, classes, methods, interfaces, etc.
//! - Relationship tracking: calls, contains, imports, exports, etc.
//! - Impact analysis: trace the effect of changes through the codebase
//! - Task context: build focused context for AI exploration
//!
//! ## MCP Tools
//!
//! - `codemap_context` - Build task-specific code context
//! - `codemap_search` - Find symbols by name
//! - `codemap_callers` - Find all callers of a symbol
//! - `codemap_callees` - Find all callees of a symbol
//! - `codemap_impact` - Analyze change impact
//! - `codemap_node` - Get detailed symbol information
//! - `codemap_status` - Get index statistics

#[cfg(feature = "sqlite")]
pub mod cli;
#[cfg(feature = "sqlite")]
pub mod context;
#[cfg(feature = "sqlite")]
pub mod db;
pub mod extraction;
#[cfg(feature = "sqlite")]
pub mod graph;
#[cfg(feature = "sqlite")]
pub mod mcp;
pub mod types;

use std::path::Path;
use std::time::SystemTime;

use anyhow::Result;
use ignore::WalkBuilder;
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

#[cfg(feature = "sqlite")]
use db::Database;
use extraction::Extractor;
use types::{FileRecord, Language};

/// Configuration for indexing
#[derive(Debug, Clone)]
pub struct IndexConfig {
    /// Root directory to index
    pub root: String,
    /// File extensions to include (empty = all supported)
    pub extensions: Vec<String>,
    /// Directories to exclude
    pub exclude_dirs: Vec<String>,
    /// Whether to follow gitignore rules
    pub respect_gitignore: bool,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            root: ".".to_string(),
            extensions: vec![
                "rs".to_string(),
                "ts".to_string(),
                "tsx".to_string(),
                "js".to_string(),
                "jsx".to_string(),
                "py".to_string(),
                "go".to_string(),
                "java".to_string(),
                "c".to_string(),
                "h".to_string(),
                "cpp".to_string(),
                "cc".to_string(),
                "hpp".to_string(),
            ],
            exclude_dirs: vec![
                "node_modules".to_string(),
                "target".to_string(),
                "dist".to_string(),
                "build".to_string(),
                ".git".to_string(),
                "__pycache__".to_string(),
                ".venv".to_string(),
                "venv".to_string(),
                "vendor".to_string(),
            ],
            respect_gitignore: true,
        }
    }
}

/// Index a codebase into the database
#[cfg(feature = "sqlite")]
pub fn index_codebase(db: &mut Database, config: &IndexConfig) -> Result<IndexStats> {
    let root = Path::new(&config.root).canonicalize()?;
    info!("Indexing codebase at {}", root.display());

    let mut extractor = Extractor::new();
    let mut stats = IndexStats::default();

    // Build the walker
    let mut walker = WalkBuilder::new(&root);
    walker
        .hidden(false)
        .git_ignore(config.respect_gitignore)
        .git_global(config.respect_gitignore)
        .git_exclude(config.respect_gitignore);

    // Begin transaction
    db.begin_transaction()?;

    for entry in walker.build() {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                warn!("Error walking directory: {}", err);
                continue;
            }
        };

        let path = entry.path();

        // Skip directories
        if !path.is_file() {
            continue;
        }

        // Check extension
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !config.extensions.is_empty() && !config.extensions.iter().any(|e| e == ext) {
            continue;
        }

        // Check if language is supported
        let language = Language::from_extension(ext);
        if language == Language::Unknown {
            continue;
        }

        // Check excluded directories
        let path_str = path.display().to_string();
        if config.exclude_dirs.iter().any(|d| {
            path_str.contains(&format!("/{}/", d)) || path_str.contains(&format!("\\{}\\", d))
        }) {
            continue;
        }

        // Read file content
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(err) => {
                debug!("Failed to read {}: {}", path.display(), err);
                stats.errors += 1;
                continue;
            }
        };

        // Compute content hash
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let content_hash = hex::encode(hasher.finalize());

        // Get relative path
        let rel_path = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string();

        // Check if file needs reindexing
        if !db.needs_reindex(&rel_path, &content_hash)? {
            debug!("Skipping unchanged file: {}", rel_path);
            stats.skipped += 1;
            continue;
        }

        debug!("Indexing: {}", rel_path);

        // Delete existing data for this file
        db.delete_file(&rel_path)?;

        // Extract symbols
        let result = extractor.extract_file(&rel_path, &content);

        // Store file record FIRST (nodes have FK to files)
        let file_record = FileRecord {
            path: rel_path.clone(),
            content_hash,
            language,
            size: content.len() as u64,
            modified_at: entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            indexed_at: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            node_count: result.nodes.len() as u32,
        };
        db.insert_or_update_file(&file_record)?;

        // Store nodes
        let mut node_count = 0;
        let mut id_map: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();

        for mut node in result.nodes {
            let old_id = node.id;
            node.id = 0; // Will be assigned by DB
            let new_id = db.insert_node(&node)?;
            id_map.insert(old_id, new_id);
            node_count += 1;
        }

        // Store edges with mapped IDs
        for mut edge in result.edges {
            if let (Some(&new_source), Some(&new_target)) =
                (id_map.get(&edge.source_id), id_map.get(&edge.target_id))
            {
                edge.source_id = new_source;
                edge.target_id = new_target;
                db.insert_edge(&edge)?;
                stats.edges += 1;
            }
        }

        // Store unresolved references with mapped IDs
        for mut uref in result.unresolved_refs {
            if let Some(&new_source) = id_map.get(&uref.source_node_id) {
                uref.source_node_id = new_source;
                db.insert_unresolved_ref(&uref)?;
            }
        }

        // Update node count in file record
        let file_record = FileRecord {
            path: rel_path.clone(),
            content_hash: file_record.content_hash,
            language,
            size: content.len() as u64,
            modified_at: file_record.modified_at,
            indexed_at: file_record.indexed_at,
            node_count,
        };
        db.insert_or_update_file(&file_record)?;

        stats.files += 1;
        stats.nodes += node_count as u64;
        stats.errors += result.errors.len() as u64;
    }

    // Resolve references
    info!("Resolving references...");
    let resolved = db.resolve_references()?;
    stats.resolved_refs = resolved as u64;

    // Commit transaction
    db.commit()?;

    info!(
        "Indexed {} files, {} nodes, {} edges ({} refs resolved)",
        stats.files, stats.nodes, stats.edges, stats.resolved_refs
    );

    Ok(stats)
}

/// Statistics from indexing
#[derive(Debug, Default)]
pub struct IndexStats {
    pub files: u64,
    pub nodes: u64,
    pub edges: u64,
    pub skipped: u64,
    pub errors: u64,
    pub resolved_refs: u64,
}
