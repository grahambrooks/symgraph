//! File listing handler

use crate::db::Database;
use crate::mcp::types::FileRequest;
use crate::ops::{self, present, Format};

pub fn handle_file(db: &Database, req: &FileRequest) -> Result<String, String> {
    present(
        &ops::file_symbols(db, &req.path)?,
        Format::from_request(&req.format),
    )
}
