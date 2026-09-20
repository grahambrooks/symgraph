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
pub mod git;
#[cfg(feature = "sqlite")]
pub mod graph;
#[cfg(feature = "sqlite")]
pub mod mcp;
#[cfg(feature = "sqlite")]
pub mod ops;
pub mod security;
pub mod types;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;
use ignore::WalkBuilder;
use indicatif::{ProgressBar, ProgressStyle};

/// Default ceiling on the size of a file symgraph will parse (2 MiB). Hand-
/// written source almost never approaches this; generated and minified files
/// routinely exceed it.
pub const DEFAULT_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// How many files to read, parse and store before moving on.
///
/// The bound that makes indexing streaming rather than whole-repository: peak
/// memory is this many file contents plus their extracted nodes and edges,
/// regardless of how large the tree is.
#[cfg(feature = "sqlite")]
const INDEX_CHUNK_SIZE: usize = 256;

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

/// Rewrite a native path to the forward-slash form the index is keyed on.
/// A no-op on platforms that already use `/`.
#[cfg(feature = "sqlite")]
fn normalize_separators(path: &str) -> String {
    if std::path::MAIN_SEPARATOR == '/' {
        path.to_string()
    } else {
        path.replace(std::path::MAIN_SEPARATOR, "/")
    }
}

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
    /// Restrict indexing to these repo-relative paths.
    ///
    /// The walk still runs — it is the only thing that knows which files
    /// exist and are not ignored — but everything outside the set is dropped
    /// before it is read, hashed or parsed. Without this, reindexing one file
    /// costs a full incremental pass over the tree.
    pub only_files: Option<Vec<String>>,
    /// Render progress bars to stderr during indexing (disable for library/server use)
    pub show_progress: bool,
    /// Treat a file whose size and mtime match the index as unchanged, without
    /// reading or hashing it.
    ///
    /// On by default: reading and SHA-256ing every file is the dominant cost
    /// of a no-op incremental run. Turn it off when mtimes cannot be trusted —
    /// a checkout that preserves them across a content change would otherwise
    /// be missed until the next full rebuild.
    pub trust_file_stat: bool,
    /// Skip files larger than this many bytes.
    ///
    /// One generated or minified file — a bundled `.js`, a vendored amalgamation —
    /// can be tens of megabytes on a single line. tree-sitter will parse it, at a
    /// cost in time and memory out of all proportion to the symbols it yields,
    /// and nothing else in the pipeline bounds it.
    pub max_file_bytes: u64,
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
            only_files: None,
            show_progress: false,
            trust_file_stat: true,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        }
    }
}

/// A file the walk selected, before anything has been read.
#[cfg(feature = "sqlite")]
struct Candidate {
    path: PathBuf,
    rel_path: String,
    language: Language,
    modified_at: i64,
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

    // An incremental pass only touches changed files, so running one over a
    // stale index leaves the untouched rows in their old semantics and mixes
    // the two. Refuse rather than silently produce a half-migrated index.
    if matches!(mode, IndexMode::Incremental) {
        if let Some(stale) = db.staleness()? {
            anyhow::bail!(
                "cannot incrementally index a stale index ({}). Run a full rebuild: \
                 `symgraph reindex`.",
                stale
            );
        }
    }

    let mut stats = IndexingStats::default();
    let candidates = collect_candidates(db, config, &root, mode, &mut stats)?;

    match mode {
        IndexMode::Incremental => store_incremental_index(db, config, candidates, &mut stats)?,
        IndexMode::FullBuild => store_full_index(db, config, candidates, &mut stats)?,
    }

    info!(
        "Indexed {} files, {} nodes, {} edges ({} refs resolved)",
        stats.files, stats.nodes, stats.edges, stats.resolved_refs
    );

    Ok(stats)
}

/// Walk the tree and list the files worth looking at, *without* reading any of
/// them.
///
/// Collecting only metadata here is what lets indexing stream: the walk used
/// to read every file into a `Vec<FileEntry>` and hand the whole repository's
/// contents to the extractor at once, so peak memory was the source tree plus
/// the entire graph. Now the walk is cheap and the contents are pulled in a
/// chunk at a time.
#[cfg(feature = "sqlite")]
fn collect_candidates(
    db: &Database,
    config: &IndexConfig,
    root: &Path,
    mode: IndexMode,
    stats: &mut IndexingStats,
) -> Result<Vec<Candidate>> {
    let mut walker = WalkBuilder::new(root);
    walker
        .hidden(false)
        .git_ignore(config.respect_gitignore)
        .git_global(config.respect_gitignore)
        .git_exclude(config.respect_gitignore);

    let mut candidates = Vec::new();
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

        let metadata = entry.metadata().ok();
        let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);

        // Check size from the directory entry, before reading: the point is
        // to not pull a 40 MB bundle into memory in the first place.
        if config.max_file_bytes > 0 && size > config.max_file_bytes {
            debug!(
                "Skipping {} ({} bytes exceeds max_file_bytes {})",
                path.display(),
                size,
                config.max_file_bytes
            );
            stats.skipped_too_large += 1;
            continue;
        }

        let modified_at = metadata
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // Key the index on forward slashes everywhere. `display()` emits the
        // platform separator, which on Windows stored `src\foo.rs` while every
        // MCP client and CLI caller passes `src/foo.rs` — so lookups by path
        // silently found nothing. This is the one place paths enter the index,
        // so normalizing here covers every downstream consumer.
        let rel_path = normalize_separators(
            &path
                .strip_prefix(root)
                .unwrap_or(path)
                .display()
                .to_string(),
        );

        // An explicit file list drops everything else before it is read.
        if let Some(only) = &config.only_files {
            if !only.iter().any(|f| f == &rel_path) {
                continue;
            }
        }

        // Incremental only: if size and mtime both match what was indexed,
        // take the file as unchanged without reading or hashing it. Reading
        // and SHA-256ing every file on every pass is the dominant cost of a
        // no-op incremental run, and this skips almost all of it. The content
        // hash still decides for anything whose stat differs, so an edit that
        // preserves both size and mtime is the only thing this can miss —
        // set `trust_file_stat = false` (or run `reindex`) if that matters.
        if matches!(mode, IndexMode::Incremental)
            && config.trust_file_stat
            && db.matches_indexed_stat(&rel_path, size, modified_at)?
        {
            debug!("Skipping unchanged file (stat): {}", rel_path);
            stats.skipped += 1;
            continue;
        }

        scan_pb.set_message(format!("{} files queued", candidates.len() + 1));
        candidates.push(Candidate {
            path: path.to_path_buf(),
            rel_path,
            language,
            modified_at,
        });
    }

    scan_pb.finish_and_clear();
    Ok(candidates)
}

