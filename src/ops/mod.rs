//! Tool operations: gather typed, serializable results and render them as
//! markdown or JSON.
//!
//! This is the presentation-neutral layer shared by the MCP server
//! (`mcp::handlers`) and the CLI (`cli::tools`). Each operation returns a typed
//! result that implements both [`serde::Serialize`] (for `--format json` /
//! `format: "json"`) and [`Render`] (the markdown the server has always
//! produced). [`present`] picks the representation.
//!
//! `ops` is the lowest tool layer: it owns the shared `format` and `constants`
//! helpers and never depends on `mcp` (so the dependency is one-way, mcp → ops).

pub mod constants;
pub mod format;

use std::fs;

use serde::Serialize;

use crate::db::Database;
use crate::graph::Graph;
use crate::security::{safe_join, validate_relative};
use crate::types::{EdgeKind, Node};

use constants::{DEFAULT_CONTEXT_LINES, DEFAULT_GRAPH_LIMIT, MAX_REFERENCES_PER_KIND};

/// Output representation for a tool result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Json,
}

impl Format {
    /// Markdown unless the request's `format` field is `"json"`.
    pub fn from_request(format: &Option<String>) -> Format {
        match format.as_deref() {
            Some(s) if s.eq_ignore_ascii_case("json") => Format::Json,
            _ => Format::Markdown,
        }
    }
}

/// Markdown rendering for a tool result (the server's historical text output).
pub trait Render {
    fn to_markdown(&self) -> String;
}

/// Render `value` as markdown or pretty JSON.
pub fn present<T: Serialize + Render>(value: &T, format: Format) -> Result<String, String> {
    match format {
        Format::Json => serde_json::to_string_pretty(value).map_err(|e| e.to_string()),
        Format::Markdown => Ok(value.to_markdown()),
    }
}

/// Result for a single-symbol lookup that didn't resolve.
#[derive(Serialize)]
pub struct NotFound {
    pub found: bool,
    pub symbol: String,
}

impl NotFound {
    pub fn new(symbol: &str) -> Self {
        Self {
            found: false,
            symbol: symbol.to_string(),
        }
    }
}

impl Render for NotFound {
    fn to_markdown(&self) -> String {
        format!("Symbol '{}' not found", self.symbol)
    }
}

// ===========================================================================
// node — detailed symbol info
// ===========================================================================

#[derive(Serialize)]
pub struct NodeInfo {
    #[serde(flatten)]
    pub node: Node,
}

impl Render for NodeInfo {
    fn to_markdown(&self) -> String {
        let n = &self.node;
        let mut out = format!("## {}: `{}`\n\n", n.kind.as_str(), n.name);
        out.push_str(&format!(
            "**File:** {}:{}-{}\n",
            n.file_path, n.start_line, n.end_line
        ));
        out.push_str(&format!("**Language:** {}\n", n.language.as_str()));
        out.push_str(&format!("**Visibility:** {}\n", n.visibility.as_str()));
        if n.is_async {
            out.push_str("**Async:** yes\n");
        }
        if n.is_static {
            out.push_str("**Static:** yes\n");
        }
        if n.is_exported {
            out.push_str("**Exported:** yes\n");
        }
        if let Some(ref sig) = n.signature {
            out.push_str(&format!("\n**Signature:**\n```\n{}\n```\n", sig));
        }
        if let Some(ref doc) = n.docstring {
            out.push_str(&format!("\n**Documentation:**\n{}\n", doc));
        }
        out
    }
}

pub fn node_info(db: &Database, symbol: &str) -> Result<Option<NodeInfo>, String> {
    match db.find_node_by_name(symbol).map_err(|e| e.to_string())? {
        Some(node) => Ok(Some(NodeInfo { node })),
        None => Ok(None),
    }
}

// ===========================================================================
// definition — source with surrounding context
// ===========================================================================

#[derive(Serialize)]
pub struct DefinitionResult {
    pub symbol: String,
    pub kind: String,
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub language: String,
    pub signature: Option<String>,
    /// The definition's source lines, joined.
    pub code: String,
    #[serde(skip)]
    before: Vec<String>,
    #[serde(skip)]
    after: Vec<String>,
    #[serde(skip)]
    before_start: usize,
    #[serde(skip)]
    def_start: usize,
    #[serde(skip)]
    after_start: usize,
}

