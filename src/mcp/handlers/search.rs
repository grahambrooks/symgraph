//! Symbol search handler

use crate::db::Database;
use crate::mcp::types::SearchRequest;
use crate::ops::{self, present, Format};

pub fn handle_search(db: &Database, req: &SearchRequest) -> Result<String, String> {
    let result = ops::search(db, &req.query, req.semantic.unwrap_or(false), req.limit)?;
    present(&result, Format::from_request(&req.format))
}
