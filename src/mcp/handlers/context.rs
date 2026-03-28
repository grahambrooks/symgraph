//! Context building handler

use crate::context::{format_context_markdown, ContextBuilder, ContextOptions};
use crate::db::Database;
use crate::mcp::constants::DEFAULT_CONTEXT_MAX_NODES;
use crate::mcp::types::ContextRequest;

pub fn handle_context(
    db: &Database,
    project_root: &str,
    req: &ContextRequest,
) -> Result<String, String> {
    let builder = ContextBuilder::new(db, project_root.to_string());
    let options = ContextOptions {
        max_nodes: DEFAULT_CONTEXT_MAX_NODES,
        include_code: true,
        ..Default::default()
    };

    match builder.build_context(&req.task, &options) {
        Ok(context) => Ok(format_context_markdown(&context)),
        Err(e) => Err(e.to_string()),
    }
}
