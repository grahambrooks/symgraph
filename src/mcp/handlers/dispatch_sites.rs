//! Dispatch-sites: where is an enum matched/switched on?
//!
//! Returns the files that dispatch on a member of the given enum (control
//! coupling). Useful to verify completeness before replacing scattered enum
//! dispatch with a trait/strategy.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::db::Database;
use crate::mcp::types::{wants_json, DispatchSitesRequest};

#[derive(Debug, Serialize)]
struct DispatchReport {
    enum_name: String,
    file_count: usize,
    sites: Vec<DispatchFile>,
}

#[derive(Debug, Serialize)]
struct DispatchFile {
    file: String,
    members: Vec<String>,
}

pub fn handle_dispatch_sites(db: &Database, req: &DispatchSitesRequest) -> Result<String, String> {
    let rows = db
        .get_dispatch_sites(&req.symbol)
        .map_err(|e| e.to_string())?;

    // Group members by file.
    let mut by_file: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (file, member) in rows {
        by_file.entry(file).or_default().push(member);
    }
    for members in by_file.values_mut() {
        members.sort();
        members.dedup();
    }

    let report = DispatchReport {
        enum_name: req.symbol.clone(),
        file_count: by_file.len(),
        sites: by_file
            .into_iter()
            .map(|(file, members)| DispatchFile { file, members })
            .collect(),
    };

    if wants_json(&req.format) {
        return serde_json::to_string_pretty(&report).map_err(|e| e.to_string());
    }

    if report.sites.is_empty() {
        return Ok(format!(
            "No dispatch sites found for enum `{}`. (Enum members are detected inside match/switch constructs; reindex if the codebase changed.)",
            report.enum_name
        ));
    }

    let mut out = format!(
        "# Dispatch sites for `{}`\n\n{} file(s) match on this enum's members.\n\n",
        report.enum_name, report.file_count
    );
    for s in &report.sites {
        out.push_str(&format!("- **{}** — {}\n", s.file, s.members.join(", ")));
    }
    Ok(out)
}
