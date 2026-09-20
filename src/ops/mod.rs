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

use crate::db::{Database, SymbolHint, SymbolMatch};
use crate::graph::Graph;
use crate::security::{safe_join, validate_relative};
use crate::types::{EdgeKind, Node};

use constants::{
    effective_limit, DEFAULT_CONTEXT_LINES, DEFAULT_GRAPH_LIMIT, DEFAULT_UNUSED_LIMIT,
    MAX_REFERENCES_PER_KIND,
};

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

/// Where one of several competing definitions of a name lives.
#[derive(Serialize)]
pub struct SymbolLocation {
    pub kind: String,
    pub file: String,
    pub line: u32,
}

impl From<&Node> for SymbolLocation {
    fn from(n: &Node) -> Self {
        SymbolLocation {
            kind: n.kind.as_str().to_string(),
            file: n.file_path.clone(),
            line: n.start_line,
        }
    }
}

/// How confidently a symbol name was resolved to one definition.
///
/// Carried on every single-symbol result so a caller can tell an exact answer
/// from a best guess. Name-based resolution picks the highest-preference
/// definition when several share a name; saying so is the difference between a
/// usable answer and a misleading one.
#[derive(Serialize)]
pub struct Resolution {
    /// How many definitions shared this name (after any narrowing hints).
    pub candidates: usize,
    /// True when `candidates > 1` — the chosen definition is one of several.
    pub ambiguous: bool,
    /// Some of the definitions that were not chosen.
    pub alternatives: Vec<SymbolLocation>,
}

impl Resolution {
    pub fn of(matched: &SymbolMatch) -> Self {
        Resolution {
            candidates: matched.candidates,
            ambiguous: matched.is_ambiguous(),
            alternatives: matched
                .alternatives
                .iter()
                .map(SymbolLocation::from)
                .collect(),
        }
    }

    /// A markdown warning naming the other definitions, or nothing when the
    /// resolution was unambiguous.
    fn note(&self) -> String {
        if !self.ambiguous {
            return String::new();
        }
        let mut out = format!(
            "\n> **Ambiguous:** {} definitions share this name; showing the first. \
             Narrow it with `file` or `qualified_name`.\n",
            self.candidates
        );
        for alt in &self.alternatives {
            out.push_str(&format!("> - {} at {}:{}\n", alt.kind, alt.file, alt.line));
        }
        if self.candidates > self.alternatives.len() + 1 {
            out.push_str(&format!(
                "> - … and {} more\n",
                self.candidates - self.alternatives.len() - 1
            ));
        }
        out.push('\n');
        out
    }
}

/// The bounds of one page of a larger result set.
///
/// `shown` is what came back; `total` is what exists. Reporting only `shown` —
/// as these tools used to — makes a truncated list indistinguishable from a
/// complete one, which is precisely the wrong thing to get wrong when someone
/// is deciding whether a change is safe.
#[derive(Serialize)]
pub struct Page {
    pub total: usize,
    pub shown: usize,
    pub offset: u32,
    pub limit: u32,
    pub truncated: bool,
}

impl Page {
    pub fn new(total: usize, shown: usize, limit: u32, offset: u32) -> Self {
        Page {
            total,
            shown,
            offset,
            limit,
            truncated: offset as usize + shown < total,
        }
    }

    /// "20 of 413" when the list is partial, "413" when it is whole.
    fn describe(&self) -> String {
        if self.truncated || self.offset > 0 {
            format!("{} of {}", self.shown, self.total)
        } else {
            self.total.to_string()
        }
    }

    /// A markdown footer telling the reader how to get the next page.
    fn note(&self) -> String {
        if !self.truncated {
            return String::new();
        }
        format!(
            "\n> **Truncated:** showing {} of {}. Pass `offset={}` for the next page, \
             or raise `limit`.\n",
            self.shown,
            self.total,
            self.offset as usize + self.shown
        )
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
    pub resolution: Resolution,
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
        out.push_str(&self.resolution.note());
        out
    }
}

