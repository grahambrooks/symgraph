//! Handler for diff impact tool

use crate::db::Database;
use crate::git::run_git;
use crate::mcp::types::DiffImpactRequest;
use crate::ops::{present, DiffImpactResult, Format, RegionImpact};
use crate::security::safe_join;

pub fn handle_diff_impact(
    db: &Database,
    project_root: &str,
    req: &DiffImpactRequest,
) -> Result<String, String> {
    let fmt = Format::from_request(&req.format);

    let result = match req.git_ref.as_deref() {
        Some(git_ref) => git_ref_impact(db, project_root, git_ref)?,
        None => explicit_region_impact(db, project_root, req)?,
    };
    present(&result, fmt)
}

/// Impact of the caller-supplied file and line range.
fn explicit_region_impact(
    db: &Database,
    project_root: &str,
    req: &DiffImpactRequest,
) -> Result<DiffImpactResult, String> {
    let file_path = req
        .file_path
        .as_deref()
        .ok_or("file_path is required when git_ref is not set")?;
    let start_line = req
        .start_line
        .ok_or("start_line is required when git_ref is not set")?;
    let end_line = req
        .end_line
        .ok_or("end_line is required when git_ref is not set")?;

    safe_join(project_root, file_path).map_err(|e| e.to_string())?;

    let nodes = db
        .get_diff_impact(file_path, start_line, end_line)
        .map_err(|e| e.to_string())?;
    Ok(DiffImpactResult {
        git_ref: None,
        regions: vec![RegionImpact::new(file_path, start_line, end_line, nodes)],
    })
}

/// Impact of every region git reports as changed against `git_ref`.
fn git_ref_impact(
    db: &Database,
    project_root: &str,
    git_ref: &str,
) -> Result<DiffImpactResult, String> {
    validate_git_ref(git_ref)?;
    let mut regions = Vec::new();
    for (file, start, end) in git_changed_regions(project_root, git_ref)? {
        let nodes = db
            .get_diff_impact(&file, start, end)
            .map_err(|e| e.to_string())?;
        regions.push(RegionImpact::new(&file, start, end, nodes));
    }
    Ok(DiffImpactResult {
        git_ref: Some(git_ref.to_string()),
        regions,
    })
}

/// Conservative allow-list for git refs coming from MCP callers.
///
/// Rejects leading `-` (would look like a flag to `git diff`) and any
/// character outside the usual ref/rev-spec alphabet. This is intentionally
/// stricter than `git check-ref-format` — we'd rather bounce a valid-but-
/// weird ref than risk argument injection.
fn validate_git_ref(git_ref: &str) -> Result<(), String> {
    if git_ref.is_empty() {
        return Err("git_ref must not be empty".to_string());
    }
    if git_ref.starts_with('-') {
        return Err("git_ref must not start with '-'".to_string());
    }
    for ch in git_ref.chars() {
        let ok = ch.is_ascii_alphanumeric()
            || matches!(ch, '_' | '-' | '.' | '/' | '~' | '^' | ':' | '@');
        if !ok {
            return Err(format!("git_ref contains unsupported character: {:?}", ch));
        }
    }
    Ok(())
}

/// Run `git diff --unified=0 <ref>` and parse the output into per-file
/// (path, start_line, end_line) regions covering the post-image hunks.
fn git_changed_regions(
    project_root: &str,
    git_ref: &str,
) -> Result<Vec<(String, u32, u32)>, String> {
    // `--` separates revs from paths; we pass no paths, but explicitly
    // closing the rev-spec section still hardens against future callers.
    let output = run_git(
        std::path::Path::new(project_root),
        ["diff", "--unified=0", git_ref, "--"],
    )?;

    if !output.status.success() {
        return Err(format!("git diff failed: {}", output.stderr_message()));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut regions = Vec::new();
    let mut current_file: Option<String> = None;

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("+++ b/") {
            current_file = Some(rest.to_string());
            continue;
        }
        if line.starts_with("+++ /dev/null") {
            current_file = None;
            continue;
        }
        if let Some(hunk) = line.strip_prefix("@@") {
            // Format: @@ -<old> +<new_start>[,<new_count>] @@
            if let Some(plus_idx) = hunk.find('+') {
                let after = &hunk[plus_idx + 1..];
                let end_idx = after.find(' ').unwrap_or(after.len());
                let spec = &after[..end_idx];
                let mut parts = spec.split(',');
                let start: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                let count: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);
                if start > 0 {
                    if let Some(file) = current_file.clone() {
                        let end = if count == 0 { start } else { start + count - 1 };
                        regions.push((file, start, end));
                    }
                }
            }
        }
    }

    Ok(regions)
}
