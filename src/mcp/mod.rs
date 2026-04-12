//! MCP (Model Context Protocol) server implementation
//!
//! Exposes the code graph functionality as MCP tools:
//! - symgraph-context: Build task-specific code context
//! - symgraph-search: Find symbols by name
//! - symgraph-callers: Find all callers of a symbol
//! - symgraph-callees: Find all callees of a symbol
//! - symgraph-impact: Analyze change impact
//! - symgraph-node: Get detailed symbol information
//! - symgraph-status: Get index statistics
//! - symgraph-definition: Get source code of a symbol
//! - symgraph-file: List all symbols in a file
//! - symgraph-references: Find all references to a symbol
//! - symgraph-reindex: Trigger incremental reindexing
//! - symgraph-hierarchy: Get class/module hierarchy
//! - symgraph-path: Find call paths between symbols
//! - symgraph-unused: Find unused/dead code
//! - symgraph-implementations: Find implementations of interfaces/traits
//! - symgraph-diff-impact: Analyze impact of code changes

mod constants;
mod format;
mod handlers;
mod types;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router, ServerHandler,
};

use crate::db::Database;

pub use types::*;

/// Wrapper around `Database` that opts in to `Sync`.
///
/// `rusqlite::Connection` is `!Sync` because it uses `RefCell` internally.
/// We wrap it in an `RwLock` so that access is serialized at the lock level.
///
/// NOTE: concurrent read-locks are technically unsound with raw `RefCell`,
/// but the underlying SQLite C library is compiled with `SQLITE_THREADSAFE=1`
/// (serialized mode) by default, which serializes all access at the C level.
/// If true concurrent reader parallelism is needed in the future, switch to a
/// connection pool (e.g., `r2d2_sqlite`).
pub struct SyncDatabase(pub Database);

// SAFETY: All `Database` access is mediated by an `RwLock`. The underlying
// SQLite library provides its own thread-safety guarantees in serialized mode.
unsafe impl Sync for SyncDatabase {}

impl std::ops::Deref for SyncDatabase {
    type Target = Database;
    fn deref(&self) -> &Database {
        &self.0
    }
}

impl std::ops::DerefMut for SyncDatabase {
    fn deref_mut(&mut self) -> &mut Database {
        &mut self.0
    }
}

/// MCP server handler for symgraph
#[derive(Clone)]
pub struct SymgraphHandler {
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
    db: Arc<RwLock<SyncDatabase>>,
    project_root: String,
    /// Flag indicating whether a background reindex is currently in progress.
    is_reindexing: Arc<AtomicBool>,
}

#[tool_router]
impl SymgraphHandler {
    pub fn new(db: Database, project_root: String) -> Self {
        Self {
            tool_router: Self::tool_router(),
            db: Arc::new(RwLock::new(SyncDatabase(db))),
            project_root,
            is_reindexing: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create a handler with a pre-wrapped database (for sharing across HTTP sessions)
    pub fn new_shared(db: Arc<RwLock<SyncDatabase>>, project_root: String) -> Self {
        Self {
            tool_router: Self::tool_router(),
            db,
            project_root,
            is_reindexing: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Helper to acquire a read lock on the database and run a closure.
    fn with_db<F>(&self, f: F) -> String
    where
        F: FnOnce(&Database) -> Result<String, String>,
    {
        match self.db.read() {
            Ok(guard) => match f(&guard) {
                Ok(output) => output,
                Err(e) => format!("Error: {}", e),
            },
            Err(e) => format!("Error: {}", e),
        }
    }

    /// Helper to acquire a mutable lock on the database and run a closure.
    #[allow(dead_code)]
    fn with_db_mut<F>(&self, f: F) -> String
    where
        F: FnOnce(&mut Database) -> Result<String, String>,
    {
        match self.db.write() {
            Ok(mut guard) => match f(&mut guard) {
                Ok(output) => output,
                Err(e) => format!("Error: {}", e),
            },
            Err(e) => format!("Error: {}", e),
        }
    }

    /// Build focused context for a specific task
    #[tool(
        name = "symgraph-context",
        description = "Build focused code context for a specific task. Returns entry points, related symbols, and code snippets."
    )]
    fn symgraph_context(&self, Parameters(req): Parameters<ContextRequest>) -> String {
        let project_root = &self.project_root;
        self.with_db(|db| handlers::context::handle_context(db, project_root, &req))
    }

    /// Quick symbol search by name
    #[tool(
        name = "symgraph-search",
        description = "Quick symbol search by name. Returns locations only (no code)."
    )]
    fn symgraph_search(&self, Parameters(req): Parameters<SearchRequest>) -> String {
        self.with_db(|db| handlers::search::handle_search(db, &req))
    }

