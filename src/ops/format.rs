//! Shared formatting utilities for MCP tool outputs

use std::borrow::Cow;

use crate::types::Node;

/// Format a single node as a list item with location
pub fn format_node_list_item(node: &Node) -> String {
    format!(
        "- **{}** `{}` - {}:{}-{}",
        node.kind.as_str(),
        node.name,
        node.file_path,
        node.start_line,
        node.end_line
    )
}

/// Format a node with signature
pub fn format_node_with_signature(node: &Node) -> String {
    let mut output = format_node_list_item(node);
    if let Some(ref sig) = node.signature {
        output.push_str(&format!("\n  `{}`", sig));
    }
    output.push('\n');
    output
}

/// Format a node with basic location (for callers/callees)
pub fn format_node_simple(node: &Node) -> String {
    format!(
        "- **{}** `{}` - {}:{}",
        node.kind.as_str(),
        node.name,
        node.file_path,
        node.start_line
    )
}

/// Normalize a caller-supplied file path to the form the index is keyed on:
/// forward slashes, no leading `./`.
///
/// Indexed paths are stored with `/` on every platform, but a caller on
/// Windows will naturally pass `src\\foo.rs` — from its own shell, from a
/// tool that printed a native path, or from `Path::display`. Folding the
/// separator here means those lookups hit instead of silently returning
/// nothing. Returns `Cow` so the common already-normalized case stays
/// allocation-free.
pub fn normalize_path(path: &str) -> Cow<'_, str> {
    // Fold separators first: `.\src\lib.rs` has to lose the `.\` too, which a
    // `./`-only trim would miss.
    if path.contains('\\') {
        let folded = path.replace('\\', "/");
        Cow::Owned(folded.trim_start_matches("./").to_string())
    } else {
        Cow::Borrowed(path.trim_start_matches("./"))
    }
}

/// Format a node with full details
pub fn format_node(node: &Node) -> String {
    let mut output = String::new();
    output.push_str(&format!("**{}** `{}`\n", node.kind.as_str(), node.name));
    output.push_str(&format!(
        "- Location: {}:{}-{}\n",
        node.file_path, node.start_line, node.end_line
    ));
    if let Some(ref sig) = node.signature {
        output.push_str(&format!("- Signature: `{}`\n", sig));
    }
    if let Some(ref doc) = node.docstring {
        output.push_str(&format!("- Doc: {}\n", doc));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_leading_dot_slash() {
        assert_eq!(normalize_path("./src/foo.rs"), "src/foo.rs");
        assert_eq!(normalize_path("src/foo.rs"), "src/foo.rs");
    }

    /// The index is keyed on forward slashes on every platform, so a native
    /// Windows path has to fold onto the same key.
    #[test]
    fn normalize_folds_backslashes() {
        assert_eq!(normalize_path("src\\db\\mod.rs"), "src/db/mod.rs");
        // A native relative path carries both a `.\` prefix and backslashes.
        assert_eq!(normalize_path(".\\src\\lib.rs"), "src/lib.rs");
    }

    /// An already-normal path must not allocate.
    #[test]
    fn normalize_borrows_when_nothing_changes() {
        assert!(matches!(normalize_path("src/foo.rs"), Cow::Borrowed(_)));
        assert!(matches!(normalize_path("src\\foo.rs"), Cow::Owned(_)));
    }
}
