//! Symbol search handler

use crate::db::Database;
use crate::mcp::types::SearchRequest;
use crate::ops::constants::{effective_limit, DEFAULT_SEARCH_LIMIT};
use crate::ops::format::format_node_with_signature;

pub fn handle_search(db: &Database, req: &SearchRequest) -> Result<String, String> {
    let semantic = req.semantic.unwrap_or(false);
    // Ask for one more than we will show: if it comes back, there are more
    // matches than the page, and saying so beats implying the list is complete.
    let limit = effective_limit(req.limit, DEFAULT_SEARCH_LIMIT);
    let mut results = if semantic {
        db.semantic_search(&req.query, limit + 1)
            .map_err(|e| e.to_string())?
    } else {
        db.search_nodes(&req.query, None, limit + 1)
            .map_err(|e| e.to_string())?
    };
    let truncated = results.len() > limit as usize;
    results.truncate(limit as usize);

    if results.is_empty() {
        return Ok(format!("No symbols found matching '{}'", req.query));
    }

    let mode = if semantic { "semantic " } else { "" };
    let mut output = format!(
        "Found {} symbols ({}match) for '{}':\n\n",
        results.len(),
        mode,
        req.query
    );

    for node in results {
        output.push_str(&format_node_with_signature(&node));
    }

    if truncated {
        output.push_str(&format!(
            "\n> **Truncated:** more than {} symbols match. Raise `limit` or narrow the query.\n",
            limit
        ));
    }

    Ok(output)
}