pub fn node_info(
    db: &Database,
    symbol: &str,
    hint: &SymbolHint,
) -> Result<Option<NodeInfo>, String> {
    match db.resolve_symbol(symbol, hint).map_err(|e| e.to_string())? {
        Some(matched) => Ok(Some(NodeInfo {
            resolution: Resolution::of(&matched),
            node: matched.node,
        })),
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
    pub resolution: Resolution,
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
        out.push_str(&self.resolution.note());
        out
    }
}

pub fn definition(
    db: &Database,
    project_root: &str,
    symbol: &str,
    context_lines: Option<u32>,
    hint: &SymbolHint,
) -> Result<Option<DefinitionResult>, String> {
    let matched = match db.resolve_symbol(symbol, hint).map_err(|e| e.to_string())? {
        Some(m) => m,
        None => return Ok(None),
    };
    let resolution = Resolution::of(&matched);
    let node = matched.node;

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
        resolution,
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
    /// How many references of this kind exist in total.
    pub count: usize,
    /// True when `shown` holds fewer than `count`.
    pub truncated: bool,
    pub shown: Vec<ReferenceItem>,
}

#[derive(Serialize)]
pub struct ReferencesResult {
    pub symbol: String,
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub total: usize,
    /// True when any group was cut short by the per-kind limit.
    pub truncated: bool,
    pub groups: Vec<RefGroup>,
    pub resolution: Resolution,
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
        if self.truncated {
            out.push_str(
                "\n> **Truncated:** some groups list only the first few references. \
                 Raise `limit` to see the rest.\n",
            );
        }
        out.push_str(&self.resolution.note());
        out
    }
}

pub fn references(
    db: &Database,
    symbol: &str,
    hint: &SymbolHint,
    limit: Option<u32>,
) -> Result<Option<ReferencesResult>, String> {
    let per_kind = effective_limit(limit, MAX_REFERENCES_PER_KIND as u32) as usize;
    let matched = match db.resolve_symbol(symbol, hint).map_err(|e| e.to_string())? {
        Some(m) => m,
        None => return Ok(None),
    };
    let resolution = Resolution::of(&matched);
    let node = matched.node;
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
            for edge in group_edges.iter().take(per_kind) {
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
                truncated: group_edges.len() > shown.len(),
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
        truncated: groups.iter().any(|g| g.truncated),
        groups,
        resolution,
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
    #[serde(flatten)]
    pub page: Page,
    pub nodes: Vec<Node>,
    pub resolution: Resolution,
}

impl Render for CallList {
    fn to_markdown(&self) -> String {
        if self.nodes.is_empty() {
            let empty = if self.direction == "callers" {
                format!("No callers found for '{}'", self.symbol)
            } else {
                format!("No callees found for '{}'", self.symbol)
            };
            return format!("{}\n{}", empty, self.resolution.note());
        }
        let mut out = if self.direction == "callers" {
            format!(
                "Found {} callers of '{}':\n\n",
                self.page.describe(),
                self.symbol
            )
        } else {
            format!(
                "'{}' calls {} functions:\n\n",
                self.symbol,
                self.page.describe()
            )
        };
        for node in &self.nodes {
            out.push_str(&format::format_node_simple(node));
            out.push('\n');
        }
        out.push_str(&self.page.note());
        out.push_str(&self.resolution.note());
        out
    }
}

/// Shape a `CallPage` (or its absence) into the rendered result.
fn call_list(
    symbol: &str,
    direction: &str,
    page: Option<crate::graph::CallPage>,
    limit: u32,
    offset: u32,
) -> CallList {
    match page {
        Some(p) => CallList {
            symbol: symbol.to_string(),
            direction: direction.to_string(),
            page: Page::new(p.total, p.nodes.len(), limit, offset),
            resolution: Resolution::of(&p.matched),
            nodes: p.nodes,
        },
        // Unresolved symbol: an empty page, not a page of zero out of zero.
        None => CallList {
            symbol: symbol.to_string(),
            direction: direction.to_string(),
            page: Page::new(0, 0, limit, offset),
            nodes: Vec::new(),
            resolution: Resolution {
                candidates: 0,
                ambiguous: false,
                alternatives: Vec::new(),
            },
        },
    }
}

pub fn callers(
    db: &Database,
    symbol: &str,
    hint: &SymbolHint,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<CallList, String> {
    let (limit, offset) = (
        effective_limit(limit, DEFAULT_GRAPH_LIMIT),
        offset.unwrap_or(0),
    );
    let page = Graph::new(db)
        .callers_page(symbol, hint, limit, offset)
        .map_err(|e| e.to_string())?;
    Ok(call_list(symbol, "callers", page, limit, offset))
}

pub fn callees(
    db: &Database,
    symbol: &str,
    hint: &SymbolHint,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<CallList, String> {
    let (limit, offset) = (
        effective_limit(limit, DEFAULT_GRAPH_LIMIT),
        offset.unwrap_or(0),
    );
    let page = Graph::new(db)
        .callees_page(symbol, hint, limit, offset)
        .map_err(|e| e.to_string())?;
    Ok(call_list(symbol, "callees", page, limit, offset))
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
    let path = validate_relative(&normalized).map_err(|e| e.to_string())?;
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
    pub resolution: Resolution,
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
                    return format!(
                        "No hierarchy found for symbol '{}'\n{}",
                        self.symbol,
                        self.resolution.note()
                    );
                }
                let mut out = format!("# Hierarchy for '{}'\n\n", self.symbol);
                out.push_str(&format!("Found {} related symbols:\n\n", self.count));
                for node in &self.nodes {
                    out.push_str(&format::format_node(node));
                    out.push_str("\n\n");
                }
                out.push_str(&self.resolution.note());
                out
            }
            ListingStyle::Implementations => {
                if self.nodes.is_empty() {
                    return format!(
                        "No implementations found for '{}'\n{}",
                        self.symbol,
                        self.resolution.note()
                    );
                }
                let mut out = format!("# Implementations of '{}'\n\n", self.symbol);
                out.push_str(&format!("Found {} implementation(s):\n\n", self.count));
                for node in &self.nodes {
                    out.push_str(&format::format_node(node));
                    out.push_str("\n\n");
                }
                out.push_str(&self.resolution.note());
                out
            }
        }
    }
}

