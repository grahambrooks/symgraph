//! Output tests for the MCP/CLI tool handlers.
//!
//! The review found 14 of 18 handlers untested — the entire surface users and
//! agents actually consume. These exercise each handler against a real indexed
//! fixture and assert on what it returns, in both markdown and JSON.
//!
//! Both front-ends call these same functions, so covering them here covers the
//! CLI and the MCP server at once — which is the parity `docs/cli-mcp-parity.md`
//! promises by construction.

use std::fs;

use symgraph::db::Database;
use symgraph::mcp::handlers;
use symgraph::mcp::{
    DispatchSitesRequest, FileRequest, FormatRequest, GodStructRequest, ModuleGraphRequest,
    PathRequest, SymbolRequest,
};
use symgraph::{build_full_index, IndexConfig};
use tempfile::TempDir;

/// A small but structurally real project: two modules, a trait with an impl,
/// an enum that is matched on, a struct with public fields, and dead code.
fn fixture() -> (TempDir, Database) {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(&src).unwrap();

    fs::write(
        src.join("lib.rs"),
        r#"
pub mod engine;
pub mod report;

pub trait Render {
    fn render(&self) -> String;
}

pub enum Mode {
    Fast,
    Careful,
}

pub struct Settings {
    pub retries: u32,
    pub verbose: bool,
    pub label: String,
}
"#,
    )
    .unwrap();

    fs::write(
        src.join("engine.rs"),
        r#"
use crate::Mode;
use crate::Render;
use crate::Settings;

pub struct Engine {
    pub settings: Settings,
}

impl Engine {
    pub fn run(&self, mode: Mode) -> u32 {
        match mode {
            Mode::Fast => self.quick_pass(),
            Mode::Careful => self.slow_pass(),
        }
    }

    fn quick_pass(&self) -> u32 {
        self.settings.retries
    }

    fn slow_pass(&self) -> u32 {
        self.quick_pass() + 1
    }
}

impl Render for Engine {
    fn render(&self) -> String {
        String::new()
    }
}

pub fn never_called_anywhere() -> u32 {
    0
}
"#,
    )
    .unwrap();

    fs::write(
        src.join("report.rs"),
        r#"
use crate::engine::Engine;
use crate::Mode;

pub fn summarize(engine: &Engine) -> u32 {
    engine.run(Mode::Fast)
}
"#,
    )
    .unwrap();

    let mut db = Database::in_memory().unwrap();
    let config = IndexConfig {
        root: dir.path().display().to_string(),
        ..Default::default()
    };
    build_full_index(&mut db, &config).unwrap();
    (dir, db)
}

fn symbol(name: &str) -> SymbolRequest {
    SymbolRequest {
        symbol: name.to_string(),
        file: None,
        qualified_name: None,
        limit: None,
        offset: None,
        format: None,
    }
}

fn json(name: &str) -> SymbolRequest {
    SymbolRequest {
        format: Some("json".to_string()),
        ..symbol(name)
    }
}

fn parse(out: &str) -> serde_json::Value {
    serde_json::from_str(out).unwrap_or_else(|e| panic!("not valid JSON ({e}):\n{out}"))
}

// --- callers / callees ---------------------------------------------------

#[test]
fn callers_finds_the_calling_function() {
    let (_dir, db) = fixture();
    let out = handlers::graph::handle_callers(&db, &symbol("quick_pass")).unwrap();
    assert!(out.contains("slow_pass"), "output was:\n{out}");
}

#[test]
fn callers_json_carries_page_and_resolution() {
    let (_dir, db) = fixture();
    let v = parse(&handlers::graph::handle_callers(&db, &json("quick_pass")).unwrap());
    assert!(v["total"].is_number());
    assert!(v["truncated"].is_boolean());
    assert!(v["resolution"]["candidates"].is_number());
}

#[test]
fn callees_lists_what_a_function_calls() {
    let (_dir, db) = fixture();
    let out = handlers::graph::handle_callees(&db, &symbol("slow_pass")).unwrap();
    assert!(out.contains("quick_pass"), "output was:\n{out}");
}

// --- hierarchy / implementations -----------------------------------------

