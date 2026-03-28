//! Status handler

use crate::db::Database;

pub fn handle_status(db: &Database) -> Result<String, String> {
    let stats = db.get_stats().map_err(|e| e.to_string())?;

    let mut output = String::from("## codemap Index Status\n\n");

    output.push_str(&format!("**Total Files:** {}\n", stats.total_files));
    output.push_str(&format!("**Total Symbols:** {}\n", stats.total_nodes));
    output.push_str(&format!("**Total Relationships:** {}\n", stats.total_edges));
    output.push_str(&format!(
        "**Database Size:** {:.2} KB\n",
        stats.db_size_bytes as f64 / 1024.0
    ));

    if !stats.languages.is_empty() {
        output.push_str("\n**Languages:**\n");
        for (lang, count) in &stats.languages {
            output.push_str(&format!("- {}: {} symbols\n", lang.as_str(), count));
        }
    }

    if !stats.node_kinds.is_empty() {
        output.push_str("\n**Symbol Types:**\n");
        for (kind, count) in &stats.node_kinds {
            output.push_str(&format!("- {}: {}\n", kind.as_str(), count));
        }
    }

    Ok(output)
}
