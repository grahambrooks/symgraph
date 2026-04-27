//! Integration tests for symgraph
//!
//! These tests verify the end-to-end workflow of indexing and querying code.

use std::fs;

use symgraph::cli::{open_project_database, rebuild_project_database};
use symgraph::db::Database;
use symgraph::extraction::Extractor;
use symgraph::graph::Graph;
use symgraph::types::{EdgeKind, FileRecord, Language, NodeKind};
use symgraph::{build_full_index, index_codebase, IndexConfig};
use tempfile::tempdir;

/// Helper to set up a test database with indexed code
fn setup_indexed_db(code: &str, filename: &str) -> Database {
    let db = Database::in_memory().unwrap();

    // Create file record
    let file = FileRecord {
        path: filename.to_string(),
        content_hash: "test_hash".to_string(),
        language: Language::from_extension(
            std::path::Path::new(filename)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or(""),
        ),
        size: code.len() as u64,
        modified_at: 0,
        indexed_at: 0,
        node_count: 0,
    };
    db.insert_or_update_file(&file).unwrap();

    // Extract symbols
    let mut extractor = Extractor::new();
    let result = extractor.extract_file(filename, code);

    // Store nodes with ID mapping
    let mut id_map = std::collections::HashMap::new();
    for mut node in result.nodes {
        let old_id = node.id;
        node.id = 0;
        let new_id = db.insert_node(&node).unwrap();
        id_map.insert(old_id, new_id);
    }

    // Store edges with mapped IDs
    for mut edge in result.edges {
        if let (Some(&new_source), Some(&new_target)) =
            (id_map.get(&edge.source_id), id_map.get(&edge.target_id))
        {
            edge.source_id = new_source;
            edge.target_id = new_target;
            db.insert_edge(&edge).unwrap();
        }
    }

    // Store unresolved references
    for mut uref in result.unresolved_refs {
        if let Some(&new_source) = id_map.get(&uref.source_node_id) {
            uref.source_node_id = new_source;
            db.insert_unresolved_ref(&uref).unwrap();
        }
    }

    // Resolve references
    db.resolve_references().unwrap();

    db
}

#[test]
fn test_end_to_end_rust_indexing() {
    let code = r#"
fn main() {
    helper();
    println!("Hello!");
}

fn helper() {
    utility();
}

fn utility() {
    // Does some work
}
"#;

    let db = setup_indexed_db(code, "main.rs");

    // Verify nodes were created
    let stats = db.get_stats().unwrap();
    assert!(stats.total_nodes >= 4); // file + 3 functions

    // Verify we can search for functions
    let results = db.search_nodes("main", None, 10).unwrap();
    assert!(!results.is_empty());
    assert!(results.iter().any(|n| n.name == "main"));

    // Verify we can find the helper function
    let helper = db.find_node_by_name("helper").unwrap();
    assert!(helper.is_some());
}

#[test]
fn test_end_to_end_call_graph() {
    let code = r#"
fn caller() {
    callee();
}

fn callee() {
    // Implementation
}
"#;

    let db = setup_indexed_db(code, "calls.rs");
    let graph = Graph::new(&db);

    // Find callers of callee
    let callers = graph.find_callers("callee", 10).unwrap();
    assert_eq!(callers.len(), 1);
    assert_eq!(callers[0].name, "caller");

    // Find callees of caller
    let callees = graph.find_callees("caller", 10).unwrap();
    assert_eq!(callees.len(), 1);
    assert_eq!(callees[0].name, "callee");
}

#[test]
fn test_end_to_end_impact_analysis() {
    let code = r#"
fn base_function() {
    // Core logic
}

fn direct_user() {
    base_function();
}

fn indirect_user() {
    direct_user();
}
"#;

    let db = setup_indexed_db(code, "impact.rs");
    let graph = Graph::new(&db);

    let analysis = graph.analyze_impact("base_function", 3).unwrap();

    assert!(analysis.root.is_some());
    assert_eq!(analysis.root.as_ref().unwrap().name, "base_function");
    assert!(!analysis.direct_callers.is_empty());
    assert!(analysis.total_impact >= 1);
}

