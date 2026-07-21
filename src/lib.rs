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
pub mod coupling;
#[cfg(feature = "sqlite")]
pub mod db;
pub mod extraction;
#[cfg(feature = "sqlite")]
pub mod graph;
#[cfg(feature = "sqlite")]
pub mod mcp;
#[cfg(feature = "sqlite")]
pub mod ops;
pub mod security;
pub mod types;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::SystemTime;

use anyhow::Result;
use ignore::WalkBuilder;
use indicatif::{ProgressBar, ProgressStyle};

/// Commit and checkpoint the WAL after this many files during bulk indexing.
/// Keeps WAL size bounded without requiring a single massive transaction.
const CHECKPOINT_INTERVAL: usize = 200;

/// Extensions that denote program source code. Used only to report *unsupported*
/// source files encountered during indexing (those whose extension is not in the
/// active `IndexConfig::extensions`) without treating data/markup/config files
/// like `.json`, `.md`, or `.png` as source. Membership here does not imply
/// symgraph can parse the language — support is decided by `IndexConfig`.
#[cfg(feature = "sqlite")]
#[rustfmt::skip]
const SOURCE_CODE_EXTENSIONS: &[&str] = &[
    // C family / systems
    "c", "h", "cpp", "cc", "cxx", "c++", "hpp", "hxx", "hh", "cs", "rs", "go", "zig", "d", "nim",
    "v", "cu", "cuh",
    // JVM / .NET
    "java", "kt", "kts", "scala", "sc", "groovy", "clj", "cljs", "cljc", "vb", "fs", "fsx",
    // scripting / dynamic
    "py", "pyi", "pyw", "rb", "rake", "php", "phtml", "pl", "pm", "lua", "tcl", "r", "jl", "dart",
    "ex", "exs", "erl", "hrl", "cr",
    // JS / TS
    "js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts", "coffee",
    // functional / lisp
    "hs", "lhs", "ml", "mli", "elm", "rkt", "scm", "lisp", "el", "clj",
    // apple
    "swift", "m", "mm",
    // shells
    "sh", "bash", "zsh", "fish", "ps1",
    // db / other
    "sql", "pas", "f90", "f95", "f03", "for",
];

/// Whether `ext` (assumed lowercase) names a program-source-code file type.
#[cfg(feature = "sqlite")]
fn is_source_code_extension(ext: &str) -> bool {
    SOURCE_CODE_EXTENSIONS.contains(&ext)
}

/// Record a file that was skipped because its extension is not indexed, but only
/// when the extension is a recognized source-code type (and not merely a cased
/// variant of an already-supported extension). Feeds the "unsupported source
/// types" report emitted by `index`/`reindex`.
#[cfg(feature = "sqlite")]
fn record_unsupported_source(config: &IndexConfig, counts: &mut BTreeMap<String, u64>, ext: &str) {
    if ext.is_empty() {
        return;
    }
    let ext_lc = ext.to_ascii_lowercase();
    // `.CPP` is not "unsupported" — it's a cased spelling of a supported ext.
    if config
        .extensions
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(&ext_lc))
    {
        return;
    }
    if is_source_code_extension(&ext_lc) {
        *counts.entry(ext_lc).or_insert(0) += 1;
    }
}

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
    /// Render progress bars to stderr during indexing (disable for library/server use)
    pub show_progress: bool,
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
            show_progress: false,
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

/// Indexing behavior for a storage target.
#[cfg(feature = "sqlite")]
#[derive(Clone, Copy)]
enum IndexMode {
    Incremental,
    FullBuild,
}

/// Incrementally index only changed files into an existing database.
#[cfg(feature = "sqlite")]
pub fn index_codebase(db: &mut Database, config: &IndexConfig) -> Result<IndexingStats> {
    run_index_codebase(db, config, IndexMode::Incremental)
}

/// Build a complete index into an empty target database.
#[cfg(feature = "sqlite")]
pub fn build_full_index(db: &mut Database, config: &IndexConfig) -> Result<IndexingStats> {
    run_index_codebase(db, config, IndexMode::FullBuild)
}

#[cfg(feature = "sqlite")]
fn run_index_codebase(
    db: &mut Database,
    config: &IndexConfig,
    mode: IndexMode,
) -> Result<IndexingStats> {
    let root = Path::new(&config.root).canonicalize()?;
    if !root.is_dir() {
        anyhow::bail!("index root is not a directory: {}", root.display());
    }
    info!("Indexing codebase at {}", root.display());

    let mut stats = IndexingStats::default();
    let entries_to_extract = collect_entries(db, config, &root, mode, &mut stats)?;
    let extracted = extract_entries(entries_to_extract, config.show_progress);

    match mode {
        IndexMode::Incremental => store_incremental_index(db, config, extracted, &mut stats)?,
        IndexMode::FullBuild => store_full_index(db, config, extracted, &mut stats)?,
    }

    info!(
        "Indexed {} files, {} nodes, {} edges ({} refs resolved)",
        stats.files, stats.nodes, stats.edges, stats.resolved_refs
    );

    Ok(stats)
}

