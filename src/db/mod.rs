//! Database module for symgraph
//!
//! Handles SQLite storage for the code graph including:
//! - Schema creation and migrations
//! - Node and edge storage
//! - File tracking
//! - Query operations

mod schema;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::types::{
    Edge, EdgeKind, FileRecord, IndexStats, Language, Node, NodeKind, UnresolvedReference,
    Visibility,
};

// `auto_vacuum = INCREMENTAL` is set first, before any table is created, so a
// freshly created database carries it in its file header (changing it later
// would require a full VACUUM). It keeps freed pages on a freelist that
// `compact()` / `PRAGMA incremental_vacuum` can return to the OS, so the index
// file does not bloat as files are deleted and reindexed over time.
//
// `busy_timeout` matters because one index is routinely open in several
// processes at once — the MCP server, a CLI query, and `symgraph-cli watch`
// all resolve to the same file. Without it SQLite returns `SQLITE_BUSY` the
// instant a writer holds the lock, so a query fails rather than waiting out a
// reindex that would have finished in milliseconds.
const CONNECTION_PRAGMAS: &str = "PRAGMA auto_vacuum = INCREMENTAL; \
             PRAGMA foreign_keys = ON; \
             PRAGMA journal_mode = WAL; \
             PRAGMA synchronous = NORMAL; \
             PRAGMA busy_timeout = 5000; \
             PRAGMA cache_size = -64000;";

/// Why an existing index cannot be trusted as-is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Staleness {
    /// Built before symgraph recorded provenance at all.
    Unversioned,
    /// The SQL schema has changed since this index was written.
    Schema { found: u32, expected: u32 },
    /// The schema still fits, but the extractor now produces different nodes
    /// and edges for the same source. The rows read fine and mean something
    /// else — the failure mode that makes this worth tracking separately.
    Extractor { found: u32, expected: u32 },
}

impl std::fmt::Display for Staleness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Staleness::Unversioned => {
                write!(f, "index predates version tracking")
            }
            Staleness::Schema { found, expected } => write!(
                f,
                "index schema v{}, this symgraph expects v{}",
                found, expected
            ),
            Staleness::Extractor { found, expected } => write!(
                f,
                "index built by extractor v{}, this symgraph expects v{}",
                found, expected
            ),
        }
    }
}

/// Provenance recorded when an index was built.
#[derive(Debug, Clone, serde::Serialize)]
pub struct IndexVersion {
    pub schema: u32,
    pub extractor: u32,
    /// The `symgraph` crate version that built the index.
    pub symgraph: String,
    /// Unix seconds at which the build finished.
    pub built_at: i64,
}

/// Which references a resolution pass handles. Imports go first because the
/// file scope they establish is what the second pass resolves against.
#[derive(Debug, Clone, Copy)]
enum Phase {
    Imports,
    Rest,
}

/// A SQL predicate over `u.kind` (the reference) and `n.kind` (the candidate
/// definition) that is true only where the two are compatible.
///
/// Built once from [`EdgeKind::resolvable_target_kinds`] so the rule lives in
/// one place rather than being restated in SQL. Kinds with no constraint fall
/// through to the `ELSE 1`, as does any kind string the enum does not know.
fn kind_compatible_sql() -> &'static str {
    static SQL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        let mut sql = String::from("CASE u.kind");
        for edge in EdgeKind::ALL {
            let Some(kinds) = edge.resolvable_target_kinds() else {
                continue;
            };
            let list = kinds
                .iter()
                .map(|k| format!("'{}'", k.as_str()))
                .collect::<Vec<_>>()
                .join(",");
            sql.push_str(&format!(
                " WHEN '{}' THEN n.kind IN ({})",
                edge.as_str(),
                list
            ));
        }
        sql.push_str(" ELSE 1 END");
        sql
    });
    SQL.as_str()
}

/// How far the index can be trusted, as opposed to how large it is.
#[derive(Debug, Clone, serde::Serialize)]
pub struct IndexHealth {
    /// Set when the index was built by an incompatible symgraph.
    pub staleness: Option<String>,
    pub version: Option<IndexVersion>,
    /// Symbol names carried by more than one definition.
    pub ambiguous_names: u64,
    /// Distinct symbol names in the index.
    pub distinct_names: u64,
    /// References that resolved to no definition (third-party calls, and
    /// anything extraction failed to link).
    pub unresolved_refs: u64,
    /// The FTS index still agrees with the `nodes` table.
    pub fts_ok: bool,
}

impl IndexHealth {
    /// Share of names that more than one definition answers to, as a
    /// percentage. The higher this is, the more of the call graph rests on a
    /// guess.
    pub fn ambiguous_percent(&self) -> f64 {
        if self.distinct_names == 0 {
            return 0.0;
        }
        self.ambiguous_names as f64 * 100.0 / self.distinct_names as f64
    }
}

/// Deterministic preference order among definitions that share one name.
///
/// Production code outranks tests and generated code — a `new` in a test
/// fixture should never stand in for the real one — and the remaining columns
/// are a stable tiebreak, so the same index always resolves a name the same
/// way. Before this existed the fallback was a bare `LIMIT 1`, which made call
/// attribution (and therefore every coupling number derived from it) depend on
/// insertion order.
const SYMBOL_PREFERENCE: &str =
    "is_test ASC, is_generated ASC, file_path ASC, start_line ASC, id ASC";

/// How many competing definitions to carry back for the "did you mean" list.
const MAX_ALTERNATIVES: usize = 5;

/// Narrowing hints a caller supplies to pick between same-named definitions.
#[derive(Debug, Default, Clone)]
pub struct SymbolHint {
    /// Restrict to definitions in this file (repo-relative path).
    pub file: Option<String>,
    /// Restrict to definitions with this exact qualified name.
    pub qualified_name: Option<String>,
}

impl SymbolHint {
    pub fn is_empty(&self) -> bool {
        self.file.is_none() && self.qualified_name.is_none()
    }
}

/// A symbol name resolved to one definition, carrying enough context for the
/// caller to say how confident that resolution was.
#[derive(Debug, Clone)]
pub struct SymbolMatch {
    /// The chosen definition: first under [`SYMBOL_PREFERENCE`].
    pub node: Node,
    /// How many definitions matched the name (and hints) in total.
    pub candidates: usize,
    /// Up to [`MAX_ALTERNATIVES`] of the definitions that were *not* chosen.
    pub alternatives: Vec<Node>,
}

impl SymbolMatch {
    /// More than one definition matched, so the chosen one is a guess.
    pub fn is_ambiguous(&self) -> bool {
        self.candidates > 1
    }
}

/// Database handle for the code graph
pub struct Database {
    conn: Connection,
    path: Option<PathBuf>,
}

/// Resolved edges reduced to their source/target file paths and kind, with
/// identical rows collapsed into a count — the raw material for folding the
/// graph to a module/file boundary.
///
/// Aggregated rather than one row per edge because the consumers only ever
/// count by `(source_file, target_file, kind)`. On symgraph's own index that
/// is 559 rows instead of 7246, and the ratio grows with the codebase.
#[derive(Debug, Clone)]
pub struct EdgeEndpoint {
    pub source_file: String,
    pub target_file: String,
    pub kind: EdgeKind,
    pub detail: Option<String>,
    /// How many individual edges this row stands for.
    pub count: u32,
}

