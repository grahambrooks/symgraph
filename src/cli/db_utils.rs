//! Database path and initialization utilities

use anyhow::{Context, Result};
use std::{
    path::{Path, PathBuf},
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::db::Database;
use crate::{build_full_index, IndexConfig, IndexingStats};

const DB_DIR: &str = ".symgraph";
const DB_FILE: &str = "index.db";
const SHADOW_DB_PREFIX: &str = "index.shadow";

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

/// Open a fresh shadow database in the project index directory.
pub fn open_shadow_database(project_root: &str) -> Result<Database> {
    let shadow_path = shadow_database_path(project_root)?;
    Database::cleanup_on_disk_path(&shadow_path)?;
    Database::open(&shadow_path)
}

/// Best-effort cleanup for a shadow database path and its SQLite sidecars.
pub fn cleanup_shadow_database_path(path: &Path) -> Result<()> {
    Database::cleanup_on_disk_path(path)
}

/// Best-effort cleanup for a shadow database handle and its SQLite sidecars.
pub fn cleanup_shadow_database(db: &Database) -> Result<()> {
    match db.path() {
        Some(path) => cleanup_shadow_database_path(path),
        None => Ok(()),
    }
}

/// Flush, close, and atomically swap a prepared shadow database into place.
pub fn swap_shadow_database(live_db: &mut Database, shadow_db: Database) -> Result<()> {
    let shadow_path = shadow_db.prepare_for_swap()?;
    live_db.replace_with_shadow(&shadow_path)
}

/// Build a full shadow index and atomically swap it into the live database handle.
pub fn rebuild_project_database(
    live_db: &mut Database,
    config: &IndexConfig,
) -> Result<IndexingStats> {
    let mut shadow_db = open_shadow_database(&config.root)?;
    let shadow_path = shadow_db
        .path()
        .map(Path::to_path_buf)
        .context("shadow database is not file-backed")?;

    match build_full_index(&mut shadow_db, config) {
        Ok(stats) => {
            if let Err(err) = swap_shadow_database(live_db, shadow_db) {
                let _ = cleanup_shadow_database_path(&shadow_path);
                Err(err)
            } else {
                Ok(stats)
            }
        }
        Err(err) => {
            let _ = cleanup_shadow_database_path(&shadow_path);
            Err(err)
        }
    }
}

fn shadow_database_path(project_root: &str) -> Result<PathBuf> {
    ensure_database_directory(project_root)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("System clock is before Unix epoch")?
        .as_nanos();
    Ok(PathBuf::from(project_root).join(DB_DIR).join(format!(
        "{SHADOW_DB_PREFIX}.{}.{}.db",
        process::id(),
        nonce
    )))
}

/// Canonicalize and validate a path
pub fn canonicalize_path(path: &str) -> Result<String> {
    let canonical = std::path::Path::new(path)
        .canonicalize()
        .context("Invalid path")?;
    Ok(canonical.display().to_string())
}
