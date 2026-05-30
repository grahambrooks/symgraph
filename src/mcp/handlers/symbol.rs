//! Symbol information handlers (node, definition, references).
//!
//! Thin wrappers over `crate::ops`: gather a typed result, then render it as
//! markdown or JSON via `present`.

use crate::db::Database;
use crate::mcp::types::{DefinitionRequest, SymbolRequest};
use crate::ops::{self, present, Format, NotFound};

pub fn handle_node(db: &Database, req: &SymbolRequest) -> Result<String, String> {
    let fmt = Format::from_request(&req.format);
    match ops::node_info(db, &req.symbol)? {
        Some(r) => present(&r, fmt),
        None => present(&NotFound::new(&req.symbol), fmt),
    }
}

pub fn handle_definition(
    db: &Database,
    project_root: &str,
    req: &DefinitionRequest,
) -> Result<String, String> {
    let fmt = Format::from_request(&req.format);
    match ops::definition(db, project_root, &req.symbol, req.context_lines)? {
        Some(r) => present(&r, fmt),
        None => present(&NotFound::new(&req.symbol), fmt),
    }
}

pub fn handle_references(db: &Database, req: &SymbolRequest) -> Result<String, String> {
    let fmt = Format::from_request(&req.format);
    match ops::references(db, &req.symbol)? {
        Some(r) => present(&r, fmt),
        None => present(&NotFound::new(&req.symbol), fmt),
    }
}

#[cfg(test)]
mod security_tests {
    use super::*;
    use crate::types::{Language, Node, NodeKind, Visibility};

    fn seed(db: &Database, file_path: &str) {
        let file = crate::types::FileRecord {
            path: file_path.to_string(),
            content_hash: "h".into(),
            language: Language::Rust,
            size: 0,
            modified_at: 0,
            indexed_at: 0,
            node_count: 1,
        };
        db.insert_or_update_file(&file).unwrap();
        let node = Node {
            id: 0,
            kind: NodeKind::Function,
            name: "victim".to_string(),
            qualified_name: None,
            file_path: file_path.to_string(),
            start_line: 1,
            end_line: 2,
            start_column: 0,
            end_column: 0,
            signature: None,
            visibility: Visibility::Public,
            docstring: None,
            is_async: false,
            is_static: false,
            is_exported: false,
            is_test: false,
            is_generated: false,
            language: Language::Rust,
        };
        db.insert_node(&node).unwrap();
    }

    #[test]
    fn definition_rejects_traversal_file_path_in_db() {
        // Simulate an attacker-controlled DB entry with a traversal path.
        let tmp = tempfile::tempdir().unwrap();
        let db = Database::in_memory().unwrap();
        seed(&db, "../../../etc/passwd");

        let req = DefinitionRequest {
            symbol: "victim".into(),
            context_lines: None,
            format: None,
        };
        let err = handle_definition(&db, tmp.path().to_str().unwrap(), &req).unwrap_err();
        assert!(
            err.contains("path security") || err.contains("traversal"),
            "got: {err}"
        );
    }
}
