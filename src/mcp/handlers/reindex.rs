//! Reindexing handler

use crate::db::Database;
use crate::mcp::format::normalize_path;
use crate::mcp::types::ReindexRequest;
use crate::{index_codebase, IndexConfig};

pub fn handle_reindex(
    db: &mut Database,
    project_root: &str,
    req: &ReindexRequest,
) -> Result<String, String> {
    // If specific files requested, delete and reindex just those
    if let Some(files) = &req.files {
        if files.is_empty() {
            return Ok("No files specified. Provide file paths or omit the parameter to reindex all changed files.".to_string());
        }

        let mut errors = Vec::new();

        for file_path in files {
            // Normalize path
            let path = normalize_path(file_path);

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
        // Full incremental reindex
        let config = IndexConfig {
            root: project_root.to_string(),
            ..Default::default()
        };

        match index_codebase(db, &config) {
            Ok(stats) => {
                Ok(format!(
                    "## Reindex Complete\n\n**Files processed:** {}\n**Files skipped (unchanged):** {}\n**Symbols found:** {}\n**Edges created:** {}\n**References resolved:** {}\n**Errors:** {}\n",
                    stats.files, stats.skipped, stats.nodes, stats.edges, stats.resolved_refs, stats.errors
                ))
            }
            Err(e) => Err(format!("Reindex failed: {}", e)),
        }
    }
}