impl Database {
    /// Open or create a database at the given path
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let conn = Connection::open(&path)?;
        Self::from_connection(conn, Some(path))
    }

    /// Create an in-memory database (for testing)
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::from_connection(conn, None)
    }

    /// Shared initialization: set PRAGMAs and create schema
    fn from_connection(conn: Connection, path: Option<PathBuf>) -> Result<Self> {
        conn.execute_batch(CONNECTION_PRAGMAS)?;
        let db = Self { conn, path };
        db.initialize()?;
        Ok(db)
    }

    /// Read a value from `index_meta`.
    fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM index_meta WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .optional()?)
    }

    fn meta_u32(&self, key: &str) -> Result<Option<u32>> {
        Ok(self.meta(key)?.and_then(|v| v.parse().ok()))
    }

    /// The provenance recorded for this index, or `None` if it was built
    /// before symgraph tracked it.
    pub fn index_version(&self) -> Result<Option<IndexVersion>> {
        let (Some(schema), Some(extractor)) = (
            self.meta_u32("schema_version")?,
            self.meta_u32("extractor_version")?,
        ) else {
            return Ok(None);
        };
        Ok(Some(IndexVersion {
            schema,
            extractor,
            symgraph: self.meta("symgraph_version")?.unwrap_or_default(),
            built_at: self
                .meta("built_at")?
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
        }))
    }

    /// Stamp this index with the schema, extractor and crate versions that
    /// built it. Called once a full build completes, so a half-written index
    /// never looks current.
    pub fn record_index_version(&self) -> Result<()> {
        let built_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let entries: [(&str, String); 4] = [
            ("schema_version", schema::SCHEMA_VERSION.to_string()),
            ("extractor_version", schema::EXTRACTOR_VERSION.to_string()),
            ("symgraph_version", env!("CARGO_PKG_VERSION").to_string()),
            ("built_at", built_at.to_string()),
        ];
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO index_meta (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )?;
        for (key, value) in entries {
            stmt.execute(params![key, value])?;
        }
        Ok(())
    }

    /// Signals about how far the index can be trusted, independent of how big
    /// it is.
    ///
    /// These are the numbers that answer "should I believe this result?" —
    /// `get_stats` answers "how much is in here", which is a different
    /// question and was the only one the status tool used to ask.
    pub fn health(&self) -> Result<IndexHealth> {
        // Names carried by more than one definition. Every one of these is a
        // place where name-based resolution has to guess (see `resolve_symbol`),
        // so the ratio is a direct read on how much of the graph is approximate.
        let ambiguous_names: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM (SELECT name FROM nodes WHERE kind != 'file' \
             GROUP BY name HAVING COUNT(*) > 1)",
            [],
            |row| row.get(0),
        )?;
        let distinct_names: i64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT name) FROM nodes WHERE kind != 'file'",
            [],
            |row| row.get(0),
        )?;
        // References the resolver could not attach to any definition — calls
        // into third-party crates, but also anything extraction got wrong.
        let unresolved_refs: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM unresolved_refs", [], |row| row.get(0))?;

        Ok(IndexHealth {
            staleness: self.staleness()?.map(|s| s.to_string()),
            version: self.index_version()?,
            ambiguous_names: ambiguous_names as u64,
            distinct_names: distinct_names as u64,
            unresolved_refs: unresolved_refs as u64,
            fts_ok: self.check_fts_integrity(),
        })
    }

    /// Verify the external-content FTS index still agrees with `nodes`.
    ///
    /// `nodes_fts` is maintained by hand rather than by triggers, so a bug in
    /// the insert or delete path desynchronises it and search silently starts
    /// missing or inventing rows. FTS5 can check this for us.
    fn check_fts_integrity(&self) -> bool {
        self.conn
            .execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('integrity-check');")
            .is_ok()
    }

    /// Why this index cannot be trusted, or `None` when it is current.
    ///
    /// An empty index is never stale — there is nothing in it to be wrong.
    pub fn staleness(&self) -> Result<Option<Staleness>> {
        let empty: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))?;
        if empty == 0 {
            return Ok(None);
        }

        let Some(version) = self.index_version()? else {
            return Ok(Some(Staleness::Unversioned));
        };
        if version.schema != schema::SCHEMA_VERSION {
            return Ok(Some(Staleness::Schema {
                found: version.schema,
                expected: schema::SCHEMA_VERSION,
            }));
        }
        if version.extractor != schema::EXTRACTOR_VERSION {
            return Ok(Some(Staleness::Extractor {
                found: version.extractor,
                expected: schema::EXTRACTOR_VERSION,
            }));
        }
        Ok(None)
    }

    /// Path to the on-disk database, if this handle is file-backed.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Close the underlying SQLite connection.
    pub fn close(self) -> Result<()> {
        match self.conn.close() {
            Ok(()) => Ok(()),
            Err((_conn, err)) => Err(err.into()),
        }
    }

    /// Initialize the database schema and run additive migrations.
    fn initialize(&self) -> Result<()> {
        self.conn.execute_batch(schema::SCHEMA)?;
        for stmt in schema::MIGRATIONS {
            if let Err(e) = self.conn.execute(stmt, []) {
                let msg = e.to_string();
                if !msg.contains("duplicate column name") {
                    return Err(e.into());
                }
            }
        }
        Ok(())
    }

    // =========================================================================
    // File Operations
    // =========================================================================

    /// Insert or update a file record (upsert operation)
    pub fn insert_or_update_file(&self, file: &FileRecord) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO files (path, content_hash, language, size, modified_at, indexed_at, node_count)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(path) DO UPDATE SET
                content_hash = excluded.content_hash,
                language = excluded.language,
                size = excluded.size,
                modified_at = excluded.modified_at,
                indexed_at = excluded.indexed_at,
                node_count = excluded.node_count
            "#,
            params![
                file.path,
                file.content_hash,
                file.language.as_str(),
                file.size as i64,
                file.modified_at,
                file.indexed_at,
                file.node_count as i64,
            ],
        )?;
        Ok(())
    }

    /// Get a file record by path
    pub fn get_file(&self, path: &str) -> Result<Option<FileRecord>> {
        let result = self
            .conn
            .query_row(
                "SELECT path, content_hash, language, size, modified_at, indexed_at, node_count FROM files WHERE path = ?1",
                params![path],
                |row| {
                    Ok(FileRecord {
                        path: row.get(0)?,
                        content_hash: row.get(1)?,
                        language: Language::from_extension(row.get::<_, String>(2)?.as_str()),
                        size: row.get::<_, i64>(3)? as u64,
                        modified_at: row.get(4)?,
                        indexed_at: row.get(5)?,
                        node_count: row.get::<_, i64>(6)? as u32,
                    })
                },
            )
            .optional()?;
        Ok(result)
    }

    /// Whether the indexed record for `path` already has this exact size and
    /// mtime — in which case the file can be taken as unchanged without
    /// reading it.
    ///
    /// A cheap pre-filter for [`Database::needs_reindex`], which is
    /// authoritative but needs the file's contents to compute a hash.
    pub fn matches_indexed_stat(&self, path: &str, size: u64, modified_at: i64) -> Result<bool> {
        let matched: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM files WHERE path = ?1 AND size = ?2 AND modified_at = ?3",
                params![path, size as i64, modified_at],
                |row| row.get(0),
            )
            .optional()?;
        Ok(matched.is_some())
    }

    /// Check if a file needs reindexing
    pub fn needs_reindex(&self, path: &str, content_hash: &str) -> Result<bool> {
        match self.get_file(path)? {
            Some(file) => Ok(file.content_hash != content_hash),
            None => Ok(true),
        }
    }

    /// Delete a file and its nodes/edges
    pub fn delete_file(&self, path: &str) -> Result<()> {
        // Delete edges where source or target is in this file
        self.conn.execute(
            "DELETE FROM edges WHERE source_id IN (SELECT id FROM nodes WHERE file_path = ?1)",
            params![path],
        )?;
        self.conn.execute(
            "DELETE FROM edges WHERE target_id IN (SELECT id FROM nodes WHERE file_path = ?1)",
            params![path],
        )?;
        // Delete FTS entries for nodes in this file
        self.conn.execute(
            "INSERT INTO nodes_fts(nodes_fts, rowid, name, qualified_name) SELECT 'delete', id, name, qualified_name FROM nodes WHERE file_path = ?1",
            params![path],
        )?;
        // Delete semantic FTS entries (standalone table — direct delete by rowid)
        self.conn.execute(
            "DELETE FROM nodes_semantic_fts WHERE rowid IN (SELECT id FROM nodes WHERE file_path = ?1)",
            params![path],
        )?;
        // Unresolved refs before nodes: `unresolved_refs.source_node_id` has a
        // foreign key to `nodes.id`, so dropping the nodes first fails the
        // constraint. That went unnoticed while resolution emptied the table
        // on every pass; once unresolved refs began to be *kept* (so the
        // health figure could mean something), every targeted reindex started
        // failing here instead.
        //
        // Matched on `source_node_id` rather than `file_path`: a ref recorded
        // against a node in this file may carry a different `file_path`, and
        // leaving it behind would re-fail the constraint.
        self.conn.execute(
            "DELETE FROM unresolved_refs WHERE file_path = ?1 \
             OR source_node_id IN (SELECT id FROM nodes WHERE file_path = ?1)",
            params![path],
        )?;
        // Delete nodes
        self.conn
            .execute("DELETE FROM nodes WHERE file_path = ?1", params![path])?;
        // Delete file record
        self.conn
            .execute("DELETE FROM files WHERE path = ?1", params![path])?;
        Ok(())
    }

    // =========================================================================
    // Node Operations
    // =========================================================================

    /// Insert a node and return its ID
    pub fn insert_node(&self, node: &Node) -> Result<i64> {
        self.conn.execute(
            r#"
            INSERT INTO nodes (
                kind, name, qualified_name, file_path, start_line, end_line,
                start_column, end_column, signature, visibility, docstring,
                is_async, is_static, is_exported, is_test, is_generated, language
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
            "#,
            params![
                node.kind.as_str(),
                node.name,
                node.qualified_name,
                node.file_path,
                node.start_line as i64,
                node.end_line as i64,
                node.start_column as i64,
                node.end_column as i64,
                node.signature,
                node.visibility.as_str(),
                node.docstring,
                node.is_async,
                node.is_static,
                node.is_exported,
                node.is_test,
                node.is_generated,
                node.language.as_str(),
            ],
        )?;
        let rowid = self.conn.last_insert_rowid();

        // Insert into FTS index
        self.conn.execute(
            "INSERT INTO nodes_fts(rowid, name, qualified_name) VALUES (?1, ?2, ?3)",
            params![rowid, node.name, node.qualified_name],
        )?;

        // Semantic FTS: camelCase/snake_case tokens + docstring
        let tokens = build_semantic_tokens(node);
        self.conn.execute(
            "INSERT INTO nodes_semantic_fts(rowid, tokens) VALUES (?1, ?2)",
            params![rowid, tokens],
        )?;

        Ok(rowid)
    }

    /// Get a node by ID
    pub fn get_node(&self, id: i64) -> Result<Option<Node>> {
        let result = self
            .conn
            .query_row("SELECT * FROM nodes WHERE id = ?1", params![id], |row| {
                Self::row_to_node(row)
            })
            .optional()?;
        Ok(result)
    }

    /// Search nodes by name using FTS5 full-text search with LIKE fallback.
    ///
    /// Uses FTS5 prefix matching for queries of 2+ characters, falling back to
    /// LIKE-based search for single-character queries or if the FTS query fails.
    pub fn search_nodes(
        &self,
        query: &str,
        kind: Option<NodeKind>,
        limit: u32,
    ) -> Result<Vec<Node>> {
        // FTS5 requires at least 2 characters for useful prefix matching
        let use_fts = query.len() >= 2;

        if use_fts {
            // FTS5 prefix query: lowercase the query and append * for prefix matching
            let fts_query = format!("\"{}\"*", query.to_lowercase());

            let sql = if kind.is_some() {
                r#"
                SELECT n.* FROM nodes n
                INNER JOIN nodes_fts fts ON n.id = fts.rowid
                WHERE nodes_fts MATCH ?1 AND n.kind = ?2
                ORDER BY LENGTH(n.name), n.name
                LIMIT ?3
                "#
            } else {
                r#"
                SELECT n.* FROM nodes n
                INNER JOIN nodes_fts fts ON n.id = fts.rowid
                WHERE nodes_fts MATCH ?1
                ORDER BY LENGTH(n.name), n.name
                LIMIT ?2
                "#
            };

            let result = (|| -> Result<Vec<Node>> {
                let mut stmt = self.conn.prepare(sql)?;
                let mut nodes = Vec::new();

                if let Some(k) = kind {
                    let rows = stmt.query_map(
                        params![fts_query, k.as_str(), limit as i64],
                        Self::row_to_node,
                    )?;
                    for row in rows {
                        nodes.push(row?);
                    }
                } else {
                    let rows =
                        stmt.query_map(params![fts_query, limit as i64], Self::row_to_node)?;
                    for row in rows {
                        nodes.push(row?);
                    }
                }

                Ok(nodes)
            })();

            // If FTS succeeds, return results; otherwise fall through to LIKE.
            // The fallback silently returns *different* (prefix-only) results,
            // so a desynchronised or corrupt FTS index would degrade search
            // quality with nothing anywhere to say why.
            match result {
                Ok(nodes) => return Ok(nodes),
                Err(e) => tracing::warn!(
                    query = query,
                    error = %e,
                    "FTS search failed; falling back to LIKE. Search quality is degraded — \
                     run `symgraph-status` to check index integrity."
                ),
            }
        }

        // Fallback: LIKE-based search for short queries or FTS failures
        let pattern = format!("{}%", query.to_lowercase());

        let sql = if kind.is_some() {
            r#"
            SELECT * FROM nodes
            WHERE LOWER(name) LIKE ?1 AND kind = ?2
            ORDER BY LENGTH(name), name
            LIMIT ?3
            "#
        } else {
            r#"
            SELECT * FROM nodes
            WHERE LOWER(name) LIKE ?1
            ORDER BY LENGTH(name), name
            LIMIT ?2
            "#
        };

        let mut stmt = self.conn.prepare(sql)?;
        let mut nodes = Vec::new();

        if let Some(k) = kind {
            let rows = stmt.query_map(params![pattern, k.as_str(), limit as i64], |row| {
                Self::row_to_node(row)
            })?;
            for row in rows {
                nodes.push(row?);
            }
        } else {
            let rows = stmt.query_map(params![pattern, limit as i64], Self::row_to_node)?;
            for row in rows {
                nodes.push(row?);
            }
        }

        Ok(nodes)
    }

    /// Get nodes by file path
    pub fn get_nodes_by_file(&self, file_path: &str) -> Result<Vec<Node>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM nodes WHERE file_path = ?1 ORDER BY start_line")?;
        let rows = stmt.query_map(params![file_path], Self::row_to_node)?;

        let mut nodes = Vec::new();
        for row in rows {
            nodes.push(row?);
        }
        Ok(nodes)
    }

    /// Resolved edges folded to file pairs, aggregated in SQL.
    ///
    /// The bulk-edge accessor used to fold the graph to a file / dir / module
    /// boundary for coupling analysis. Self-edges (same source and target
    /// file) are kept; callers filter them out as needed.
    ///
    /// The `GROUP BY` is what keeps this from materialising the whole edge
    /// table in memory on every module-graph and coupling-score call. It is
    /// lossless for the consumers, which count rather than inspect
    /// individual edges.
    pub fn get_edge_endpoints(&self, include_tests: bool) -> Result<Vec<EdgeEndpoint>> {
        // Test code is excluded by default. It is a third of the edges on
        // symgraph's own index, and folding it into the architecture graph
        // makes a test file look like a module every production module
        // depends on — which is backwards, and enough on its own to merge
        // unrelated modules into one enormous cycle.
        let test_filter = if include_tests {
            ""
        } else {
            " WHERE s.is_test = 0 AND t.is_test = 0"
        };
        let mut stmt = self.conn.prepare(&format!(
            "SELECT s.file_path, t.file_path, e.kind, e.detail, COUNT(*) \
             FROM edges e \
             JOIN nodes s ON e.source_id = s.id \
             JOIN nodes t ON e.target_id = t.id\
             {test_filter} \
             GROUP BY s.file_path, t.file_path, e.kind, e.detail"
        ))?;
        let rows = stmt.query_map([], |row| {
            Ok(EdgeEndpoint {
                source_file: row.get(0)?,
                target_file: row.get(1)?,
                kind: EdgeKind::parse(&row.get::<_, String>(2)?).unwrap_or(EdgeKind::References),
                detail: row.get(3)?,
                count: row.get::<_, i64>(4)? as u32,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// All nodes of a given kind (e.g. every struct/class), optionally
    /// including ones defined in test code.
    pub fn get_nodes_by_kind(&self, kind: NodeKind, include_tests: bool) -> Result<Vec<Node>> {
        let test_filter = if include_tests {
            ""
        } else {
            " AND is_test = 0"
        };
        let mut stmt = self.conn.prepare(&format!(
            "SELECT * FROM nodes WHERE kind = ?1{test_filter} ORDER BY name"
        ))?;
        let rows = stmt.query_map(params![kind.as_str()], Self::row_to_node)?;
        let mut nodes = Vec::new();
        for row in rows {
            nodes.push(row?);
        }
        Ok(nodes)
    }

    /// Fields/properties contained by the named struct/class/interface.
    pub fn get_struct_fields(&self, struct_name: &str) -> Result<Vec<Node>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.* FROM nodes t \
             JOIN edges e ON e.target_id = t.id AND e.kind = 'contains' \
             JOIN nodes s ON e.source_id = s.id \
             WHERE s.name = ?1 AND s.kind IN ('struct','class','interface','trait','protocol') \
             AND t.kind IN ('field','property')",
        )?;
        let rows = stmt.query_map(params![struct_name], Self::row_to_node)?;
        let mut nodes = Vec::new();
        for row in rows {
            nodes.push(row?);
        }
        Ok(nodes)
    }

    /// Files that dispatch on a member of the named enum (control coupling).
    /// Returns (file_path, member_name) pairs for `references` edges whose
    /// target is an enum member contained by `enum_name`.
    pub fn get_dispatch_sites(&self, enum_name: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT e.file_path, m.name \
             FROM edges e \
             JOIN nodes m ON e.target_id = m.id AND m.kind = 'enum_member' \
             JOIN edges c ON c.target_id = m.id AND c.kind = 'contains' \
             JOIN nodes en ON c.source_id = en.id AND en.kind = 'enum' \
             WHERE e.kind = 'references' AND en.name = ?1 AND e.file_path IS NOT NULL",
        )?;
        let rows = stmt.query_map(params![enum_name], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Resolve a symbol name to a single definition, reporting how many
    /// definitions competed for it.
    ///
    /// Callers that need to know whether the answer is trustworthy should use
    /// this rather than [`Database::find_node_by_name`]: `candidates > 1` means
    /// the name was ambiguous and the chosen definition is only the
    /// highest-preference one, not the only one.
    pub fn resolve_symbol(&self, name: &str, hint: &SymbolHint) -> Result<Option<SymbolMatch>> {
        let mut where_sql = String::from("name = ?1");
        let mut args: Vec<&dyn rusqlite::ToSql> = vec![&name];
        if let Some(file) = &hint.file {
            args.push(file);
            where_sql.push_str(&format!(" AND file_path = ?{}", args.len()));
        }
        if let Some(qualified) = &hint.qualified_name {
            args.push(qualified);
            where_sql.push_str(&format!(" AND qualified_name = ?{}", args.len()));
        }

        let candidates: i64 = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM nodes WHERE {}", where_sql),
            args.as_slice(),
            |row| row.get(0),
        )?;
        if candidates == 0 {
            return Ok(None);
        }

        // One row for the match plus MAX_ALTERNATIVES to report alongside it.
        let mut stmt = self.conn.prepare(&format!(
            "SELECT * FROM nodes WHERE {} ORDER BY {} LIMIT {}",
            where_sql,
            SYMBOL_PREFERENCE,
            MAX_ALTERNATIVES + 1
        ))?;
        let mut rows = stmt.query_map(args.as_slice(), Self::row_to_node)?;

        let node = match rows.next() {
            Some(row) => row?,
            None => return Ok(None),
        };
        let mut alternatives = Vec::new();
        for row in rows {
            alternatives.push(row?);
        }

        Ok(Some(SymbolMatch {
            node,
            candidates: candidates as usize,
            alternatives,
        }))
    }

    /// Find a node by name (exact match), taking the highest-preference
    /// definition when several share the name. Prefer
    /// [`Database::resolve_symbol`] when the caller can surface ambiguity.
    pub fn find_node_by_name(&self, name: &str) -> Result<Option<Node>> {
        Ok(self
            .resolve_symbol(name, &SymbolHint::default())?
            .map(|m| m.node))
    }

    fn row_to_node(row: &rusqlite::Row) -> rusqlite::Result<Node> {
        Ok(Node {
            id: row.get("id")?,
            kind: NodeKind::parse(&row.get::<_, String>("kind")?).unwrap_or(NodeKind::Function),
            name: row.get("name")?,
            qualified_name: row.get("qualified_name")?,
            file_path: row.get("file_path")?,
            start_line: row.get::<_, i64>("start_line")? as u32,
            end_line: row.get::<_, i64>("end_line")? as u32,
            start_column: row.get::<_, i64>("start_column")? as u32,
            end_column: row.get::<_, i64>("end_column")? as u32,
            signature: row.get("signature")?,
            visibility: Visibility::parse(&row.get::<_, String>("visibility").unwrap_or_default()),
            docstring: row.get("docstring")?,
            is_async: row.get("is_async")?,
            is_static: row.get("is_static")?,
            is_exported: row.get("is_exported")?,
            is_test: row.get("is_test").unwrap_or(false),
            is_generated: row.get("is_generated").unwrap_or(false),
            language: Language::parse(&row.get::<_, String>("language").unwrap_or_default()),
        })
    }

    // =========================================================================
    // Edge Operations
    // =========================================================================

    /// Insert an edge
    pub fn insert_edge(&self, edge: &Edge) -> Result<i64> {
        self.conn.execute(
            r#"
            INSERT INTO edges (source_id, target_id, kind, file_path, line, column, detail)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                edge.source_id,
                edge.target_id,
                edge.kind.as_str(),
                edge.file_path,
                edge.line.map(|l| l as i64),
                edge.column.map(|c| c as i64),
                edge.detail,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// One page of the distinct nodes that call `node_id`.
    ///
    /// `DISTINCT` matters: a caller that invokes the target three times has
    /// three `calls` edges, and without it the same function would fill three
    /// slots of the page and be counted three times. The `ORDER BY` makes the
    /// page boundary reproducible — a `LIMIT` without one returns an arbitrary
    /// subset, so "the first 20 callers" meant nothing in particular.
    pub fn get_callers(&self, node_id: i64, limit: u32, offset: u32) -> Result<Vec<Node>> {
        self.query_call_page(
            "SELECT DISTINCT n.* FROM nodes n \
             INNER JOIN edges e ON e.source_id = n.id \
             WHERE e.target_id = ?1 AND e.kind = 'calls' \
             ORDER BY n.file_path, n.start_line, n.id \
             LIMIT ?2 OFFSET ?3",
            node_id,
            limit,
            offset,
        )
    }

    /// How many distinct nodes call `node_id`. Pairs with
    /// [`Database::get_callers`] so a page can say what it is a page *of*.
    pub fn count_callers(&self, node_id: i64) -> Result<usize> {
        self.count_calls(
            "SELECT COUNT(DISTINCT e.source_id) FROM edges e \
             WHERE e.target_id = ?1 AND e.kind = 'calls'",
            node_id,
        )
    }

    /// One page of the distinct nodes that `node_id` calls.
    pub fn get_callees(&self, node_id: i64, limit: u32, offset: u32) -> Result<Vec<Node>> {
        self.query_call_page(
            "SELECT DISTINCT n.* FROM nodes n \
             INNER JOIN edges e ON e.target_id = n.id \
             WHERE e.source_id = ?1 AND e.kind = 'calls' \
             ORDER BY n.file_path, n.start_line, n.id \
             LIMIT ?2 OFFSET ?3",
            node_id,
            limit,
            offset,
        )
    }

    /// How many distinct nodes `node_id` calls.
    pub fn count_callees(&self, node_id: i64) -> Result<usize> {
        self.count_calls(
            "SELECT COUNT(DISTINCT e.target_id) FROM edges e \
             WHERE e.source_id = ?1 AND e.kind = 'calls'",
            node_id,
        )
    }

    fn query_call_page(
        &self,
        sql: &str,
        node_id: i64,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Node>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(
            params![node_id, limit as i64, offset as i64],
            Self::row_to_node,
        )?;
        let mut nodes = Vec::new();
        for row in rows {
            nodes.push(row?);
        }
        Ok(nodes)
    }

    fn count_calls(&self, sql: &str, node_id: i64) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row(sql, params![node_id], |row| row.get(0))?;
        Ok(n as usize)
    }

    /// Get all edges from a node
    pub fn get_outgoing_edges(&self, node_id: i64) -> Result<Vec<Edge>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM edges WHERE source_id = ?1")?;
        let rows = stmt.query_map(params![node_id], Self::row_to_edge)?;

        let mut edges = Vec::new();
        for row in rows {
            edges.push(row?);
        }
        Ok(edges)
    }

    /// Get all edges to a node
    pub fn get_incoming_edges(&self, node_id: i64) -> Result<Vec<Edge>> {
        self.incoming_edges(node_id, true)
    }

    /// Incoming edges, optionally dropping the ones whose *source* is test
    /// code.
    ///
    /// Separate from [`Database::get_incoming_edges`] because most callers
    /// want every caller, tests included — `symgraph-callers` in particular,
    /// where "the tests call this" is exactly what a user is asking. Only the
    /// coupling reports, which claim to describe the architecture, want the
    /// filtered view.
    pub fn incoming_edges(&self, node_id: i64, include_tests: bool) -> Result<Vec<Edge>> {
        let sql = if include_tests {
            "SELECT e.* FROM edges e WHERE e.target_id = ?1".to_string()
        } else {
            "SELECT e.* FROM edges e JOIN nodes s ON e.source_id = s.id \
             WHERE e.target_id = ?1 AND s.is_test = 0"
                .to_string()
        };
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![node_id], Self::row_to_edge)?;

        let mut edges = Vec::new();
        for row in rows {
            edges.push(row?);
        }
        Ok(edges)
    }

    fn row_to_edge(row: &rusqlite::Row) -> rusqlite::Result<Edge> {
        Ok(Edge {
            id: row.get(0)?,
            source_id: row.get(1)?,
            target_id: row.get(2)?,
            kind: EdgeKind::parse(&row.get::<_, String>(3)?).unwrap_or(EdgeKind::References),
            file_path: row.get(4)?,
            line: row.get::<_, Option<i64>>(5)?.map(|l| l as u32),
            column: row.get::<_, Option<i64>>(6)?.map(|c| c as u32),
            detail: row.get(7)?,
        })
    }

    // =========================================================================
    // Unresolved References
    // =========================================================================

    /// Insert an unresolved reference
    pub fn insert_unresolved_ref(&self, uref: &UnresolvedReference) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO unresolved_refs (source_node_id, reference_name, kind, file_path, line, column, detail)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                uref.source_node_id,
                uref.reference_name,
                uref.kind.as_str(),
                uref.file_path,
                uref.line as i64,
                uref.column as i64,
                uref.detail,
            ],
        )?;
        Ok(())
    }

    /// Get all unresolved references
    pub fn get_unresolved_refs(&self) -> Result<Vec<UnresolvedReference>> {
        let mut stmt = self.conn.prepare("SELECT * FROM unresolved_refs")?;
        let rows = stmt.query_map([], |row| {
            Ok(UnresolvedReference {
                source_node_id: row.get(1)?,
                reference_name: row.get(2)?,
                kind: EdgeKind::parse(&row.get::<_, String>(3)?).unwrap_or(EdgeKind::Calls),
                file_path: row.get(4)?,
                line: row.get::<_, i64>(5)? as u32,
                column: row.get::<_, i64>(6)? as u32,
                detail: row.get(7)?,
            })
        })?;

        let mut refs = Vec::new();
        for row in rows {
            refs.push(row?);
        }
        Ok(refs)
    }

    /// Resolve references by matching names to definitions.
    ///
    /// Resolution is name-based, so a name carried by several definitions has
    /// to be chosen between. The preference ladder, best first:
    ///
    /// 1. a definition in the *same file* as the reference;
    /// 2. a definition in a file the referencing file **imports from**;
    /// 3. any definition, in the standard preference order.
    ///
    /// Tier 2 is what makes this better than a guess. `a.rs` calling `new()`
    /// almost always means the `new` on a type it imported, not the first
    /// `new` in the repository — and imports are already extracted, so the
    /// information was there to use.
    ///
    /// Import edges are themselves resolved references, so they are resolved
    /// first, in their own pass, and the scope they define is then available
    /// to everything else.
    pub fn resolve_references(&self) -> Result<u32> {
        self.resolve_refs(None)
    }

    /// Resolve references only for specific files (scoped resolution).
    ///
    /// More efficient than [`Database::resolve_references`] for incremental
    /// reindexing — it only considers refs originating in the given files.
    pub fn resolve_references_for_files(&self, files: &[String]) -> Result<u32> {
        if files.is_empty() {
            return Ok(0);
        }
        self.resolve_refs(Some(files))
    }

    /// The shared implementation behind both entry points.
    fn resolve_refs(&self, only_files: Option<&[String]>) -> Result<u32> {
        // Imports first: they establish the file scope every later tier uses.
        let mut resolved = self.resolve_phase(only_files, Phase::Imports)?;
        resolved += self.resolve_phase(only_files, Phase::Rest)?;
        Ok(resolved)
    }

    /// Resolve one phase of references in a single set-based statement.
    ///
    /// This used to be a loop issuing three or four queries per reference,
    /// which dominated indexing time on any real codebase. SQLite can express
    /// the whole thing — including the preference ladder — as one INSERT.
    fn resolve_phase(&self, only_files: Option<&[String]>, phase: Phase) -> Result<u32> {
        self.rebuild_import_scope()?;

        let file_filter = match only_files {
            Some(files) => format!(
                " AND u.file_path IN ({})",
                files
                    .iter()
                    .map(|f| format!("'{}'", f.replace('\'', "''")))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            None => String::new(),
        };
        let kind_filter = match phase {
            Phase::Imports => " AND u.kind = 'imports'",
            Phase::Rest => " AND u.kind <> 'imports'",
        };

        // The chosen target for each reference, as a correlated subquery. The
        // first CASE rejects candidates the reference could not denote (a call
        // landing on a field); the second is the preference ladder; the
        // remaining columns are the same stable tiebreak `resolve_symbol`
        // uses, so a name resolves the same way whether it is reached through
        // an edge or a lookup.
        let pick_target = format!(
            "\
            SELECT n.id FROM nodes n \
            WHERE n.name = u.reference_name \
              AND ({kind_compatible}) \
            ORDER BY \
                CASE \
                    WHEN n.file_path = u.file_path THEN 0 \
                    WHEN EXISTS ( \
                        SELECT 1 FROM import_scope i \
                        WHERE i.source_file = u.file_path \
                          AND i.target_file = n.file_path \
                    ) THEN 1 \
                    ELSE 2 \
                END, \
                n.is_test ASC, n.is_generated ASC, \
                n.file_path ASC, n.start_line ASC, n.id ASC \
            LIMIT 1",
            kind_compatible = kind_compatible_sql()
        );
        let pick_target = pick_target.as_str();

        let before: i64 =
            self.conn
                .query_row("SELECT COALESCE(MAX(id), 0) FROM edges", [], |r| r.get(0))?;

        let inserted = self.conn.execute(
            &format!(
                "INSERT INTO edges (source_id, target_id, kind, file_path, line, column, detail) \
                 SELECT u.source_node_id, ({pick}), u.kind, u.file_path, u.line, u.column, u.detail \
                 FROM unresolved_refs u \
                 WHERE ({pick}) IS NOT NULL{kind}{files}",
                pick = pick_target,
                kind = kind_filter,
                files = file_filter
            ),
            [],
        )?;

        // A test calling production code gets an extra `tests` edge alongside
        // the `calls` one. Derived from the edges just inserted rather than
        // re-queried per reference.
        if matches!(phase, Phase::Rest) {
            self.conn.execute(
                "INSERT INTO edges (source_id, target_id, kind, file_path, line, column) \
                 SELECT e.source_id, e.target_id, 'tests', e.file_path, e.line, e.column \
                 FROM edges e JOIN nodes s ON e.source_id = s.id \
                 WHERE e.id > ?1 AND e.kind = 'calls' AND s.is_test = 1",
                params![before],
            )?;
        }

        // Drop only the references that actually resolved. What remains is a
        // real signal — calls into code outside the index, plus anything
        // extraction got wrong — and `health()` reports the count. Deleting
        // the lot, as this used to, made that number permanently zero.
        self.conn.execute(
            &format!(
                "DELETE FROM unresolved_refs AS u WHERE ({pick}) IS NOT NULL{kind}{files}",
                pick = pick_target,
                kind = kind_filter,
                files = file_filter
            ),
            [],
        )?;

        Ok(inserted as u32)
    }

    /// Materialise "which files does each file import from" for the duration
    /// of a resolution phase.
    ///
    /// Recomputed per phase because the imports pass adds to it. A temp table
    /// with an index beats evaluating the join inside a per-row correlated
    /// subquery.
    fn rebuild_import_scope(&self) -> Result<()> {
        self.conn.execute_batch(
            "DROP TABLE IF EXISTS import_scope; \
             CREATE TEMP TABLE import_scope AS \
                 SELECT DISTINCT s.file_path AS source_file, t.file_path AS target_file \
                 FROM edges e \
                 JOIN nodes s ON e.source_id = s.id \
                 JOIN nodes t ON e.target_id = t.id \
                 WHERE e.kind = 'imports' AND s.file_path <> t.file_path; \
             CREATE INDEX idx_import_scope ON import_scope(source_file, target_file);",
        )?;
        Ok(())
    }

    // =========================================================================
    // Batch Insert Operations
    // =========================================================================

    /// Insert multiple nodes using a single prepared statement.
    /// Each node's `id` field is updated to the database-assigned row id.
    /// Returns a map from old (extraction-time) id to new (database) id.
    pub fn insert_nodes_batch(
        &self,
        nodes: &mut [Node],
    ) -> Result<std::collections::HashMap<i64, i64>> {
        let id_map = self.insert_nodes_base_batch(nodes)?;
        self.insert_node_fts_rows(nodes)?;
        self.insert_semantic_fts_rows(nodes)?;
        Ok(id_map)
    }

    /// Insert multiple nodes without updating FTS tables.
    /// Used by full shadow builds, which rebuild FTS once at the end.
    pub fn insert_nodes_batch_without_fts(
        &self,
        nodes: &mut [Node],
    ) -> Result<std::collections::HashMap<i64, i64>> {
        self.insert_nodes_base_batch(nodes)
    }

    /// Rebuild both FTS indexes from the canonical `nodes` table.
    pub fn rebuild_fts_indexes(&self) -> Result<()> {
        self.conn
            .execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('rebuild');")
            .context("rebuild_fts_indexes: nodes_fts")?;
        self.conn
            .execute("DELETE FROM nodes_semantic_fts", [])
            .context("rebuild_fts_indexes: nodes_semantic_fts clear")?;

        let mut select = self
            .conn
            .prepare_cached("SELECT * FROM nodes ORDER BY id")?;
        let rows = select.query_map([], Self::row_to_node)?;
        let mut insert = self
            .conn
            .prepare_cached("INSERT INTO nodes_semantic_fts(rowid, tokens) VALUES (?1, ?2)")?;
        for node in rows {
            let node = node?;
            insert.execute(params![node.id, build_semantic_tokens(&node)])?;
        }

        self.optimize_fts()
    }

    fn insert_nodes_base_batch(
        &self,
        nodes: &mut [Node],
    ) -> Result<std::collections::HashMap<i64, i64>> {
        let mut id_map = std::collections::HashMap::with_capacity(nodes.len());
        let mut stmt = self.conn.prepare_cached(
            r#"
            INSERT INTO nodes (
                kind, name, qualified_name, file_path, start_line, end_line,
                start_column, end_column, signature, visibility, docstring,
                is_async, is_static, is_exported, is_test, is_generated, language
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
            "#,
        )?;
        for node in nodes.iter_mut() {
            let old_id = node.id;
            stmt.execute(params![
                node.kind.as_str(),
                node.name,
                node.qualified_name,
                node.file_path,
                node.start_line as i64,
                node.end_line as i64,
                node.start_column as i64,
                node.end_column as i64,
                node.signature,
                node.visibility.as_str(),
                node.docstring,
                node.is_async,
                node.is_static,
                node.is_exported,
                node.is_test,
                node.is_generated,
                node.language.as_str(),
            ])?;
            let new_id = self.conn.last_insert_rowid();
            node.id = new_id;
            id_map.insert(old_id, new_id);
        }
        Ok(id_map)
    }

    fn insert_node_fts_rows(&self, nodes: &[Node]) -> Result<()> {
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO nodes_fts(rowid, name, qualified_name) VALUES (?1, ?2, ?3)",
        )?;
        for node in nodes {
            stmt.execute(params![node.id, node.name, node.qualified_name])?;
        }
        Ok(())
    }

    fn insert_semantic_fts_rows(&self, nodes: &[Node]) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare_cached("INSERT INTO nodes_semantic_fts(rowid, tokens) VALUES (?1, ?2)")?;
        for node in nodes {
            stmt.execute(params![node.id, build_semantic_tokens(node)])?;
        }
        Ok(())
    }

    /// Insert multiple edges using a single prepared statement.
    /// Maps source_id and target_id through `id_map`; edges whose ids
    /// cannot be mapped are silently skipped.
    /// Returns the number of edges actually inserted.
    pub fn insert_edges_batch(
        &self,
        edges: &[Edge],
        id_map: &std::collections::HashMap<i64, i64>,
    ) -> Result<u64> {
        let mut count: u64 = 0;
        let mut dropped: usize = 0;
        let mut stmt = self.conn.prepare_cached(
            r#"
            INSERT INTO edges (source_id, target_id, kind, file_path, line, column, detail)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
        )?;
        for edge in edges {
            // Bind the remapped ids directly rather than mutating the edge.
            if let (Some(&new_source), Some(&new_target)) =
                (id_map.get(&edge.source_id), id_map.get(&edge.target_id))
            {
                stmt.execute(params![
                    new_source,
                    new_target,
                    edge.kind.as_str(),
                    edge.file_path,
                    edge.line.map(|l| l as i64),
                    edge.column.map(|c| c as i64),
                    edge.detail,
                ])?;
                count += 1;
            } else {
                dropped += 1;
            }
        }
        if dropped > 0 {
            // An unmappable endpoint means extraction emitted an edge to a node
            // id that was never inserted — a bug, not a data condition. Losing
            // edges is how a graph quietly under-reports impact.
            tracing::warn!(
                dropped,
                total = edges.len(),
                "dropped edges whose endpoints could not be mapped to inserted nodes"
            );
        }
        Ok(count)
    }

    /// Insert multiple unresolved references using a single prepared statement.
    /// Maps source_node_id through `id_map`; refs whose id cannot be mapped
    /// are silently skipped.
    pub fn insert_unresolved_refs_batch(
        &self,
        refs: &[UnresolvedReference],
        id_map: &std::collections::HashMap<i64, i64>,
    ) -> Result<()> {
        let mut stmt = self.conn.prepare_cached(
            r#"
            INSERT INTO unresolved_refs (source_node_id, reference_name, kind, file_path, line, column, detail)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
        )?;
        let mut dropped: usize = 0;
        for uref in refs {
            // Bind the remapped source id directly rather than mutating the ref.
            if let Some(&new_source) = id_map.get(&uref.source_node_id) {
                stmt.execute(params![
                    new_source,
                    uref.reference_name,
                    uref.kind.as_str(),
                    uref.file_path,
                    uref.line as i64,
                    uref.column as i64,
                    uref.detail,
                ])?;
            } else {
                dropped += 1;
            }
        }
        if dropped > 0 {
            tracing::warn!(
                dropped,
                total = refs.len(),
                "dropped unresolved refs whose source node could not be mapped"
            );
        }
        Ok(())
    }

    // =========================================================================
    // Statistics
    // =========================================================================

    /// Get index statistics
    pub fn get_stats(&self) -> Result<IndexStats> {
        let total_files: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))?;
        let total_nodes: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM nodes", [], |row| row.get(0))?;
        let total_edges: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM edges", [], |row| row.get(0))?;

        // Get database file size
        let db_size_bytes: i64 = self
            .conn
            .query_row(
                "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        // Get language distribution
        let mut stmt = self
            .conn
            .prepare("SELECT language, COUNT(*) FROM nodes GROUP BY language")?;
        let lang_rows = stmt.query_map([], |row| {
            let lang_str: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok((Language::parse(&lang_str), count as u64))
        })?;
        let mut languages = Vec::new();
        for row in lang_rows {
            languages.push(row?);
        }

        // Get node kind distribution
        let mut stmt = self
            .conn
            .prepare("SELECT kind, COUNT(*) FROM nodes GROUP BY kind")?;
        let kind_rows = stmt.query_map([], |row| {
            let kind_str: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok((
                NodeKind::parse(&kind_str).unwrap_or(NodeKind::Function),
                count as u64,
            ))
        })?;
        let mut node_kinds = Vec::new();
        for row in kind_rows {
            node_kinds.push(row?);
        }

        Ok(IndexStats {
            total_files: total_files as u64,
            total_nodes: total_nodes as u64,
            total_edges: total_edges as u64,
            db_size_bytes: db_size_bytes as u64,
            languages,
            node_kinds,
        })
    }

    /// Disable FTS5 automerge on both search tables before a bulk insert.
    /// Prevents per-insert segment merges; call `optimize_fts` when done.
    ///
    /// Note: FTS5 configuration commands like `automerge` require the
    /// two-column form `INSERT INTO ft(ft, rank) VALUES('option', value)`.
    /// The single-column form `VALUES('automerge=0')` is only for parameterless
    /// maintenance commands (`optimize`, `rebuild`, `delete-all`, etc.).
    pub fn disable_fts_automerge(&self) -> Result<()> {
        use anyhow::Context;
        self.conn
            .execute(
                "INSERT INTO nodes_fts(nodes_fts, rank) VALUES('automerge', 0)",
                [],
            )
            .context("disable_fts_automerge: nodes_fts")?;
        self.conn
            .execute(
                "INSERT INTO nodes_semantic_fts(nodes_semantic_fts, rank) VALUES('automerge', 0)",
                [],
            )
            .context("disable_fts_automerge: nodes_semantic_fts")?;
        Ok(())
    }

    /// Merge all FTS5 segments into one after a bulk insert.
    /// Much faster than the incremental per-insert merges that `automerge=8`
    /// (the default) would have done.
    pub fn optimize_fts(&self) -> Result<()> {
        use anyhow::Context;
        self.conn
            .execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('optimize');")
            .context("optimize_fts: nodes_fts")?;
        self.conn
            .execute_batch("INSERT INTO nodes_semantic_fts(nodes_semantic_fts) VALUES('optimize');")
            .context("optimize_fts: nodes_semantic_fts")?;
        Ok(())
    }

    /// Begin a transaction
    pub fn begin_transaction(&mut self) -> Result<()> {
        use anyhow::Context;
        self.conn
            .execute("BEGIN TRANSACTION", [])
            .context("begin_transaction")?;
        Ok(())
    }

    /// Commit a transaction without checkpointing the WAL.
    pub fn commit_transaction(&mut self) -> Result<()> {
        self.conn.execute("COMMIT", []).context("commit: COMMIT")?;
        Ok(())
    }

    /// Checkpoint the WAL and truncate it so the database file stays compact.
    pub fn checkpoint_wal_truncate(&self) -> Result<()> {
        self.conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .context("wal_checkpoint")?;
        Ok(())
    }

    /// Return free pages to the OS and truncate the WAL so the on-disk file
    /// stays compact after incremental edits. `incremental_vacuum` is a no-op on
    /// legacy databases created without incremental auto-vacuum; those reclaim
    /// their space on the next full rebuild (which VACUUMs the shadow). Must be
    /// called outside a transaction.
    pub fn compact(&self) -> Result<()> {
        self.conn
            .execute_batch("PRAGMA incremental_vacuum;")
            .context("compact: incremental_vacuum")?;
        self.checkpoint_wal_truncate()
    }

    /// Commit a transaction and checkpoint the WAL so the file stays compact.
    pub fn commit(&mut self) -> Result<()> {
        self.commit_transaction()?;
        self.checkpoint_wal_truncate()
    }

    /// Rollback a transaction
    pub fn rollback(&mut self) -> Result<()> {
        self.conn.execute("ROLLBACK", []).context("rollback")?;
        Ok(())
    }

    /// Flush a shadow database to disk and close it before an atomic swap.
    pub fn prepare_for_swap(self) -> Result<PathBuf> {
        let path = self
            .path
            .clone()
            .context("prepare_for_swap requires an on-disk database")?;
        // Fully compact the freshly built shadow before it becomes the live
        // index: VACUUM rebuilds the file with no freelist bloat and writes it
        // with incremental auto-vacuum enabled, so replacing an older live
        // database also migrates it to the compacting layout.
        self.conn
            .execute_batch("VACUUM;")
            .context("prepare_for_swap: VACUUM")?;
        self.checkpoint_wal_truncate()?;
        self.close()?;
        cleanup_sqlite_sidecars(&path)?;
        Ok(path)
    }

    /// Atomically replace this on-disk database file with a prepared shadow database.
    pub fn replace_with_shadow<P: AsRef<Path>>(&mut self, shadow_path: P) -> Result<()> {
        let live_path = self
            .path
            .clone()
            .context("replace_with_shadow requires an on-disk database")?;
        let shadow_path = shadow_path.as_ref().to_path_buf();

        self.checkpoint_wal_truncate()?;

        let placeholder = Connection::open_in_memory().context("opening placeholder connection")?;
        let live_conn = std::mem::replace(&mut self.conn, placeholder);
        if let Err((conn, err)) = live_conn.close() {
            self.conn = conn;
            return Err(err.into());
        }

        cleanup_sqlite_sidecars(&live_path)?;

        if let Err(err) = fs::rename(&shadow_path, &live_path) {
            self.reopen_from_path(&live_path)?;
            return Err(err.into());
        }

        cleanup_sqlite_sidecars(&shadow_path)?;
        self.reopen_from_path(&live_path)
    }

    /// Delete a database file and any SQLite sidecars if they exist.
    pub fn cleanup_on_disk_path<P: AsRef<Path>>(path: P) -> Result<()> {
        let path = path.as_ref();
        remove_file_if_exists(path)?;
        cleanup_sqlite_sidecars(path)
    }

    fn reopen_from_path(&mut self, path: &Path) -> Result<()> {
        let reopened = Self::open(path)?;
        self.conn = reopened.conn;
        self.path = reopened.path;
        Ok(())
    }

    /// Get the hierarchy of a symbol (parent contains relationships)
    pub fn get_hierarchy(&self, symbol: &str) -> Result<Vec<Node>> {
        let mut stmt = self.conn.prepare(
            "SELECT n.* FROM nodes n
             INNER JOIN edges e ON e.source_id = n.id
             INNER JOIN nodes target ON e.target_id = target.id
             WHERE e.kind = 'contains' AND target.name = ?
             UNION
             SELECT n.* FROM nodes n
             INNER JOIN edges e ON e.target_id = n.id
             INNER JOIN nodes source ON e.source_id = source.id
             WHERE e.kind = 'contains' AND source.name = ?
             ORDER BY 5, 6, 1",
        )?;

        let rows = stmt.query_map(params![symbol, symbol], Self::row_to_node)?;

        let mut nodes = Vec::new();
        for row in rows {
            nodes.push(row?);
        }
        Ok(nodes)
    }

    /// Find call path between two symbols using BFS
    pub fn find_call_path(&self, from: &str, to: &str) -> Result<Vec<Vec<Node>>> {
        // Get source and target nodes
        let source = self.find_node_by_name(from)?;
        let target = self.find_node_by_name(to)?;

        match (source, target) {
            (Some(src), Some(tgt)) => {
                let mut paths = Vec::new();
                let mut visited = std::collections::HashSet::new();
                let mut queue = std::collections::VecDeque::new();
                queue.push_back((src.id, vec![src.clone()]));

                while let Some((current_id, path)) = queue.pop_front() {
                    if current_id == tgt.id {
                        paths.push(path);
                        if paths.len() >= 5 {
                            // Limit to first 5 paths
                            break;
                        }
                        continue;
                    }

                    if path.len() > 10 || visited.contains(&current_id) {
                        // Depth limit and cycle prevention
                        continue;
                    }
                    visited.insert(current_id);

                    // Get all callees
                    let callees = self.get_callees(current_id, 100, 0)?;
                    for callee in callees {
                        let mut new_path = path.clone();
                        new_path.push(callee.clone());
                        queue.push_back((callee.id, new_path));
                    }
                }

                Ok(paths)
            }
            _ => Ok(Vec::new()),
        }
    }

    /// Predicate shared by the unused-symbol page and its count, so the two
    /// can never drift apart.
    ///
    /// `ignore_test_callers` changes what counts as a use. By default any
    /// incoming call or reference keeps a symbol alive, tests included; with
    /// the flag set, only non-test code does. The difference is production
    /// code that nothing but its own tests exercises — live by the first
    /// measure, dead by the second, and the second is usually the question
    /// being asked.
    fn unused_predicate(ignore_test_callers: bool) -> String {
        let uses = if ignore_test_callers {
            // A `tests` edge is by definition test-sourced, so it drops out
            // of the kind list entirely rather than being filtered afterwards.
            "SELECT DISTINCT e.target_id FROM edges e \
             JOIN nodes s ON e.source_id = s.id \
             WHERE e.kind IN ('calls', 'references', 'instantiates') AND s.is_test = 0"
        } else {
            "SELECT DISTINCT target_id FROM edges \
             WHERE kind IN ('calls', 'references', 'instantiates', 'tests')"
        };
        format!(
            "n.kind IN ('function', 'method', 'class', 'struct', 'interface') \
             AND n.is_test = 0 \
             AND n.is_generated = 0 \
             AND n.id NOT IN ({uses})"
        )
    }

    /// One page of symbols with no incoming calls or references.
    pub fn find_unused_symbols(
        &self,
        limit: u32,
        offset: u32,
        ignore_test_callers: bool,
    ) -> Result<Vec<Node>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT n.* FROM nodes n WHERE {} \
             ORDER BY n.file_path, n.start_line, n.id LIMIT ?1 OFFSET ?2",
            Self::unused_predicate(ignore_test_callers)
        ))?;
        let rows = stmt.query_map(params![limit as i64, offset as i64], Self::row_to_node)?;
        let mut nodes = Vec::new();
        for row in rows {
            nodes.push(row?);
        }
        Ok(nodes)
    }

    /// How many unused symbols exist in total. On a large codebase this is
    /// routinely in the thousands, which is exactly why the listing is paged.
    pub fn count_unused_symbols(&self, ignore_test_callers: bool) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM nodes n WHERE {}",
                Self::unused_predicate(ignore_test_callers)
            ),
            [],
            |row| row.get(0),
        )?;
        Ok(n as usize)
    }

    /// Find all implementations of an interface/trait
    pub fn find_implementations(&self, symbol: &str) -> Result<Vec<Node>> {
        let mut stmt = self.conn.prepare(
            "SELECT n.* FROM nodes n
             INNER JOIN edges e ON e.source_id = n.id
             INNER JOIN nodes target ON e.target_id = target.id
             WHERE e.kind IN ('implements', 'extends') AND target.name = ?
             ORDER BY n.file_path, n.start_line, n.id",
        )?;

        let rows = stmt.query_map([symbol], Self::row_to_node)?;

        let mut nodes = Vec::new();
        for row in rows {
            nodes.push(row?);
        }
        Ok(nodes)
    }

    /// Get symbols that would be affected by changing a file region
    pub fn get_diff_impact(
        &self,
        file_path: &str,
        start_line: u32,
        end_line: u32,
    ) -> Result<Vec<Node>> {
        // Find all symbols in the affected region
        let mut affected = Vec::new();

        let mut stmt = self.conn.prepare(
            "SELECT * FROM nodes
             WHERE file_path = ?
             AND ((start_line <= ? AND end_line >= ?)
                  OR (start_line >= ? AND start_line <= ?))",
        )?;

        let rows = stmt.query_map(
            params![file_path, end_line, start_line, start_line, end_line],
            Self::row_to_node,
        )?;

        for row in rows {
            affected.push(row?);
        }

        // For each affected symbol, find all callers
        let mut impacted = affected.clone();
        for node in &affected {
            let callers = self.get_callers(node.id, 100, 0)?;
            for caller in callers {
                if !impacted.iter().any(|n| n.id == caller.id) {
                    impacted.push(caller);
                }
            }
        }

        Ok(impacted)
    }

    /// Semantic search using bm25-ranked tokenized identifiers + docstrings.
    ///
    /// Tokens are camelCase/snake_case-split identifiers plus the cleaned
    /// docstring. Falls back to an empty result if the FTS query is invalid.
    pub fn semantic_search(&self, query: &str, limit: u32) -> Result<Vec<Node>> {
        let normalized = normalize_query_for_fts(query);
        if normalized.is_empty() {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare(
            "SELECT n.* FROM nodes n
             INNER JOIN nodes_semantic_fts s ON s.rowid = n.id
             WHERE nodes_semantic_fts MATCH ?1
             ORDER BY bm25(nodes_semantic_fts)
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![normalized, limit as i64], Self::row_to_node);
        let mut nodes = Vec::new();
        match rows {
            Ok(iter) => {
                for row in iter {
                    nodes.push(row?);
                }
            }
            Err(_) => return Ok(Vec::new()),
        }
        Ok(nodes)
    }

    /// Find a target node by name, preferring same-file matches.
    ///
    /// Used by import-aware reference resolution: when a symbol name is
    /// referenced from `source_file`, prefer a definition in that file before
    /// falling back to any global match. This is a lightweight stand-in for
    /// full import resolution and reduces false positives in dynamic languages.
    pub fn find_target_preferring_file(
        &self,
        name: &str,
        source_file: &str,
    ) -> Result<Option<Node>> {
        let local = self
            .conn
            .query_row(
                "SELECT * FROM nodes WHERE name = ?1 AND file_path = ?2 LIMIT 1",
                params![name, source_file],
                Self::row_to_node,
            )
            .optional()?;
        if local.is_some() {
            return Ok(local);
        }
        self.find_node_by_name(name)
    }
}