#[cfg(feature = "sqlite")]
fn collect_entries(
    db: &Database,
    config: &IndexConfig,
    root: &Path,
    mode: IndexMode,
    stats: &mut IndexingStats,
) -> Result<Vec<FileEntry>> {
    let mut walker = WalkBuilder::new(root);
    walker
        .hidden(false)
        .git_ignore(config.respect_gitignore)
        .git_global(config.respect_gitignore)
        .git_exclude(config.respect_gitignore);

    let mut entries_to_extract = Vec::new();
    let scan_pb = if config.show_progress {
        let pb = ProgressBar::new_spinner();
        pb.set_prefix("Scanning");
        pb.enable_steady_tick(std::time::Duration::from_millis(100));
        pb
    } else {
        ProgressBar::hidden()
    };

    for entry in walker.build() {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                warn!("Error walking directory: {}", err);
                continue;
            }
        };

        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        if path.components().any(|component| {
            config
                .exclude_dirs
                .iter()
                .any(|dir| component.as_os_str() == dir.as_str())
        }) {
            continue;
        }

        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let is_manifest = extraction::manifest::is_manifest_file(filename);

        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let language = if is_manifest {
            extraction::manifest::manifest_language(filename)
        } else {
            Language::from_extension(ext)
        };
        // Files with no parser get skipped. Track the extension (when it's a
        // recognized source type) so the CLI can report which source file types
        // were left out of the index.
        if language == Language::Unknown {
            record_unsupported_source(config, &mut stats.unsupported_types, ext);
            continue;
        }

        // A language symgraph can parse, but its extension isn't in the active
        // allow-list — skip it, and report it as an unsupported source type if
        // it names source code the caller might have wanted indexed.
        if !is_manifest
            && !config.extensions.is_empty()
            && !config.extensions.iter().any(|allowed| allowed == ext)
        {
            record_unsupported_source(config, &mut stats.unsupported_types, ext);
            continue;
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(err) => {
                debug!("Failed to read {}: {}", path.display(), err);
                stats.errors += 1;
                continue;
            }
        };

        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let content_hash = hex::encode(hasher.finalize());

        let rel_path = path
            .strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string();

        if matches!(mode, IndexMode::Incremental) && !db.needs_reindex(&rel_path, &content_hash)? {
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

        scan_pb.set_message(format!("{} files queued", entries_to_extract.len() + 1));
        entries_to_extract.push(FileEntry {
            rel_path,
            content,
            content_hash,
            language,
            modified_at,
        });
    }

    scan_pb.finish_and_clear();
    Ok(entries_to_extract)
}

#[cfg(feature = "sqlite")]
fn extract_entries(entries_to_extract: Vec<FileEntry>, show_progress: bool) -> Vec<ExtractedFile> {
    let parse_pb = if show_progress {
        let pb = ProgressBar::new(entries_to_extract.len() as u64);
        pb.set_style(
            ProgressStyle::with_template(
                "  {prefix:<10} [{bar:40.cyan/blue}] {pos:>5}/{len:<5} {msg}",
            )
            .unwrap()
            .progress_chars("=> "),
        );
        pb.set_prefix("Parsing");
        pb
    } else {
        ProgressBar::hidden()
    };

    let extracted: Vec<ExtractedFile> = entries_to_extract
        .into_par_iter()
        .map(|entry| {
            let mut extractor = Extractor::new();
            let result = extractor.extract_file(&entry.rel_path, &entry.content);
            parse_pb.inc(1);
            ExtractedFile { entry, result }
        })
        .collect();
    parse_pb.finish_and_clear();
    extracted
}

#[cfg(feature = "sqlite")]
fn store_incremental_index(
    db: &mut Database,
    config: &IndexConfig,
    extracted: Vec<ExtractedFile>,
    stats: &mut IndexingStats,
) -> Result<()> {
    let total = extracted.len();
    db.begin_transaction()?;
    db.disable_fts_automerge()?;

    let store_pb = make_store_progress_bar(total, config.show_progress);
    for (i, extracted_file) in extracted.into_iter().enumerate() {
        store_extracted_file(db, extracted_file, stats, true, true)?;
        store_pb.inc(1);

        let is_last = i + 1 == total;
        if (i + 1) % CHECKPOINT_INTERVAL == 0 && !is_last {
            db.commit()?;
            db.begin_transaction()?;
        }
    }
    store_pb.finish_and_clear();

    db.optimize_fts()?;
    resolve_references_if_needed(db, config, stats)?;
    db.commit()?;
    Ok(())
}

