//! Handler for unused symbols tool

use crate::db::Database;
use crate::ops::{self, present, Format};

pub fn handle_unused(db: &Database, format: &Option<String>) -> Result<String, String> {
    present(&ops::unused(db)?, Format::from_request(format))
}