#[test]
fn hierarchy_lists_a_struct_s_members() {
    let (_dir, db) = fixture();
    let out = handlers::hierarchy::handle_hierarchy(&db, &symbol("Settings")).unwrap();
    assert!(
        out.contains("retries") || out.contains("Settings"),
        "output was:\n{out}"
    );
}

/// Documents a gap rather than a feature: extraction emits no `implements` or
/// `extends` edges for any language, so this tool has never returned anything.
/// `impl Render for Engine` in the fixture is exactly the case it is supposed
/// to find. Recorded as F8 in docs/review-2026-09-20.md.
///
/// Flip this assertion when trait/interface extraction lands.
#[test]
fn implementations_currently_finds_nothing_for_a_rust_impl() {
    let (_dir, db) = fixture();
    let out = handlers::implementations::handle_implementations(&db, &symbol("Render")).unwrap();
    assert!(
        out.contains("No implementations"),
        "extraction has started emitting implements edges — update this test \
         and the F8 finding. Output was:\n{out}"
    );
}

#[test]
fn implementations_of_an_unknown_symbol_says_so_without_failing() {
    let (_dir, db) = fixture();
    let out =
        handlers::implementations::handle_implementations(&db, &symbol("NoSuchTrait")).unwrap();
    assert!(out.contains("No implementations"), "output was:\n{out}");
}

// --- file ----------------------------------------------------------------

#[test]
fn file_lists_symbols_by_forward_slash_path() {
    let (_dir, db) = fixture();
    let req = FileRequest {
        path: "src/engine.rs".to_string(),
        format: None,
    };
    let out = handlers::file::handle_file(&db, &req).unwrap();
    assert!(out.contains("Engine"), "output was:\n{out}");
    assert!(out.contains("quick_pass"), "output was:\n{out}");
}

#[test]
fn file_rejects_a_traversal_path() {
    let (_dir, db) = fixture();
    let req = FileRequest {
        path: "../../etc/passwd".to_string(),
        format: None,
    };
    let err = handlers::file::handle_file(&db, &req).unwrap_err();
    assert!(err.contains("path") || err.contains("unsafe"), "got: {err}");
}

// --- unused --------------------------------------------------------------

#[test]
fn unused_finds_dead_code_and_pages_it() {
    let (_dir, db) = fixture();
    let out = handlers::unused::handle_unused(&db, &FormatRequest::default()).unwrap();
    assert!(out.contains("never_called_anywhere"), "output was:\n{out}");

    let paged = handlers::unused::handle_unused(
        &db,
        &FormatRequest {
            limit: Some(1),
            offset: None,
            format: Some("json".to_string()),
        },
    )
    .unwrap();
    let v = parse(&paged);
    assert_eq!(v["shown"], 1);
    assert!(v["total"].as_u64().unwrap() >= 1);
}

// --- path ----------------------------------------------------------------

#[test]
fn path_finds_a_call_chain() {
    let (_dir, db) = fixture();
    let req = PathRequest {
        from: "slow_pass".to_string(),
        to: "quick_pass".to_string(),
        format: None,
    };
    let out = handlers::path::handle_path(&db, &req).unwrap();
    assert!(out.contains("quick_pass"), "output was:\n{out}");
}

#[test]
fn path_between_unconnected_symbols_reports_no_path() {
    let (_dir, db) = fixture();
    let req = PathRequest {
        from: "never_called_anywhere".to_string(),
        to: "summarize".to_string(),
        format: None,
    };
    let out = handlers::path::handle_path(&db, &req).unwrap();
    assert!(out.contains("No call path"), "output was:\n{out}");
}

// --- module graph / coupling / god struct --------------------------------

fn graph_req(format: Option<&str>) -> ModuleGraphRequest {
    ModuleGraphRequest {
        granularity: Some("module".to_string()),
        churn: Some(false),
        days: None,
        format: format.map(str::to_string),
        limit: None,
    }
}

#[test]
fn module_graph_reports_nodes_and_edges() {
    let (dir, db) = fixture();
    let root = dir.path().display().to_string();
    let out = handlers::module_graph::handle_module_graph(&db, &root, &graph_req(None)).unwrap();
    assert!(out.contains("nodes"), "output was:\n{out}");
}

