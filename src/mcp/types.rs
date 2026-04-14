//! Request and response types for MCP tools

use rmcp::schemars;
use serde::Deserialize;

/// Request for context tool
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ContextRequest {
    #[schemars(description = "Description of the task, bug, or feature to explore")]
    pub task: String,
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
}

/// Request for symbol-based tools (callers, callees, impact, node)
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SymbolRequest {
    #[schemars(description = "Function/method/class name")]
    pub symbol: String,
}

/// Request for file-based tools
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FileRequest {
    #[schemars(description = "File path relative to project root (e.g., 'src/main.rs')")]
    pub path: String,
}

/// Request for definition tool with context options
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DefinitionRequest {
    #[schemars(description = "Function/method/class name")]
    pub symbol: String,
    #[schemars(description = "Number of context lines before/after (default: 3)")]
    pub context_lines: Option<u32>,
}

/// Request for reindex tool
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReindexRequest {
    #[schemars(
        description = "Optional: specific files to reindex. If empty, reindexes all changed files."
    )]
    pub files: Option<Vec<String>>,
}

/// Request for call path tool
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PathRequest {
    #[schemars(description = "Starting symbol name")]
    pub from: String,
    #[schemars(description = "Target symbol name")]
    pub to: String,
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
}

/// Request for blame tool
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BlameRequest {
    #[schemars(description = "Symbol name to blame")]
    pub symbol: String,
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
}