#[test]
fn test_end_to_end_typescript() {
    let code = r#"
interface User {
    name: string;
    age: number;
}

class UserService {
    getUser(id: number): User {
        return { name: "Test", age: 25 };
    }

    saveUser(user: User): void {
        console.log(user);
    }
}

function main(): void {
    const service = new UserService();
    const user = service.getUser(1);
    service.saveUser(user);
}
"#;

    let db = setup_indexed_db(code, "user.ts");

    // Check that interface was extracted
    let results = db.search_nodes("User", None, 10).unwrap();
    assert!(results.iter().any(|n| n.kind == NodeKind::Interface));

    // Check that class was extracted
    let results = db.search_nodes("UserService", None, 10).unwrap();
    assert!(results.iter().any(|n| n.kind == NodeKind::Class));

    // Check that function was extracted
    let main = db.find_node_by_name("main").unwrap();
    assert!(main.is_some());
    // Language may be stored differently, just verify the node exists
    assert_eq!(main.unwrap().kind, NodeKind::Function);
}

#[test]
fn test_end_to_end_python() {
    let code = r#"
class Calculator:
    def __init__(self):
        self.value = 0

    def add(self, x):
        self.value += x
        return self

    def result(self):
        return self.value

def main():
    calc = Calculator()
    calc.add(5).add(3)
    print(calc.result())
"#;

    let db = setup_indexed_db(code, "calc.py");

    // Check class
    let calc = db.find_node_by_name("Calculator").unwrap();
    assert!(calc.is_some());
    assert_eq!(calc.unwrap().kind, NodeKind::Class);

    // Check methods
    let add = db.find_node_by_name("add").unwrap();
    assert!(add.is_some());

    // Check function
    let main = db.find_node_by_name("main").unwrap();
    assert!(main.is_some());
    // Language stored as string, just verify the node exists
    assert_eq!(main.unwrap().kind, NodeKind::Function);
}

#[test]
fn test_database_persistence() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    // Create and populate database
    {
        let db = Database::open(&db_path).unwrap();
        let file = FileRecord {
            path: "test.rs".to_string(),
            content_hash: "abc123".to_string(),
            language: Language::Rust,
            size: 100,
            modified_at: 0,
            indexed_at: 0,
            node_count: 1,
        };
        db.insert_or_update_file(&file).unwrap();

        let mut extractor = Extractor::new();
        let result = extractor.extract_file("test.rs", "fn hello() {}");
        for node in result.nodes {
            db.insert_node(&node).unwrap();
        }
    }

    // Reopen and verify
    {
        let db = Database::open(&db_path).unwrap();
        let stats = db.get_stats().unwrap();
        assert!(stats.total_files >= 1);
        assert!(stats.total_nodes >= 1);

        let file = db.get_file("test.rs").unwrap();
        assert!(file.is_some());
    }
}

#[test]
fn test_build_full_index_populates_empty_target_db() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir(&src).unwrap();
    fs::write(
        src.join("main.rs"),
        "fn main() { helper(); }\nfn helper() {}\n",
    )
    .unwrap();

    let db_path = dir.path().join("full-build.db");
    let mut db = Database::open(&db_path).unwrap();
    let config = IndexConfig {
        root: dir.path().display().to_string(),
        ..Default::default()
    };

    let stats = build_full_index(&mut db, &config).unwrap();

    assert_eq!(stats.files, 1);
    assert_eq!(stats.skipped, 0);
    assert!(stats.nodes >= 2);
    assert!(db.find_node_by_name("main").unwrap().is_some());
    assert!(db.find_node_by_name("helper").unwrap().is_some());
    assert!(!db.semantic_search("helper", 10).unwrap().is_empty());
}

#[test]
fn test_rebuild_project_database_replaces_stale_rows() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("old.rs"), "fn old_symbol() {}\n").unwrap();

    let project_root = dir.path().display().to_string();
    let mut db = open_project_database(&project_root).unwrap();
    let config = IndexConfig {
        root: project_root.clone(),
        ..Default::default()
    };

    rebuild_project_database(&mut db, &config).unwrap();
    assert!(db.find_node_by_name("old_symbol").unwrap().is_some());

    fs::remove_file(src.join("old.rs")).unwrap();
    fs::write(src.join("new.rs"), "fn new_symbol() {}\n").unwrap();

    rebuild_project_database(&mut db, &config).unwrap();

    assert!(db.find_node_by_name("old_symbol").unwrap().is_none());
    assert!(db.find_node_by_name("new_symbol").unwrap().is_some());
    assert_eq!(db.get_stats().unwrap().total_files, 1);
}

