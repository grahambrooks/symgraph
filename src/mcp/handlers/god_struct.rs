//! God-struct / hub report.
//!
//! Ranks structs/classes by architectural debt: public-field count ×
//! inbound-reference count × churn. The widest, most-referenced, most-volatile
//! types float to the top — the first place a coupling reviewer should look.

use serde::Serialize;

use crate::db::Database;
use crate::mcp::handlers::churn::file_churn;
use crate::mcp::types::{wants_json, GodStructRequest};

const DEFAULT_DAYS: u32 = 90;
const DEFAULT_LIMIT: usize = 20;

#[derive(Debug, Serialize)]
struct GodStruct {
    name: String,
    file: String,
    pub_fields: usize,
    total_fields: usize,
    inbound_refs: usize,
    churn: u32,
    score: u64,
}

pub fn handle_god_struct(
    db: &Database,
    project_root: &str,
    req: &GodStructRequest,
) -> Result<String, String> {
    let churn = if req.churn.unwrap_or(false) {
        file_churn(project_root, req.days.unwrap_or(DEFAULT_DAYS), None)
            .inspect_err(
                |e| tracing::warn!(error = %e, "churn unavailable; debt score omits volatility"),
            )
            .ok()
    } else {
        None
    };

    // Test code is out of scope by default, on both sides: a fixture struct
    // is not architectural debt, and a reference from a test is not a module
    // depending on this type.
    let include_tests = req.include_tests.unwrap_or(false);

    // One query for the whole report. This used to be a loop over every
    // struct issuing a field lookup plus an incoming-edge query per struct
    // and per field — 98 seconds on a 4,000-file index, and wrong besides,
    // because the field lookup matched on the struct's name.
    let rows = db
        .god_struct_rows(include_tests)
        .map_err(|e| e.to_string())?;

    let mut ranked: Vec<GodStruct> = rows
        .into_iter()
        .map(|r| {
            let churn_n = churn
                .as_ref()
                .and_then(|c| c.get(&r.file_path).copied())
                .unwrap_or(0);

            // Score multiplies the dimensions, treating absent churn as
            // neutral (1) so structs aren't all zeroed when churn isn't
            // requested.
            let vol = churn_n.max(1) as u64;
            let score = r.pub_fields.max(1) as u64 * r.inbound_files.max(1) as u64 * vol;

            GodStruct {
                name: r.name,
                file: r.file_path,
                pub_fields: r.pub_fields,
                total_fields: r.total_fields,
                inbound_refs: r.inbound_files,
                churn: churn_n,
                score,
            }
        })
        .collect();

    ranked.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.name.cmp(&b.name)));
    let limit = req.limit.unwrap_or(DEFAULT_LIMIT as u32) as usize;
    ranked.truncate(limit);

    if wants_json(&req.format) {
        return serde_json::to_string_pretty(&ranked).map_err(|e| e.to_string());
    }

    let scope = if include_tests {
        "Scope: all code, tests included.\n\n"
    } else {
        "Scope: production code. Test code is excluded — pass include_tests to count it.\n\n"
    };
    let mut out = format!(
        "# God-struct / hub report\n\nscore = pub_fields × inbound_refs × churn (each floored at 1).\n\n{scope}",
    );
    out.push_str("| Score | Struct | Pub fields | Fields | Inbound files | Churn | File |\n");
    out.push_str("|---:|---|---:|---:|---:|---:|---|\n");
    for s in &ranked {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            s.score, s.name, s.pub_fields, s.total_fields, s.inbound_refs, s.churn, s.file
        ));
    }
    Ok(out)
}