/// Build the token text indexed in `nodes_semantic_fts` for a node.
///
/// Splits camelCase / snake_case / kebab-case identifiers into individual
/// tokens and appends the cleaned docstring so bm25 can score by both name
/// fragments and natural-language documentation.
fn build_semantic_tokens(node: &Node) -> String {
    let mut out = String::new();
    push_split_tokens(&mut out, &node.name);
    if let Some(qn) = &node.qualified_name {
        out.push(' ');
        push_split_tokens(&mut out, qn);
    }
    if let Some(doc) = &node.docstring {
        out.push(' ');
        out.push_str(doc);
    }
    out.to_lowercase()
}

fn push_split_tokens(out: &mut String, s: &str) {
    out.push(' ');
    out.push_str(s);
    out.push(' ');
    let mut current = String::new();
    let mut prev_lower = false;
    for ch in s.chars() {
        if ch.is_ascii_uppercase() && prev_lower && !current.is_empty() {
            out.push_str(&current);
            out.push(' ');
            current.clear();
        }
        if ch.is_alphanumeric() {
            current.push(ch);
            prev_lower = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        } else {
            if !current.is_empty() {
                out.push_str(&current);
                out.push(' ');
                current.clear();
            }
            prev_lower = false;
        }
    }
    if !current.is_empty() {
        out.push_str(&current);
        out.push(' ');
    }
}

