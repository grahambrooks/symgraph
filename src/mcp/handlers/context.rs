//! Context building handler

use crate::db::Database;
use crate::mcp::types::ContextRequest;
use crate::ops::constants::{effective_limit, DEFAULT_CONTEXT_MAX_NODES};
use crate::ops::{self, present, Format};

pub fn handle_context(
    db: &Database,
    project_root: &str,
    req: &ContextRequest,
) -> Result<String, String> {
    let max_nodes = effective_limit(req.limit, DEFAULT_CONTEXT_MAX_NODES);
    let result = ops::context(db, project_root, &req.task, max_nodes)?;
    present(&result, Format::from_request(&req.format))
}