#[test]
fn test_rebuild_project_database_keeps_live_db_on_failure() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("live.rs"), "fn live_symbol() {}\n").unwrap();

    let project_root = dir.path().display().to_string();
    let mut db = open_project_database(&project_root).unwrap();
    let good_config = IndexConfig {
        root: project_root.clone(),
        ..Default::default()
    };
    rebuild_project_database(&mut db, &good_config).unwrap();

    let bad_config = IndexConfig {
        root: src.join("live.rs").display().to_string(),
        ..Default::default()
    };
    assert!(rebuild_project_database(&mut db, &bad_config).is_err());

    assert!(db.find_node_by_name("live_symbol").unwrap().is_some());
}

#[test]
fn test_incremental_indexing() {
    let db = Database::in_memory().unwrap();

    // First index
    let file1 = FileRecord {
        path: "module.rs".to_string(),
        content_hash: "hash_v1".to_string(),
        language: Language::Rust,
        size: 100,
        modified_at: 0,
        indexed_at: 0,
        node_count: 0,
    };
    db.insert_or_update_file(&file1).unwrap();

    // Check that file doesn't need reindexing with same hash
    assert!(!db.needs_reindex("module.rs", "hash_v1").unwrap());

    // Check that file needs reindexing with different hash
    assert!(db.needs_reindex("module.rs", "hash_v2").unwrap());

    // Check that new file needs indexing
    assert!(db.needs_reindex("new_file.rs", "any_hash").unwrap());
}

#[test]
fn test_multi_file_references() {
    let db = Database::in_memory().unwrap();

    // Set up two files
    for path in ["file1.rs", "file2.rs"] {
        let file = FileRecord {
            path: path.to_string(),
            content_hash: "hash".to_string(),
            language: Language::Rust,
            size: 100,
            modified_at: 0,
            indexed_at: 0,
            node_count: 0,
        };
        db.insert_or_update_file(&file).unwrap();
    }

    // Extract and insert nodes from both files
    let mut extractor = Extractor::new();

    let result1 = extractor.extract_file("file1.rs", "fn shared_helper() {}");
    let mut id_map1 = std::collections::HashMap::new();
    for mut node in result1.nodes {
        let old_id = node.id;
        node.id = 0;
        let new_id = db.insert_node(&node).unwrap();
        id_map1.insert(old_id, new_id);
    }

    let result2 = extractor.extract_file("file2.rs", "fn caller() { shared_helper(); }");
    let mut id_map2 = std::collections::HashMap::new();
    for mut node in result2.nodes {
        let old_id = node.id;
        node.id = 0;
        let new_id = db.insert_node(&node).unwrap();
        id_map2.insert(old_id, new_id);
    }

    // Insert unresolved references
    for mut uref in result2.unresolved_refs {
        if let Some(&new_source) = id_map2.get(&uref.source_node_id) {
            uref.source_node_id = new_source;
            db.insert_unresolved_ref(&uref).unwrap();
        }
    }

    // Resolve cross-file references
    let resolved = db.resolve_references().unwrap();
    assert!(resolved >= 1);

    // Verify the cross-file call was resolved
    let graph = Graph::new(&db);
    let callers = graph.find_callers("shared_helper", 10).unwrap();
    assert!(!callers.is_empty());
}

#[test]
fn test_contains_relationship() {
    let code = r#"
mod outer {
    fn inner() {}
}
"#;

    let db = setup_indexed_db(code, "nested.rs");

    // Should have contains edges
    let stats = db.get_stats().unwrap();
    assert!(stats.total_edges > 0);
}

#[test]
fn test_search_with_limit() {
    let db = Database::in_memory().unwrap();
    let file = FileRecord {
        path: "many.rs".to_string(),
        content_hash: "hash".to_string(),
        language: Language::Rust,
        size: 1000,
        modified_at: 0,
        indexed_at: 0,
        node_count: 0,
    };
    db.insert_or_update_file(&file).unwrap();

    // Insert many similar nodes
    let mut extractor = Extractor::new();
    let code = (0..20)
        .map(|i| format!("fn process_item_{}() {{}}", i))
        .collect::<Vec<_>>()
        .join("\n");

    let result = extractor.extract_file("many.rs", &code);
    for mut node in result.nodes {
        node.id = 0;
        db.insert_node(&node).unwrap();
    }

    // Search with limit
    let results = db.search_nodes("process", None, 5).unwrap();
    assert_eq!(results.len(), 5);

    let results = db.search_nodes("process", None, 100).unwrap();
    assert_eq!(results.len(), 20);
}

