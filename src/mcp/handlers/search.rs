//! Symbol search handler

use crate::db::Database;
use crate::mcp::constants::DEFAULT_SEARCH_LIMIT;
use crate::mcp::format::format_node_with_signature;
use crate::mcp::types::SearchRequest;

pub fn handle_search(db: &Database, req: &SearchRequest) -> Result<String, String> {
    let semantic = req.semantic.unwrap_or(false);
    let results = if semantic {
        db.semantic_search(&req.query, DEFAULT_SEARCH_LIMIT)
            .map_err(|e| e.to_string())?
    } else {
        db.search_nodes(&req.query, None, DEFAULT_SEARCH_LIMIT)
            .map_err(|e| e.to_string())?
    };

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

    Ok(output)
}
