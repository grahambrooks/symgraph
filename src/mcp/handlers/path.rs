//! Handler for call path tool

use crate::db::Database;
use crate::mcp::types::PathRequest;
use crate::ops::{self, present, Format};

pub fn handle_path(db: &Database, req: &PathRequest) -> Result<String, String> {
    present(
        &ops::call_paths(db, &req.from, &req.to)?,
        Format::from_request(&req.format),
    )
}