/// Sanitize a free-text query for FTS5 — strips characters that have special
/// meaning to the FTS5 query parser and joins remaining tokens with spaces
/// (implicit AND).
fn normalize_query_for_fts(query: &str) -> String {
    let cleaned: String = query
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    cleaned
        .split_whitespace()
        .map(|t| t.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ")
}

fn sqlite_sidecar_paths(path: &Path) -> [PathBuf; 2] {
    [
        PathBuf::from(format!("{}-wal", path.display())),
        PathBuf::from(format!("{}-shm", path.display())),
    ]
}

fn cleanup_sqlite_sidecars(path: &Path) -> Result<()> {
    for sidecar in sqlite_sidecar_paths(path) {
        remove_file_if_exists(&sidecar)?;
    }
    Ok(())
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn create_test_node(name: &str, kind: NodeKind, file_path: &str) -> Node {
        Node {
            id: 0,
            kind,
            name: name.to_string(),
            qualified_name: Some(format!("test::{}", name)),
            file_path: file_path.to_string(),
            start_line: 1,
            end_line: 10,
            start_column: 0,
            end_column: 1,
            signature: Some(format!("fn {}()", name)),
            visibility: Visibility::Public,
            docstring: None,
            is_async: false,
            is_static: false,
            is_exported: true,
            is_test: false,
            is_generated: false,
            language: Language::Rust,
        }
    }

    fn mk_file(path: &str) -> FileRecord {
        FileRecord {
            path: path.to_string(),
            content_hash: "abc123".to_string(),
            language: Language::Rust,
            size: 1000,
            modified_at: 1234567890,
            indexed_at: 1234567890,
            node_count: 5,
        }
    }

    // Database initialization tests

    /// The middle tier of the resolution ladder: when a name is defined in
    /// more than one file and none of them is the caller's own, prefer the one
    /// the caller imports from. Without this the choice falls back to
    /// alphabetical order, which is arbitrary with respect to meaning.
    ///
    /// Modelled on the ordinary Rust shape — `use crate::real::RealThing;`
    /// followed by `RealThing::helper()`. The import names the type, which is
    /// unambiguous, and that is what establishes the file relationship the
    /// ambiguous `helper` call then rides on.
    #[test]
    fn test_resolution_prefers_an_imported_file_over_an_arbitrary_one() {
        let db = Database::in_memory().unwrap();
        for f in ["src/aaa_decoy.rs", "src/caller.rs", "src/real.rs"] {
            db.insert_or_update_file(&mk_file(f)).unwrap();
        }

        // Two definitions of `helper`. `aaa_decoy.rs` sorts first, so it wins
        // on the plain preference order.
        db.insert_node(&create_test_node(
            "helper",
            NodeKind::Function,
            "src/aaa_decoy.rs",
        ))
        .unwrap();
        let real = db
            .insert_node(&create_test_node(
                "helper",
                NodeKind::Function,
                "src/real.rs",
            ))
            .unwrap();
        // The imported type is defined only in real.rs, so the import resolves
        // unambiguously and puts real.rs in caller.rs's scope.
        db.insert_node(&create_test_node(
            "RealThing",
            NodeKind::Struct,
            "src/real.rs",
        ))
        .unwrap();

        let caller = db
            .insert_node(&create_test_node(
                "caller",
                NodeKind::Function,
                "src/caller.rs",
            ))
            .unwrap();
        db.insert_unresolved_ref(&UnresolvedReference {
            source_node_id: caller,
            reference_name: "RealThing".to_string(),
            kind: EdgeKind::Imports,
            file_path: "src/caller.rs".to_string(),
            line: 1,
            column: 0,
            detail: None,
        })
        .unwrap();
        db.insert_unresolved_ref(&UnresolvedReference {
            source_node_id: caller,
            reference_name: "helper".to_string(),
            kind: EdgeKind::Calls,
            file_path: "src/caller.rs".to_string(),
            line: 5,
            column: 0,
            detail: None,
        })
        .unwrap();

        db.resolve_references().unwrap();

        let call = db
            .get_outgoing_edges(caller)
            .unwrap()
            .into_iter()
            .find(|e| e.kind == EdgeKind::Calls)
            .expect("the call resolved");
        let target = db.get_node(call.target_id).unwrap().unwrap();
        assert_eq!(
            target.file_path, "src/real.rs",
            "the call should follow the import, not alphabetical order"
        );
        assert_eq!(target.id, real);
    }

    /// The tier cannot rescue an import that is itself ambiguous: if `helper`
    /// is imported and two files define it, the import picks one on the plain
    /// preference order and the call follows it there. Scoping narrows a
    /// choice, it does not manufacture information that extraction never had.
    #[test]
    fn test_an_ambiguous_import_does_not_disambiguate_the_call() {
        let db = Database::in_memory().unwrap();
        for f in ["src/aaa_decoy.rs", "src/caller.rs", "src/real.rs"] {
            db.insert_or_update_file(&mk_file(f)).unwrap();
        }
        db.insert_node(&create_test_node(
            "helper",
            NodeKind::Function,
            "src/aaa_decoy.rs",
        ))
        .unwrap();
        db.insert_node(&create_test_node(
            "helper",
            NodeKind::Function,
            "src/real.rs",
        ))
        .unwrap();

        let caller = db
            .insert_node(&create_test_node(
                "caller",
                NodeKind::Function,
                "src/caller.rs",
            ))
            .unwrap();
        for kind in [EdgeKind::Imports, EdgeKind::Calls] {
            db.insert_unresolved_ref(&UnresolvedReference {
                source_node_id: caller,
                reference_name: "helper".to_string(),
                kind,
                file_path: "src/caller.rs".to_string(),
                line: 1,
                column: 0,
                detail: None,
            })
            .unwrap();
        }

        db.resolve_references().unwrap();

        let call = db
            .get_outgoing_edges(caller)
            .unwrap()
            .into_iter()
            .find(|e| e.kind == EdgeKind::Calls)
            .unwrap();
        let target = db.get_node(call.target_id).unwrap().unwrap();
        // Documents the limitation rather than asserting a fix that is not there.
        assert_eq!(target.file_path, "src/aaa_decoy.rs");
    }

    /// The same-file tier still outranks the import tier.
    #[test]
    fn test_same_file_still_beats_an_imported_file() {
        let db = Database::in_memory().unwrap();
        for f in ["src/caller.rs", "src/imported.rs"] {
            db.insert_or_update_file(&mk_file(f)).unwrap();
        }
        let local = db
            .insert_node(&create_test_node(
                "helper",
                NodeKind::Function,
                "src/caller.rs",
            ))
            .unwrap();
        db.insert_node(&create_test_node(
            "helper",
            NodeKind::Function,
            "src/imported.rs",
        ))
        .unwrap();

        let caller = db
            .insert_node(&create_test_node(
                "caller",
                NodeKind::Function,
                "src/caller.rs",
            ))
            .unwrap();
        for kind in [EdgeKind::Imports, EdgeKind::Calls] {
            db.insert_unresolved_ref(&UnresolvedReference {
                source_node_id: caller,
                reference_name: "helper".to_string(),
                kind,
                file_path: "src/caller.rs".to_string(),
                line: 1,
                column: 0,
                detail: None,
            })
            .unwrap();
        }

        db.resolve_references().unwrap();

        let call = db
            .get_outgoing_edges(caller)
            .unwrap()
            .into_iter()
            .find(|e| e.kind == EdgeKind::Calls)
            .unwrap();
        assert_eq!(call.target_id, local);
    }

    /// References that match no definition are kept, not silently dropped.
    /// They are the raw material for the unresolved-reference health figure.
    #[test]
    fn test_unresolvable_refs_survive_resolution() {
        let db = Database::in_memory().unwrap();
        db.insert_or_update_file(&mk_file("src/a.rs")).unwrap();
        let caller = db
            .insert_node(&create_test_node("caller", NodeKind::Function, "src/a.rs"))
            .unwrap();
        db.insert_unresolved_ref(&UnresolvedReference {
            source_node_id: caller,
            reference_name: "println".to_string(),
            kind: EdgeKind::Calls,
            file_path: "src/a.rs".to_string(),
            line: 2,
            column: 0,
            detail: None,
        })
        .unwrap();

        assert_eq!(db.resolve_references().unwrap(), 0);
        assert_eq!(
            db.get_unresolved_refs().unwrap().len(),
            1,
            "a call into code outside the index is unresolved, not resolved-to-nothing"
        );
        assert_eq!(db.health().unwrap().unresolved_refs, 1);
    }

    /// Deleting a file must not trip the foreign key from `unresolved_refs`
    /// to `nodes`. This was masked for as long as resolution emptied
    /// `unresolved_refs` wholesale; keeping unresolvable refs made every
    /// targeted reindex fail with "FOREIGN KEY constraint failed".
    #[test]
    fn test_delete_file_removes_unresolved_refs_before_nodes() {
        let db = Database::in_memory().unwrap();
        db.insert_or_update_file(&mk_file("src/a.rs")).unwrap();
        let node = db
            .insert_node(&create_test_node("caller", NodeKind::Function, "src/a.rs"))
            .unwrap();
        db.insert_unresolved_ref(&UnresolvedReference {
            source_node_id: node,
            reference_name: "println".to_string(),
            kind: EdgeKind::Calls,
            file_path: "src/a.rs".to_string(),
            line: 1,
            column: 0,
            detail: None,
        })
        .unwrap();

        db.delete_file("src/a.rs").expect("delete must not fail");

        assert!(db.get_nodes_by_file("src/a.rs").unwrap().is_empty());
        assert!(db.get_unresolved_refs().unwrap().is_empty());
        assert!(db.get_file("src/a.rs").unwrap().is_none());
    }

    /// A ref whose own `file_path` differs from its source node's file still
    /// has to go, or the constraint fails on the node delete.
    #[test]
    fn test_delete_file_removes_refs_anchored_to_its_nodes() {
        let db = Database::in_memory().unwrap();
        db.insert_or_update_file(&mk_file("src/a.rs")).unwrap();
        db.insert_or_update_file(&mk_file("src/b.rs")).unwrap();
        let node = db
            .insert_node(&create_test_node("caller", NodeKind::Function, "src/a.rs"))
            .unwrap();
        db.insert_unresolved_ref(&UnresolvedReference {
            source_node_id: node,
            reference_name: "thing".to_string(),
            kind: EdgeKind::Calls,
            // Recorded against a different file than the node lives in.
            file_path: "src/b.rs".to_string(),
            line: 1,
            column: 0,
            detail: None,
        })
        .unwrap();

        db.delete_file("src/a.rs").expect("delete must not fail");
        assert!(db.get_unresolved_refs().unwrap().is_empty());
    }

    // --- index provenance ---------------------------------------------

    /// An index with rows but no recorded provenance predates version
    /// tracking, so it must read as stale rather than as current.
    #[test]
    fn test_unversioned_index_with_rows_is_stale() {
        let db = Database::in_memory().unwrap();
        db.insert_or_update_file(&mk_file("src/a.rs")).unwrap();
        assert_eq!(db.staleness().unwrap(), Some(Staleness::Unversioned));
    }

    /// An empty index has nothing in it that could be wrong.
    #[test]
    fn test_empty_index_is_never_stale() {
        let db = Database::in_memory().unwrap();
        assert_eq!(db.staleness().unwrap(), None);
    }

    #[test]
    fn test_recording_the_version_clears_staleness() {
        let db = Database::in_memory().unwrap();
        db.insert_or_update_file(&mk_file("src/a.rs")).unwrap();
        db.record_index_version().unwrap();

        assert_eq!(db.staleness().unwrap(), None);
        let version = db.index_version().unwrap().unwrap();
        assert_eq!(version.schema, schema::SCHEMA_VERSION);
        assert_eq!(version.extractor, schema::EXTRACTOR_VERSION);
        assert_eq!(version.symgraph, env!("CARGO_PKG_VERSION"));
        assert!(version.built_at > 0);
    }

    /// The schema still fits, but extraction semantics moved on: the rows read
    /// fine and mean something else, which is the case worth catching.
    #[test]
    fn test_extractor_version_mismatch_is_stale() {
        let db = Database::in_memory().unwrap();
        db.insert_or_update_file(&mk_file("src/a.rs")).unwrap();
        db.record_index_version().unwrap();
        db.conn
            .execute(
                "UPDATE index_meta SET value = ?1 WHERE key = 'extractor_version'",
                params![(schema::EXTRACTOR_VERSION - 1).to_string()],
            )
            .unwrap();

        assert_eq!(
            db.staleness().unwrap(),
            Some(Staleness::Extractor {
                found: schema::EXTRACTOR_VERSION - 1,
                expected: schema::EXTRACTOR_VERSION,
            })
        );
    }

    #[test]
    fn test_schema_version_mismatch_is_stale() {
        let db = Database::in_memory().unwrap();
        db.insert_or_update_file(&mk_file("src/a.rs")).unwrap();
        db.record_index_version().unwrap();
        db.conn
            .execute(
                "UPDATE index_meta SET value = '999' WHERE key = 'schema_version'",
                [],
            )
            .unwrap();

        assert!(matches!(
            db.staleness().unwrap(),
            Some(Staleness::Schema { found: 999, .. })
        ));
    }

    /// Recording the version twice updates in place rather than conflicting.
    #[test]
    fn test_recording_the_version_is_idempotent() {
        let db = Database::in_memory().unwrap();
        db.insert_or_update_file(&mk_file("src/a.rs")).unwrap();
        db.record_index_version().unwrap();
        db.record_index_version().unwrap();
        assert_eq!(db.staleness().unwrap(), None);
    }

    /// Two definitions of one name, one of them in a test file. Resolution
    /// must pick the production one and say that it had a choice — the old
    /// bare `LIMIT 1` did neither.
    #[test]
    fn test_resolve_symbol_prefers_production_and_reports_candidates() {
        let db = Database::in_memory().unwrap();
        db.insert_or_update_file(&mk_file("tests/helper.rs"))
            .unwrap();
        db.insert_or_update_file(&mk_file("src/real.rs")).unwrap();

        let mut test_node = create_test_node("build", NodeKind::Function, "tests/helper.rs");
        test_node.is_test = true;
        db.insert_node(&test_node).unwrap();
        db.insert_node(&create_test_node(
            "build",
            NodeKind::Function,
            "src/real.rs",
        ))
        .unwrap();

        let matched = db
            .resolve_symbol("build", &SymbolHint::default())
            .unwrap()
            .expect("symbol resolves");

        assert_eq!(matched.node.file_path, "src/real.rs");
        assert_eq!(matched.candidates, 2);
        assert!(matched.is_ambiguous());
        assert_eq!(matched.alternatives.len(), 1);
        assert_eq!(matched.alternatives[0].file_path, "tests/helper.rs");
    }

    /// Insertion order must not decide which definition wins.
    #[test]
    fn test_resolve_symbol_is_stable_regardless_of_insert_order() {
        fn resolve_with(order: [&str; 3]) -> String {
            let db = Database::in_memory().unwrap();
            for path in order {
                db.insert_or_update_file(&mk_file(path)).unwrap();
                db.insert_node(&create_test_node("handle", NodeKind::Function, path))
                    .unwrap();
            }
            db.resolve_symbol("handle", &SymbolHint::default())
                .unwrap()
                .unwrap()
                .node
                .file_path
        }

        let forward = resolve_with(["src/a.rs", "src/b.rs", "src/c.rs"]);
        let reverse = resolve_with(["src/c.rs", "src/b.rs", "src/a.rs"]);
        assert_eq!(forward, reverse);
        assert_eq!(forward, "src/a.rs");
    }

    /// A `file` hint narrows the candidate set, and an unambiguous result
    /// reports exactly one candidate.
    #[test]
    fn test_resolve_symbol_honours_file_hint() {
        let db = Database::in_memory().unwrap();
        for path in ["src/a.rs", "src/b.rs"] {
            db.insert_or_update_file(&mk_file(path)).unwrap();
            db.insert_node(&create_test_node("parse", NodeKind::Function, path))
                .unwrap();
        }

        let hint = SymbolHint {
            file: Some("src/b.rs".to_string()),
            qualified_name: None,
        };
        let matched = db.resolve_symbol("parse", &hint).unwrap().unwrap();
        assert_eq!(matched.node.file_path, "src/b.rs");
        assert_eq!(matched.candidates, 1);
        assert!(!matched.is_ambiguous());
    }

    /// A hint that matches nothing resolves to nothing, rather than silently
    /// falling back to some other file's definition.
    #[test]
    fn test_resolve_symbol_hint_with_no_match_is_none() {
        let db = Database::in_memory().unwrap();
        db.insert_or_update_file(&mk_file("src/a.rs")).unwrap();
        db.insert_node(&create_test_node("parse", NodeKind::Function, "src/a.rs"))
            .unwrap();

        let hint = SymbolHint {
            file: Some("src/nowhere.rs".to_string()),
            qualified_name: None,
        };
        assert!(db.resolve_symbol("parse", &hint).unwrap().is_none());
    }

    /// Repeated calls from one function are one caller, not three. Before
    /// `DISTINCT` the same node filled three slots of the page and was counted
    /// three times over.
    #[test]
    fn test_callers_are_distinct_and_counted_once() {
        let db = Database::in_memory().unwrap();
        db.insert_or_update_file(&mk_file("src/lib.rs")).unwrap();
        let caller = db
            .insert_node(&create_test_node(
                "caller",
                NodeKind::Function,
                "src/lib.rs",
            ))
            .unwrap();
        let callee = db
            .insert_node(&create_test_node(
                "callee",
                NodeKind::Function,
                "src/lib.rs",
            ))
            .unwrap();

        for line in 1..=3 {
            db.insert_edge(&Edge::new(caller, callee, EdgeKind::Calls).at(
                "src/lib.rs".to_string(),
                line,
                0,
            ))
            .unwrap();
        }

        assert_eq!(db.count_callers(callee).unwrap(), 1);
        assert_eq!(db.get_callers(callee, 10, 0).unwrap().len(), 1);
        assert_eq!(db.count_callees(caller).unwrap(), 1);
    }

    /// Paging must cover the set exactly once, with no overlap and no gaps.
    #[test]
    fn test_caller_pages_partition_the_set() {
        let db = Database::in_memory().unwrap();
        db.insert_or_update_file(&mk_file("src/lib.rs")).unwrap();
        let callee = db
            .insert_node(&create_test_node(
                "target",
                NodeKind::Function,
                "src/lib.rs",
            ))
            .unwrap();
        for i in 0..5 {
            let mut caller = create_test_node(&format!("c{}", i), NodeKind::Function, "src/lib.rs");
            caller.start_line = 10 + i;
            let id = db.insert_node(&caller).unwrap();
            db.insert_edge(&Edge::new(id, callee, EdgeKind::Calls))
                .unwrap();
        }

        assert_eq!(db.count_callers(callee).unwrap(), 5);

        let first: Vec<String> = db
            .get_callers(callee, 2, 0)
            .unwrap()
            .into_iter()
            .map(|n| n.name)
            .collect();
        let second: Vec<String> = db
            .get_callers(callee, 2, 2)
            .unwrap()
            .into_iter()
            .map(|n| n.name)
            .collect();
        let third: Vec<String> = db
            .get_callers(callee, 2, 4)
            .unwrap()
            .into_iter()
            .map(|n| n.name)
            .collect();

        assert_eq!(first, vec!["c0", "c1"]);
        assert_eq!(second, vec!["c2", "c3"]);
        assert_eq!(third, vec!["c4"]);
    }

    /// The unused count is over the whole set, not over the page.
    #[test]
    fn test_unused_count_is_independent_of_page_size() {
        let db = Database::in_memory().unwrap();
        db.insert_or_update_file(&mk_file("src/lib.rs")).unwrap();
        for i in 0..5 {
            let mut node =
                create_test_node(&format!("dead{}", i), NodeKind::Function, "src/lib.rs");
            node.start_line = 10 + i;
            db.insert_node(&node).unwrap();
        }

        assert_eq!(db.count_unused_symbols(false).unwrap(), 5);
        assert_eq!(db.find_unused_symbols(2, 0, false).unwrap().len(), 2);
        assert_eq!(db.find_unused_symbols(2, 4, false).unwrap().len(), 1);
    }

    /// Several processes share one index file, so a query must wait out a
    /// concurrent writer instead of failing with SQLITE_BUSY straight away.
    #[test]
    fn test_busy_timeout_is_configured() {
        let db = Database::in_memory().unwrap();
        let timeout: i64 = db
            .conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert_eq!(timeout, 5000);
    }

    #[test]
    fn test_in_memory_database_creation() {
        let db = Database::in_memory();
        assert!(db.is_ok());
    }

    #[test]
    fn test_open_tracks_on_disk_path() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("tracked.db");

        let db = Database::open(&path).unwrap();
        assert_eq!(db.path(), Some(path.as_path()));
    }

    #[test]
    fn test_database_stats_empty() {
        let db = Database::in_memory().unwrap();
        let stats = db.get_stats().unwrap();
        assert_eq!(stats.total_files, 0);
        assert_eq!(stats.total_nodes, 0);
        assert_eq!(stats.total_edges, 0);
    }

    // File operations tests
    #[test]
    fn test_upsert_and_get_file() {
        let db = Database::in_memory().unwrap();
        let file = mk_file("test.rs");

        db.insert_or_update_file(&file).unwrap();
        let retrieved = db.get_file("test.rs").unwrap();

        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.path, "test.rs");
        assert_eq!(retrieved.content_hash, "abc123");
        assert_eq!(retrieved.node_count, 5);
    }

    #[test]
    fn test_file_upsert_updates_existing() {
        let db = Database::in_memory().unwrap();
        let mut file = mk_file("src/lib.rs");

        db.insert_or_update_file(&file).unwrap();

        file.content_hash = "updated_hash".to_string();
        file.node_count = 10;
        db.insert_or_update_file(&file).unwrap();

        let retrieved = db.get_file("src/lib.rs").unwrap().unwrap();
        assert_eq!(retrieved.content_hash, "updated_hash");
        assert_eq!(retrieved.node_count, 10);
    }

    #[test]
    fn test_get_nonexistent_file() {
        let db = Database::in_memory().unwrap();
        let result = db.get_file("nonexistent.rs").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_needs_reindex_new_file() {
        let db = Database::in_memory().unwrap();
        let needs = db.needs_reindex("new_file.rs", "somehash").unwrap();
        assert!(needs);
    }

    #[test]
    fn test_needs_reindex_unchanged_file() {
        let db = Database::in_memory().unwrap();
        let file = mk_file("test.rs");
        db.insert_or_update_file(&file).unwrap();

        let needs = db.needs_reindex("test.rs", "abc123").unwrap();
        assert!(!needs);
    }

    #[test]
    fn test_needs_reindex_changed_file() {
        let db = Database::in_memory().unwrap();
        let file = mk_file("test.rs");
        db.insert_or_update_file(&file).unwrap();

        let needs = db.needs_reindex("test.rs", "different_hash").unwrap();
        assert!(needs);
    }

    // Node operations tests
    #[test]
    fn test_insert_and_get_node() {
        let db = Database::in_memory().unwrap();
        let file = mk_file("test.rs");
        db.insert_or_update_file(&file).unwrap();

        let node = create_test_node("my_function", NodeKind::Function, "test.rs");
        let id = db.insert_node(&node).unwrap();

        let retrieved = db.get_node(id).unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.name, "my_function");
        assert_eq!(retrieved.kind, NodeKind::Function);
    }

    #[test]
    fn test_get_nonexistent_node() {
        let db = Database::in_memory().unwrap();
        let result = db.get_node(999).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_search_nodes() {
        let db = Database::in_memory().unwrap();
        let file = mk_file("test.rs");
        db.insert_or_update_file(&file).unwrap();

        db.insert_node(&create_test_node(
            "process_data",
            NodeKind::Function,
            "test.rs",
        ))
        .unwrap();
        db.insert_node(&create_test_node(
            "process_input",
            NodeKind::Function,
            "test.rs",
        ))
        .unwrap();
        db.insert_node(&create_test_node(
            "handle_error",
            NodeKind::Function,
            "test.rs",
        ))
        .unwrap();

        let results = db.search_nodes("process", None, 10).unwrap();
        assert_eq!(results.len(), 2);

        let results = db.search_nodes("handle", None, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "handle_error");
    }

    #[test]
    fn test_search_nodes_with_kind_filter() {
        let db = Database::in_memory().unwrap();
        let file = mk_file("test.rs");
        db.insert_or_update_file(&file).unwrap();

        db.insert_node(&create_test_node("MyClass", NodeKind::Class, "test.rs"))
            .unwrap();
        db.insert_node(&create_test_node(
            "my_function",
            NodeKind::Function,
            "test.rs",
        ))
        .unwrap();

        let results = db.search_nodes("my", Some(NodeKind::Function), 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, NodeKind::Function);
    }

    #[test]
    fn test_search_nodes_case_insensitive() {
        let db = Database::in_memory().unwrap();
        let file = mk_file("test.rs");
        db.insert_or_update_file(&file).unwrap();

        db.insert_node(&create_test_node(
            "MyFunction",
            NodeKind::Function,
            "test.rs",
        ))
        .unwrap();

        let results = db.search_nodes("myfunction", None, 10).unwrap();
        assert_eq!(results.len(), 1);

        let results = db.search_nodes("MYFUNCTION", None, 10).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_find_node_by_name() {
        let db = Database::in_memory().unwrap();
        let file = mk_file("test.rs");
        db.insert_or_update_file(&file).unwrap();

        db.insert_node(&create_test_node(
            "unique_name",
            NodeKind::Function,
            "test.rs",
        ))
        .unwrap();

        let result = db.find_node_by_name("unique_name").unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().name, "unique_name");

        let result = db.find_node_by_name("nonexistent").unwrap();
        assert!(result.is_none());
    }

    // Edge operations tests
    #[test]
    fn test_insert_edge() {
        let db = Database::in_memory().unwrap();
        let file = mk_file("test.rs");
        db.insert_or_update_file(&file).unwrap();

        let id1 = db
            .insert_node(&create_test_node("caller", NodeKind::Function, "test.rs"))
            .unwrap();
        let id2 = db
            .insert_node(&create_test_node("callee", NodeKind::Function, "test.rs"))
            .unwrap();

        let edge = Edge {
            id: 0,
            source_id: id1,
            target_id: id2,
            kind: EdgeKind::Calls,
            file_path: Some("test.rs".to_string()),
            line: Some(5),
            column: Some(10),
            detail: None,
        };

        let edge_id = db.insert_edge(&edge).unwrap();
        assert!(edge_id > 0);
    }

    #[test]
    fn test_get_callers_and_callees() {
        let db = Database::in_memory().unwrap();
        let file = mk_file("test.rs");
        db.insert_or_update_file(&file).unwrap();

        let caller_id = db
            .insert_node(&create_test_node("caller", NodeKind::Function, "test.rs"))
            .unwrap();
        let callee_id = db
            .insert_node(&create_test_node("callee", NodeKind::Function, "test.rs"))
            .unwrap();

        let edge = Edge {
            id: 0,
            source_id: caller_id,
            target_id: callee_id,
            kind: EdgeKind::Calls,
            file_path: None,
            line: None,
            column: None,
            detail: None,
        };
        db.insert_edge(&edge).unwrap();

        let callers = db.get_callers(callee_id, 10, 0).unwrap();
        assert_eq!(callers.len(), 1);
        assert_eq!(callers[0].name, "caller");

        let callees = db.get_callees(caller_id, 10, 0).unwrap();
        assert_eq!(callees.len(), 1);
        assert_eq!(callees[0].name, "callee");
    }

    #[test]
    fn test_get_outgoing_and_incoming_edges() {
        let db = Database::in_memory().unwrap();
        let file = mk_file("test.rs");
        db.insert_or_update_file(&file).unwrap();

        let id1 = db
            .insert_node(&create_test_node("node1", NodeKind::Function, "test.rs"))
            .unwrap();
        let id2 = db
            .insert_node(&create_test_node("node2", NodeKind::Function, "test.rs"))
            .unwrap();

        let edge = Edge {
            id: 0,
            source_id: id1,
            target_id: id2,
            kind: EdgeKind::Calls,
            file_path: None,
            line: None,
            column: None,
            detail: None,
        };
        db.insert_edge(&edge).unwrap();

        let outgoing = db.get_outgoing_edges(id1).unwrap();
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].target_id, id2);

        let incoming = db.get_incoming_edges(id2).unwrap();
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].source_id, id1);
    }

    // Unresolved references tests
    #[test]
    fn test_unresolved_refs() {
        let db = Database::in_memory().unwrap();
        let file = mk_file("test.rs");
        db.insert_or_update_file(&file).unwrap();

        let node_id = db
            .insert_node(&create_test_node("caller", NodeKind::Function, "test.rs"))
            .unwrap();

        let uref = UnresolvedReference {
            source_node_id: node_id,
            reference_name: "some_function".to_string(),
            kind: EdgeKind::Calls,
            file_path: "src/lib.rs".to_string(),
            line: 5,
            column: 10,
            detail: None,
        };

        db.insert_unresolved_ref(&uref).unwrap();

        let refs = db.get_unresolved_refs().unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].reference_name, "some_function");
    }

    #[test]
    fn test_resolve_references() {
        let db = Database::in_memory().unwrap();
        let file1 = mk_file("test.rs");
        db.insert_or_update_file(&file1).unwrap();

        let caller_id = db
            .insert_node(&create_test_node("caller", NodeKind::Function, "test.rs"))
            .unwrap();
        let _callee_id = db
            .insert_node(&create_test_node(
                "target_func",
                NodeKind::Function,
                "test.rs",
            ))
            .unwrap();

        let uref = UnresolvedReference {
            source_node_id: caller_id,
            reference_name: "target_func".to_string(),
            kind: EdgeKind::Calls,
            file_path: "test.rs".to_string(),
            line: 5,
            column: 10,
            detail: None,
        };
        db.insert_unresolved_ref(&uref).unwrap();

        let resolved = db.resolve_references().unwrap();
        assert_eq!(resolved, 1);

        // Check that the edge was created
        let outgoing = db.get_outgoing_edges(caller_id).unwrap();
        assert_eq!(outgoing.len(), 1);

        // Check that unresolved refs are cleared
        let refs = db.get_unresolved_refs().unwrap();
        assert!(refs.is_empty());
    }

    #[test]
    fn test_resolve_references_for_files() {
        let db = Database::in_memory().unwrap();

        // Set up two files
        let file1 = mk_file("src/a.rs");
        let file2 = mk_file("src/b.rs");
        db.insert_or_update_file(&file1).unwrap();
        db.insert_or_update_file(&file2).unwrap();

        // Create target node in file2
        let _target_id = db
            .insert_node(&create_test_node(
                "target_func",
                NodeKind::Function,
                "src/b.rs",
            ))
            .unwrap();

        // Create callers in both files, each with an unresolved ref to target_func
        let caller_a = db
            .insert_node(&create_test_node(
                "caller_a",
                NodeKind::Function,
                "src/a.rs",
            ))
            .unwrap();
        let caller_b = db
            .insert_node(&create_test_node(
                "caller_b",
                NodeKind::Function,
                "src/b.rs",
            ))
            .unwrap();

        db.insert_unresolved_ref(&UnresolvedReference {
            source_node_id: caller_a,
            reference_name: "target_func".to_string(),
            kind: EdgeKind::Calls,
            file_path: "src/a.rs".to_string(),
            line: 10,
            column: 5,
            detail: None,
        })
        .unwrap();
        db.insert_unresolved_ref(&UnresolvedReference {
            source_node_id: caller_b,
            reference_name: "target_func".to_string(),
            kind: EdgeKind::Calls,
            file_path: "src/b.rs".to_string(),
            line: 20,
            column: 5,
            detail: None,
        })
        .unwrap();

        // Resolve only for file a
        let resolved = db
            .resolve_references_for_files(&["src/a.rs".to_string()])
            .unwrap();
        assert_eq!(resolved, 1);

        // caller_a should now have an edge
        let outgoing_a = db.get_outgoing_edges(caller_a).unwrap();
        assert_eq!(outgoing_a.len(), 1);

        // caller_b should still have no edge (its ref is still unresolved)
        let outgoing_b = db.get_outgoing_edges(caller_b).unwrap();
        assert_eq!(outgoing_b.len(), 0);

        // Only file b's ref should remain unresolved
        let refs = db.get_unresolved_refs().unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].file_path, "src/b.rs");
    }

    #[test]
    fn test_resolve_references_for_files_empty() {
        let db = Database::in_memory().unwrap();
        let resolved = db.resolve_references_for_files(&[]).unwrap();
        assert_eq!(resolved, 0);
    }

    #[test]
    fn test_stats() {
        let db = Database::in_memory().unwrap();
        let file = mk_file("test.rs");
        db.insert_or_update_file(&file).unwrap();

        db.insert_node(&create_test_node("func1", NodeKind::Function, "test.rs"))
            .unwrap();
        db.insert_node(&create_test_node("func2", NodeKind::Function, "test.rs"))
            .unwrap();
        db.insert_node(&create_test_node("MyClass", NodeKind::Class, "test.rs"))
            .unwrap();

        let stats = db.get_stats().unwrap();
        assert_eq!(stats.total_files, 1);
        assert_eq!(stats.total_nodes, 3);
        assert_eq!(stats.total_edges, 0);
    }

    #[test]
    fn test_delete_file() {
        let db = Database::in_memory().unwrap();
        let file = mk_file("test.rs");
        db.insert_or_update_file(&file).unwrap();

        let id1 = db
            .insert_node(&create_test_node("func1", NodeKind::Function, "test.rs"))
            .unwrap();
        let id2 = db
            .insert_node(&create_test_node("func2", NodeKind::Function, "test.rs"))
            .unwrap();

        let edge = Edge {
            id: 0,
            source_id: id1,
            target_id: id2,
            kind: EdgeKind::Calls,
            file_path: Some("test.rs".to_string()),
            line: None,
            column: None,
            detail: None,
        };
        db.insert_edge(&edge).unwrap();

        db.delete_file("test.rs").unwrap();

        // File should be gone
        assert!(db.get_file("test.rs").unwrap().is_none());

        // Nodes should be gone
        assert!(db.get_node(id1).unwrap().is_none());
        assert!(db.get_node(id2).unwrap().is_none());

        // Stats should show zeros
        let stats = db.get_stats().unwrap();
        assert_eq!(stats.total_files, 0);
        assert_eq!(stats.total_nodes, 0);
        assert_eq!(stats.total_edges, 0);
    }

    // Transaction tests
    #[test]
    fn test_transaction_commit() {
        let mut db = Database::in_memory().unwrap();
        let file = mk_file("test.rs");
        db.insert_or_update_file(&file).unwrap();

        db.begin_transaction().unwrap();
        db.insert_node(&create_test_node("func1", NodeKind::Function, "test.rs"))
            .unwrap();
        db.commit().unwrap();

        let stats = db.get_stats().unwrap();
        assert_eq!(stats.total_nodes, 1);
    }

    #[test]
    fn test_transaction_rollback() {
        let mut db = Database::in_memory().unwrap();
        let file = mk_file("test.rs");
        db.insert_or_update_file(&file).unwrap();

        db.begin_transaction().unwrap();
        db.insert_node(&create_test_node("func1", NodeKind::Function, "test.rs"))
            .unwrap();
        db.rollback().unwrap();

        let stats = db.get_stats().unwrap();
        assert_eq!(stats.total_nodes, 0);
    }

    #[test]
    fn test_prepare_for_swap_cleans_sidecars() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("shadow.db");
        let db = Database::open(&path).unwrap();
        let file = mk_file("shadow.rs");

        db.insert_or_update_file(&file).unwrap();
        db.insert_node(&create_test_node(
            "shadow_fn",
            NodeKind::Function,
            "shadow.rs",
        ))
        .unwrap();

        let prepared_path = db.prepare_for_swap().unwrap();
        assert_eq!(prepared_path, path);
        assert!(prepared_path.exists());
        assert!(!PathBuf::from(format!("{}-wal", prepared_path.display())).exists());
        assert!(!PathBuf::from(format!("{}-shm", prepared_path.display())).exists());
    }

    #[test]
    fn test_replace_with_shadow_reopens_new_contents() {
        let dir = tempdir().unwrap();
        let live_path = dir.path().join("live.db");
        let shadow_path = dir.path().join("shadow.db");

        let mut live = Database::open(&live_path).unwrap();
        let old_file = mk_file("old.rs");
        live.insert_or_update_file(&old_file).unwrap();
        live.insert_node(&create_test_node("old_fn", NodeKind::Function, "old.rs"))
            .unwrap();

        let shadow = Database::open(&shadow_path).unwrap();
        let new_file = mk_file("new.rs");
        shadow.insert_or_update_file(&new_file).unwrap();
        shadow
            .insert_node(&create_test_node("new_fn", NodeKind::Function, "new.rs"))
            .unwrap();

        let prepared_shadow = shadow.prepare_for_swap().unwrap();
        live.replace_with_shadow(&prepared_shadow).unwrap();

        assert_eq!(live.path(), Some(live_path.as_path()));
        assert!(live.find_node_by_name("old_fn").unwrap().is_none());
        assert!(live.find_node_by_name("new_fn").unwrap().is_some());
        assert!(live.get_file("old.rs").unwrap().is_none());
        assert!(live.get_file("new.rs").unwrap().is_some());
        assert!(!prepared_shadow.exists());
    }

    #[test]
    fn test_cleanup_on_disk_path_removes_db_and_sidecars() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cleanup.db");

        std::fs::write(&path, b"db").unwrap();
        std::fs::write(format!("{}-wal", path.display()), b"wal").unwrap();
        std::fs::write(format!("{}-shm", path.display()), b"shm").unwrap();

        Database::cleanup_on_disk_path(&path).unwrap();

        assert!(!path.exists());
        assert!(!PathBuf::from(format!("{}-wal", path.display())).exists());
        assert!(!PathBuf::from(format!("{}-shm", path.display())).exists());
    }

    #[test]
    fn test_rebuild_fts_indexes_restores_search() {
        let db = Database::in_memory().unwrap();
        let file = mk_file("auth.rs");
        db.insert_or_update_file(&file).unwrap();

        let mut node = create_test_node("validateToken", NodeKind::Function, "auth.rs");
        node.docstring = Some("Validate JWT bearer token".to_string());
        db.insert_nodes_batch_without_fts(&mut [node]).unwrap();

        assert!(db.semantic_search("jwt bearer", 10).unwrap().is_empty());

        db.disable_fts_automerge().unwrap();
        db.rebuild_fts_indexes().unwrap();

        let hits = db.semantic_search("jwt bearer", 10).unwrap();
        assert!(hits.iter().any(|n| n.name == "validateToken"));
    }

    #[test]
    fn test_get_hierarchy() {
        let db = Database::in_memory().unwrap();
        let file = mk_file("test.rs");
        db.insert_or_update_file(&file).unwrap();

        // Create a class and its methods
        let class_id = db
            .insert_node(&create_test_node("MyClass", NodeKind::Class, "test.rs"))
            .unwrap();
        let method_id = db
            .insert_node(&create_test_node("my_method", NodeKind::Method, "test.rs"))
            .unwrap();

        // Create contains relationship
        let edge = Edge {
            id: 0,
            source_id: class_id,
            target_id: method_id,
            kind: EdgeKind::Contains,
            file_path: None,
            line: None,
            column: None,
            detail: None,
        };
        db.insert_edge(&edge).unwrap();

        // Get hierarchy for the method
        let hierarchy = db.get_hierarchy("my_method").unwrap();
        assert_eq!(hierarchy.len(), 1);
        assert_eq!(hierarchy[0].name, "MyClass");

        // Get hierarchy for the class
        let hierarchy = db.get_hierarchy("MyClass").unwrap();
        assert_eq!(hierarchy.len(), 1);
        assert_eq!(hierarchy[0].name, "my_method");
    }

    #[test]
    fn test_find_call_path() {
        let db = Database::in_memory().unwrap();
        let file = mk_file("test.rs");
        db.insert_or_update_file(&file).unwrap();

        // Create a call chain: a -> b -> c
        let a_id = db
            .insert_node(&create_test_node("a", NodeKind::Function, "test.rs"))
            .unwrap();
        let b_id = db
            .insert_node(&create_test_node("b", NodeKind::Function, "test.rs"))
            .unwrap();
        let c_id = db
            .insert_node(&create_test_node("c", NodeKind::Function, "test.rs"))
            .unwrap();

        db.insert_edge(&Edge {
            id: 0,
            source_id: a_id,
            target_id: b_id,
            kind: EdgeKind::Calls,
            file_path: None,
            line: None,
            column: None,
            detail: None,
        })
        .unwrap();

        db.insert_edge(&Edge {
            id: 0,
            source_id: b_id,
            target_id: c_id,
            kind: EdgeKind::Calls,
            file_path: None,
            line: None,
            column: None,
            detail: None,
        })
        .unwrap();

        // Find path from a to c
        let paths = db.find_call_path("a", "c").unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].len(), 3);
        assert_eq!(paths[0][0].name, "a");
        assert_eq!(paths[0][1].name, "b");
        assert_eq!(paths[0][2].name, "c");
    }

    #[test]
    fn test_find_unused_symbols() {
        let db = Database::in_memory().unwrap();
        let file = mk_file("test.rs");
        db.insert_or_update_file(&file).unwrap();

        // Create used and unused functions
        let used_id = db
            .insert_node(&create_test_node(
                "used_func",
                NodeKind::Function,
                "test.rs",
            ))
            .unwrap();
        let _unused_id = db
            .insert_node(&create_test_node(
                "unused_func",
                NodeKind::Function,
                "test.rs",
            ))
            .unwrap();
        let caller_id = db
            .insert_node(&create_test_node("caller", NodeKind::Function, "test.rs"))
            .unwrap();

        // Create a call to used_func
        db.insert_edge(&Edge {
            id: 0,
            source_id: caller_id,
            target_id: used_id,
            kind: EdgeKind::Calls,
            file_path: None,
            line: None,
            column: None,
            detail: None,
        })
        .unwrap();

        // Find unused symbols
        let unused = db.find_unused_symbols(100, 0, false).unwrap();
        assert_eq!(unused.len(), 2); // unused_func and caller (no one calls caller)
        assert!(unused.iter().any(|n| n.name == "unused_func"));
        assert!(unused.iter().any(|n| n.name == "caller"));
    }

    #[test]
    fn test_find_implementations() {
        let db = Database::in_memory().unwrap();
        let file = mk_file("test.rs");
        db.insert_or_update_file(&file).unwrap();

        // Create an interface and implementations
        let interface_id = db
            .insert_node(&create_test_node("MyTrait", NodeKind::Interface, "test.rs"))
            .unwrap();
        let impl1_id = db
            .insert_node(&create_test_node("Impl1", NodeKind::Struct, "test.rs"))
            .unwrap();
        let impl2_id = db
            .insert_node(&create_test_node("Impl2", NodeKind::Struct, "test.rs"))
            .unwrap();

        // Create implements relationships
        db.insert_edge(&Edge {
            id: 0,
            source_id: impl1_id,
            target_id: interface_id,
            kind: EdgeKind::Implements,
            file_path: None,
            line: None,
            column: None,
            detail: None,
        })
        .unwrap();

        db.insert_edge(&Edge {
            id: 0,
            source_id: impl2_id,
            target_id: interface_id,
            kind: EdgeKind::Implements,
            file_path: None,
            line: None,
            column: None,
            detail: None,
        })
        .unwrap();

        // Find implementations
        let impls = db.find_implementations("MyTrait").unwrap();
        assert_eq!(impls.len(), 2);
        assert!(impls.iter().any(|n| n.name == "Impl1"));
        assert!(impls.iter().any(|n| n.name == "Impl2"));
    }

    #[test]
    fn test_get_diff_impact() {
        let db = Database::in_memory().unwrap();
        let file = mk_file("test.rs");
        db.insert_or_update_file(&file).unwrap();

        // Create a function in lines 10-20
        let mut affected_node = create_test_node("affected_func", NodeKind::Function, "test.rs");
        affected_node.start_line = 10;
        affected_node.end_line = 20;
        let affected_id = db.insert_node(&affected_node).unwrap();

        // Create a caller
        let caller_id = db
            .insert_node(&create_test_node(
                "caller_func",
                NodeKind::Function,
                "test.rs",
            ))
            .unwrap();

        db.insert_edge(&Edge {
            id: 0,
            source_id: caller_id,
            target_id: affected_id,
            kind: EdgeKind::Calls,
            file_path: None,
            line: None,
            column: None,
            detail: None,
        })
        .unwrap();

        // Test diff impact for lines 15-18 (overlaps with affected_func)
        let impacted = db.get_diff_impact("test.rs", 15, 18).unwrap();
        assert_eq!(impacted.len(), 2); // affected_func and its caller
        assert!(impacted.iter().any(|n| n.name == "affected_func"));
        assert!(impacted.iter().any(|n| n.name == "caller_func"));
    }
}

