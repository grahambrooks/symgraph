//! CLI subcommands that mirror the MCP tools.
//!
//! Each command builds the same request struct the MCP server uses and calls
//! the same `mcp::handlers::*` function, so CLI output is identical to the
//! server's. `--format json` is routed through each request's `format` field;
//! `blame`, `churn`, and `diff-impact` are markdown-only for now.

use anyhow::{bail, Result};

use crate::db::Database;
use crate::mcp::handlers;
use crate::mcp::{
    BlameRequest, ChurnRequest, DefinitionRequest, DiffImpactRequest, DispatchSitesRequest,
    FileRequest, GodStructRequest, ImpactRequest, ModuleGraphRequest, PathRequest, SymbolRequest,
};

use super::commands::OutputFormat;
use super::db_utils::{canonicalize_path, resolve_db};

/// Open the project's existing index read-only, erroring if it isn't built yet.
fn query_context(path: &str) -> Result<(String, Database)> {
    let project_root = canonicalize_path(path)?;
    let resolved = resolve_db(&project_root)?;
    if !resolved.path.exists() {
        bail!(
            "No index found at {} [{}]. Run 'symgraph index' first.",
            resolved.path.display(),
            resolved.label
        );
    }
    let db = Database::open(&resolved.path)?;
    Ok((project_root, db))
}

/// Print a handler's result; map its `Err(String)` to an anyhow error.
fn emit(result: Result<String, String>) -> Result<()> {
    match result {
        Ok(text) => {
            println!("{text}");
            Ok(())
        }
        Err(e) => bail!(e),
    }
}

fn symbol_req(symbol: &str, fmt: OutputFormat) -> SymbolRequest {
    SymbolRequest {
        symbol: symbol.to_string(),
        format: fmt.request_format(),
    }
}

/// A churn flag present on the command line means "use churn"; absence leaves
/// the choice to the handler's own default (e.g. coupling-score defaults on).
fn churn_opt(flag: bool) -> Option<bool> {
    if flag {
        Some(true)
    } else {
        None
    }
}

// --- symbol relationships ---

pub fn callers(path: &str, symbol: &str, fmt: OutputFormat) -> Result<()> {
    let (_root, db) = query_context(path)?;
    emit(handlers::graph::handle_callers(
        &db,
        &symbol_req(symbol, fmt),
    ))
}

pub fn callees(path: &str, symbol: &str, fmt: OutputFormat) -> Result<()> {
    let (_root, db) = query_context(path)?;
    emit(handlers::graph::handle_callees(
        &db,
        &symbol_req(symbol, fmt),
    ))
}

pub fn node(path: &str, symbol: &str, fmt: OutputFormat) -> Result<()> {
    let (_root, db) = query_context(path)?;
    emit(handlers::symbol::handle_node(&db, &symbol_req(symbol, fmt)))
}

pub fn references(path: &str, symbol: &str, fmt: OutputFormat) -> Result<()> {
    let (_root, db) = query_context(path)?;
    emit(handlers::symbol::handle_references(
        &db,
        &symbol_req(symbol, fmt),
    ))
}

pub fn hierarchy(path: &str, symbol: &str, fmt: OutputFormat) -> Result<()> {
    let (_root, db) = query_context(path)?;
    emit(handlers::hierarchy::handle_hierarchy(
        &db,
        &symbol_req(symbol, fmt),
    ))
}

pub fn implementations(path: &str, symbol: &str, fmt: OutputFormat) -> Result<()> {
    let (_root, db) = query_context(path)?;
    emit(handlers::implementations::handle_implementations(
        &db,
        &symbol_req(symbol, fmt),
    ))
}

pub fn unused(path: &str, fmt: OutputFormat) -> Result<()> {
    let (_root, db) = query_context(path)?;
    emit(handlers::unused::handle_unused(&db, &fmt.request_format()))
}

pub fn file(path: &str, file_path: &str, fmt: OutputFormat) -> Result<()> {
    let (_root, db) = query_context(path)?;
    emit(handlers::file::handle_file(
        &db,
        &FileRequest {
            path: file_path.to_string(),
            format: fmt.request_format(),
        },
    ))
}