#[test]
fn test_large_codebase_simulation() {
    let db = Database::in_memory().unwrap();

    // Simulate a large codebase with interdependent files
    let file_a = FileRecord {
        path: "a.rs".to_string(),
        content_hash: "hash_a".to_string(),
        language: Language::Rust,
        size: 500,
        modified_at: 0,
        indexed_at: 0,
        node_count: 0,
    };
    db.insert_or_update_file(&file_a).unwrap();

    let file_b = FileRecord {
        path: "b.rs".to_string(),
        content_hash: "hash_b".to_string(),
        language: Language::Rust,
        size: 600,
        modified_at: 0,
        indexed_at: 0,
        node_count: 0,
    };
    db.insert_or_update_file(&file_b).unwrap();

    let file_c = FileRecord {
        path: "c.rs".to_string(),
        content_hash: "hash_c".to_string(),
        language: Language::Rust,
        size: 700,
        modified_at: 0,
        indexed_at: 0,
        node_count: 0,
    };
    db.insert_or_update_file(&file_c).unwrap();

    // Extract and insert nodes for each file
    let mut extractor = Extractor::new();
    let code_a = "pub fn a() { b(); }";
    let code_b = "pub fn b() { c(); }";
    let code_c = "pub fn c() { a(); }";

    let result_a = extractor.extract_file("a.rs", code_a);
    let mut node_id_a = None;
    for node in result_a.nodes {
        let id = db.insert_node(&node).unwrap();
        if node.kind == NodeKind::Function && node.name == "a" {
            node_id_a = Some(id);
        }
    }
    let node_id_a = node_id_a.expect("Function 'a' not found");

    let result_b = extractor.extract_file("b.rs", code_b);
    let mut node_id_b = None;
    for node in result_b.nodes {
        let id = db.insert_node(&node).unwrap();
        if node.kind == NodeKind::Function && node.name == "b" {
            node_id_b = Some(id);
        }
    }
    let node_id_b = node_id_b.expect("Function 'b' not found");

    let result_c = extractor.extract_file("c.rs", code_c);
    let mut node_id_c = None;
    for node in result_c.nodes {
        let id = db.insert_node(&node).unwrap();
        if node.kind == NodeKind::Function && node.name == "c" {
            node_id_c = Some(id);
        }
    }
    let node_id_c = node_id_c.expect("Function 'c' not found");

    // Manually create edges to simulate cross-file calls
    let edge_ab = symgraph::types::Edge {
        id: 0,
        source_id: node_id_a,
        target_id: node_id_b,
        kind: EdgeKind::Calls,
        file_path: Some("a.rs".to_string()),
        line: None,
        column: None,
    };
    db.insert_edge(&edge_ab).unwrap();

    let edge_bc = symgraph::types::Edge {
        id: 0,
        source_id: node_id_b,
        target_id: node_id_c,
        kind: EdgeKind::Calls,
        file_path: Some("b.rs".to_string()),
        line: None,
        column: None,
    };
    db.insert_edge(&edge_bc).unwrap();

    let edge_ca = symgraph::types::Edge {
        id: 0,
        source_id: node_id_c,
        target_id: node_id_a,
        kind: EdgeKind::Calls,
        file_path: Some("c.rs".to_string()),
        line: None,
        column: None,
    };
    db.insert_edge(&edge_ca).unwrap();

    // Resolve references across the codebase (none to resolve since edges are already created)
    let resolved = db.resolve_references().unwrap();
    // No unresolved references in this test since we created edges directly
    assert_eq!(resolved, 0);

    // Perform impact analysis on an exported function
    let graph = Graph::new(&db);
    let analysis = graph.analyze_impact("a", 3).unwrap();

    assert!(analysis.root.is_some());
    assert_eq!(analysis.root.as_ref().unwrap().name, "a");
    assert!(!analysis.direct_callers.is_empty());
    assert!(analysis.total_impact >= 1);
}

#[test]
fn test_incremental_reindexing() {
    let db = Database::in_memory().unwrap();

    // Initial indexing
    let file = FileRecord {
        path: "module.rs".to_string(),
        content_hash: "hash_v1".to_string(),
        language: Language::Rust,
        size: 100,
        modified_at: 0,
        indexed_at: 0,
        node_count: 0,
    };
    db.insert_or_update_file(&file).unwrap();

    // Simulate a change by updating the content hash
    let updated_file = FileRecord {
        path: "module.rs".to_string(),
        content_hash: "hash_v2".to_string(),
        language: Language::Rust,
        size: 100,
        modified_at: 1,
        indexed_at: 0,
        node_count: 0,
    };
    db.insert_or_update_file(&updated_file).unwrap();

    // Ensure the file is NOT marked for reindexing when hash matches
    assert!(!db.needs_reindex("module.rs", "hash_v2").unwrap());

    // But IS marked for reindexing when hash differs
    assert!(db.needs_reindex("module.rs", "hash_v3").unwrap());
}