impl Render for DefinitionResult {
    fn to_markdown(&self) -> String {
        let mut out = format!(
            "## {} `{}`\n\n**File:** {}:{}-{}\n**Language:** {}\n\n",
            self.kind, self.symbol, self.file, self.start_line, self.end_line, self.language
        );
        if let Some(ref sig) = self.signature {
            out.push_str(&format!("**Signature:** `{}`\n\n", sig));
        }
        out.push_str("```");
        out.push_str(&self.language);
        if self.before.is_empty() {
            out.push('\n');
        } else {
            out.push_str("\n// ... context before\n");
            for (i, line) in self.before.iter().enumerate() {
                out.push_str(&format!("{:4} │ {}\n", self.before_start + i, line));
            }
            out.push_str("// --- definition starts ---\n");
        }
        for (i, line) in self.code.lines().enumerate() {
            out.push_str(&format!("{:4} │ {}\n", self.def_start + i, line));
        }
        if !self.after.is_empty() {
            out.push_str("// --- definition ends ---\n");
            for (i, line) in self.after.iter().enumerate() {
                out.push_str(&format!("{:4} │ {}\n", self.after_start + i, line));
            }
            out.push_str("// ... context after\n");
        }
        out.push_str("```\n");
        out
    }
}

pub fn definition(
    db: &Database,
    project_root: &str,
    symbol: &str,
    context_lines: Option<u32>,
) -> Result<Option<DefinitionResult>, String> {
    let node = match db.find_node_by_name(symbol).map_err(|e| e.to_string())? {
        Some(n) => n,
        None => return Ok(None),
    };

    let context_lines = context_lines.unwrap_or(DEFAULT_CONTEXT_LINES) as usize;
    let file_path = safe_join(project_root, &node.file_path).map_err(|e| e.to_string())?;
    let content = fs::read_to_string(&file_path)
        .map_err(|e| format!("reading file {}: {}", node.file_path, e))?;
    let lines: Vec<&str> = content.lines().collect();

    let start = (node.start_line as usize).saturating_sub(1);
    let end = (node.end_line as usize).min(lines.len());
    if start >= lines.len() {
        return Err(format!(
            "line range {}-{} out of bounds",
            node.start_line, node.end_line
        ));
    }

    let ctx_start = start.saturating_sub(context_lines);
    let ctx_end = (end + context_lines).min(lines.len());

    Ok(Some(DefinitionResult {
        symbol: node.name.clone(),
        kind: node.kind.as_str().to_string(),
        file: node.file_path.clone(),
        start_line: node.start_line,
        end_line: node.end_line,
        language: node.language.as_str().to_string(),
        signature: node.signature.clone(),
        code: lines[start..end].join("\n"),
        before: lines[ctx_start..start]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        after: lines[end..ctx_end].iter().map(|s| s.to_string()).collect(),
        before_start: ctx_start + 1,
        def_start: start + 1,
        after_start: end + 1,
    }))
}

// ===========================================================================
// references — incoming edges grouped by kind
// ===========================================================================

#[derive(Serialize)]
pub struct ReferenceItem {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub line: Option<u32>,
}

#[derive(Serialize)]
pub struct RefGroup {
    pub edge_kind: String,
    pub count: usize,
    pub shown: Vec<ReferenceItem>,
}

#[derive(Serialize)]
pub struct ReferencesResult {
    pub symbol: String,
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub total: usize,
    pub groups: Vec<RefGroup>,
}

impl Render for ReferencesResult {
    fn to_markdown(&self) -> String {
        if self.groups.is_empty() {
            return format!("No references found for '{}'", self.symbol);
        }
        let mut out = format!(
            "## References to `{}`\n\n**Location:** {}:{}-{}\n\n",
            self.symbol, self.file, self.start_line, self.end_line
        );
        for g in &self.groups {
            out.push_str(&format!("### {} ({}):\n\n", g.edge_kind, g.count));
            for r in &g.shown {
                out.push_str(&format!("- `{}` ({}) - {}", r.name, r.kind, r.file));
                if let Some(line) = r.line {
                    out.push_str(&format!(":{}", line));
                }
                out.push('\n');
            }
            if g.count > g.shown.len() {
                out.push_str(&format!("  ... and {} more\n", g.count - g.shown.len()));
            }
            out.push('\n');
        }
        out.push_str(&format!("**Total references:** {}\n", self.total));
        out
    }
}

pub fn references(db: &Database, symbol: &str) -> Result<Option<ReferencesResult>, String> {
    let node = match db.find_node_by_name(symbol).map_err(|e| e.to_string())? {
        Some(n) => n,
        None => return Ok(None),
    };
    let edges = db.get_incoming_edges(node.id).map_err(|e| e.to_string())?;

    let mut by_kind: std::collections::HashMap<EdgeKind, Vec<_>> = std::collections::HashMap::new();
    for edge in &edges {
        by_kind.entry(edge.kind).or_default().push(edge);
    }

    let mut groups = Vec::new();
    let mut total = 0;
    for kind in [
        EdgeKind::Calls,
        EdgeKind::Imports,
        EdgeKind::Extends,
        EdgeKind::Implements,
        EdgeKind::Contains,
        EdgeKind::References,
        EdgeKind::Exports,
    ] {
        if let Some(group_edges) = by_kind.get(&kind) {
            total += group_edges.len();
            let mut shown = Vec::new();
            for edge in group_edges.iter().take(MAX_REFERENCES_PER_KIND) {
                if let Ok(Some(source)) = db.get_node(edge.source_id) {
                    shown.push(ReferenceItem {
                        name: source.name,
                        kind: source.kind.as_str().to_string(),
                        file: source.file_path,
                        line: edge.line,
                    });
                }
            }
            groups.push(RefGroup {
                edge_kind: kind.as_str().to_string(),
                count: group_edges.len(),
                shown,
            });
        }
    }

    Ok(Some(ReferencesResult {
        symbol: node.name.clone(),
        file: node.file_path.clone(),
        start_line: node.start_line,
        end_line: node.end_line,
        total,
        groups,
    }))
}