#[cfg(test)]
mod language_tests {
    use super::*;
    use crate::types::FileRecord;

    #[test]
    fn test_language_roundtrip() {
        let db = Database::in_memory().unwrap();

        // First insert a file
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

        let node = Node {
            id: 0,
            kind: NodeKind::Function,
            name: "test_func".to_string(),
            qualified_name: None,
            file_path: "test.rs".to_string(),
            start_line: 1,
            end_line: 10,
            start_column: 0,
            end_column: 0,
            signature: Some("fn test_func()".to_string()),
            visibility: Visibility::Private,
            docstring: None,
            is_async: false,
            is_static: false,
            is_exported: false,
            is_test: false,
            is_generated: false,
            language: Language::Rust,
        };

        db.insert_node(&node).unwrap();
        let retrieved = db.find_node_by_name("test_func").unwrap().unwrap();

        assert_eq!(
            retrieved.language,
            Language::Rust,
            "Language should be Rust, got {:?}",
            retrieved.language
        );
        assert_eq!(
            retrieved.visibility,
            Visibility::Private,
            "Visibility should be Private, got {:?}",
            retrieved.visibility
        );
    }
}

#[cfg(test)]
mod language_tests_2 {
    use super::*;
    use crate::types::{FileRecord, NodeKind, UnresolvedReference, Visibility};

