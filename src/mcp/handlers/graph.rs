//! Graph traversal handlers (callers, callees, impact)

use crate::db::Database;
use crate::graph::Graph;
use crate::mcp::handlers::churn::file_churn;
use crate::mcp::types::{wants_json, ImpactRequest, SymbolRequest};
use crate::ops::constants::DEFAULT_IMPACT_DEPTH;
use crate::ops::{self, present, Format};

pub fn handle_callers(db: &Database, req: &SymbolRequest) -> Result<String, String> {
    present(
        &ops::callers(db, &req.symbol)?,
        Format::from_request(&req.format),
    )
}

pub fn handle_callees(db: &Database, req: &SymbolRequest) -> Result<String, String> {
    present(
        &ops::callees(db, &req.symbol)?,
        Format::from_request(&req.format),
    )
}

pub fn handle_impact(
    db: &Database,
    project_root: &str,
    req: &ImpactRequest,
) -> Result<String, String> {
    let graph = Graph::new(db);

    let churn = if req.churn.unwrap_or(false) {
        file_churn(project_root, req.days.unwrap_or(90), None).ok()
    } else {
        None
    };

    let breakdown = graph
        .impact_breakdown(&req.symbol, churn.as_ref())
        .map_err(|e| e.to_string())?;
    let Some(breakdown) = breakdown else {
        return Ok(format!("Symbol '{}' not found", req.symbol));
    };

    if wants_json(&req.format) {
        return serde_json::to_string_pretty(&breakdown).map_err(|e| e.to_string());
    }

    let analysis = graph
        .analyze_impact(&req.symbol, DEFAULT_IMPACT_DEPTH)
        .map_err(|e| e.to_string())?;
    let root = match analysis.root {
        Some(r) => r,
        None => return Ok(format!("Symbol '{}' not found", req.symbol)),
    };

    let mut output = format!(
        "## Impact Analysis for `{}`\n\n**Location:** {}:{}-{}\n\n",
        root.name, root.file_path, root.start_line, root.end_line
    );

    // Coupling breakdown: how dependents couple (contract / model / intrusive).
    output.push_str(&format!(
        "**Inbound coupling:** {} edges from {} module(s)\n\n",
        breakdown.total_inbound, breakdown.inbound_modules
    ));
    if !breakdown.by_kind.is_empty() {
        output.push_str("| Coupling kind | Count |\n|---|---:|\n");
        for (label, n) in &breakdown.by_kind {
            output.push_str(&format!("| {} | {} |\n", label, n));
        }
        output.push('\n');
    }
    if !breakdown.modules.is_empty() {
        output.push_str("**Inbound modules:**\n\n");
        for m in breakdown.modules.iter().take(15) {
            let churn = m
                .churn
                .map(|c| format!(", churn {}", c))
                .unwrap_or_default();
            output.push_str(&format!("- `{}` — {} edges{}\n", m.module, m.edges, churn));
        }
        output.push('\n');
    }

    output.push_str(&format!(
        "**Call-graph impact (depth {}):** {} symbols affected\n\n",
        DEFAULT_IMPACT_DEPTH, analysis.total_impact
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