// ===========================================================================
// callers / callees — simple node lists
// ===========================================================================

#[derive(Serialize)]
pub struct CallList {
    pub symbol: String,
    /// "callers" or "callees".
    pub direction: String,
    pub count: usize,
    pub nodes: Vec<Node>,
}

impl Render for CallList {
    fn to_markdown(&self) -> String {
        if self.nodes.is_empty() {
            return if self.direction == "callers" {
                format!("No callers found for '{}'", self.symbol)
            } else {
                format!("No callees found for '{}'", self.symbol)
            };
        }
        let mut out = if self.direction == "callers" {
            format!("Found {} callers of '{}':\n\n", self.count, self.symbol)
        } else {
            format!("'{}' calls {} functions:\n\n", self.symbol, self.count)
        };
        for node in &self.nodes {
            out.push_str(&format::format_node_simple(node));
            out.push('\n');
        }
        out
    }
}

pub fn callers(db: &Database, symbol: &str) -> Result<CallList, String> {
    let nodes = Graph::new(db)
        .find_callers(symbol, DEFAULT_GRAPH_LIMIT)
        .map_err(|e| e.to_string())?;
    Ok(CallList {
        symbol: symbol.to_string(),
        direction: "callers".to_string(),
        count: nodes.len(),
        nodes,
    })
}

pub fn callees(db: &Database, symbol: &str) -> Result<CallList, String> {
    let nodes = Graph::new(db)
        .find_callees(symbol, DEFAULT_GRAPH_LIMIT)
        .map_err(|e| e.to_string())?;
    Ok(CallList {
        symbol: symbol.to_string(),
        direction: "callees".to_string(),
        count: nodes.len(),
        nodes,
    })
}

// ===========================================================================
// file — symbols defined in a file, grouped by kind
// ===========================================================================

#[derive(Serialize)]
pub struct FileSymbols {
    pub file: String,
    pub count: usize,
    pub symbols: Vec<Node>,
}

impl Render for FileSymbols {
    fn to_markdown(&self) -> String {
        if self.symbols.is_empty() {
            return format!(
                "No symbols found in '{}'. File may not be indexed.",
                self.file
            );
        }
        let mut out = format!("## Symbols in `{}`\n\n", self.file);
        out.push_str(&format!("Found {} symbols:\n\n", self.count));

        let mut by_kind: std::collections::HashMap<String, Vec<&Node>> =
            std::collections::HashMap::new();
        for node in &self.symbols {
            by_kind
                .entry(node.kind.as_str().to_string())
                .or_default()
                .push(node);
        }
        let mut kinds: Vec<_> = by_kind.keys().cloned().collect();
        kinds.sort();
        for kind in kinds {
            let nodes = &by_kind[&kind];
            out.push_str(&format!("### {} ({}):\n\n", kind, nodes.len()));
            for node in nodes {
                out.push_str(&format!(
                    "- `{}` (lines {}-{})",
                    node.name, node.start_line, node.end_line
                ));
                if let Some(ref sig) = node.signature {
                    out.push_str(&format!(" - `{}`", sig));
                }
                out.push('\n');
            }
            out.push('\n');
        }
        out
    }
}

pub fn file_symbols(db: &Database, path: &str) -> Result<FileSymbols, String> {
    let normalized = format::normalize_path(path);
    let path = validate_relative(normalized).map_err(|e| e.to_string())?;
    let symbols = db.get_nodes_by_file(path).map_err(|e| e.to_string())?;
    Ok(FileSymbols {
        file: path.to_string(),
        count: symbols.len(),
        symbols,
    })
}

// ===========================================================================
// hierarchy / implementations — node lists rendered with format_node
// ===========================================================================

#[derive(Serialize)]
pub struct NodeListing {
    pub symbol: String,
    pub count: usize,
    pub nodes: Vec<Node>,
    #[serde(skip)]
    style: ListingStyle,
}

#[derive(Clone, Copy)]
enum ListingStyle {
    Hierarchy,
    Implementations,
}