/// Read, hash and extract one chunk of candidates in parallel.
///
/// Returns only the files that actually need storing: anything whose content
/// hash matches the index is counted as skipped and dropped here, so it never
/// reaches the store phase.
#[cfg(feature = "sqlite")]
fn extract_chunk(
    db: &Database,
    chunk: &[Candidate],
    mode: IndexMode,
    stats: &mut IndexingStats,
    progress: &ProgressBar,
) -> Result<Vec<ExtractedFile>> {
    // Read and hash in parallel; no database access in here.
    let read: Vec<Option<FileEntry>> = chunk
        .par_iter()
        .map(|candidate| {
            let content = std::fs::read_to_string(&candidate.path).ok()?;
            let mut hasher = Sha256::new();
            hasher.update(content.as_bytes());
            Some(FileEntry {
                rel_path: candidate.rel_path.clone(),
                content,
                content_hash: hex::encode(hasher.finalize()),
                language: candidate.language,
                modified_at: candidate.modified_at,
            })
        })
        .collect();

    // Decide what still needs indexing. Sequential because `Database` is not
    // `Sync`, but it is only a hash comparison per file.
    let mut to_extract = Vec::new();
    for (candidate, entry) in chunk.iter().zip(read) {
        match entry {
            Some(entry) => {
                if matches!(mode, IndexMode::Incremental)
                    && !db.needs_reindex(&entry.rel_path, &entry.content_hash)?
                {
                    debug!("Skipping unchanged file: {}", entry.rel_path);
                    stats.skipped += 1;
                    continue;
                }
                to_extract.push(entry);
            }
            None => {
                debug!("Failed to read {}", candidate.path.display());
                stats.errors += 1;
            }
        }
    }

    // Parse in parallel.
    let extracted: Vec<ExtractedFile> = to_extract
        .into_par_iter()
        .map(|entry| {
            let mut extractor = Extractor::new();
            let result = extractor.extract_file(&entry.rel_path, &entry.content);
            progress.inc(1);
            ExtractedFile { entry, result }
        })
        .collect();
    Ok(extracted)
}

/// Index in bounded chunks: read, parse and store `INDEX_CHUNK_SIZE` files at
/// a time so peak memory stays proportional to the chunk, not to the
/// repository.
#[cfg(feature = "sqlite")]
fn store_incremental_index(
    db: &mut Database,
    config: &IndexConfig,
    candidates: Vec<Candidate>,
    stats: &mut IndexingStats,
) -> Result<()> {
    let progress = make_store_progress_bar(candidates.len(), config.show_progress);
    db.begin_transaction()?;
    db.disable_fts_automerge()?;

    let mut since_checkpoint = 0usize;
    for chunk in candidates.chunks(INDEX_CHUNK_SIZE) {
        let extracted = extract_chunk(db, chunk, IndexMode::Incremental, stats, &progress)?;
        for extracted_file in extracted {
            store_extracted_file(db, extracted_file, stats, true, true)?;
            since_checkpoint += 1;
        }
        // Keep the WAL bounded without wrapping the whole run in one
        // transaction.
        if since_checkpoint >= CHECKPOINT_INTERVAL {
            db.commit()?;
            db.begin_transaction()?;
            since_checkpoint = 0;
        }
    }
    progress.finish_and_clear();

    db.optimize_fts()?;
    resolve_references_if_needed(db, config, stats)?;
    db.record_index_version()?;
    db.commit()?;
    Ok(())
}

#[cfg(feature = "sqlite")]
fn store_full_index(
    db: &mut Database,
    config: &IndexConfig,
    candidates: Vec<Candidate>,
    stats: &mut IndexingStats,
) -> Result<()> {
    let progress = make_store_progress_bar(candidates.len(), config.show_progress);
    db.begin_transaction()?;

    for chunk in candidates.chunks(INDEX_CHUNK_SIZE) {
        let extracted = extract_chunk(db, chunk, IndexMode::FullBuild, stats, &progress)?;
        for extracted_file in extracted {
            store_extracted_file(db, extracted_file, stats, false, false)?;
        }
    }
    progress.finish_and_clear();

    resolve_references_if_needed(db, config, stats)?;
    db.disable_fts_automerge()?;
    db.rebuild_fts_indexes()?;
    // Stamp provenance last: a build that fails before this point leaves an
    // index that correctly reads as stale rather than as current-but-partial.
    db.record_index_version()?;
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
    let parse_failed = result.parse_failed;
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
    if parse_failed {
        stats.parse_failures += 1;
    }
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
    /// Files that parsed only partially because they contain syntax errors.
    /// Their symbols are in the index but incomplete.
    pub parse_failures: u64,
    /// Files skipped for exceeding `IndexConfig::max_file_bytes`.
    pub skipped_too_large: u64,
}
