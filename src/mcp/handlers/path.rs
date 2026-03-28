//! Handler for call path tool

use crate::db::Database;
use crate::mcp::types::PathRequest;

pub fn handle_path(db: &Database, req: &PathRequest) -> Result<String, String> {
    match db.find_call_path(&req.from, &req.to) {
        Ok(paths) => {
            if paths.is_empty() {
                Ok(format!(
                    "No call path found from '{}' to '{}'",
                    req.from, req.to
                ))
            } else {
                let mut output = format!("# Call Paths from '{}' to '{}'\n\n", req.from, req.to);
                output.push_str(&format!("Found {} path(s):\n\n", paths.len()));

                for (i, path) in paths.iter().enumerate() {
                    output.push_str(&format!("## Path {}\n\n", i + 1));
                    for (j, node) in path.iter().enumerate() {
                        if j > 0 {
                            output.push_str("  ↓ calls\n");
                        }
                        output.push_str(&format!(
                            "{}. {} ({}:{})\n",
                            j + 1,
                            node.name,
                            node.file_path,
                            node.start_line
                        ));
                    }
                    output.push('\n');
                }
                Ok(output)
            }
        }
        Err(e) => Err(e.to_string()),
    }
}