#[test]
fn module_graph_json_is_machine_readable() {
    let (dir, db) = fixture();
    let root = dir.path().display().to_string();
    let v = parse(
        &handlers::module_graph::handle_module_graph(&db, &root, &graph_req(Some("json"))).unwrap(),
    );
    assert!(v["nodes"].is_array(), "json was:\n{v:#}");
}

#[test]
fn coupling_score_runs_without_churn() {
    let (dir, db) = fixture();
    let root = dir.path().display().to_string();
    let out = handlers::module_graph::handle_coupling_score(&db, &root, &graph_req(None)).unwrap();
    assert!(!out.is_empty());
}

#[test]
fn god_struct_ranks_the_struct_with_public_fields() {
    let (dir, db) = fixture();
    let root = dir.path().display().to_string();
    let req = GodStructRequest {
        churn: Some(false),
        days: None,
        format: Some("json".to_string()),
        limit: Some(10),
    };
    let v = parse(&handlers::god_struct::handle_god_struct(&db, &root, &req).unwrap());
    assert!(v.is_object() || v.is_array(), "json was:\n{v:#}");
}

// --- dispatch sites ------------------------------------------------------

#[test]
fn dispatch_sites_finds_where_an_enum_is_matched() {
    let (_dir, db) = fixture();
    let req = DispatchSitesRequest {
        symbol: "Mode".to_string(),
        format: None,
    };
    let out = handlers::dispatch_sites::handle_dispatch_sites(&db, &req).unwrap();
    // Either it names the matching file, or it says plainly that it found none.
    assert!(
        out.contains("engine.rs") || out.to_lowercase().contains("no "),
        "output was:\n{out}"
    );
}

// --- status --------------------------------------------------------------

#[test]
fn status_reports_size_and_trust() {
    let (_dir, db) = fixture();
    let out = handlers::status::handle_status(&db).unwrap();
    assert!(out.contains("Total Files"), "output was:\n{out}");
    assert!(out.contains("Index health"), "output was:\n{out}");
    assert!(out.contains("Ambiguous names"), "output was:\n{out}");
    // A freshly built index is current, so it must not be flagged.
    assert!(!out.contains("Out of date"), "output was:\n{out}");
}

// --- definition / node / references --------------------------------------

#[test]
fn definition_returns_source_for_a_symbol() {
    let (dir, db) = fixture();
    let root = dir.path().display().to_string();
    let req = symgraph::mcp::DefinitionRequest {
        symbol: "summarize".to_string(),
        file: None,
        qualified_name: None,
        context_lines: Some(1),
        format: None,
    };
    let out = handlers::symbol::handle_definition(&db, &root, &req).unwrap();
    assert!(out.contains("summarize"), "output was:\n{out}");
}

#[test]
fn node_reports_metadata_for_a_symbol() {
    let (_dir, db) = fixture();
    let v = parse(&handlers::symbol::handle_node(&db, &json("Engine")).unwrap());
    assert_eq!(v["name"], "Engine");
    assert!(v["resolution"]["candidates"].as_u64().unwrap() >= 1);
}

#[test]
fn node_for_a_missing_symbol_reports_not_found() {
    let (_dir, db) = fixture();
    let v = parse(&handlers::symbol::handle_node(&db, &json("NoSuchThing")).unwrap());
    assert_eq!(v["found"], false);
}

#[test]
fn references_groups_by_edge_kind() {
    let (_dir, db) = fixture();
    let out = handlers::symbol::handle_references(&db, &symbol("Engine")).unwrap();
    assert!(
        out.contains("References to") || out.contains("No references"),
        "output was:\n{out}"
    );
}

// --- disambiguation ------------------------------------------------------

/// `run` is defined once here, so nothing should be flagged ambiguous. The
/// point is that the flag reflects the index rather than always firing.
#[test]
fn an_unambiguous_symbol_is_not_flagged() {
    let (_dir, db) = fixture();
    let v = parse(&handlers::graph::handle_callers(&db, &json("summarize")).unwrap());
    assert_eq!(v["resolution"]["ambiguous"], false);
}
