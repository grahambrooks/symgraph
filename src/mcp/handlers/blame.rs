//! Git blame for a symbol's definition.

use std::process::Command;

use crate::db::Database;
use crate::mcp::types::BlameRequest;
use crate::security::safe_join;

pub fn handle_blame(
    db: &Database,
    project_root: &str,
    req: &BlameRequest,
) -> Result<String, String> {
    let node = match db.find_node_by_name(&req.symbol) {
        Ok(Some(n)) => n,
        Ok(None) => return Ok(format!("Symbol '{}' not found", req.symbol)),
        Err(e) => return Err(e.to_string()),
    };

    // Validate the file path before passing it to `git blame`, even though
    // it came from the DB — a malicious indexer input could seed attacker
    // paths into the DB and use this tool as a shell-out primitive.
    safe_join(project_root, &node.file_path).map_err(|e| e.to_string())?;

    let range = format!("{},{}", node.start_line, node.end_line);
    let output = Command::new("git")
        .args(["blame", "-L", &range, "--date=short", "--", &node.file_path])
        .current_dir(project_root)
        .output()
        .map_err(|e| format!("running git blame: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "git blame failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let body = String::from_utf8_lossy(&output.stdout);
    Ok(format!(
        "## blame: `{}` ({}:{}-{})\n\n```\n{}```\n",
        node.name, node.file_path, node.start_line, node.end_line, body
    ))
}
