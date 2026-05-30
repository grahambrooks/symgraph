//! Handler for implementations tool

use crate::db::Database;
use crate::mcp::types::SymbolRequest;
use crate::ops::{self, present, Format};

pub fn handle_implementations(db: &Database, req: &SymbolRequest) -> Result<String, String> {
    present(
        &ops::implementations(db, &req.symbol)?,
        Format::from_request(&req.format),
    )
}
