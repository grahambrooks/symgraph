//! Graph traversal handlers (callers, callees, impact)

use crate::db::Database;
use crate::graph::Graph;
use crate::mcp::constants::{DEFAULT_GRAPH_LIMIT, DEFAULT_IMPACT_DEPTH};
use crate::mcp::format::format_node_simple;
use crate::mcp::types::SymbolRequest;

pub fn handle_callers(db: &Database, req: &SymbolRequest) -> Result<String, String> {
    let graph = Graph::new(db);
    let callers = graph
        .find_callers(&req.symbol, DEFAULT_GRAPH_LIMIT)
        .map_err(|e| e.to_string())?;

    if callers.is_empty() {
        return Ok(format!("No callers found for '{}'", req.symbol));
    }

    let mut output = format!("Found {} callers of '{}':\n\n", callers.len(), req.symbol);

    for caller in callers {
        output.push_str(&format_node_simple(&caller));
        output.push('\n');
    }

    Ok(output)
}

pub fn handle_callees(db: &Database, req: &SymbolRequest) -> Result<String, String> {
    let graph = Graph::new(db);
    let callees = graph
        .find_callees(&req.symbol, DEFAULT_GRAPH_LIMIT)
        .map_err(|e| e.to_string())?;

    if callees.is_empty() {
        return Ok(format!("No callees found for '{}'", req.symbol));
    }

    let mut output = format!("'{}' calls {} functions:\n\n", req.symbol, callees.len());

    for callee in callees {
        output.push_str(&format_node_simple(&callee));
        output.push('\n');
    }

    Ok(output)
}

pub fn handle_impact(db: &Database, req: &SymbolRequest) -> Result<String, String> {
    let graph = Graph::new(db);
    let analysis = graph
        .analyze_impact(&req.symbol, DEFAULT_IMPACT_DEPTH)
        .map_err(|e| e.to_string())?;

    if analysis.root.is_none() {
        return Ok(format!("Symbol '{}' not found", req.symbol));
    }

    let root = analysis.root.unwrap();
    let mut output = format!(
        "## Impact Analysis for `{}`\n\n**Location:** {}:{}-{}\n\n",
        root.name, root.file_path, root.start_line, root.end_line
    );

    output.push_str(&format!(
        "**Total Impact:** {} symbols affected\n\n",
        analysis.total_impact
    ));

    if !analysis.direct_callers.is_empty() {
        output.push_str(&format!(
            "### Direct Callers ({}):\n\n",
            analysis.direct_callers.len()
        ));
        for caller in &analysis.direct_callers {
            output.push_str(&format!(
                "- `{}` ({}:{}) - {}\n",
                caller.name,
                caller.file_path,
                caller.start_line,
                caller.kind.as_str()
            ));
        }
    }

    if !analysis.indirect_callers.is_empty() {
        output.push_str(&format!(
            "\n### Indirect Callers ({}):\n\n",
            analysis.indirect_callers.len()
        ));
        for caller in analysis.indirect_callers.iter().take(20) {
            output.push_str(&format!(
                "- `{}` ({}:{}) - {}\n",
                caller.name,
                caller.file_path,
                caller.start_line,
                caller.kind.as_str()
            ));
        }
    }

    Ok(output)
}
