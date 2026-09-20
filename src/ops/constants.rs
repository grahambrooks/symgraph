//! Configuration constants for MCP tools

/// Default maximum number of search results
pub const DEFAULT_SEARCH_LIMIT: u32 = 10;

/// Default maximum number of callers/callees to return
pub const DEFAULT_GRAPH_LIMIT: u32 = 20;

/// Default maximum number of context nodes
pub const DEFAULT_CONTEXT_MAX_NODES: u32 = 20;

/// Default number of context lines before/after a definition
pub const DEFAULT_CONTEXT_LINES: u32 = 3;

/// Default impact analysis depth
pub const DEFAULT_IMPACT_DEPTH: u32 = 2;

/// Default number of references to show per category
pub const MAX_REFERENCES_PER_KIND: usize = 20;

/// Default page size for the unused-symbol listing. Unbounded, this routinely
/// runs to thousands of rows on a real codebase.
pub const DEFAULT_UNUSED_LIMIT: u32 = 100;

/// Hard ceiling on any caller-supplied `limit`. Stops one request from asking
/// for the whole table and materialising it in memory.
pub const MAX_LIMIT: u32 = 1000;

/// Apply the caller's `limit`, falling back to `default` and never exceeding
/// [`MAX_LIMIT`]. A `limit` of 0 means "use the default" rather than "no rows".
pub fn effective_limit(requested: Option<u32>, default: u32) -> u32 {
    match requested {
        Some(0) | None => default,
        Some(n) => n.min(MAX_LIMIT),
    }
}

/// Upper bound on a churn window, in days (~27 years). `days` arrives from the
/// wire as an unvalidated `u32`; without a cap, one request makes git walk the
/// entire history and buffer the whole listing in memory.
pub const MAX_CHURN_DAYS: u32 = 10_000;