    fn mk_file(path: &str) -> FileRecord {
        FileRecord {
            path: path.to_string(),
            content_hash: "h".to_string(),
            language: Language::Rust,
            size: 0,
            modified_at: 0,
            indexed_at: 0,
            node_count: 0,
        }
    }

    fn mk_node(name: &str, file_path: &str) -> Node {
        Node {
            id: 0,
            kind: NodeKind::Function,
            name: name.to_string(),
            qualified_name: None,
            file_path: file_path.to_string(),
            start_line: 1,
            end_line: 1,
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
        }
    }

    #[test]
    fn test_semantic_search_by_docstring() {
        let db = Database::in_memory().unwrap();
        let file = mk_file("auth.rs");
        db.insert_or_update_file(&file).unwrap();

        let mut node = mk_node("validate_token", "auth.rs");
        node.docstring = Some("Verifies a JWT bearer token against the signing key".to_string());
        db.insert_node(&node).unwrap();

        let other = mk_node("calculate_total", "auth.rs");
        db.insert_node(&other).unwrap();

        let hits = db.semantic_search("jwt bearer", 10).unwrap();
        assert!(
            hits.iter().any(|n| n.name == "validate_token"),
            "semantic search should find by docstring"
        );
    }

    #[test]
    fn test_semantic_search_by_camel_case_split() {
        let db = Database::in_memory().unwrap();
        let file = mk_file("svc.rs");
        db.insert_or_update_file(&file).unwrap();

        let node = mk_node("renderUserDashboard", "svc.rs");
        db.insert_node(&node).unwrap();

        let hits = db.semantic_search("dashboard", 10).unwrap();
        assert!(hits.iter().any(|n| n.name == "renderUserDashboard"));
    }

