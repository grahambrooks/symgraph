//! Request and response types for MCP tools

use serde::Deserialize;

use crate::db::SymbolHint;
use crate::ops::format::normalize_path;

/// Request for context tool
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ContextRequest {
    #[schemars(description = "Description of the task, bug, or feature to explore")]
    pub task: String,
    #[schemars(description = "Max entry points and related symbols to gather (default 20)")]
    pub limit: Option<u32>,
    #[schemars(description = "Output format: 'markdown' (default) or 'json'")]
    pub format: Option<String>,
}

/// Request for search tool
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchRequest {
    #[schemars(description = "Symbol name or partial name to search for")]
    pub query: String,
    #[schemars(
        description = "If true, run semantic (bm25) search over identifier tokens + docstrings instead of prefix-only name search"
    )]
    pub semantic: Option<bool>,
    #[schemars(description = "Max results to return (default 10, max 1000)")]
    pub limit: Option<u32>,
    #[schemars(description = "Output format: 'markdown' (default) or 'json'")]
    pub format: Option<String>,
}

/// Request for symbol-based tools (callers, callees, node, references,
/// hierarchy, implementations)
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SymbolRequest {
    #[schemars(description = "Function/method/class name")]
    pub symbol: String,
    #[schemars(
        description = "Disambiguate: only consider the definition in this file (repo-relative path). Use when the result reports ambiguous=true."
    )]
    pub file: Option<String>,
    #[schemars(
        description = "Disambiguate: only consider the definition with this exact qualified name."
    )]
    pub qualified_name: Option<String>,
    #[schemars(description = "Max results to return (default varies by tool, max 1000)")]
    pub limit: Option<u32>,
    #[schemars(
        description = "Skip this many results — use with limit to page through a truncated list"
    )]
    pub offset: Option<u32>,
    #[schemars(description = "Output format: 'markdown' (default) or 'json'")]
    pub format: Option<String>,
}

/// Request for the impact tool (symbol + coupling-breakdown options)
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ImpactRequest {
    #[schemars(description = "Function/method/class/struct name")]
    pub symbol: String,
    #[schemars(description = "Disambiguate: only consider the definition in this file")]
    pub file: Option<String>,
    #[schemars(
        description = "Disambiguate: only consider the definition with this qualified name"
    )]
    pub qualified_name: Option<String>,
    #[schemars(
        description = "If true, annotate inbound modules with git churn (volatility). Default: false"
    )]
    pub churn: Option<bool>,
    #[schemars(description = "Churn window in days when churn=true (default: 90)")]
    pub days: Option<u32>,
    #[schemars(description = "Output format: 'markdown' (default) or 'json'")]
    pub format: Option<String>,
}

/// Request for file-based tools
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FileRequest {
    #[schemars(description = "File path relative to project root (e.g., 'src/main.rs')")]
    pub path: String,
    #[schemars(description = "Output format: 'markdown' (default) or 'json'")]
    pub format: Option<String>,
}

/// Request for definition tool with context options
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DefinitionRequest {
    #[schemars(description = "Function/method/class name")]
    pub symbol: String,
    #[schemars(description = "Disambiguate: only consider the definition in this file")]
    pub file: Option<String>,
    #[schemars(
        description = "Disambiguate: only consider the definition with this qualified name"
    )]
    pub qualified_name: Option<String>,
    #[schemars(description = "Number of context lines before/after (default: 3)")]
    pub context_lines: Option<u32>,
    #[schemars(description = "Output format: 'markdown' (default) or 'json'")]
    pub format: Option<String>,
}

/// Request carrying only an output format (for parameterless tools like unused).
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct FormatRequest {
    #[schemars(description = "Max results to return (default 100, max 1000)")]
    pub limit: Option<u32>,
    #[schemars(description = "Skip this many results — use with limit to page through the list")]
    pub offset: Option<u32>,
    #[schemars(description = "Output format: 'markdown' (default) or 'json'")]
    pub format: Option<String>,
}

/// Request for the unused-symbol tool.
///
/// Separate from [`FormatRequest`] because `ignore_test_callers` is meaningful
/// only here, and `symgraph-status` shares that type.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct UnusedRequest {
    #[schemars(description = "Max results to return (default 100, max 1000)")]
    pub limit: Option<u32>,
    #[schemars(description = "Skip this many results — use with limit to page through the list")]
    pub offset: Option<u32>,
    #[schemars(
        description = "If true, a symbol called only from test code still counts as unused — which finds production code kept alive by nothing but its own tests. Default: false"
    )]
    pub ignore_test_callers: Option<bool>,
    #[schemars(description = "Output format: 'markdown' (default) or 'json'")]
    pub format: Option<String>,
}

/// Request for reindex tool
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReindexRequest {
    #[schemars(
        description = "Optional: specific files to reindex. If omitted, rebuilds the full index via a shadow database."
    )]
    pub files: Option<Vec<String>>,
    #[schemars(description = "Output format: 'markdown' (default) or 'json'")]
    pub format: Option<String>,
}

/// Request for call path tool
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PathRequest {
    #[schemars(description = "Starting symbol name")]
    pub from: String,
    #[schemars(description = "Target symbol name")]
    pub to: String,
    #[schemars(description = "Output format: 'markdown' (default) or 'json'")]
    pub format: Option<String>,
}