#[cfg(feature = "sqlite")]
fn store_full_index(
    db: &mut Database,
    config: &IndexConfig,
    extracted: Vec<ExtractedFile>,
    stats: &mut IndexingStats,
) -> Result<()> {
    db.begin_transaction()?;

    let store_pb = make_store_progress_bar(extracted.len(), config.show_progress);
    for extracted_file in extracted {
        store_extracted_file(db, extracted_file, stats, false, false)?;
        store_pb.inc(1);
    }
    store_pb.finish_and_clear();

    resolve_references_if_needed(db, config, stats)?;
    db.disable_fts_automerge()?;
    db.rebuild_fts_indexes()?;
    db.commit_transaction()?;
    Ok(())
}

#[cfg(feature = "sqlite")]
fn make_store_progress_bar(total: usize, show_progress: bool) -> ProgressBar {
    if show_progress {
        let pb = ProgressBar::new(total as u64);
        pb.set_style(
            ProgressStyle::with_template(
                "  {prefix:<10} [{bar:40.cyan/blue}] {pos:>5}/{len:<5} {msg}",
            )
            .unwrap()
            .progress_chars("=> "),
        );
        pb.set_prefix("Storing");
        pb
    } else {
        ProgressBar::hidden()
    }
}

#[cfg(feature = "sqlite")]
fn store_extracted_file(
    db: &Database,
    extracted_file: ExtractedFile,
    stats: &mut IndexingStats,
    delete_existing: bool,
    maintain_fts: bool,
) -> Result<()> {
    let entry = extracted_file.entry;
    let result = extracted_file.result;

    debug!("Indexing: {}", entry.rel_path);
    if delete_existing {
        db.delete_file(&entry.rel_path)?;
    }

    let file_record = build_file_record(&entry, result.nodes.len());
    let node_count = file_record.node_count;
    let error_count = result.errors.len() as u64;
    db.insert_or_update_file(&file_record)?;

    let mut nodes = result.nodes;
    let id_map = if maintain_fts {
        db.insert_nodes_batch(&mut nodes)?
    } else {
        db.insert_nodes_batch_without_fts(&mut nodes)?
    };

    let edge_count = db.insert_edges_batch(&result.edges, &id_map)?;
    stats.edges += edge_count;

    db.insert_unresolved_refs_batch(&result.unresolved_refs, &id_map)?;

    stats.files += 1;
    stats.nodes += node_count as u64;
    stats.errors += error_count;
    Ok(())
}

#[cfg(feature = "sqlite")]
fn build_file_record(entry: &FileEntry, node_count: usize) -> FileRecord {
    FileRecord {
        path: entry.rel_path.clone(),
        content_hash: entry.content_hash.clone(),
        language: entry.language,
        size: entry.content.len() as u64,
        modified_at: entry.modified_at,
        indexed_at: SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        node_count: node_count as u32,
    }
}

#[cfg(feature = "sqlite")]
fn resolve_references_if_needed(
    db: &Database,
    config: &IndexConfig,
    stats: &mut IndexingStats,
) -> Result<()> {
    if config.skip_resolve {
        return Ok(());
    }

    info!("Resolving references...");
    let resolve_pb = if config.show_progress {
        let pb = ProgressBar::new_spinner();
        pb.set_prefix("Resolving");
        pb.set_message("references...");
        pb.enable_steady_tick(std::time::Duration::from_millis(100));
        pb
    } else {
        ProgressBar::hidden()
    };
    let resolved = db.resolve_references()?;
    resolve_pb.finish_and_clear();
    stats.resolved_refs = resolved as u64;
    Ok(())
}

/// Statistics from an indexing operation
#[derive(Debug, Default, serde::Serialize)]
pub struct IndexingStats {
    pub files: u64,
    pub nodes: u64,
    pub edges: u64,
    pub skipped: u64,
    pub errors: u64,
    pub resolved_refs: u64,
    /// Extensions of files that were walked but not indexed because symgraph has
    /// no parser for them, keyed by lowercased extension with an occurrence count.
    /// Files in excluded directories and recognized manifests are not counted.
    pub unsupported_types: BTreeMap<String, u64>,
}
