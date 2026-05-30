//! God-struct / hub report.
//!
//! Ranks structs/classes by architectural debt: public-field count ×
//! inbound-reference count × churn. The widest, most-referenced, most-volatile
//! types float to the top — the first place a coupling reviewer should look.

use std::collections::HashSet;

use serde::Serialize;

use crate::db::Database;
use crate::mcp::handlers::churn::file_churn;
use crate::mcp::types::{wants_json, GodStructRequest};
use crate::types::{EdgeKind, NodeKind, Visibility};

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
        file_churn(project_root, req.days.unwrap_or(DEFAULT_DAYS), None).ok()
    } else {
        None
    };

    let mut structs = db
        .get_nodes_by_kind(NodeKind::Struct)
        .map_err(|e| e.to_string())?;
    structs.extend(
        db.get_nodes_by_kind(NodeKind::Class)
            .map_err(|e| e.to_string())?,
    );

    let mut ranked: Vec<GodStruct> = Vec::new();
    for s in &structs {
        let fields = db.get_struct_fields(&s.name).map_err(|e| e.to_string())?;
        let pub_fields = fields
            .iter()
            .filter(|f| f.visibility == Visibility::Public)
            .count();

        // Inbound coupling = distinct source files with a non-structural edge
        // into the struct itself or any of its fields.
        let mut files: HashSet<String> = HashSet::new();
        for node in std::iter::once(s).chain(fields.iter()) {
            let incoming = db.get_incoming_edges(node.id).map_err(|e| e.to_string())?;
            for e in incoming {
                if e.kind == EdgeKind::Contains {
                    continue;
                }
                if let Some(fp) = e.file_path {
                    files.insert(fp);
                }
            }
        }
        let inbound_refs = files.len();
        let churn_n = churn
            .as_ref()
            .and_then(|c| c.get(&s.file_path).copied())
            .unwrap_or(0);

        // Score multiplies the dimensions, treating absent churn as neutral (1)
        // so structs aren't all zeroed when churn isn't requested.
        let vol = churn_n.max(1) as u64;
        let score = pub_fields.max(1) as u64 * inbound_refs.max(1) as u64 * vol;

        ranked.push(GodStruct {
            name: s.name.clone(),
            file: s.file_path.clone(),
            pub_fields,
            total_fields: fields.len(),
            inbound_refs,
            churn: churn_n,
            score,
        });
    }

    ranked.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.name.cmp(&b.name)));
    let limit = req.limit.unwrap_or(DEFAULT_LIMIT as u32) as usize;
    ranked.truncate(limit);

    if wants_json(&req.format) {
        return serde_json::to_string_pretty(&ranked).map_err(|e| e.to_string());
    }

    let mut out = String::from(
        "# God-struct / hub report\n\nscore = pub_fields × inbound_refs × churn (each floored at 1).\n\n",
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
