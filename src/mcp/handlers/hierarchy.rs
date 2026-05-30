//! Handler for hierarchy tool

use crate::db::Database;
use crate::mcp::types::SymbolRequest;
use crate::ops::{self, present, Format};

pub fn handle_hierarchy(db: &Database, req: &SymbolRequest) -> Result<String, String> {
    present(
        &ops::hierarchy(db, &req.symbol)?,
        Format::from_request(&req.format),
    )
}
