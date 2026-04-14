//! symgraph: Semantic code intelligence MCP server
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
//! - `symgraph_context` - Build task-specific code context
//! - `symgraph_search` - Find symbols by name
//! - `symgraph_callers` - Find all callers of a symbol
//! - `symgraph_callees` - Find all callees of a symbol
//! - `symgraph_impact` - Analyze change impact
//! - `symgraph_node` - Get detailed symbol information
//! - `symgraph_status` - Get index statistics

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
pub mod security;
pub mod types;

use std::path::Path;
use std::time::SystemTime;

use anyhow::Result;
use ignore::WalkBuilder;
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

#[cfg(feature = "sqlite")]
use db::Database;
use extraction::Extractor;
use types::{ExtractionResult, FileRecord, Language};

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
    /// Skip the global resolve_references pass (for scoped resolution)
    pub skip_resolve: bool,
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
                "cs".to_string(),
                "kt".to_string(),
                "kts".to_string(),
                "scala".to_string(),
                "groovy".to_string(),
                "rb".to_string(),
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
            skip_resolve: false,
        }
    }
}

/// Collected file metadata ready for extraction
#[cfg(feature = "sqlite")]
struct FileEntry {
    rel_path: String,
    content: String,
    content_hash: String,
    language: Language,
    modified_at: i64,
}

/// Result of parallel extraction for a single file
#[cfg(feature = "sqlite")]
struct ExtractedFile {
    entry: FileEntry,
    result: ExtractionResult,
}

/// Index a codebase into the database
#[cfg(feature = "sqlite")]
pub fn index_codebase(db: &mut Database, config: &IndexConfig) -> Result<IndexingStats> {
    let root = Path::new(&config.root).canonicalize()?;
    info!("Indexing codebase at {}", root.display());

    let mut stats = IndexingStats::default();

    // Build the walker
    let mut walker = WalkBuilder::new(&root);
    walker
        .hidden(false)
        .git_ignore(config.respect_gitignore)
        .git_global(config.respect_gitignore)
        .git_exclude(config.respect_gitignore);

    // Phase 1a: Sequentially walk directory, read files, compute hashes, check reindex need
    let mut entries_to_extract: Vec<FileEntry> = Vec::new();

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

        // Check if this is a manifest file (detected by filename)
        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let is_manifest = extraction::manifest::is_manifest_file(filename);

        // Check extension
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !is_manifest
            && !config.extensions.is_empty()
            && !config.extensions.iter().any(|e| e == ext)
        {
            continue;
        }

        // Check if language is supported (manifest files use their ecosystem language)
        let language = if is_manifest {
            extraction::manifest::manifest_language(filename)
        } else {
            Language::from_extension(ext)
        };
        if language == Language::Unknown {
            continue;
        }

        // Check excluded directories
        if path.components().any(|c| {
            config
                .exclude_dirs
                .iter()
                .any(|d| c.as_os_str() == d.as_str())
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

        // Check if file needs reindexing (requires DB access, done sequentially)
        if !db.needs_reindex(&rel_path, &content_hash)? {
            debug!("Skipping unchanged file: {}", rel_path);
            stats.skipped += 1;
            continue;
        }

        let modified_at = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        entries_to_extract.push(FileEntry {
            rel_path,
            content,
            content_hash,
            language,
            modified_at,
        });
    }

    // Phase 1b: Parallel tree-sitter extraction using rayon
    // Each thread creates its own Extractor (Parser is not Send)
    let extracted: Vec<ExtractedFile> = entries_to_extract
        .into_par_iter()
        .map(|entry| {
            let mut extractor = Extractor::new();
            let result = extractor.extract_file(&entry.rel_path, &entry.content);
            ExtractedFile { entry, result }
        })
        .collect();

    // Phase 2: Sequential database operations inside a transaction
    db.begin_transaction()?;

    for extracted_file in extracted {
        let entry = extracted_file.entry;
        let result = extracted_file.result;

        debug!("Indexing: {}", entry.rel_path);

        // Delete existing data for this file
        db.delete_file(&entry.rel_path)?;

        // Store file record FIRST (nodes have FK to files)
        let file_record = FileRecord {
            path: entry.rel_path.clone(),
            content_hash: entry.content_hash,
            language: entry.language,
            size: entry.content.len() as u64,
            modified_at: entry.modified_at,
            indexed_at: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            node_count: result.nodes.len() as u32,
        };
        db.insert_or_update_file(&file_record)?;

        // Store nodes (batch: prepare statement once, reuse for all rows)
        let node_count = file_record.node_count;
        let mut nodes = result.nodes;
        let id_map = db.insert_nodes_batch(&mut nodes)?;

        // Store edges with mapped IDs (batch)
        let mut edges = result.edges;
        let edge_count = db.insert_edges_batch(&mut edges, &id_map)?;
        stats.edges += edge_count;

        // Store unresolved references with mapped IDs (batch)
        let mut unresolved_refs = result.unresolved_refs;
        db.insert_unresolved_refs_batch(&mut unresolved_refs, &id_map)?;

        stats.files += 1;
        stats.nodes += node_count as u64;
        stats.errors += result.errors.len() as u64;
    }

    // Resolve references (unless caller will handle scoped resolution)
    if !config.skip_resolve {
        info!("Resolving references...");
        let resolved = db.resolve_references()?;
        stats.resolved_refs = resolved as u64;
    }

    // Commit transaction
    db.commit()?;

    info!(
        "Indexed {} files, {} nodes, {} edges ({} refs resolved)",
        stats.files, stats.nodes, stats.edges, stats.resolved_refs
    );

    Ok(stats)
}

/// Statistics from an indexing operation
#[derive(Debug, Default)]
pub struct IndexingStats {
    pub files: u64,
    pub nodes: u64,
    pub edges: u64,
    pub skipped: u64,
    pub errors: u64,
    pub resolved_refs: u64,
}
