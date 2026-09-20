//! Status handler
//!
//! Reports both how much is in the index and how far it can be trusted. The
//! second half is the point: size tells you nothing about whether an answer
//! drawn from the index is sound.

use crate::db::{Database, IndexHealth};

/// Format a unix timestamp as an approximate age ("3 days ago").
fn age(built_at: i64) -> String {
    if built_at <= 0 {
        return "unknown".to_string();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let secs = now.saturating_sub(built_at);
    match secs {
        s if s < 90 => "just now".to_string(),
        s if s < 5400 => format!("{} minutes ago", s / 60),
        s if s < 172_800 => format!("{} hours ago", s / 3600),
        s => format!("{} days ago", s / 86_400),
    }
}

/// The trust section: staleness, provenance, ambiguity and FTS integrity.
fn render_health(health: &IndexHealth) -> String {
    let mut out = String::new();

    if let Some(stale) = &health.staleness {
        out.push_str(&format!(
            "\n> **Out of date:** {}. Results may reflect older extraction \
             semantics — run `symgraph reindex` or call symgraph-reindex.\n",
            stale
        ));
    }
    if !health.fts_ok {
        out.push_str(
            "\n> **Search index corrupt:** the full-text index disagrees with the \
             symbol table, so search results may be wrong. A full reindex rebuilds it.\n",
        );
    }

    out.push_str("\n**Index health:**\n");
    match &health.version {
        Some(v) => {
            out.push_str(&format!(
                "- Built: {} by symgraph {} (schema v{}, extractor v{})\n",
                age(v.built_at),
                if v.symgraph.is_empty() {
                    "unknown"
                } else {
                    &v.symgraph
                },
                v.schema,
                v.extractor
            ));
        }
        None => out.push_str("- Built: before symgraph recorded index provenance\n"),
    }
    out.push_str(&format!(
        "- Ambiguous names: {} of {} ({:.1}%) — these resolve by preference order, not certainty\n",
        health.ambiguous_names,
        health.distinct_names,
        health.ambiguous_percent()
    ));
    out.push_str(&format!(
        "- Unresolved references: {} (calls into code outside the index, plus extraction misses)\n",
        health.unresolved_refs
    ));
    out
}

/// Index size and trust, as one serialisable result.
#[derive(serde::Serialize)]
pub struct StatusResult {
    #[serde(flatten)]
    pub stats: crate::types::IndexStats,
    pub health: IndexHealth,
}

impl crate::ops::Render for StatusResult {
    fn to_markdown(&self) -> String {
        render_status(&self.stats, &self.health)
    }
}

pub fn handle_status(db: &Database, format: Option<String>) -> Result<String, String> {
    let stats = db.get_stats().map_err(|e| e.to_string())?;
    let health = db.health().map_err(|e| e.to_string())?;
    crate::ops::present(
        &StatusResult { stats, health },
        crate::ops::Format::from_request(&format),
    )
}

fn render_status(stats: &crate::types::IndexStats, health: &IndexHealth) -> String {
    let mut output = String::from("## symgraph Index Status\n\n");

    output.push_str(&format!("**Total Files:** {}\n", stats.total_files));
    output.push_str(&format!("**Total Symbols:** {}\n", stats.total_nodes));
    output.push_str(&format!("**Total Relationships:** {}\n", stats.total_edges));
    output.push_str(&format!(
        "**Database Size:** {:.2} KB\n",
        stats.db_size_bytes as f64 / 1024.0
    ));

    output.push_str(&render_health(health));

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

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FileRecord, Language, Node, NodeKind, Visibility};

    fn node(name: &str, file: &str) -> Node {
        Node {
            id: 0,
            kind: NodeKind::Function,
            name: name.to_string(),
            qualified_name: None,
            file_path: file.to_string(),
            start_line: 1,
            end_line: 2,
            start_column: 0,
            end_column: 0,
            signature: None,
            visibility: Visibility::Public,
            docstring: None,
            is_async: false,
            is_static: false,
            is_exported: false,
            is_test: false,
            is_generated: false,
            language: Language::Rust,
        }
    }

    fn seeded() -> Database {
        let db = Database::in_memory().unwrap();
        for path in ["src/a.rs", "src/b.rs"] {
            db.insert_or_update_file(&FileRecord {
                path: path.to_string(),
                content_hash: "h".into(),
                language: Language::Rust,
                size: 0,
                modified_at: 0,
                indexed_at: 0,
                node_count: 0,
            })
            .unwrap();
        }
        // `new` is defined twice, `solo` once.
        db.insert_node(&node("new", "src/a.rs")).unwrap();
        db.insert_node(&node("new", "src/b.rs")).unwrap();
        db.insert_node(&node("solo", "src/a.rs")).unwrap();
        db
    }

    #[test]
    fn health_counts_ambiguous_names() {
        let health = seeded().health().unwrap();
        assert_eq!(health.distinct_names, 2);
        assert_eq!(health.ambiguous_names, 1);
        assert!((health.ambiguous_percent() - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn health_reports_fts_intact_on_a_fresh_index() {
        assert!(seeded().health().unwrap().fts_ok);
    }

    /// An index with rows but no provenance is stale, and the status output
    /// has to say so rather than presenting it as current.
    #[test]
    fn status_flags_an_unversioned_index() {
        let out = handle_status(&seeded(), None).unwrap();
        assert!(out.contains("Out of date"), "status was:\n{out}");
        assert!(out.contains("before symgraph recorded index provenance"));
    }

    #[test]
    fn status_of_a_stamped_index_is_not_flagged() {
        let db = seeded();
        db.record_index_version().unwrap();
        let out = handle_status(&db, None).unwrap();
        assert!(!out.contains("Out of date"), "status was:\n{out}");
        assert!(out.contains("schema v"));
    }

    /// An empty index has nothing in it to be wrong about.
    #[test]
    fn status_of_an_empty_index_is_not_stale() {
        let db = Database::in_memory().unwrap();
        let out = handle_status(&db, None).unwrap();
        assert!(!out.contains("Out of date"), "status was:\n{out}");
    }

    #[test]
    fn age_is_described_in_human_units() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert_eq!(age(0), "unknown");
        assert_eq!(age(now), "just now");
        assert_eq!(age(now - 600), "10 minutes ago");
        assert_eq!(age(now - 7200), "2 hours ago");
        assert_eq!(age(now - 3 * 86_400), "3 days ago");
    }
}

#[cfg(test)]
mod format_tests {
    use super::*;

    /// `status` is a tool like any other, so it answers in JSON when asked —
    /// the last of the 22 to gain a `format` field (F4).
    #[test]
    fn status_renders_as_json() {
        let db = Database::in_memory().unwrap();
        let out = handle_status(&db, Some("json".to_string())).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(v["total_files"].is_number(), "json was:\n{v:#}");
        assert!(
            v["health"]["ambiguous_names"].is_number(),
            "json was:\n{v:#}"
        );
    }
}
