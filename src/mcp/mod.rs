//! MCP (Model Context Protocol) server implementation
//!
//! Exposes the code graph functionality as MCP tools:
//! - codemap-context: Build task-specific code context
//! - codemap-search: Find symbols by name
//! - codemap-callers: Find all callers of a symbol
//! - codemap-callees: Find all callees of a symbol
//! - codemap-impact: Analyze change impact
//! - codemap-node: Get detailed symbol information
//! - codemap-status: Get index statistics
//! - codemap-definition: Get source code of a symbol
//! - codemap-file: List all symbols in a file
//! - codemap-references: Find all references to a symbol
//! - codemap-reindex: Trigger incremental reindexing
//! - codemap-hierarchy: Get class/module hierarchy
//! - codemap-path: Find call paths between symbols
//! - codemap-unused: Find unused/dead code
//! - codemap-implementations: Find implementations of interfaces/traits
//! - codemap-diff-impact: Analyze impact of code changes

mod constants;
mod format;
mod handlers;
mod types;

use std::sync::{Arc, Mutex};

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router, ServerHandler,
};

use crate::db::Database;

pub use types::*;

/// MCP server handler for codemap
#[derive(Clone)]
pub struct CodeMapHandler {
    tool_router: ToolRouter<Self>,
    db: Arc<Mutex<Database>>,
    project_root: String,
}

#[tool_router]
impl CodeMapHandler {
    pub fn new(db: Database, project_root: String) -> Self {
        Self {
            tool_router: Self::tool_router(),
            db: Arc::new(Mutex::new(db)),
            project_root,
        }
    }

    /// Create a handler with a pre-wrapped database (for sharing across HTTP sessions)
    pub fn new_shared(db: Arc<Mutex<Database>>, project_root: String) -> Self {
        Self {
            tool_router: Self::tool_router(),
            db,
            project_root,
        }
    }