    /// Find all callers of a symbol
    #[tool(
        name = "symgraph-callers",
        description = "Find all functions/methods that call a specific symbol."
    )]
    fn symgraph_callers(&self, Parameters(req): Parameters<SymbolRequest>) -> String {
        self.with_db(|db| handlers::graph::handle_callers(db, &req))
    }

    /// Find all callees of a symbol
    #[tool(
        name = "symgraph-callees",
        description = "Find all functions/methods that a specific symbol calls."
    )]
    fn symgraph_callees(&self, Parameters(req): Parameters<SymbolRequest>) -> String {
        self.with_db(|db| handlers::graph::handle_callees(db, &req))
    }

    /// Analyze the impact of changing a symbol
    #[tool(
        name = "symgraph-impact",
        description = "Analyze the impact radius of changing a symbol."
    )]
    fn symgraph_impact(&self, Parameters(req): Parameters<SymbolRequest>) -> String {
        self.with_db(|db| handlers::graph::handle_impact(db, &req))
    }

    /// Get the full source code definition of a symbol
    #[tool(
        name = "symgraph-definition",
        description = "Get the full source code of a symbol. Returns the complete definition with surrounding context lines."
    )]
    fn symgraph_definition(&self, Parameters(req): Parameters<DefinitionRequest>) -> String {
        let project_root = &self.project_root;
        self.with_db(|db| handlers::symbol::handle_definition(db, project_root, &req))
    }

    /// List all symbols in a specific file
    #[tool(
        name = "symgraph-file",
        description = "List all symbols defined in a specific file. Returns functions, classes, methods, etc."
    )]
    fn symgraph_file(&self, Parameters(req): Parameters<FileRequest>) -> String {
        self.with_db(|db| handlers::file::handle_file(db, &req))
    }

    /// Find all references to a symbol
    #[tool(
        name = "symgraph-references",
        description = "Find all references to a symbol including calls, imports, type usages, and other relationships."
    )]
    fn symgraph_references(&self, Parameters(req): Parameters<SymbolRequest>) -> String {
        self.with_db(|db| handlers::symbol::handle_references(db, &req))
    }

    /// Trigger incremental reindexing (runs in background, returns immediately)
    #[tool(
        name = "symgraph-reindex",
        description = "Trigger incremental reindexing of the codebase. Only changed files are re-parsed. Runs in background and returns immediately."
    )]
    fn symgraph_reindex(&self, Parameters(req): Parameters<ReindexRequest>) -> String {
        // If a reindex is already running, refuse to start another one.
        if self
            .is_reindexing
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return "Reindex already in progress. Use symgraph-status to check.".to_string();
        }

        let db = Arc::clone(&self.db);
        let project_root = self.project_root.clone();
        let is_reindexing = Arc::clone(&self.is_reindexing);

        let file_count_hint = req.files.as_ref().map(|f| f.len());

        tokio::task::spawn_blocking(move || {
            let result = match db.write() {
                Ok(mut guard) => {
                    match handlers::reindex::handle_reindex(&mut guard, &project_root, &req) {
                        Ok(output) => output,
                        Err(e) => format!("Error: {}", e),
                    }
                }
                Err(e) => format!("Error acquiring write lock: {}", e),
            };
            is_reindexing.store(false, Ordering::SeqCst);
            tracing::info!("Background reindex finished: {}", result);
        });

        match file_count_hint {
            Some(n) => format!(
                "Reindexing {} file(s) in background. Use symgraph-status to check progress.",
                n
            ),
            None => {
                "Reindexing all changed files in background. Use symgraph-status to check progress."
                    .to_string()
            }
        }
    }

    /// Get detailed information about a symbol
    #[tool(
        name = "symgraph-node",
        description = "Get detailed information about a specific code symbol."
    )]
    fn symgraph_node(&self, Parameters(req): Parameters<SymbolRequest>) -> String {
        self.with_db(|db| handlers::symbol::handle_node(db, &req))
    }

    /// Get index statistics
    #[tool(
        name = "symgraph-status",
        description = "Get the status of the symgraph index. Shows statistics about indexed files, symbols, and relationships."
    )]
    fn symgraph_status(&self) -> String {
        let reindexing = self.is_reindexing.load(Ordering::SeqCst);
        self.with_db(|db| {
            let mut output = handlers::status::handle_status(db)?;
            if reindexing {
                output.push_str("\n**Reindex:** In progress\n");
            }
            Ok(output)
        })
    }

    /// Get class/module hierarchy
    #[tool(
        name = "symgraph-hierarchy",
        description = "Get the hierarchy of a symbol showing parent/child contains relationships (e.g., class contains methods)."
    )]
    fn symgraph_hierarchy(&self, Parameters(req): Parameters<SymbolRequest>) -> String {
        self.with_db(|db| handlers::hierarchy::handle_hierarchy(db, &req))
    }

    /// Find call path between two symbols
    #[tool(
        name = "symgraph-path",
        description = "Find call paths from one symbol to another. Shows how function A reaches function B through intermediate calls."
    )]
    fn symgraph_path(&self, Parameters(req): Parameters<PathRequest>) -> String {
        self.with_db(|db| handlers::path::handle_path(db, &req))
    }

    /// Find unused/dead code
    #[tool(
        name = "symgraph-unused",
        description = "Find unused symbols (functions, methods, classes) with no incoming references. Helps identify dead code."
    )]
    fn symgraph_unused(&self) -> String {
        self.with_db(handlers::unused::handle_unused)
    }

    /// Find implementations of an interface/trait
    #[tool(
        name = "symgraph-implementations",
        description = "Find all classes/structs that implement an interface or extend a trait/class."
    )]
    fn symgraph_implementations(&self, Parameters(req): Parameters<SymbolRequest>) -> String {
        self.with_db(|db| handlers::implementations::handle_implementations(db, &req))
    }

    /// Analyze impact of code changes
    #[tool(
        name = "symgraph-diff-impact",
        description = "Analyze the impact of changing a specific region of code. Shows directly modified symbols and their callers."
    )]
    fn symgraph_diff_impact(&self, Parameters(req): Parameters<DiffImpactRequest>) -> String {
        self.with_db(|db| handlers::diff_impact::handle_diff_impact(db, &req))
    }
}

#[tool_handler]
impl ServerHandler for SymgraphHandler {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build());
        info.instructions = Some(
            "symgraph provides semantic code intelligence for exploring codebases. \
            Use symgraph-context to build task-focused context, symgraph-search for quick lookups, \
            symgraph-callers/callees/impact for understanding code relationships, \
            symgraph-definition to view source code, symgraph-file to list symbols in a file, \
            symgraph-references for all usages of a symbol, symgraph-hierarchy for class/module structure, \
            symgraph-path to find call paths between functions, symgraph-unused to find dead code, \
            symgraph-implementations to find interface/trait implementations, \
            symgraph-diff-impact to analyze change impact, and symgraph-reindex to refresh after edits."
                .into(),
        );
        info
    }
}