impl Render for NodeListing {
    fn to_markdown(&self) -> String {
        match self.style {
            ListingStyle::Hierarchy => {
                if self.nodes.is_empty() {
                    return format!("No hierarchy found for symbol '{}'", self.symbol);
                }
                let mut out = format!("# Hierarchy for '{}'\n\n", self.symbol);
                out.push_str(&format!("Found {} related symbols:\n\n", self.count));
                for node in &self.nodes {
                    out.push_str(&format::format_node(node));
                    out.push_str("\n\n");
                }
                out
            }
            ListingStyle::Implementations => {
                if self.nodes.is_empty() {
                    return format!("No implementations found for '{}'", self.symbol);
                }
                let mut out = format!("# Implementations of '{}'\n\n", self.symbol);
                out.push_str(&format!("Found {} implementation(s):\n\n", self.count));
                for node in &self.nodes {
                    out.push_str(&format::format_node(node));
                    out.push_str("\n\n");
                }
                out
            }
        }
    }
}

pub fn hierarchy(db: &Database, symbol: &str) -> Result<NodeListing, String> {
    let nodes = db.get_hierarchy(symbol).map_err(|e| e.to_string())?;
    Ok(NodeListing {
        symbol: symbol.to_string(),
        count: nodes.len(),
        nodes,
        style: ListingStyle::Hierarchy,
    })
}

pub fn implementations(db: &Database, symbol: &str) -> Result<NodeListing, String> {
    let nodes = db.find_implementations(symbol).map_err(|e| e.to_string())?;
    Ok(NodeListing {
        symbol: symbol.to_string(),
        count: nodes.len(),
        nodes,
        style: ListingStyle::Implementations,
    })
}

// ===========================================================================
// unused — dead code grouped by file
// ===========================================================================

#[derive(Serialize)]
pub struct UnusedResult {
    pub count: usize,
    pub nodes: Vec<Node>,
}

impl Render for UnusedResult {
    fn to_markdown(&self) -> String {
        if self.nodes.is_empty() {
            return "No unused symbols found (all symbols are referenced or exported)".to_string();
        }
        let mut out = format!(
            "# Unused Symbols\n\nFound {} unused symbols:\n\n",
            self.count
        );
        let mut by_file: std::collections::HashMap<String, Vec<&Node>> =
            std::collections::HashMap::new();
        for node in &self.nodes {
            by_file
                .entry(node.file_path.clone())
                .or_default()
                .push(node);
        }
        let mut files: Vec<_> = by_file.keys().cloned().collect();
        files.sort();
        for file_path in files {
            let nodes = &by_file[&file_path];
            out.push_str(&format!("## {}\n\n", file_path));
            for node in nodes {
                out.push_str(&format!(
                    "- {} `{}` at line {}\n",
                    node.kind.as_str(),
                    node.name,
                    node.start_line
                ));
            }
            out.push('\n');
        }
        out
    }
}

pub fn unused(db: &Database) -> Result<UnusedResult, String> {
    let nodes = db.find_unused_symbols().map_err(|e| e.to_string())?;
    Ok(UnusedResult {
        count: nodes.len(),
        nodes,
    })
}

// ===========================================================================
// path — call paths between two symbols
// ===========================================================================

#[derive(Serialize)]
pub struct PathStep {
    pub name: String,
    pub file: String,
    pub line: u32,
}

#[derive(Serialize)]
pub struct CallPaths {
    pub from: String,
    pub to: String,
    pub paths: Vec<Vec<PathStep>>,
}

impl Render for CallPaths {
    fn to_markdown(&self) -> String {
        if self.paths.is_empty() {
            return format!("No call path found from '{}' to '{}'", self.from, self.to);
        }
        let mut out = format!("# Call Paths from '{}' to '{}'\n\n", self.from, self.to);
        out.push_str(&format!("Found {} path(s):\n\n", self.paths.len()));
        for (i, path) in self.paths.iter().enumerate() {
            out.push_str(&format!("## Path {}\n\n", i + 1));
            for (j, step) in path.iter().enumerate() {
                if j > 0 {
                    out.push_str("  ↓ calls\n");
                }
                out.push_str(&format!(
                    "{}. {} ({}:{})\n",
                    j + 1,
                    step.name,
                    step.file,
                    step.line
                ));
            }
            out.push('\n');
        }
        out
    }
}

pub fn call_paths(db: &Database, from: &str, to: &str) -> Result<CallPaths, String> {
    let raw = db.find_call_path(from, to).map_err(|e| e.to_string())?;
    let paths = raw
        .into_iter()
        .map(|path| {
            path.into_iter()
                .map(|n| PathStep {
                    name: n.name,
                    file: n.file_path,
                    line: n.start_line,
                })
                .collect()
        })
        .collect();
    Ok(CallPaths {
        from: from.to_string(),
        to: to.to_string(),
        paths,
    })
}
