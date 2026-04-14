//! Git churn analysis: file change frequency over a recent window.

use std::collections::HashMap;
use std::process::Command;

use crate::mcp::types::ChurnRequest;
use crate::security::safe_join;

const DEFAULT_DAYS: u32 = 90;
const DEFAULT_LIMIT: usize = 30;

pub fn handle_churn(project_root: &str, req: &ChurnRequest) -> Result<String, String> {
    let days = req.days.unwrap_or(DEFAULT_DAYS);
    let since = format!("--since={}.days.ago", days);

    let mut args: Vec<String> = vec![
        "log".into(),
        "--name-only".into(),
        "--pretty=format:".into(),
        since,
    ];
    if let Some(path) = req.path.as_deref() {
        // Validate before passing to git so callers can't pathspec-escape
        // into absolute paths or parent directories.
        safe_join(project_root, path).map_err(|e| e.to_string())?;
        args.push("--".into());
        args.push(path.into());
    }

    let output = Command::new("git")
        .args(&args)
        .current_dir(project_root)
        .output()
        .map_err(|e| format!("running git log: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "git log failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
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

    if counts.is_empty() {
        return Ok(format!(
            "No changes in the last {} days{}.",
            days,
            req.path
                .as_deref()
                .map(|p| format!(" under `{}`", p))
                .unwrap_or_default()
        ));
    }

    let mut ranked: Vec<(String, u32)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked.truncate(DEFAULT_LIMIT);

    let mut out = format!(
        "# Churn (last {} days{})\n\n",
        days,
        req.path
            .as_deref()
            .map(|p| format!(", path=`{}`", p))
            .unwrap_or_default()
    );
    out.push_str("| Commits | File |\n|---:|---|\n");
    for (path, n) in ranked {
        out.push_str(&format!("| {} | {} |\n", n, path));
    }
    Ok(out)
}
