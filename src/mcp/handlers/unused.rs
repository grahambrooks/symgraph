//! Handler for unused symbols tool

use crate::db::Database;
use crate::mcp::types::UnusedRequest;
use crate::ops::{self, present, Format};

pub fn handle_unused(db: &Database, req: &UnusedRequest) -> Result<String, String> {
    present(
        &ops::unused(
            db,
            req.limit,
            req.offset,
            req.ignore_test_callers.unwrap_or(false),
        )?,
        Format::from_request(&req.format),
    )
}
