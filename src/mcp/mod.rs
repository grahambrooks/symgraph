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
//! - symgraph-blame: Git blame a symbol's definition
//! - symgraph-churn: File change frequency (volatility)
//! - symgraph-module-graph: Dependency graph folded to a file/dir/module boundary
//! - symgraph-coupling-score: Rank coupling on strength × distance × volatility
//! - symgraph-god-struct: Rank structs by architectural debt
//! - symgraph-dispatch-sites: Find where an enum is matched (control coupling)

/// Tool handlers, shared by the MCP server and the CLI (`cli::tools`).
pub mod handlers;
mod types;

pub use types::*;

// Everything below is the MCP server itself (the `rmcp`-driven handler and its
// `Sync` database wrapper). It is gated behind the `server` feature so CLI-only
// builds — which use `handlers`/`types` directly — need not link rmcp/tokio.
#[cfg(feature = "server")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "server")]
use std::sync::{Arc, Mutex};

#[cfg(feature = "server")]
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router, ServerHandler,
};

#[cfg(feature = "server")]
use crate::db::Database;

/// The shared database handle used by every session of a running server.
///
/// A `Mutex` rather than an `RwLock`, which lets the `unsafe impl Sync` this
/// previously needed go away entirely. `rusqlite::Connection` is `Send` but
/// `!Sync`, because it keeps a `RefCell` statement cache; an `RwLock` hands
/// out `&Database` to several readers at once, and two of them calling
/// `prepare_cached` is a data race on that `RefCell` — undefined behaviour,
/// not merely a panic. The old comment credited SQLite's serialized mode for
/// safety, but that protects the C library, not the Rust-side cache.
///
/// `Mutex<Database>` is `Sync` on its own terms because `Database: Send`, and
/// nothing is lost: SQLite serializes access anyway. Real reader parallelism
/// needs a connection pool (e.g. `r2d2_sqlite`), which is a separate change.
#[cfg(feature = "server")]
pub type SharedDatabase = Arc<Mutex<Database>>;

/// MCP server handler for symgraph
#[cfg(feature = "server")]
#[derive(Clone)]
pub struct SymgraphHandler {
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
    db: SharedDatabase,
    project_root: String,
    /// Flag indicating whether a background reindex is currently in progress.
    is_reindexing: Arc<AtomicBool>,
}

#[cfg(feature = "server")]
#[tool_router]
impl SymgraphHandler {
    pub fn new(db: Database, project_root: String) -> Self {
        Self {
            tool_router: Self::tool_router(),
            db: Arc::new(Mutex::new(db)),
            project_root,
            is_reindexing: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create a handler over a pre-wrapped database shared with other sessions.
    ///
    /// `is_reindexing` is passed in rather than created here: the HTTP
    /// transport builds a fresh handler per session over the *same* database,
    /// so a per-handler flag would let each session start its own rebuild and
    /// pile up on the write lock. The guard is only a guard when every handler
    /// sharing a database shares the flag.
    pub fn new_shared(
        db: SharedDatabase,
        project_root: String,
        is_reindexing: Arc<AtomicBool>,
    ) -> Self {
        Self {
            tool_router: Self::tool_router(),
            db,
            project_root,
            is_reindexing,
        }
    }

    /// Run a database-backed handler on the blocking pool.
    ///
    /// Every handler does synchronous SQLite work, and some of it — a coupling
    /// score over a large repo, a `git blame` on a slow filesystem — takes
    /// seconds. Running that directly in an async fn blocks a tokio worker
    /// thread, and over HTTP that stalls every other session too. The
    /// `spawn_blocking` pool exists for exactly this.
    ///
    /// The lock is taken *inside* the closure so it is acquired and released
    /// on the blocking thread, never held across an await point.
    async fn blocking_db<F>(&self, f: F) -> Result<String, String>
    where
        F: FnOnce(&Database) -> Result<String, String> + Send + 'static,
    {
        let db = Arc::clone(&self.db);
        tokio::task::spawn_blocking(move || {
            let guard = db
                .lock()
                .map_err(|e| format!("database lock poisoned: {}", e))?;
            f(&guard)
        })
        .await
        .map_err(|e| format!("tool task failed: {}", e))?
    }

    /// Run work that needs no database access on the blocking pool.
    async fn blocking<F>(f: F) -> Result<String, String>
    where
        F: FnOnce() -> Result<String, String> + Send + 'static,
    {
        tokio::task::spawn_blocking(f)
            .await
            .map_err(|e| format!("tool task failed: {}", e))?
    }

    /// Build focused context for a specific task
    #[tool(
        name = "symgraph-context",
        description = "Build focused code context for a specific task. Returns entry points, related symbols, and code snippets."
    )]
    async fn symgraph_context(
        &self,
        Parameters(req): Parameters<ContextRequest>,
    ) -> Result<String, String> {
        let project_root = self.project_root.clone();
        self.blocking_db(move |db| handlers::context::handle_context(db, &project_root, &req))
            .await
    }

