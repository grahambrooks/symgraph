//! Database schema definition

pub const SCHEMA: &str = r#"
-- Files table: tracks indexed files
CREATE TABLE IF NOT EXISTS files (
    path TEXT PRIMARY KEY,
    content_hash TEXT NOT NULL,
    language TEXT NOT NULL,
    size INTEGER NOT NULL,
    modified_at INTEGER NOT NULL,
    indexed_at INTEGER NOT NULL,
    node_count INTEGER NOT NULL DEFAULT 0
);

-- Nodes table: code symbols (functions, classes, methods, etc.)
CREATE TABLE IF NOT EXISTS nodes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    qualified_name TEXT,
    file_path TEXT NOT NULL,
    start_line INTEGER NOT NULL,
    end_line INTEGER NOT NULL,
    start_column INTEGER NOT NULL,
    end_column INTEGER NOT NULL,
    signature TEXT,
    visibility TEXT NOT NULL DEFAULT 'unknown',
    docstring TEXT,
    is_async INTEGER NOT NULL DEFAULT 0,
    is_static INTEGER NOT NULL DEFAULT 0,
    is_exported INTEGER NOT NULL DEFAULT 0,
    is_test INTEGER NOT NULL DEFAULT 0,
    is_generated INTEGER NOT NULL DEFAULT 0,
    language TEXT NOT NULL,
    FOREIGN KEY (file_path) REFERENCES files(path)
);

-- Edges table: relationships between nodes
CREATE TABLE IF NOT EXISTS edges (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_id INTEGER NOT NULL,
    target_id INTEGER NOT NULL,
    kind TEXT NOT NULL,
    file_path TEXT,
    line INTEGER,
    column INTEGER,
    FOREIGN KEY (source_id) REFERENCES nodes(id),
    FOREIGN KEY (target_id) REFERENCES nodes(id)
);

-- Unresolved references: references that couldn't be resolved during extraction
CREATE TABLE IF NOT EXISTS unresolved_refs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_node_id INTEGER NOT NULL,
    reference_name TEXT NOT NULL,
    kind TEXT NOT NULL,
    file_path TEXT NOT NULL,
    line INTEGER NOT NULL,
    column INTEGER NOT NULL,
    FOREIGN KEY (source_node_id) REFERENCES nodes(id)
);

-- Indexes for efficient queries
CREATE INDEX IF NOT EXISTS idx_nodes_name ON nodes(name);
CREATE INDEX IF NOT EXISTS idx_nodes_name_lower ON nodes(LOWER(name));
CREATE INDEX IF NOT EXISTS idx_nodes_file_path ON nodes(file_path);
CREATE INDEX IF NOT EXISTS idx_nodes_kind ON nodes(kind);
CREATE INDEX IF NOT EXISTS idx_nodes_qualified_name ON nodes(qualified_name);

CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source_id);
CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target_id);
CREATE INDEX IF NOT EXISTS idx_edges_kind ON edges(kind);

CREATE INDEX IF NOT EXISTS idx_unresolved_name ON unresolved_refs(reference_name);

-- Full-text search for symbol names (external-content FTS5 table mirroring nodes)
CREATE VIRTUAL TABLE IF NOT EXISTS nodes_fts USING fts5(name, qualified_name, content=nodes, content_rowid=id);

-- Semantic search: tokenized identifier + docstring for bm25-based scoring.
-- Standalone (not external-content) so we fully control token content.
CREATE VIRTUAL TABLE IF NOT EXISTS nodes_semantic_fts USING fts5(tokens);

CREATE INDEX IF NOT EXISTS idx_nodes_is_test ON nodes(is_test);
CREATE INDEX IF NOT EXISTS idx_nodes_is_generated ON nodes(is_generated);
"#;

/// Additive schema migrations applied after CREATE TABLE IF NOT EXISTS.
///
/// Each statement is executed independently; "duplicate column name" errors
/// are ignored to support upgrade-in-place on existing databases.
pub const MIGRATIONS: &[&str] = &[
    "ALTER TABLE nodes ADD COLUMN is_test INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE nodes ADD COLUMN is_generated INTEGER NOT NULL DEFAULT 0",
];
