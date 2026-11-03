//! Handler for hierarchy tool

use crate::db::Database;
use crate::mcp::types::SymbolRequest;
use crate::ops::{self, Format, present};

pub fn handle_hierarchy(db: &Database, req: &SymbolRequest) -> Result<String, String> {
    present(
        &ops::hierarchy(db, &req.symbol, &req.hint())?,
        Format::from_request(&req.format),
    )
}