    #[test]
    fn test_resolve_prefers_same_file() {
        let db = Database::in_memory().unwrap();
        let f1 = mk_file("a.rs");
        let f2 = mk_file("b.rs");
        db.insert_or_update_file(&f1).unwrap();
        db.insert_or_update_file(&f2).unwrap();

        let caller_id = db.insert_node(&mk_node("caller", "a.rs")).unwrap();
        // Same name in two files; same-file should win.
        let local_id = db.insert_node(&mk_node("helper", "a.rs")).unwrap();
        let _foreign_id = db.insert_node(&mk_node("helper", "b.rs")).unwrap();

        db.insert_unresolved_ref(&UnresolvedReference {
            source_node_id: caller_id,
            reference_name: "helper".to_string(),
            kind: EdgeKind::Calls,
            file_path: "a.rs".to_string(),
            line: 1,
            column: 0,
            detail: None,
        })
        .unwrap();

        db.resolve_references().unwrap();
        let outgoing = db.get_outgoing_edges(caller_id).unwrap();
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].target_id, local_id);
    }

    #[test]
    fn test_unused_excludes_tests_and_generated() {
        let db = Database::in_memory().unwrap();
        let file = mk_file("x.rs");
        db.insert_or_update_file(&file).unwrap();

        let mut t = mk_node("test_thing", "x.rs");
        t.is_test = true;
        db.insert_node(&t).unwrap();

        let mut g = mk_node("generated_thing", "x.rs");
        g.is_generated = true;
        db.insert_node(&g).unwrap();

        let unused = db.find_unused_symbols(100, 0, false).unwrap();
        assert!(unused.iter().all(|n| n.name != "test_thing"));
        assert!(unused.iter().all(|n| n.name != "generated_thing"));
    }

    /// Two files, one name, two kinds. The reference is a call, so the field
    /// must not win even though it sorts first by the preference ladder —
    /// `a.rs` before `b.rs`.
    ///
    /// This is the shape that produced 434 bogus `calls` edges on symgraph's
    /// own index: test functions referring to a local `path` resolved to a
    /// struct field called `path` in an unrelated module, and the coupling
    /// report then read that as a dependency between the two files.
    #[test]
    fn a_call_skips_a_field_and_takes_the_function() {
        let db = Database::in_memory().unwrap();
        db.insert_or_update_file(&mk_file("a.rs")).unwrap();
        db.insert_or_update_file(&mk_file("b.rs")).unwrap();
        db.insert_or_update_file(&mk_file("c.rs")).unwrap();

        let caller_id = db.insert_node(&mk_node("caller", "c.rs")).unwrap();
        let mut field = mk_node("path", "a.rs");
        field.kind = NodeKind::Field;
        db.insert_node(&field).unwrap();
        let func_id = db.insert_node(&mk_node("path", "b.rs")).unwrap();

        db.insert_unresolved_ref(&UnresolvedReference {
            source_node_id: caller_id,
            reference_name: "path".to_string(),
            kind: EdgeKind::Calls,
            file_path: "c.rs".to_string(),
            line: 1,
            column: 0,
            detail: None,
        })
        .unwrap();

        db.resolve_references().unwrap();
        let outgoing: Vec<_> = db
            .get_outgoing_edges(caller_id)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .collect();
        assert_eq!(outgoing.len(), 1);
        assert_eq!(
            outgoing[0].target_id, func_id,
            "a call resolved to the field rather than the function"
        );
    }

    /// With no callable candidate at all, the reference stays unresolved
    /// rather than resolving to something it could not denote. Unresolved is
    /// counted by `health()`; a wrong edge is silent.
    #[test]
    fn a_call_with_only_a_field_to_match_stays_unresolved() {
        let db = Database::in_memory().unwrap();
        db.insert_or_update_file(&mk_file("a.rs")).unwrap();
        db.insert_or_update_file(&mk_file("c.rs")).unwrap();

        let caller_id = db.insert_node(&mk_node("caller", "c.rs")).unwrap();
        let mut field = mk_node("path", "a.rs");
        field.kind = NodeKind::Field;
        db.insert_node(&field).unwrap();

        db.insert_unresolved_ref(&UnresolvedReference {
            source_node_id: caller_id,
            reference_name: "path".to_string(),
            kind: EdgeKind::Calls,
            file_path: "c.rs".to_string(),
            line: 1,
            column: 0,
            detail: None,
        })
        .unwrap();

        assert_eq!(db.resolve_references().unwrap(), 0);
        assert!(db
            .get_outgoing_edges(caller_id)
            .unwrap()
            .iter()
            .all(|e| e.kind != EdgeKind::Calls));
        assert_eq!(db.get_unresolved_refs().unwrap().len(), 1);
    }

    /// A field read is the mirror image: it must not land on a function.
    #[test]
    fn a_field_read_skips_a_function_and_takes_the_field() {
        let db = Database::in_memory().unwrap();
        db.insert_or_update_file(&mk_file("a.rs")).unwrap();
        db.insert_or_update_file(&mk_file("b.rs")).unwrap();
        db.insert_or_update_file(&mk_file("c.rs")).unwrap();

        let reader_id = db.insert_node(&mk_node("reader", "c.rs")).unwrap();
        db.insert_node(&mk_node("name", "a.rs")).unwrap(); // a function
        let mut field = mk_node("name", "b.rs");
        field.kind = NodeKind::Field;
        let field_id = db.insert_node(&field).unwrap();

        db.insert_unresolved_ref(&UnresolvedReference {
            source_node_id: reader_id,
            reference_name: "name".to_string(),
            kind: EdgeKind::Accesses,
            file_path: "c.rs".to_string(),
            line: 1,
            column: 0,
            detail: None,
        })
        .unwrap();

        db.resolve_references().unwrap();
        let outgoing = db.get_outgoing_edges(reader_id).unwrap();
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].target_id, field_id);
    }

    /// `references` is the catch-all kind and keeps its old, unconstrained
    /// behaviour — the filter must not quietly narrow it.
    #[test]
    fn a_reference_may_still_resolve_to_any_kind() {
        let db = Database::in_memory().unwrap();
        db.insert_or_update_file(&mk_file("a.rs")).unwrap();
        db.insert_or_update_file(&mk_file("c.rs")).unwrap();

        let src_id = db.insert_node(&mk_node("src", "c.rs")).unwrap();
        let mut member = mk_node("Red", "a.rs");
        member.kind = NodeKind::EnumMember;
        let member_id = db.insert_node(&member).unwrap();

        db.insert_unresolved_ref(&UnresolvedReference {
            source_node_id: src_id,
            reference_name: "Red".to_string(),
            kind: EdgeKind::References,
            file_path: "c.rs".to_string(),
            line: 1,
            column: 0,
            detail: None,
        })
        .unwrap();

        db.resolve_references().unwrap();
        let outgoing = db.get_outgoing_edges(src_id).unwrap();
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].target_id, member_id);
    }

    // --- test exclusion ---------------------------------------------------

    /// A helper that builds two files — one production, one test — with an
    /// edge from the test into production, and returns the database.
    fn db_with_a_test_calling_production() -> Database {
        let db = Database::in_memory().unwrap();
        db.insert_or_update_file(&mk_file("src/lib.rs")).unwrap();
        db.insert_or_update_file(&mk_file("tests/it.rs")).unwrap();

        let target = db.insert_node(&mk_node("widget", "src/lib.rs")).unwrap();
        let mut tester = mk_node("it_works", "tests/it.rs");
        tester.is_test = true;
        let tester_id = db.insert_node(&tester).unwrap();

        db.insert_edge(&Edge {
            id: 0,
            source_id: tester_id,
            target_id: target,
            kind: EdgeKind::Calls,
            file_path: Some("tests/it.rs".to_string()),
            line: Some(1),
            column: Some(0),
            detail: None,
        })
        .unwrap();
        db
    }

    /// The coupling view drops test-sourced edges, so a test file never shows
    /// up as a module the production code depends on.
    #[test]
    fn edge_endpoints_exclude_test_code_by_default() {
        let db = db_with_a_test_calling_production();

        let production = db.get_edge_endpoints(false).unwrap();
        assert!(
            production.is_empty(),
            "test-sourced edge leaked into the coupling view: {production:?}"
        );

        let everything = db.get_edge_endpoints(true).unwrap();
        assert_eq!(everything.len(), 1);
        assert_eq!(everything[0].source_file, "tests/it.rs");
    }

    #[test]
    fn incoming_edges_can_exclude_test_callers() {
        let db = db_with_a_test_calling_production();
        let target = db.find_node_by_name("widget").unwrap().unwrap();

        assert_eq!(db.incoming_edges(target.id, true).unwrap().len(), 1);
        assert!(db.incoming_edges(target.id, false).unwrap().is_empty());
        // The unfiltered entry point keeps its old behaviour: `symgraph-callers`
        // is supposed to show that the tests call something.
        assert_eq!(db.get_incoming_edges(target.id).unwrap().len(), 1);
    }

    #[test]
    fn nodes_by_kind_can_exclude_test_definitions() {
        let db = Database::in_memory().unwrap();
        db.insert_or_update_file(&mk_file("src/lib.rs")).unwrap();
        db.insert_or_update_file(&mk_file("tests/it.rs")).unwrap();

        let mut prod = mk_node("Config", "src/lib.rs");
        prod.kind = NodeKind::Struct;
        db.insert_node(&prod).unwrap();

        let mut fixture = mk_node("Fixture", "tests/it.rs");
        fixture.kind = NodeKind::Struct;
        fixture.is_test = true;
        db.insert_node(&fixture).unwrap();

        let production = db.get_nodes_by_kind(NodeKind::Struct, false).unwrap();
        assert_eq!(production.len(), 1);
        assert_eq!(production[0].name, "Config");
        assert_eq!(
            db.get_nodes_by_kind(NodeKind::Struct, true).unwrap().len(),
            2
        );
    }

    /// Production code that only its own tests exercise: alive by the default
    /// measure, dead once test callers stop counting.
    #[test]
    fn unused_can_ignore_test_callers() {
        let db = db_with_a_test_calling_production();

        let default = db.find_unused_symbols(100, 0, false).unwrap();
        assert!(
            default.iter().all(|n| n.name != "widget"),
            "a test caller should keep a symbol alive by default"
        );

        let strict = db.find_unused_symbols(100, 0, true).unwrap();
        assert!(
            strict.iter().any(|n| n.name == "widget"),
            "test-only code should read as unused when test callers are ignored"
        );
        assert_eq!(db.count_unused_symbols(true).unwrap(), strict.len());
    }
}