/// The ambiguity of `symbol` itself, independent of what the listing found.
/// Both listings match on name, so a name shared by several definitions makes
/// the listing a union over all of them — worth saying out loud.
fn resolution_for(db: &Database, symbol: &str, hint: &SymbolHint) -> Resolution {
    match db.resolve_symbol(symbol, hint) {
        Ok(Some(matched)) => Resolution::of(&matched),
        _ => Resolution {
            candidates: 0,
            ambiguous: false,
            alternatives: Vec::new(),
        },
    }
}

pub fn hierarchy(db: &Database, symbol: &str, hint: &SymbolHint) -> Result<NodeListing, String> {
    let nodes = db.get_hierarchy(symbol).map_err(|e| e.to_string())?;
    Ok(NodeListing {
        symbol: symbol.to_string(),
        count: nodes.len(),
        nodes,
        resolution: resolution_for(db, symbol, hint),
        style: ListingStyle::Hierarchy,
    })
}

pub fn implementations(
    db: &Database,
    symbol: &str,
    hint: &SymbolHint,
) -> Result<NodeListing, String> {
    let nodes = db.find_implementations(symbol).map_err(|e| e.to_string())?;
    Ok(NodeListing {
        symbol: symbol.to_string(),
        count: nodes.len(),
        nodes,
        resolution: resolution_for(db, symbol, hint),
        style: ListingStyle::Implementations,
    })
}

// ===========================================================================
// unused — dead code grouped by file
// ===========================================================================

#[derive(Serialize)]
pub struct UnusedResult {
    #[serde(flatten)]
    pub page: Page,
    /// Whether a test-only caller counted as a use. Carried into the result
    /// because it changes what "unused" means, and a reader of the JSON has
    /// no other way to tell which question was answered.
    pub ignore_test_callers: bool,
    pub nodes: Vec<Node>,
}

impl UnusedResult {
    /// One line naming the measure in force, so the answer states its own
    /// scope rather than leaving the reader to assume the default.
    fn scope_note(&self) -> &'static str {
        if self.ignore_test_callers {
            "Counting only non-test callers: a symbol its own tests exercise still counts as unused.\n\n"
        } else {
            "A call from test code counts as a use. Pass ignore_test_callers to exclude them.\n\n"
        }
    }
}

