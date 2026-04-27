//! Reindexing handler

use crate::cli::rebuild_project_database;
use crate::db::Database;
use crate::mcp::format::normalize_path;
use crate::mcp::types::ReindexRequest;
use crate::security::validate_relative;
use crate::{index_codebase, IndexConfig};

pub fn handle_reindex(
    db: &mut Database,
    project_root: &str,
    req: &ReindexRequest,
) -> Result<String, String> {
    // If specific files requested, delete and reindex just those
    if let Some(files) = &req.files {
        if files.is_empty() {
            return Ok(
                "No files specified. Provide file paths or omit the parameter to rebuild the full index."
                    .to_string(),
            );
        }

        let mut errors = Vec::new();

        for file_path in files {
            // Normalize and validate — reindex is a rare write path, so we
            // fail loudly on traversal attempts rather than silently skip.
            let path = match validate_relative(normalize_path(file_path)) {
                Ok(p) => p,
                Err(e) => {
                    errors.push(format!("{}: {}", file_path, e));
                    continue;
                }
            };

            // Delete existing data for this file
            if let Err(e) = db.delete_file(path) {
                errors.push(format!("{}: {}", path, e));
            }
        }

        // Now run reindex to pick up the deleted files, but skip global
        // reference resolution — we'll do scoped resolution instead.
        let config = IndexConfig {
            root: project_root.to_string(),
            skip_resolve: true,
            ..Default::default()
        };

        match index_codebase(db, &config) {
            Ok(mut stats) => {
                // Scoped resolution: only resolve refs from the reindexed files
                let normalized_files: Vec<String> = files
                    .iter()
                    .map(|f| normalize_path(f).to_string())
                    .collect();
                match db.resolve_references_for_files(&normalized_files) {
                    Ok(resolved) => stats.resolved_refs = resolved as u64,
                    Err(e) => errors.push(format!("resolve refs: {}", e)),
                }

                let mut output = format!(
                    "## Reindex Complete\n\n**Files reindexed:** {}\n**Symbols found:** {}\n**Edges created:** {}\n**References resolved:** {}\n",
                    stats.files, stats.nodes, stats.edges, stats.resolved_refs
                );
                if !errors.is_empty() {
                    output.push_str(&format!("\n**Errors:** {}\n", errors.join(", ")));
                }
                Ok(output)
            }
            Err(e) => Err(format!("Reindex failed: {}", e)),
        }
    } else {
        // Full shadow rebuild
        let config = IndexConfig {
            root: project_root.to_string(),
            ..Default::default()
        };

        match rebuild_project_database(db, &config) {
            Ok(stats) => Ok(format!(
                "## Reindex Complete\n\n**Mode:** full rebuild\n**Files indexed:** {}\n**Symbols found:** {}\n**Edges created:** {}\n**References resolved:** {}\n**Errors:** {}\n",
                stats.files, stats.nodes, stats.edges, stats.resolved_refs, stats.errors
            )),
            Err(e) => Err(format!("Reindex failed: {}", e)),
        }
    }
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
        let output =
            handle_reindex(&mut db, &project_root, &ReindexRequest { files: None }).unwrap();

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
            },
        )
        .unwrap();

        assert!(output.contains("**Files reindexed:** 1"));
        assert!(db.find_node_by_name("old_a").unwrap().is_none());
        assert!(db.find_node_by_name("new_a").unwrap().is_some());
        assert!(db.find_node_by_name("stable_b").unwrap().is_some());
    }
}