pub fn path_between(path: &str, from: &str, to: &str, fmt: OutputFormat) -> Result<()> {
    let (_root, db) = query_context(path)?;
    emit(handlers::path::handle_path(
        &db,
        &PathRequest {
            from: from.to_string(),
            to: to.to_string(),
            format: fmt.request_format(),
        },
    ))
}

pub fn definition(
    path: &str,
    symbol: &str,
    context_lines: Option<u32>,
    fmt: OutputFormat,
) -> Result<()> {
    let (root, db) = query_context(path)?;
    emit(handlers::symbol::handle_definition(
        &db,
        &root,
        &DefinitionRequest {
            symbol: symbol.to_string(),
            context_lines,
            format: fmt.request_format(),
        },
    ))
}

// --- impact / change analysis ---

pub fn impact(
    path: &str,
    symbol: &str,
    fmt: OutputFormat,
    churn: bool,
    days: Option<u32>,
) -> Result<()> {
    let (root, db) = query_context(path)?;
    let req = ImpactRequest {
        symbol: symbol.to_string(),
        churn: churn_opt(churn),
        days,
        format: fmt.request_format(),
    };
    emit(handlers::graph::handle_impact(&db, &root, &req))
}

pub fn diff_impact(
    path: &str,
    file_path: Option<String>,
    start_line: Option<u32>,
    end_line: Option<u32>,
    git_ref: Option<String>,
) -> Result<()> {
    let (root, db) = query_context(path)?;
    emit(handlers::diff_impact::handle_diff_impact(
        &db,
        &root,
        &DiffImpactRequest {
            file_path,
            start_line,
            end_line,
            git_ref,
        },
    ))
}

// --- git history ---

pub fn blame(path: &str, symbol: &str) -> Result<()> {
    let (root, db) = query_context(path)?;
    emit(handlers::blame::handle_blame(
        &db,
        &root,
        &BlameRequest {
            symbol: symbol.to_string(),
        },
    ))
}

pub fn churn(path: &str, path_filter: Option<String>, days: Option<u32>) -> Result<()> {
    // Churn reads git history, not the index, so no index is required.
    let project_root = canonicalize_path(path)?;
    emit(handlers::churn::handle_churn(
        &project_root,
        &ChurnRequest {
            path: path_filter,
            days,
        },
    ))
}

// --- coupling & architecture ---

pub fn module_graph(
    path: &str,
    granularity: Option<String>,
    churn: bool,
    days: Option<u32>,
    limit: Option<u32>,
    fmt: OutputFormat,
) -> Result<()> {
    let (root, db) = query_context(path)?;
    let req = ModuleGraphRequest {
        granularity,
        churn: churn_opt(churn),
        days,
        format: fmt.request_format(),
        limit,
    };
    emit(handlers::module_graph::handle_module_graph(
        &db, &root, &req,
    ))
}

pub fn coupling_score(
    path: &str,
    granularity: Option<String>,
    churn: bool,
    days: Option<u32>,
    limit: Option<u32>,
    fmt: OutputFormat,
) -> Result<()> {
    let (root, db) = query_context(path)?;
    let req = ModuleGraphRequest {
        granularity,
        churn: churn_opt(churn),
        days,
        format: fmt.request_format(),
        limit,
    };
    emit(handlers::module_graph::handle_coupling_score(
        &db, &root, &req,
    ))
}

pub fn god_struct(
    path: &str,
    churn: bool,
    days: Option<u32>,
    limit: Option<u32>,
    fmt: OutputFormat,
) -> Result<()> {
    let (root, db) = query_context(path)?;
    let req = GodStructRequest {
        churn: churn_opt(churn),
        days,
        format: fmt.request_format(),
        limit,
    };
    emit(handlers::god_struct::handle_god_struct(&db, &root, &req))
}

pub fn dispatch_sites(path: &str, symbol: &str, fmt: OutputFormat) -> Result<()> {
    let (_root, db) = query_context(path)?;
    emit(handlers::dispatch_sites::handle_dispatch_sites(
        &db,
        &DispatchSitesRequest {
            symbol: symbol.to_string(),
            format: fmt.request_format(),
        },
    ))
}