#[test]
fn test_cross_file_references() {
    let db = Database::in_memory().unwrap();

    // Set up two files
    for path in ["file1.rs", "file2.rs"] {
        let file = FileRecord {
            path: path.to_string(),
            content_hash: "hash".to_string(),
            language: Language::Rust,
            size: 100,
            modified_at: 0,
            indexed_at: 0,
            node_count: 0,
        };
        db.insert_or_update_file(&file).unwrap();
    }

    // Extract and insert nodes from both files
    let mut extractor = Extractor::new();

    let result1 = extractor.extract_file("file1.rs", "fn shared_helper() {}");
    let mut id_map1 = std::collections::HashMap::new();
    for mut node in result1.nodes {
        let old_id = node.id;
        node.id = 0;
        let new_id = db.insert_node(&node).unwrap();
        id_map1.insert(old_id, new_id);
    }

    let result2 = extractor.extract_file("file2.rs", "fn caller() { shared_helper(); }");
    let mut id_map2 = std::collections::HashMap::new();
    for mut node in result2.nodes {
        let old_id = node.id;
        node.id = 0;
        let new_id = db.insert_node(&node).unwrap();
        id_map2.insert(old_id, new_id);
    }

    // Insert unresolved references
    for mut uref in result2.unresolved_refs {
        if let Some(&new_source) = id_map2.get(&uref.source_node_id) {
            uref.source_node_id = new_source;
            db.insert_unresolved_ref(&uref).unwrap();
        }
    }

    // Resolve cross-file references
    let resolved = db.resolve_references().unwrap();
    assert!(resolved >= 1);

    // Verify the cross-file call was resolved
    let graph = Graph::new(&db);
    let callers = graph.find_callers("shared_helper", 10).unwrap();
    assert!(!callers.is_empty());
}

#[test]
fn test_search_performance() {
    let db = Database::in_memory().unwrap();
    let file = FileRecord {
        path: "many.rs".to_string(),
        content_hash: "hash".to_string(),
        language: Language::Rust,
        size: 1000,
        modified_at: 0,
        indexed_at: 0,
        node_count: 0,
    };
    db.insert_or_update_file(&file).unwrap();

    // Insert many similar nodes
    let mut extractor = Extractor::new();
    let code = (0..1000)
        .map(|i| format!("fn process_item_{}() {{}}", i))
        .collect::<Vec<_>>()
        .join("\n");

    let result = extractor.extract_file("many.rs", &code);
    for mut node in result.nodes {
        node.id = 0;
        db.insert_node(&node).unwrap();
    }

    // Measure search performance with limit
    let start = std::time::Instant::now();
    let results = db.search_nodes("process", None, 10).unwrap();
    let duration = start.elapsed();
    assert_eq!(results.len(), 10);
    assert!(
        duration.as_millis() < 50,
        "Search took too long: {:?}",
        duration
    );

    let start = std::time::Instant::now();
    let results = db.search_nodes("process", None, 100).unwrap();
    let duration = start.elapsed();
    assert_eq!(results.len(), 100);
    assert!(
        duration.as_millis() < 100,
        "Search took too long: {:?}",
        duration
    );
}

/// Stress test: exercise the periodic-commit / WAL-checkpoint branch for
/// in-place incremental indexing by crossing CHECKPOINT_INTERVAL (200).
/// Uses an on-disk database to mirror the write path that still batches
/// commits mid-loop.
#[test]
fn test_incremental_index_codebase_periodic_checkpoint() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir(&src).unwrap();

    // 250 small Rust files — enough to cross the 200-file checkpoint boundary.
    for i in 0..250 {
        let path = src.join(format!("mod_{:03}.rs", i));
        let body = format!(
            "pub fn handler_{i}() {{ helper_{i}(); }}\nfn helper_{i}() {{}}\n",
            i = i
        );
        fs::write(&path, body).unwrap();
    }

    let db_path = dir.path().join("symgraph.db");
    let mut db = Database::open(&db_path).unwrap();
    let config = IndexConfig {
        root: dir.path().display().to_string(),
        ..Default::default()
    };

    let stats = index_codebase(&mut db, &config).expect("index_codebase failed");
    assert_eq!(stats.files, 250);
    assert!(stats.nodes >= 500);
}
