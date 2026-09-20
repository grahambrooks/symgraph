//! Reindexing handler

use crate::cli::rebuild_project_database;
use crate::db::Database;
use crate::mcp::types::ReindexRequest;
use crate::ops::format::normalize_path;
use crate::ops::{present, Format, ReindexResult};
use crate::security::validate_relative;
use crate::{index_codebase, IndexConfig};

pub fn handle_reindex(
    db: &mut Database,
    project_root: &str,
    req: &ReindexRequest,
) -> Result<String, String> {
    let fmt = Format::from_request(&req.format);
    let result = match &req.files {
        Some(files) if files.is_empty() => ReindexResult::message(
            "none",
            "No files specified. Provide file paths or omit the parameter to rebuild the full index.",
        ),
        Some(files) => reindex_files(db, project_root, files)?,
        None => {
            let config = IndexConfig {
                root: project_root.to_string(),
                ..Default::default()
            };
            let stats = rebuild_project_database(db, &config)
                .map_err(|e| format!("Reindex failed: {}", e))?;
            ReindexResult::from_stats("full rebuild", &stats)
        }
    };
    present(&result, fmt)
}

/// Delete and re-index the named files, then resolve only their references.
fn reindex_files(
    db: &mut Database,
    project_root: &str,
    files: &[String],
) -> Result<ReindexResult, String> {
    let mut warnings = Vec::new();
    let normalized: Vec<String> = files
        .iter()
        .map(|f| normalize_path(f).to_string())
        .collect();

    for file_path in files {
        // Normalize and validate — reindex is a rare write path, so we
        // fail loudly on traversal attempts rather than silently skip.
        let normalized = normalize_path(file_path);
        let path = match validate_relative(&normalized) {
            Ok(p) => p,
            Err(e) => {
                warnings.push(format!("{}: {}", file_path, e));
                continue;
            }
        };
        if let Err(e) = db.delete_file(path) {
            warnings.push(format!("{}: {}", path, e));
        }
    }

    // Re-index only the named files, skipping global reference resolution —
    // scoped resolution follows.
    let config = IndexConfig {
        root: project_root.to_string(),
        skip_resolve: true,
        only_files: Some(normalized.clone()),
        ..Default::default()
    };
    let mut stats = index_codebase(db, &config).map_err(|e| format!("Reindex failed: {}", e))?;

    match db.resolve_references_for_files(&normalized) {
        Ok(resolved) => stats.resolved_refs = resolved as u64,
        Err(e) => warnings.push(format!("resolve refs: {}", e)),
    }

    // Reclaim pages freed by the delete-and-reinsert above so the live index
    // file does not grow with each incremental reindex.
    if let Err(e) = db.compact() {
        warnings.push(format!("compact: {}", e));
    }

    let mut result = ReindexResult::from_stats("files", &stats);
    result.warnings = warnings;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    use crate::cli::{open_project_database, rebuild_project_database};

    fn write_file(path: &std::path::Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn test_handle_reindex_full_rebuild_reopens_live_handle() {
        let dir = tempdir().unwrap();
        let project_root = dir.path().display().to_string();
        let file_path = dir.path().join("src/lib.rs");
        write_file(&file_path, "pub fn old_symbol() {}\n");

        let mut db = open_project_database(&project_root).unwrap();
        let config = IndexConfig {
            root: project_root.clone(),
            ..Default::default()
        };
        rebuild_project_database(&mut db, &config).unwrap();
        assert!(db.find_node_by_name("old_symbol").unwrap().is_some());

        write_file(&file_path, "pub fn new_symbol() {}\n");
        let output = handle_reindex(
            &mut db,
            &project_root,
            &ReindexRequest {
                files: None,
                format: None,
            },
        )
        .unwrap();

        assert!(output.contains("**Mode:** full rebuild"));
        assert!(db.find_node_by_name("old_symbol").unwrap().is_none());
        assert!(db.find_node_by_name("new_symbol").unwrap().is_some());
    }

    #[test]
    fn test_handle_reindex_specific_files_stays_in_place() {
        let dir = tempdir().unwrap();
        let project_root = dir.path().display().to_string();
        let a_path = dir.path().join("src/a.rs");
        let b_path = dir.path().join("src/b.rs");
        write_file(&a_path, "pub fn old_a() {}\n");
        write_file(&b_path, "pub fn stable_b() {}\n");

        let mut db = open_project_database(&project_root).unwrap();
        let config = IndexConfig {
            root: project_root.clone(),
            ..Default::default()
        };
        rebuild_project_database(&mut db, &config).unwrap();

        write_file(&a_path, "pub fn new_a() {}\n");
        let output = handle_reindex(
            &mut db,
            &project_root,
            &ReindexRequest {
                files: Some(vec!["src/a.rs".to_string()]),
                format: None,
            },
        )
        .unwrap();

        assert!(
            output.contains("**Files indexed:** 1"),
            "output was:\n{output}"
        );
        assert!(db.find_node_by_name("old_a").unwrap().is_none());
        assert!(db.find_node_by_name("new_a").unwrap().is_some());
        assert!(db.find_node_by_name("stable_b").unwrap().is_some());
    }
}
