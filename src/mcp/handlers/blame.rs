//! Git blame for a symbol's definition.

use crate::db::Database;
use crate::git::run_git;
use crate::mcp::types::BlameRequest;
use crate::ops::{self, present, Format, NotFound};
use crate::security::safe_join;

pub fn handle_blame(
    db: &Database,
    project_root: &str,
    req: &BlameRequest,
) -> Result<String, String> {
    let fmt = Format::from_request(&req.format);
    let matched = match db.resolve_symbol(&req.symbol, &req.hint()) {
        Ok(Some(m)) => m,
        Ok(None) => return present(&NotFound::new(&req.symbol), fmt),
        Err(e) => return Err(e.to_string()),
    };
    let resolution = ops::Resolution::of(&matched);
    let node = matched.node;

    // Validate the file path before passing it to `git blame`, even though
    // it came from the DB — a malicious indexer input could seed attacker
    // paths into the DB and use this tool as a shell-out primitive.
    safe_join(project_root, &node.file_path).map_err(|e| e.to_string())?;

    let range = format!("{},{}", node.start_line, node.end_line);
    let output = run_git(
        std::path::Path::new(project_root),
        ["blame", "-L", &range, "--date=short", "--", &node.file_path],
    )?;

    if !output.status.success() {
        return Err(format!("git blame failed: {}", output.stderr_message()));
    }

    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    let result = ops::blame_result(&node, resolution, raw);
    present(&result, fmt)
}
