//! File listing handler

use crate::db::Database;
use crate::mcp::format::normalize_path;
use crate::mcp::types::FileRequest;

pub fn handle_file(db: &Database, req: &FileRequest) -> Result<String, String> {
    let path = normalize_path(&req.path);

    let nodes = db.get_nodes_by_file(path).map_err(|e| e.to_string())?;

    if nodes.is_empty() {
        return Ok(format!(
            "No symbols found in '{}'. File may not be indexed.",
            path
        ));
    }

    let mut output = format!("## Symbols in `{}`\n\n", path);
    output.push_str(&format!("Found {} symbols:\n\n", nodes.len()));

    // Group by kind for better readability
    let mut by_kind: std::collections::HashMap<String, Vec<_>> = std::collections::HashMap::new();
    for node in &nodes {
        by_kind
            .entry(node.kind.as_str().to_string())
            .or_default()
            .push(node);
    }

    // Sort kinds for consistent output
    let mut kinds: Vec<_> = by_kind.keys().cloned().collect();
    kinds.sort();

    for kind in kinds {
        let nodes = &by_kind[&kind];
        output.push_str(&format!("### {} ({}):\n\n", kind, nodes.len()));
        for node in nodes {
            output.push_str(&format!(
                "- `{}` (lines {}-{})",
                node.name, node.start_line, node.end_line
            ));
            if let Some(ref sig) = node.signature {
                output.push_str(&format!(" - `{}`", sig));
            }
            output.push('\n');
        }
        output.push('\n');
    }

    Ok(output)
}
