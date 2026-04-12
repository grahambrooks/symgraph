//! Database path and initialization utilities

use anyhow::{Context, Result};
use std::path::PathBuf;

use crate::db::Database;

const DB_DIR: &str = ".symgraph";
const DB_FILE: &str = "index.db";

/// Get the database path for a project root
pub fn database_path(project_root: &str) -> PathBuf {
    PathBuf::from(project_root).join(DB_DIR).join(DB_FILE)
}

/// Ensure the database directory exists
pub fn ensure_database_directory(project_root: &str) -> Result<()> {
    let dir = PathBuf::from(project_root).join(DB_DIR);
    std::fs::create_dir_all(&dir)?;
    Ok(())
}

/// Open or initialize database for a project
pub fn open_project_database(project_root: &str) -> Result<Database> {
    ensure_database_directory(project_root)?;
    let db_path = database_path(project_root);
    Database::open(&db_path)
}

/// Canonicalize and validate a path
pub fn canonicalize_path(path: &str) -> Result<String> {
    let canonical = std::path::Path::new(path)
        .canonicalize()
        .context("Invalid path")?;
    Ok(canonical.display().to_string())
}
