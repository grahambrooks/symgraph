//! Handler for unused symbols tool

use crate::db::Database;
use crate::mcp::types::FormatRequest;
use crate::ops::{self, present, Format};

pub fn handle_unused(db: &Database, req: &FormatRequest) -> Result<String, String> {
    present(
        &ops::unused(db, req.limit, req.offset)?,
        Format::from_request(&req.format),
    )
}