    /// Quick symbol search by name
    #[tool(
        name = "symgraph-search",
        description = "Quick symbol search by name. Returns locations only (no code)."
    )]
    async fn symgraph_search(
        &self,
        Parameters(req): Parameters<SearchRequest>,
    ) -> Result<String, String> {
        self.blocking_db(move |db| handlers::search::handle_search(db, &req))
            .await
    }

    /// Find all callers of a symbol
    #[tool(
        name = "symgraph-callers",
        description = "Find all functions/methods that call a specific symbol."
    )]
    async fn symgraph_callers(
        &self,
        Parameters(req): Parameters<SymbolRequest>,
    ) -> Result<String, String> {
        self.blocking_db(move |db| handlers::graph::handle_callers(db, &req))
            .await
    }

    /// Find all callees of a symbol
    #[tool(
        name = "symgraph-callees",
        description = "Find all functions/methods that a specific symbol calls."
    )]
    async fn symgraph_callees(
        &self,
        Parameters(req): Parameters<SymbolRequest>,
    ) -> Result<String, String> {
        self.blocking_db(move |db| handlers::graph::handle_callees(db, &req))
            .await
    }

    /// Analyze the impact of changing a symbol
    #[tool(
        name = "symgraph-impact",
        description = "Analyze the impact of changing a symbol. Breaks inbound coupling down by edge kind (method-call/contract, field-read/model, field-write/intrusive), counts inbound modules, and (with churn=true) annotates volatility. Supports format='json'."
    )]
    async fn symgraph_impact(
        &self,
        Parameters(req): Parameters<ImpactRequest>,
    ) -> Result<String, String> {
        let project_root = self.project_root.clone();
        self.blocking_db(move |db| handlers::graph::handle_impact(db, &project_root, &req))
            .await
    }

    /// Get the full source code definition of a symbol
    #[tool(
        name = "symgraph-definition",
        description = "Get the full source code of a symbol. Returns the complete definition with surrounding context lines."
    )]
    async fn symgraph_definition(
        &self,
        Parameters(req): Parameters<DefinitionRequest>,
    ) -> Result<String, String> {
        let project_root = self.project_root.clone();
        self.blocking_db(move |db| handlers::symbol::handle_definition(db, &project_root, &req))
            .await
    }

    /// List all symbols in a specific file
    #[tool(
        name = "symgraph-file",
        description = "List all symbols defined in a specific file. Returns functions, classes, methods, etc."
    )]
    async fn symgraph_file(
        &self,
        Parameters(req): Parameters<FileRequest>,
    ) -> Result<String, String> {
        self.blocking_db(move |db| handlers::file::handle_file(db, &req))
            .await
    }

    /// Find all references to a symbol
    #[tool(
        name = "symgraph-references",
        description = "Find all references to a symbol including calls, imports, type usages, and other relationships."
    )]
    async fn symgraph_references(
        &self,
        Parameters(req): Parameters<SymbolRequest>,
    ) -> Result<String, String> {
        self.blocking_db(move |db| handlers::symbol::handle_references(db, &req))
            .await
    }

    /// Trigger background reindexing (runs in background, returns immediately)
    #[tool(
        name = "symgraph-reindex",
        description = "Trigger reindexing of the codebase. When files are provided, only those files are updated in place; otherwise a full shadow rebuild runs in the background."
    )]
    fn symgraph_reindex(
        &self,
        Parameters(req): Parameters<ReindexRequest>,
    ) -> Result<String, String> {
        // If a reindex is already running, refuse to start another one.
        if self
            .is_reindexing
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Ok("Reindex already in progress. Use symgraph-status to check.".to_string());
        }

        let db = Arc::clone(&self.db);
        let project_root = self.project_root.clone();
        let is_reindexing = Arc::clone(&self.is_reindexing);

        let file_count_hint = req.files.as_ref().map(|f| f.len());

        tokio::task::spawn_blocking(move || {
            let result = match db.lock() {
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

        Ok(match file_count_hint {
            Some(n) => format!(
                "Reindexing {} file(s) in background. Use symgraph-status to check progress.",
                n
            ),
            None => {
                "Rebuilding the full index in background. Use symgraph-status to check progress."
                    .to_string()
            }
        })
    }

    /// Get detailed information about a symbol
    #[tool(
        name = "symgraph-node",
        description = "Get detailed information about a specific code symbol."
    )]
    async fn symgraph_node(
        &self,
        Parameters(req): Parameters<SymbolRequest>,
    ) -> Result<String, String> {
        self.blocking_db(move |db| handlers::symbol::handle_node(db, &req))
            .await
    }

    /// Get index statistics
    #[tool(
        name = "symgraph-status",
        description = "Get the status of the symgraph index. Shows statistics about indexed files, symbols, and relationships."
    )]
    async fn symgraph_status(
        &self,
        Parameters(req): Parameters<FormatRequest>,
    ) -> Result<String, String> {
        let reindexing = self.is_reindexing.load(Ordering::SeqCst);
        let json = crate::mcp::types::wants_json(&req.format);
        self.blocking_db(move |db| {
            let mut output = handlers::status::handle_status(db, req.format.clone())?;
            // The in-progress note is a markdown affordance; the JSON result
            // has its own shape and must stay parseable.
            if reindexing && !json {
                output.push_str("\n**Reindex:** In progress\n");
            }
            Ok(output)
        })
        .await
    }

    /// Get class/module hierarchy
    #[tool(
        name = "symgraph-hierarchy",
        description = "Get the hierarchy of a symbol showing parent/child contains relationships (e.g., class contains methods)."
    )]
    async fn symgraph_hierarchy(
        &self,
        Parameters(req): Parameters<SymbolRequest>,
    ) -> Result<String, String> {
        self.blocking_db(move |db| handlers::hierarchy::handle_hierarchy(db, &req))
            .await
    }

    /// Find call path between two symbols
    #[tool(
        name = "symgraph-path",
        description = "Find call paths from one symbol to another. Shows how function A reaches function B through intermediate calls."
    )]
    async fn symgraph_path(
        &self,
        Parameters(req): Parameters<PathRequest>,
    ) -> Result<String, String> {
        self.blocking_db(move |db| handlers::path::handle_path(db, &req))
            .await
    }

    /// Find unused/dead code
    #[tool(
        name = "symgraph-unused",
        description = "Find unused symbols (functions, methods, classes) with no incoming references. Helps identify dead code."
    )]
    async fn symgraph_unused(
        &self,
        Parameters(req): Parameters<FormatRequest>,
    ) -> Result<String, String> {
        self.blocking_db(move |db| handlers::unused::handle_unused(db, &req))
            .await
    }

    /// Find implementations of an interface/trait
    #[tool(
        name = "symgraph-implementations",
        description = "Find all classes/structs that implement an interface or extend a trait/class."
    )]
    async fn symgraph_implementations(
        &self,
        Parameters(req): Parameters<SymbolRequest>,
    ) -> Result<String, String> {
        self.blocking_db(move |db| handlers::implementations::handle_implementations(db, &req))
            .await
    }

    /// Analyze impact of code changes
    #[tool(
        name = "symgraph-diff-impact",
        description = "Analyze the impact of changing a specific region of code. Shows directly modified symbols and their callers."
    )]
    async fn symgraph_diff_impact(
        &self,
        Parameters(req): Parameters<DiffImpactRequest>,
    ) -> Result<String, String> {
        let project_root = self.project_root.clone();
        self.blocking_db(move |db| {
            handlers::diff_impact::handle_diff_impact(db, &project_root, &req)
        })
        .await
    }

    /// Git blame a symbol's definition lines
    #[tool(
        name = "symgraph-blame",
        description = "Run git blame over the lines of a symbol's definition. Shows who last changed each line and when."
    )]
    async fn symgraph_blame(
        &self,
        Parameters(req): Parameters<BlameRequest>,
    ) -> Result<String, String> {
        let project_root = self.project_root.clone();
        self.blocking_db(move |db| handlers::blame::handle_blame(db, &project_root, &req))
            .await
    }

    /// Git churn / change-frequency analysis
    #[tool(
        name = "symgraph-churn",
        description = "Show file change frequency (churn) over a recent window. Highlights hotspots most likely to harbor bugs."
    )]
    async fn symgraph_churn(
        &self,
        Parameters(req): Parameters<ChurnRequest>,
    ) -> Result<String, String> {
        let project_root = self.project_root.clone();
        // No database access, but it still shells out to git — which is
        // exactly the kind of multi-second blocking call the reactor must not
        // be asked to wait on.
        Self::blocking(move || handlers::churn::handle_churn(&project_root, &req)).await
    }

    /// Module dependency graph: fan-in/out and cycles at a chosen boundary
    #[tool(
        name = "symgraph-module-graph",
        description = "Aggregate the resolved graph to a file/dir/module boundary. Returns the dependency adjacency list with edge counts, fan-in/fan-out per node, and detected cycles (SCCs). Supports format='json'. Reindex after edits."
    )]
    async fn symgraph_module_graph(
        &self,
        Parameters(req): Parameters<ModuleGraphRequest>,
    ) -> Result<String, String> {
        let project_root = self.project_root.clone();
        self.blocking_db(move |db| {
            handlers::module_graph::handle_module_graph(db, &project_root, &req)
        })
        .await
    }

    /// Coupling score: strength × distance × volatility per module pair
    #[tool(
        name = "symgraph-coupling-score",
        description = "Rank module-pair coupling on strength (contract/model/intrusive) × distance × volatility (churn). Produces the hotspots table directly. Supports format='json'. Reindex after edits."
    )]
    async fn symgraph_coupling_score(
        &self,
        Parameters(req): Parameters<ModuleGraphRequest>,
    ) -> Result<String, String> {
        let project_root = self.project_root.clone();
        self.blocking_db(move |db| {
            handlers::module_graph::handle_coupling_score(db, &project_root, &req)
        })
        .await
    }

    /// God-struct / hub report: structs ranked by architectural debt
    #[tool(
        name = "symgraph-god-struct",
        description = "Rank structs/classes by pub-field count × inbound-reference count × churn — the 'where is the architectural debt' entry point. Supports format='json'."
    )]
    async fn symgraph_god_struct(
        &self,
        Parameters(req): Parameters<GodStructRequest>,
    ) -> Result<String, String> {
        let project_root = self.project_root.clone();
        self.blocking_db(move |db| handlers::god_struct::handle_god_struct(db, &project_root, &req))
            .await
    }

    /// Dispatch sites: files that match/switch on an enum's members
    #[tool(
        name = "symgraph-dispatch-sites",
        description = "Find every file that dispatches on a member of the given enum (control coupling). Verifies completeness before a trait/strategy refactor. Supports format='json'."
    )]
    async fn symgraph_dispatch_sites(
        &self,
        Parameters(req): Parameters<DispatchSitesRequest>,
    ) -> Result<String, String> {
        self.blocking_db(move |db| handlers::dispatch_sites::handle_dispatch_sites(db, &req))
            .await
    }
}

