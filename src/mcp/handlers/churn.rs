//! Git churn analysis: file change frequency over a recent window.

use std::collections::HashMap;

use crate::git::run_git;
use crate::mcp::types::ChurnRequest;
use crate::ops::constants::{effective_limit, MAX_CHURN_DAYS};
use crate::ops::{present, ChurnEntry, ChurnResult, Format, Page};
use crate::security::safe_join;

const DEFAULT_DAYS: u32 = 90;
const DEFAULT_LIMIT: u32 = 30;

/// Compute per-file change frequency (commits touching each file) over the
/// last `days` days, optionally scoped to `path`. Returns a map of
/// repo-relative file path → commit count. Reusable by coupling tools that
/// need the volatility dimension.
pub fn file_churn(
    project_root: &str,
    days: u32,
    path: Option<&str>,
) -> Result<HashMap<String, u32>, String> {
    // An unbounded window makes git walk the entire history and buffer the
    // whole name-only listing in memory, so cap what a caller can ask for.
    let days = days.clamp(1, MAX_CHURN_DAYS);
    let since = format!("--since={}.days.ago", days);
    let mut args: Vec<String> = vec![
        "log".into(),
        "--name-only".into(),
        "--pretty=format:".into(),
        since,
    ];
    if let Some(path) = path {
        // Validate before passing to git so callers can't pathspec-escape
        // into absolute paths or parent directories.
        safe_join(project_root, path).map_err(|e| e.to_string())?;
        // `:(literal)` disables pathspec magic, which would otherwise let a
        // leading `:/` re-anchor the spec at the top of the worktree — above
        // `project_root` when the project is a subdirectory of a larger repo.
        args.push("--".into());
        args.push(format!(":(literal){}", path));
    }

    let output = run_git(std::path::Path::new(project_root), &args)?;

    if !output.status.success() {
        return Err(format!("git log failed: {}", output.stderr_message()));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut counts: HashMap<String, u32> = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        *counts.entry(line.to_string()).or_insert(0) += 1;
    }
    Ok(counts)
}

pub fn handle_churn(project_root: &str, req: &ChurnRequest) -> Result<String, String> {
    // Clamp here too, so the window the output names is the one git was asked for.
    let days = req.days.unwrap_or(DEFAULT_DAYS).clamp(1, MAX_CHURN_DAYS);
    let counts = file_churn(project_root, days, req.path.as_deref())?;
    let total = counts.len();

    let mut ranked: Vec<(String, u32)> = counts.into_iter().collect();
    // Most-changed first, then by path so equal counts order predictably.
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let limit = effective_limit(req.limit, DEFAULT_LIMIT);
    ranked.truncate(limit as usize);

    let result = ChurnResult {
        days,
        path: req.path.clone(),
        page: Page::new(total, ranked.len(), limit, 0),
        files: ranked
            .into_iter()
            .map(|(file, commits)| ChurnEntry { file, commits })
            .collect(),
    };
    present(&result, Format::from_request(&req.format))
}