impl Render for UnusedResult {
    fn to_markdown(&self) -> String {
        if self.nodes.is_empty() {
            return "No unused symbols found (all symbols are referenced or exported)".to_string();
        }
        let mut out = format!(
            "# Unused Symbols\n\nFound {} unused symbols:\n\n{}",
            self.page.describe(),
            self.scope_note()
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
        out.push_str(&self.page.note());
        out
    }
}

pub fn unused(
    db: &Database,
    limit: Option<u32>,
    offset: Option<u32>,
    ignore_test_callers: bool,
) -> Result<UnusedResult, String> {
    let (limit, offset) = (
        effective_limit(limit, DEFAULT_UNUSED_LIMIT),
        offset.unwrap_or(0),
    );
    let total = db
        .count_unused_symbols(ignore_test_callers)
        .map_err(|e| e.to_string())?;
    let nodes = db
        .find_unused_symbols(limit, offset, ignore_test_callers)
        .map_err(|e| e.to_string())?;
    Ok(UnusedResult {
        page: Page::new(total, nodes.len(), limit, offset),
        ignore_test_callers,
        nodes,
    })
}

// ===========================================================================
// context — task-focused entry points, neighbours and code
// ===========================================================================

/// Task context as a typed result.
///
/// Wraps `TaskContext` so the tool gains `format=json` through the same
/// `present` path as everything else. The CLI previously serialised
/// `TaskContext` directly while the MCP tool had no `format` field at all —
/// one tool, two shapes.
#[derive(Serialize)]
pub struct ContextResult {
    pub task: String,
    #[serde(flatten)]
    pub context: crate::types::TaskContext,
}

impl Render for ContextResult {
    fn to_markdown(&self) -> String {
        crate::context::format_context_markdown(&self.context)
    }
}

pub fn context(
    db: &Database,
    project_root: &str,
    task: &str,
    max_nodes: u32,
) -> Result<ContextResult, String> {
    let options = crate::context::ContextOptions {
        max_nodes,
        include_code: true,
        ..Default::default()
    };
    let context = crate::context::ContextBuilder::new(db, project_root.to_string())
        .build_context(task, &options)
        .map_err(|e| e.to_string())?;
    Ok(ContextResult {
        task: task.to_string(),
        context,
    })
}

// ===========================================================================
// search — symbols matching a query
// ===========================================================================

#[derive(Serialize)]
pub struct SearchResult {
    pub query: String,
    /// True when the search ran over identifier tokens and docstrings rather
    /// than symbol-name prefixes.
    pub semantic: bool,
    #[serde(flatten)]
    pub page: Page,
    pub results: Vec<Node>,
}

impl Render for SearchResult {
    fn to_markdown(&self) -> String {
        if self.results.is_empty() {
            return format!("No symbols found matching '{}'", self.query);
        }
        let mode = if self.semantic { "semantic " } else { "" };
        let mut out = format!(
            "Found {} symbols ({}match) for '{}':\n\n",
            self.page.describe(),
            mode,
            self.query
        );
        for node in &self.results {
            out.push_str(&format::format_node_with_signature(node));
        }
        out.push_str(&self.page.note());
        out
    }
}

pub fn search(
    db: &Database,
    query: &str,
    semantic: bool,
    limit: Option<u32>,
) -> Result<SearchResult, String> {
    let limit = effective_limit(limit, constants::DEFAULT_SEARCH_LIMIT);
    // Ask for one more than we will show: if it comes back, more symbols match
    // than fit the page, and saying so beats implying the list is complete.
    let mut results = if semantic {
        db.semantic_search(query, limit + 1)
    } else {
        db.search_nodes(query, None, limit + 1)
    }
    .map_err(|e| e.to_string())?;

    // A prefix search cannot report a true total without a second count
    // query, so the page reports "at least this many" by counting the probe
    // row. `truncated` is what callers act on, and it is exact.
    let truncated = results.len() > limit as usize;
    results.truncate(limit as usize);
    let total = results.len() + usize::from(truncated);

    Ok(SearchResult {
        query: query.to_string(),
        semantic,
        page: Page::new(total, results.len(), limit, 0),
        results,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Edge, EdgeKind, FileRecord, Language, NodeKind, Visibility};

    fn db_with(files: &[&str]) -> Database {
        let db = Database::in_memory().unwrap();
        for path in files {
            db.insert_or_update_file(&FileRecord {
                path: path.to_string(),
                content_hash: "h".into(),
                language: Language::Rust,
                size: 0,
                modified_at: 0,
                indexed_at: 0,
                node_count: 0,
            })
            .unwrap();
        }
        db
    }

    fn node(name: &str, file: &str, line: u32) -> Node {
        Node {
            id: 0,
            kind: NodeKind::Function,
            name: name.to_string(),
            qualified_name: None,
            file_path: file.to_string(),
            start_line: line,
            end_line: line + 1,
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

    // --- Page -------------------------------------------------------------

    #[test]
    fn page_is_not_truncated_when_it_holds_everything() {
        let p = Page::new(3, 3, 20, 0);
        assert!(!p.truncated);
        assert_eq!(p.describe(), "3");
        assert!(p.note().is_empty());
    }

    #[test]
    fn page_reports_truncation_and_the_next_offset() {
        let p = Page::new(413, 20, 20, 0);
        assert!(p.truncated);
        assert_eq!(p.describe(), "20 of 413");
        assert!(p.note().contains("offset=20"), "note was: {}", p.note());
    }

    #[test]
    fn last_page_of_several_is_not_truncated_but_still_says_of() {
        let p = Page::new(25, 5, 20, 20);
        assert!(!p.truncated);
        assert_eq!(p.describe(), "5 of 25");
    }

    // --- Resolution -------------------------------------------------------

    #[test]
    fn unambiguous_resolution_adds_no_note() {
        let r = Resolution {
            candidates: 1,
            ambiguous: false,
            alternatives: vec![],
        };
        assert!(r.note().is_empty());
    }

    #[test]
    fn ambiguous_resolution_names_the_alternatives_and_the_remainder() {
        let r = Resolution {
            candidates: 9,
            ambiguous: true,
            alternatives: vec![SymbolLocation {
                kind: "function".into(),
                file: "src/b.rs".into(),
                line: 7,
            }],
        };
        let note = r.note();
        assert!(note.contains("9 definitions share this name"));
        assert!(note.contains("src/b.rs:7"));
        // 9 total, 1 shown, 1 chosen => 7 unlisted.
        assert!(note.contains("7 more"), "note was: {}", note);
    }

    // --- callers ----------------------------------------------------------

    #[test]
    fn callers_reports_the_total_not_just_the_page() {
        let db = db_with(&["src/lib.rs"]);
        let target = db.insert_node(&node("target", "src/lib.rs", 1)).unwrap();
        for i in 0..5 {
            let id = db
                .insert_node(&node(&format!("c{}", i), "src/lib.rs", 10 + i))
                .unwrap();
            db.insert_edge(&Edge::new(id, target, EdgeKind::Calls))
                .unwrap();
        }

        let result = callers(&db, "target", &SymbolHint::default(), Some(2), None).unwrap();
        assert_eq!(result.page.total, 5);
        assert_eq!(result.page.shown, 2);
        assert!(result.page.truncated);

        let md = result.to_markdown();
        assert!(md.contains("Found 2 of 5 callers"), "markdown was:\n{}", md);
        assert!(md.contains("Truncated"));
    }

    #[test]
    fn callers_of_an_unambiguous_symbol_carry_no_warning() {
        let db = db_with(&["src/lib.rs"]);
        let target = db.insert_node(&node("solo", "src/lib.rs", 1)).unwrap();
        let caller = db.insert_node(&node("caller", "src/lib.rs", 10)).unwrap();
        db.insert_edge(&Edge::new(caller, target, EdgeKind::Calls))
            .unwrap();

        let result = callers(&db, "solo", &SymbolHint::default(), None, None).unwrap();
        assert!(!result.resolution.ambiguous);
        assert!(!result.page.truncated);
        let md = result.to_markdown();
        assert!(md.contains("Found 1 callers"));
        assert!(!md.contains("Truncated"));
        assert!(!md.contains("Ambiguous"));
    }

    #[test]
    fn callers_of_an_ambiguous_symbol_say_so() {
        let db = db_with(&["src/a.rs", "src/b.rs"]);
        db.insert_node(&node("new", "src/a.rs", 1)).unwrap();
        db.insert_node(&node("new", "src/b.rs", 1)).unwrap();

        let result = callers(&db, "new", &SymbolHint::default(), None, None).unwrap();
        assert!(result.resolution.ambiguous);
        assert_eq!(result.resolution.candidates, 2);
        assert!(result.to_markdown().contains("Ambiguous"));
    }

    #[test]
    fn a_file_hint_resolves_the_ambiguity() {
        let db = db_with(&["src/a.rs", "src/b.rs"]);
        db.insert_node(&node("new", "src/a.rs", 1)).unwrap();
        db.insert_node(&node("new", "src/b.rs", 1)).unwrap();

        let hint = SymbolHint {
            file: Some("src/b.rs".into()),
            qualified_name: None,
        };
        let result = callers(&db, "new", &hint, None, None).unwrap();
        assert!(!result.resolution.ambiguous);
        assert_eq!(result.resolution.candidates, 1);
        assert!(!result.to_markdown().contains("Ambiguous"));
    }

    #[test]
    fn callers_of_a_missing_symbol_is_an_empty_page_not_a_full_one() {
        let db = db_with(&["src/lib.rs"]);
        let result = callers(&db, "nope", &SymbolHint::default(), None, None).unwrap();
        assert_eq!(result.page.total, 0);
        assert!(!result.page.truncated);
        assert_eq!(result.resolution.candidates, 0);
        assert!(result.to_markdown().contains("No callers found"));
    }

    // --- unused -----------------------------------------------------------

    #[test]
    fn unused_pages_and_reports_the_full_total() {
        let db = db_with(&["src/lib.rs"]);
        for i in 0..7 {
            db.insert_node(&node(&format!("dead{}", i), "src/lib.rs", 10 + i))
                .unwrap();
        }

        let result = unused(&db, Some(3), None, false).unwrap();
        assert_eq!(result.page.total, 7);
        assert_eq!(result.page.shown, 3);
        assert!(result.page.truncated);
        assert!(result.to_markdown().contains("Found 3 of 7 unused symbols"));
    }

    // --- JSON shape -------------------------------------------------------

    #[test]
    fn json_exposes_total_and_ambiguity_for_machine_callers() {
        let db = db_with(&["src/a.rs", "src/b.rs"]);
        let target = db.insert_node(&node("run", "src/a.rs", 1)).unwrap();
        db.insert_node(&node("run", "src/b.rs", 1)).unwrap();
        for i in 0..3 {
            let id = db
                .insert_node(&node(&format!("c{}", i), "src/a.rs", 20 + i))
                .unwrap();
            db.insert_edge(&Edge::new(id, target, EdgeKind::Calls))
                .unwrap();
        }

        let result = callers(&db, "run", &SymbolHint::default(), Some(1), None).unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&present(&result, Format::Json).unwrap()).unwrap();

        assert_eq!(json["total"], 3);
        assert_eq!(json["shown"], 1);
        assert_eq!(json["truncated"], true);
        assert_eq!(json["resolution"]["ambiguous"], true);
        assert_eq!(json["resolution"]["candidates"], 2);
    }

    // --- limits -----------------------------------------------------------

    #[test]
    fn limit_is_clamped_and_zero_means_default() {
        use constants::{effective_limit, MAX_LIMIT};
        assert_eq!(effective_limit(None, 20), 20);
        assert_eq!(effective_limit(Some(0), 20), 20);
        assert_eq!(effective_limit(Some(5), 20), 5);
        assert_eq!(effective_limit(Some(MAX_LIMIT * 10), 20), MAX_LIMIT);
    }
}

// ===========================================================================
// git-backed results: blame, churn, diff impact
// ===========================================================================

/// One `git blame` line, parsed into its parts.
#[derive(Serialize)]
pub struct BlameLine {
    pub commit: String,
    pub author: String,
    pub date: String,
    pub line: u32,
    pub text: String,
}

#[derive(Serialize)]
pub struct BlameResult {
    pub symbol: String,
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub lines: Vec<BlameLine>,
    pub resolution: Resolution,
    /// The raw `git blame` output, kept so the markdown rendering is
    /// byte-identical to what this tool has always printed.
    #[serde(skip)]
    raw: String,
}

impl Render for BlameResult {
    fn to_markdown(&self) -> String {
        format!(
            "## blame: `{}` ({}:{}-{})\n\n```\n{}```\n{}",
            self.symbol,
            self.file,
            self.start_line,
            self.end_line,
            self.raw,
            self.resolution.note()
        )
    }
}

/// Build a blame result from a resolved symbol and raw `git blame` output.
pub fn blame_result(node: &Node, resolution: Resolution, raw: String) -> BlameResult {
    BlameResult {
        symbol: node.name.clone(),
        file: node.file_path.clone(),
        start_line: node.start_line,
        end_line: node.end_line,
        lines: raw.lines().filter_map(parse_blame_line).collect(),
        resolution,
        raw,
    }
}

/// Parse `git blame --date=short` porcelain-less output.
///
/// The shape is `<sha> (<author> <date> <line>) <text>`; author names contain
/// spaces, so the fields are taken from the ends of the parenthesised group
/// rather than by splitting it. A line that does not match is still carried,
/// with empty metadata, so JSON output never silently loses one.
pub fn parse_blame_line(line: &str) -> Option<BlameLine> {
    let (commit, rest) = line.split_once(' ')?;
    let open = rest.find('(')?;
    let close = rest.find(')')?;
    if close < open {
        return None;
    }
    let meta = &rest[open + 1..close];
    let text = rest[close + 1..].trim_start_matches(' ').to_string();

    let mut fields: Vec<&str> = meta.split_whitespace().collect();
    let line_no = fields.pop()?.parse().ok()?;
    let date = fields.pop().unwrap_or("").to_string();
    let author = fields.join(" ");

    Some(BlameLine {
        commit: commit.trim_start_matches('^').to_string(),
        author,
        date,
        line: line_no,
        text,
    })
}

/// A file and how many commits touched it in the window.
#[derive(Serialize)]
pub struct ChurnEntry {
    pub file: String,
    pub commits: u32,
}

#[derive(Serialize)]
pub struct ChurnResult {
    pub days: u32,
    pub path: Option<String>,
    #[serde(flatten)]
    pub page: Page,
    pub files: Vec<ChurnEntry>,
}

impl Render for ChurnResult {
    fn to_markdown(&self) -> String {
        let scope = |sep: &str| {
            self.path
                .as_deref()
                .map(|p| format!("{sep}`{p}`"))
                .unwrap_or_default()
        };
        if self.files.is_empty() {
            return format!(
                "No changes in the last {} days{}.",
                self.days,
                scope(" under ")
            );
        }
        let mut out = format!("# Churn (last {} days{})\n\n", self.days, scope(", path="));
        out.push_str("| Commits | File |\n|---:|---|\n");
        for entry in &self.files {
            out.push_str(&format!("| {} | {} |\n", entry.commits, entry.file));
        }
        out.push_str(&self.page.note());
        out
    }
}

/// Symbols affected by a change to one region of one file.
#[derive(Serialize)]
pub struct RegionImpact {
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub total: usize,
    /// Symbols whose own span overlaps the changed region.
    pub direct: Vec<Node>,
    /// Symbols that reach the changed region through the call graph.
    pub indirect: Vec<Node>,
}

impl RegionImpact {
    pub fn new(file: &str, start_line: u32, end_line: u32, nodes: Vec<Node>) -> Self {
        let (direct, indirect): (Vec<Node>, Vec<Node>) = nodes.into_iter().partition(|node| {
            node.file_path == file && node.start_line <= end_line && node.end_line >= start_line
        });
        RegionImpact {
            file: file.to_string(),
            start_line,
            end_line,
            total: direct.len() + indirect.len(),
            direct,
            indirect,
        }
    }
}

impl Render for RegionImpact {
    fn to_markdown(&self) -> String {
        if self.total == 0 {
            return format!(
                "No symbols affected by changes to {}:{}—{}\n",
                self.file, self.start_line, self.end_line
            );
        }
        let mut out = format!(
            "## Impact: {}:{}—{}\n\nPotentially affected: {} symbol(s)\n\n",
            self.file, self.start_line, self.end_line, self.total
        );
        if !self.direct.is_empty() {
            out.push_str("### Directly Modified\n\n");
            for node in &self.direct {
                out.push_str(&format::format_node(node));
                out.push_str("\n\n");
            }
        }
        if !self.indirect.is_empty() {
            out.push_str("### Indirect Impact (Callers)\n\n");
            for node in &self.indirect {
                out.push_str(&format::format_node(node));
                out.push_str("\n\n");
            }
        }
        out
    }
}

#[derive(Serialize)]
pub struct DiffImpactResult {
    /// The ref this was diffed against, when discovered from git rather than
    /// given as an explicit region.
    pub git_ref: Option<String>,
    pub regions: Vec<RegionImpact>,
}

impl Render for DiffImpactResult {
    fn to_markdown(&self) -> String {
        match &self.git_ref {
            Some(git_ref) => {
                if self.regions.is_empty() {
                    return format!("No changes detected against `{}`.", git_ref);
                }
                let mut out = format!(
                    "# Diff impact vs `{}`\n\nChanged regions: {}\n\n",
                    git_ref,
                    self.regions.len()
                );
                for region in &self.regions {
                    out.push_str(&region.to_markdown());
                    out.push_str("\n---\n\n");
                }
                out
            }
            // An explicit region was asked for, so there is exactly one.
            None => self
                .regions
                .first()
                .map(Render::to_markdown)
                .unwrap_or_default(),
        }
    }
}

// ===========================================================================
// reindex — what a rebuild did
// ===========================================================================

/// The outcome of a reindex, however it was triggered.
#[derive(Serialize, Default)]
pub struct ReindexResult {
    /// "full rebuild", "files", or "queued" when the server started one in
    /// the background and has nothing to report yet.
    pub mode: String,
    pub files: u64,
    pub nodes: u64,
    pub edges: u64,
    pub resolved_refs: u64,
    pub errors: u64,
    pub parse_failures: u64,
    pub skipped_too_large: u64,
    /// Problems that did not stop the reindex — a path that failed
    /// validation, a compaction that did not run.
    pub warnings: Vec<String>,
    /// Set when there are no statistics to report, e.g. a background rebuild
    /// that has only just been queued.
    pub message: Option<String>,
}

impl ReindexResult {
    /// A result carrying only a message — nothing was measured.
    pub fn message(mode: &str, message: &str) -> Self {
        ReindexResult {
            mode: mode.to_string(),
            message: Some(message.to_string()),
            ..Default::default()
        }
    }

    pub fn from_stats(mode: &str, stats: &crate::IndexingStats) -> Self {
        ReindexResult {
            mode: mode.to_string(),
            files: stats.files,
            nodes: stats.nodes,
            edges: stats.edges,
            resolved_refs: stats.resolved_refs,
            errors: stats.errors,
            parse_failures: stats.parse_failures,
            skipped_too_large: stats.skipped_too_large,
            warnings: Vec::new(),
            message: None,
        }
    }
}

impl Render for ReindexResult {
    fn to_markdown(&self) -> String {
        if let Some(message) = &self.message {
            return message.clone();
        }
        let mut out = format!(
            "## Reindex Complete\n\n**Mode:** {}\n**Files indexed:** {}\n\
             **Symbols found:** {}\n**Edges created:** {}\n\
             **References resolved:** {}\n**Errors:** {}\n",
            self.mode, self.files, self.nodes, self.edges, self.resolved_refs, self.errors
        );
        if self.parse_failures > 0 {
            out.push_str(&format!(
                "**Files with syntax errors:** {} (their symbols are incomplete)\n",
                self.parse_failures
            ));
        }
        if self.skipped_too_large > 0 {
            out.push_str(&format!(
                "**Files skipped as too large:** {}\n",
                self.skipped_too_large
            ));
        }
        if !self.warnings.is_empty() {
            out.push_str(&format!("\n**Errors:** {}\n", self.warnings.join(", ")));
        }
        out
    }
}