    /// Build focused context for a specific task
    #[tool(
        name = "codemap-context",
        description = "Build focused code context for a specific task. Returns entry points, related symbols, and code snippets."
    )]
    fn codemap_context(&self, Parameters(req): Parameters<ContextRequest>) -> String {
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(e) => return format!("Error: {}", e),
        };

        handlers::context::handle_context(&db, &self.project_root, &req)
    }

    /// Quick symbol search by name
    #[tool(
        name = "codemap-search",
        description = "Quick symbol search by name. Returns locations only (no code)."
    )]
    fn codemap_search(&self, Parameters(req): Parameters<SearchRequest>) -> String {
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(e) => return format!("Error: {}", e),
        };

        handlers::search::handle_search(&db, &req)
    }

    /// Find all callers of a symbol
    #[tool(
        name = "codemap-callers",
        description = "Find all functions/methods that call a specific symbol."
    )]
    fn codemap_callers(&self, Parameters(req): Parameters<SymbolRequest>) -> String {
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(e) => return format!("Error: {}", e),
        };

        handlers::graph::handle_callers(&db, &req)
    }

    /// Find all callees of a symbol
    #[tool(
        name = "codemap-callees",
        description = "Find all functions/methods that a specific symbol calls."
    )]
    fn codemap_callees(&self, Parameters(req): Parameters<SymbolRequest>) -> String {
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(e) => return format!("Error: {}", e),
        };

        handlers::graph::handle_callees(&db, &req)
    }

    /// Analyze the impact of changing a symbol
    #[tool(
        name = "codemap-impact",
        description = "Analyze the impact radius of changing a symbol."
    )]
    fn codemap_impact(&self, Parameters(req): Parameters<SymbolRequest>) -> String {
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(e) => return format!("Error: {}", e),
        };

        handlers::graph::handle_impact(&db, &req)
    }

    /// Get the full source code definition of a symbol
    #[tool(
        name = "codemap-definition",
        description = "Get the full source code of a symbol. Returns the complete definition with surrounding context lines."
    )]
    fn codemap_definition(&self, Parameters(req): Parameters<DefinitionRequest>) -> String {
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(e) => return format!("Error: {}", e),
        };

        handlers::symbol::handle_definition(&db, &self.project_root, &req)
    }

    /// List all symbols in a specific file
    #[tool(
        name = "codemap-file",
        description = "List all symbols defined in a specific file. Returns functions, classes, methods, etc."
    )]
    fn codemap_file(&self, Parameters(req): Parameters<FileRequest>) -> String {
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(e) => return format!("Error: {}", e),
        };

        handlers::file::handle_file(&db, &req)
    }

    /// Find all references to a symbol
    #[tool(
        name = "codemap-references",
        description = "Find all references to a symbol including calls, imports, type usages, and other relationships."
    )]
    fn codemap_references(&self, Parameters(req): Parameters<SymbolRequest>) -> String {
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(e) => return format!("Error: {}", e),
        };

        handlers::symbol::handle_references(&db, &req)
    }

    /// Trigger incremental reindexing
    #[tool(
        name = "codemap-reindex",
        description = "Trigger incremental reindexing of the codebase. Only changed files are re-parsed."
    )]
    fn codemap_reindex(&self, Parameters(req): Parameters<ReindexRequest>) -> String {
        let mut db = match self.db.lock() {
            Ok(db) => db,
            Err(e) => return format!("Error: {}", e),
        };

        handlers::reindex::handle_reindex(&mut db, &self.project_root, &req)
    }

    /// Get detailed information about a symbol
    #[tool(
        name = "codemap-node",
        description = "Get detailed information about a specific code symbol."
    )]
    fn codemap_node(&self, Parameters(req): Parameters<SymbolRequest>) -> String {
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(e) => return format!("Error: {}", e),
        };

        handlers::symbol::handle_node(&db, &req)
    }

    /// Get index statistics
    #[tool(
        name = "codemap-status",
        description = "Get the status of the codemap index. Shows statistics about indexed files, symbols, and relationships."
    )]
    fn codemap_status(&self) -> String {
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(e) => return format!("Error: {}", e),
        };

        handlers::status::handle_status(&db)
    }

    /// Get class/module hierarchy
    #[tool(
        name = "codemap-hierarchy",
        description = "Get the hierarchy of a symbol showing parent/child contains relationships (e.g., class contains methods)."
    )]
    fn codemap_hierarchy(&self, Parameters(req): Parameters<SymbolRequest>) -> String {
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(e) => return format!("Error: {}", e),
        };

        handlers::hierarchy::handle_hierarchy(&db, &req)
    }

    /// Find call path between two symbols
    #[tool(
        name = "codemap-path",
        description = "Find call paths from one symbol to another. Shows how function A reaches function B through intermediate calls."
    )]
    fn codemap_path(&self, Parameters(req): Parameters<PathRequest>) -> String {
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(e) => return format!("Error: {}", e),
        };

        handlers::path::handle_path(&db, &req)
    }

    /// Find unused/dead code
    #[tool(
        name = "codemap-unused",
        description = "Find unused symbols (functions, methods, classes) with no incoming references. Helps identify dead code."
    )]
    fn codemap_unused(&self) -> String {
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(e) => return format!("Error: {}", e),
        };

        handlers::unused::handle_unused(&db)
    }

    /// Find implementations of an interface/trait
    #[tool(
        name = "codemap-implementations",
        description = "Find all classes/structs that implement an interface or extend a trait/class."
    )]
    fn codemap_implementations(&self, Parameters(req): Parameters<SymbolRequest>) -> String {
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(e) => return format!("Error: {}", e),
        };

        handlers::implementations::handle_implementations(&db, &req)
    }

    /// Analyze impact of code changes
    #[tool(
        name = "codemap-diff-impact",
        description = "Analyze the impact of changing a specific region of code. Shows directly modified symbols and their callers."
    )]
    fn codemap_diff_impact(&self, Parameters(req): Parameters<DiffImpactRequest>) -> String {
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(e) => return format!("Error: {}", e),
        };

        handlers::diff_impact::handle_diff_impact(&db, &req)
    }
}

#[tool_handler]
impl ServerHandler for CodeMapHandler {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build());
        info.instructions = Some(
            "codemap provides semantic code intelligence for exploring codebases. \
            Use codemap-context to build task-focused context, codemap-search for quick lookups, \
            codemap-callers/callees/impact for understanding code relationships, \
            codemap-definition to view source code, codemap-file to list symbols in a file, \
            codemap-references for all usages of a symbol, codemap-hierarchy for class/module structure, \
            codemap-path to find call paths between functions, codemap-unused to find dead code, \
            codemap-implementations to find interface/trait implementations, \
            codemap-diff-impact to analyze change impact, and codemap-reindex to refresh after edits."
                .into(),
        );
        info
    }
}
