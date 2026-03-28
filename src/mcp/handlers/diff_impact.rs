//! Handler for diff impact tool

use crate::db::Database;
use crate::mcp::format;
use crate::mcp::types::DiffImpactRequest;

pub fn handle_diff_impact(db: &Database, req: &DiffImpactRequest) -> Result<String, String> {
    match db.get_diff_impact(&req.file_path, req.start_line, req.end_line) {
        Ok(nodes) => {
            if nodes.is_empty() {
                Ok(format!(
                    "No symbols affected by changes to {}:{}—{}",
                    req.file_path, req.start_line, req.end_line
                ))
            } else {
                let mut output = format!(
                    "# Impact Analysis: {}:{}—{}\n\n",
                    req.file_path, req.start_line, req.end_line
                );
                output.push_str(&format!(
                    "Potentially affected: {} symbol(s)\n\n",
                    nodes.len()
                ));

                // Separate direct hits from indirect callers
                let mut direct = Vec::new();
                let mut indirect = Vec::new();

                for node in nodes {
                    if node.file_path == req.file_path
                        && node.start_line <= req.end_line
                        && node.end_line >= req.start_line
                    {
                        direct.push(node);
                    } else {
                        indirect.push(node);
                    }
                }

                if !direct.is_empty() {
                    output.push_str("## Directly Modified\n\n");
                    for node in direct {
                        output.push_str(&format::format_node(&node));
                        output.push_str("\n\n");
                    }
                }

                if !indirect.is_empty() {
                    output.push_str("## Indirect Impact (Callers)\n\n");
                    for node in indirect {
                        output.push_str(&format::format_node(&node));
                        output.push_str("\n\n");
                    }
                }

                Ok(output)
            }
        }
        Err(e) => Err(e.to_string()),
    }
}
