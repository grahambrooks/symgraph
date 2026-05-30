//! Module-graph and coupling-score handlers.
//!
//! Both fold the resolved symbol graph to a chosen boundary. `module-graph`
//! reports the dependency structure (fan-in/out, cycles); `coupling-score`
//! ranks module-pair edges on strength × distance × volatility.

use crate::coupling::{build_module_graph, score_coupling, Granularity};
use crate::db::Database;
use crate::mcp::handlers::churn::file_churn;
use crate::mcp::types::{wants_json, ModuleGraphRequest};

const DEFAULT_DAYS: u32 = 90;
const DEFAULT_LIMIT: usize = 30;

/// Resolve churn best-effort: returns None (with no error) when churn wasn't
/// requested or git is unavailable, so analysis degrades gracefully.
fn maybe_churn(
    project_root: &str,
    want: bool,
    days: u32,
) -> Option<std::collections::HashMap<String, u32>> {
    if !want {
        return None;
    }
    file_churn(project_root, days, None).ok()
}

pub fn handle_module_graph(
    db: &Database,
    project_root: &str,
    req: &ModuleGraphRequest,
) -> Result<String, String> {
    let g = Granularity::parse(req.granularity.as_deref().unwrap_or("module"));
    let endpoints = db.get_edge_endpoints().map_err(|e| e.to_string())?;
    let churn = maybe_churn(
        project_root,
        req.churn.unwrap_or(false),
        req.days.unwrap_or(DEFAULT_DAYS),
    );
    let graph = build_module_graph(&endpoints, g, churn.as_ref());

    if wants_json(&req.format) {
        return serde_json::to_string_pretty(&graph).map_err(|e| e.to_string());
    }

    let limit = req.limit.unwrap_or(DEFAULT_LIMIT as u32) as usize;
    let mut out = format!(
        "# Module graph ({} granularity)\n\n{} nodes, {} edges, {} cycle(s).\n\n",
        graph.granularity,
        graph.nodes.len(),
        graph.edges.len(),
        graph.cycles.len()
    );

    out.push_str("## Hubs (by fan-in)\n\n");
    out.push_str("| Module | Fan-in | Fan-out | Churn |\n|---|---:|---:|---:|\n");
    for n in graph.nodes.iter().take(limit) {
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            n.id,
            n.fan_in,
            n.fan_out,
            n.churn.map(|c| c.to_string()).unwrap_or_else(|| "-".into())
        ));
    }

    if graph.cycles.is_empty() {
        out.push_str("\n## Cycles\n\nNone — the module graph is acyclic.\n");
    } else {
        out.push_str("\n## Cycles (SCCs)\n\n");
        for c in &graph.cycles {
            out.push_str(&format!("- {}\n", c.join(" → ")));
        }
    }

    out.push_str("\n## Top dependencies (by edge count)\n\n");
    out.push_str("| From | To | Edges | Strength | Kinds |\n|---|---|---:|---:|---|\n");
    for e in graph.edges.iter().take(limit) {
        let kinds = e
            .by_kind
            .iter()
            .map(|(k, n)| format!("{}:{}", k, n))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            e.from, e.to, e.count, e.strength, kinds
        ));
    }

    Ok(out)
}

pub fn handle_coupling_score(
    db: &Database,
    project_root: &str,
    req: &ModuleGraphRequest,
) -> Result<String, String> {
    let g = Granularity::parse(req.granularity.as_deref().unwrap_or("module"));
    let endpoints = db.get_edge_endpoints().map_err(|e| e.to_string())?;
    // Volatility is one of the three dimensions, so coupling-score uses churn
    // by default (best-effort: omitted if git is unavailable).
    let want_churn = req.churn.unwrap_or(true);
    let churn = maybe_churn(project_root, want_churn, req.days.unwrap_or(DEFAULT_DAYS));
    let graph = build_module_graph(&endpoints, g, churn.as_ref());
    let scores = score_coupling(&graph, g, churn.as_ref());

    if wants_json(&req.format) {
        return serde_json::to_string_pretty(&scores).map_err(|e| e.to_string());
    }

    let limit = req.limit.unwrap_or(DEFAULT_LIMIT as u32) as usize;
    let volatility_note = if churn.is_some() {
        "volatility = max churn of the two modules"
    } else {
        "volatility = 1 (churn not available; pass churn=true in a git repo)"
    };
    let mut out = format!(
        "# Coupling score ({} granularity)\n\nimpact = strength × distance × volatility ({}).\n\
        Strength: 1 contract (calls), 2 model (field reads / imports), 3 intrusive (field writes / &mut).\n\
        Note: edges are heuristic (name-based resolution), best for ranking.\n\n",
        graph.granularity, volatility_note
    );
    out.push_str("| Impact | From | To | Str | Dist | Vol | Edges | Kinds |\n");
    out.push_str("|---:|---|---|---:|---:|---:|---:|---|\n");
    for s in scores.iter().take(limit) {
        let kinds = s
            .by_kind
            .iter()
            .map(|(k, n)| format!("{}:{}", k, n))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            s.impact, s.from, s.to, s.strength, s.distance, s.volatility, s.edge_count, kinds
        ));
    }
    Ok(out)
}