#[cfg(feature = "server")]
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
            symgraph-diff-impact to analyze change impact, symgraph-blame and symgraph-churn for \
            git history/volatility, and symgraph-reindex to refresh after edits. \
            For coupling analysis: symgraph-module-graph aggregates dependencies to a file/dir/module \
            boundary with fan-in/out and cycles; symgraph-coupling-score ranks hotspots on \
            strength × distance × volatility; symgraph-god-struct surfaces architectural debt; and \
            symgraph-dispatch-sites finds where an enum is matched. Coupling tools rely on field/import/ \
            dispatch edges, so run symgraph-reindex after code changes."
                .into(),
        );
        info
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    /// The HTTP transport builds one handler per session over a shared
    /// database. If each got its own `is_reindexing`, the "already in
    /// progress" guard would be invisible to every other session and two
    /// clients could kick off concurrent full rebuilds.
    #[test]
    fn shared_handlers_share_the_reindex_guard() {
        let db = Arc::new(Mutex::new(Database::in_memory().unwrap()));
        let flag = Arc::new(AtomicBool::new(false));

        let session_a = SymgraphHandler::new_shared(db.clone(), ".".to_string(), flag.clone());
        let session_b = SymgraphHandler::new_shared(db.clone(), ".".to_string(), flag.clone());

        assert!(Arc::ptr_eq(
            &session_a.is_reindexing,
            &session_b.is_reindexing
        ));

        session_a.is_reindexing.store(true, Ordering::SeqCst);
        assert!(
            session_b.is_reindexing.load(Ordering::SeqCst),
            "a reindex started in one session must be visible to the others"
        );
    }

    /// A handler that owns its database is the only user of it, so it owns the
    /// guard too.
    #[test]
    fn owned_handler_gets_its_own_guard() {
        let handler = SymgraphHandler::new(Database::in_memory().unwrap(), ".".to_string());
        assert!(!handler.is_reindexing.load(Ordering::SeqCst));
    }

    /// A failing tool must reach the client as an MCP error, not as a
    /// successful result whose text happens to begin "Error:". rmcp turns the
    /// `Err` arm into a result with `is_error` set; the `Ok` arm must not.
    #[test]
    fn tool_failure_becomes_an_mcp_error_result() {
        use rmcp::handler::server::tool::IntoCallToolResult;

        let ok = Ok::<String, String>("fine".to_string())
            .into_call_tool_result()
            .unwrap();
        assert_ne!(ok.is_error, Some(true));

        let err = Err::<String, String>("boom".to_string())
            .into_call_tool_result()
            .unwrap();
        assert_eq!(
            err.is_error,
            Some(true),
            "a failed tool call must be flagged so clients can tell it apart from output"
        );
    }

    /// The definition tool rejects a traversal path from the database, and
    /// that rejection has to surface as a failure rather than as text.
    #[test]
    fn handler_errors_propagate_rather_than_stringify() {
        let handler = SymgraphHandler::new(Database::in_memory().unwrap(), ".".to_string());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result =
            runtime.block_on(handler.blocking_db(|_db| Err("something broke".to_string())));
        assert_eq!(result, Err("something broke".to_string()));
    }
}