/// Request for diff impact tool
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DiffImpactRequest {
    #[schemars(description = "File path relative to project root (optional when git_ref is set)")]
    pub file_path: Option<String>,
    #[schemars(description = "Start line of the change (1-indexed); ignored when git_ref is set")]
    pub start_line: Option<u32>,
    #[schemars(description = "End line of the change (1-indexed); ignored when git_ref is set")]
    pub end_line: Option<u32>,
    #[schemars(
        description = "Optional git ref (commit, branch, or HEAD~N) to diff against working tree. Discovers changed files and line ranges automatically."
    )]
    pub git_ref: Option<String>,
    #[schemars(description = "Output format: 'markdown' (default) or 'json'")]
    pub format: Option<String>,
}

/// Request for blame tool
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BlameRequest {
    #[schemars(description = "Symbol name to blame")]
    pub symbol: String,
    #[schemars(description = "Disambiguate: only consider the definition in this file")]
    pub file: Option<String>,
    #[schemars(
        description = "Disambiguate: only consider the definition with this qualified name"
    )]
    pub qualified_name: Option<String>,
    #[schemars(description = "Output format: 'markdown' (default) or 'json'")]
    pub format: Option<String>,
}

/// Request for churn tool
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ChurnRequest {
    #[schemars(
        description = "Optional path filter (file or directory). If omitted, returns top churn across the project."
    )]
    pub path: Option<String>,
    #[schemars(description = "How many days of history to scan (default: 90)")]
    pub days: Option<u32>,
    #[schemars(description = "Max files to list (default 30, max 1000)")]
    pub limit: Option<u32>,
    #[schemars(description = "Output format: 'markdown' (default) or 'json'")]
    pub format: Option<String>,
}

/// Request for the module-graph and coupling-score tools.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ModuleGraphRequest {
    #[schemars(
        description = "Aggregation boundary: 'file', 'dir', or 'module' (default: 'module')"
    )]
    pub granularity: Option<String>,
    #[schemars(
        description = "If true, annotate nodes/scores with git churn (commits in the window) for the volatility dimension. Default: false"
    )]
    pub churn: Option<bool>,
    #[schemars(description = "Churn window in days when churn=true (default: 90)")]
    pub days: Option<u32>,
    #[schemars(
        description = "If true, count edges into and out of test code. Off by default: tests depend on everything, so including them inflates fan-in and merges unrelated modules into one cycle."
    )]
    pub include_tests: Option<bool>,
    #[schemars(description = "Output format: 'markdown' (default) or 'json'")]
    pub format: Option<String>,
    #[schemars(description = "Max rows to show in markdown output (default: 30)")]
    pub limit: Option<u32>,
}

/// Request for the god-struct / hub report.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GodStructRequest {
    #[schemars(
        description = "If true, factor git churn into the debt score for the volatility dimension. Default: false"
    )]
    pub churn: Option<bool>,
    #[schemars(description = "Churn window in days when churn=true (default: 90)")]
    pub days: Option<u32>,
    #[schemars(
        description = "If true, rank structs defined in test code too, and count references from test code. Default: false"
    )]
    pub include_tests: Option<bool>,
    #[schemars(description = "Output format: 'markdown' (default) or 'json'")]
    pub format: Option<String>,
    #[schemars(description = "Max structs to show (default: 20)")]
    pub limit: Option<u32>,
}

/// Request for the dispatch-sites tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DispatchSitesRequest {
    #[schemars(description = "Enum name whose member dispatch sites to find (e.g. 'ViewKind')")]
    pub symbol: String,
    #[schemars(description = "Output format: 'markdown' (default) or 'json'")]
    pub format: Option<String>,
}

/// Helper: does this format string request JSON output?
pub fn wants_json(format: &Option<String>) -> bool {
    format.as_deref().map(|f| f.eq_ignore_ascii_case("json")) == Some(true)
}

/// Build the resolution hints from a request's optional `file` /
/// `qualified_name` fields.
///
/// `file` is normalised the same way indexed paths are, so a caller copying a
/// `./src/foo.rs` out of one tool's output can paste it into the next.
pub fn symbol_hint(file: &Option<String>, qualified_name: &Option<String>) -> SymbolHint {
    SymbolHint {
        file: file
            .as_deref()
            .map(|f| normalize_path(f).to_string())
            .filter(|f| !f.is_empty()),
        qualified_name: qualified_name.clone().filter(|q| !q.is_empty()),
    }
}

impl SymbolRequest {
    pub fn hint(&self) -> SymbolHint {
        symbol_hint(&self.file, &self.qualified_name)
    }
}

impl ImpactRequest {
    pub fn hint(&self) -> SymbolHint {
        symbol_hint(&self.file, &self.qualified_name)
    }
}

impl DefinitionRequest {
    pub fn hint(&self) -> SymbolHint {
        symbol_hint(&self.file, &self.qualified_name)
    }
}

impl BlameRequest {
    pub fn hint(&self) -> SymbolHint {
        symbol_hint(&self.file, &self.qualified_name)
    }
}
