//! Context builder for AI tasks
//!
//! Builds focused context for code exploration tasks by:
//! - Finding relevant entry points
//! - Extracting related symbols
//! - Building code snippets

use std::fs;

use anyhow::Result;

use crate::db::Database;
use crate::graph::Graph;
use crate::types::{CodeBlock, Node, NodeKind, TaskContext};

/// Options for building context
#[derive(Debug, Clone)]
pub struct ContextOptions {
    /// Maximum number of nodes to include
    pub max_nodes: u32,
    /// Whether to include code snippets
    pub include_code: bool,
    /// Maximum number of code blocks
    pub max_code_blocks: u32,
    /// Maximum size of each code block
    pub max_block_size: usize,
    /// Graph traversal depth
    pub depth: u32,
}

impl Default for ContextOptions {
    fn default() -> Self {
        Self {
            max_nodes: 20,
            include_code: true,
            max_code_blocks: 5,
            max_block_size: 1500,
            depth: 1,
        }
    }
}

/// Builds task context from the code graph
pub struct ContextBuilder<'a> {
    db: &'a Database,
    graph: Graph<'a>,
    project_root: String,
}

impl<'a> ContextBuilder<'a> {
    pub fn new(db: &'a Database, project_root: String) -> Self {
        Self {
            db,
            graph: Graph::new(db),
            project_root,
        }
    }

    /// Build context for a task description
    pub fn build_context(&self, task: &str, options: &ContextOptions) -> Result<TaskContext> {
        // Step 1: Find entry points by searching for relevant symbols
        let entry_points = self.find_entry_points(task, options.max_nodes / 2)?;

        // Step 2: Find related nodes through graph traversal
        let related_nodes = if !entry_points.is_empty() {
            self.graph
                .find_related(&entry_points, options.max_nodes / 2)?
        } else {
            Vec::new()
        };

        // Step 3: Get edges between all nodes
        let all_node_ids: Vec<i64> = entry_points
            .iter()
            .chain(related_nodes.iter())
            .map(|n| n.id)
            .collect();

        let mut edges = Vec::new();
        for &id in &all_node_ids {
            let out_edges = self.db.get_outgoing_edges(id)?;
            for edge in out_edges {
                if all_node_ids.contains(&edge.target_id) {
                    edges.push(edge);
                }
            }
        }

        // Step 4: Build code blocks for priority nodes
        let code_blocks = if options.include_code {
            self.build_code_blocks(&entry_points, options)?
        } else {
            Vec::new()
        };

        Ok(TaskContext {
            entry_points,
            related_nodes,
            edges,
            code_blocks,
        })
    }

    /// Find entry points for a task by searching symbol names
    fn find_entry_points(&self, task: &str, limit: u32) -> Result<Vec<Node>> {
        let mut entry_points = Vec::new();

        // Extract keywords from the task description
        let keywords = self.parse_task_keywords(task);

        for keyword in keywords {
            if entry_points.len() >= limit as usize {
                break;
            }

            // Search for symbols matching the keyword
            let mut results = self.db.search_nodes(&keyword, None, 5)?;

            // Prioritize functions and methods
            results.sort_by_key(|n| match n.kind {
                NodeKind::Function | NodeKind::Method => 0,
                NodeKind::Class | NodeKind::Struct => 1,
                NodeKind::Interface | NodeKind::Trait => 2,
                _ => 3,
            });

            for node in results {
                if !entry_points.iter().any(|n: &Node| n.id == node.id) {
                    entry_points.push(node);
                    if entry_points.len() >= limit as usize {
                        break;
                    }
                }
            }
        }

        Ok(entry_points)
    }

    /// Parse keywords from a task description for symbol search
    fn parse_task_keywords(&self, task: &str) -> Vec<String> {
        // Simple keyword extraction: split on whitespace and filter
        let stop_words = [
            "the",
            "a",
            "an",
            "is",
            "are",
            "was",
            "were",
            "be",
            "been",
            "being",
            "have",
            "has",
            "had",
            "do",
            "does",
            "did",
            "will",
            "would",
            "could",
            "should",
            "may",
            "might",
            "must",
            "shall",
            "can",
            "need",
            "to",
            "of",
            "in",
            "for",
            "on",
            "with",
            "at",
            "by",
            "from",
            "as",
            "into",
            "through",
            "during",
            "before",
            "after",
            "above",
            "below",
            "between",
            "under",
            "again",
            "further",
            "then",
            "once",
            "here",
            "there",
            "when",
            "where",
            "why",
            "how",
            "all",
            "each",
            "few",
            "more",
            "most",
            "other",
            "some",
            "such",
            "no",
            "nor",
            "not",
            "only",
            "own",
            "same",
            "so",
            "than",
            "too",
            "very",
            "just",
            "and",
            "but",
            "if",
            "or",
            "because",
            "until",
            "while",
            "this",
            "that",
            "these",
            "those",
            "what",
            "which",
            "who",
            "whom",
            "find",
            "get",
            "look",
            "see",
            "use",
            "make",
            "want",
            "fix",
            "add",
            "update",
            "change",
            "modify",
            "implement",
            "create",
            "delete",
            "remove",
        ];

        task.split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|word| {
                let lower = word.to_lowercase();
                word.len() > 2 && !stop_words.contains(&lower.as_str())
            })
            .map(|s| s.to_string())
            .collect()
    }

    /// Build code blocks for the given nodes
    fn build_code_blocks(
        &self,
        nodes: &[Node],
        options: &ContextOptions,
    ) -> Result<Vec<CodeBlock>> {
        let mut blocks = Vec::new();

        for node in nodes.iter().take(options.max_code_blocks as usize) {
            // Skip file nodes
            if node.kind == NodeKind::File {
                continue;
            }

            // Read the source file (traversal-guarded — node.file_path came
            // from the DB, but the DB is populated from user-controlled
            // indexing input, so we still gate reads through safe_join).
            let file_path = match crate::security::safe_join(&self.project_root, &node.file_path) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let content = match fs::read_to_string(&file_path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            // Extract the relevant lines
            let lines: Vec<&str> = content.lines().collect();
            let start = (node.start_line as usize).saturating_sub(1);
            let end = (node.end_line as usize).min(lines.len());

            if start >= lines.len() {
                continue;
            }

            let code_lines: Vec<&str> = lines[start..end].to_vec();
            let mut code = code_lines.join("\n");

            // Truncate if too long
            if code.len() > options.max_block_size {
                code = code[..options.max_block_size].to_string();
                code.push_str("\n// ... truncated");
            }

            // Get context before (up to 3 lines)
            let context_before = if start > 0 {
                let ctx_start = start.saturating_sub(3);
                Some(lines[ctx_start..start].join("\n"))
            } else {
                None
            };

            // Get context after (up to 3 lines)
            let context_after = if end < lines.len() {
                let ctx_end = (end + 3).min(lines.len());
                Some(lines[end..ctx_end].join("\n"))
            } else {
                None
            };

            blocks.push(CodeBlock {
                node: node.clone(),
                code,
                context_before,
                context_after,
            });
        }

        Ok(blocks)
    }
}

/// Format task context as markdown
pub fn format_context_markdown(context: &TaskContext) -> String {
    let mut output = String::new();

    output.push_str("## Entry Points\n\n");
    for node in &context.entry_points {
        output.push_str(&format!(
            "- **{}** `{}` ({}) - {}:{}-{}\n",
            node.kind.as_str(),
            node.name,
            node.language.as_str(),
            node.file_path,
            node.start_line,
            node.end_line
        ));
        if let Some(ref sig) = node.signature {
            output.push_str(&format!("  ```\n  {}\n  ```\n", sig));
        }
    }

    if !context.related_nodes.is_empty() {
        output.push_str("\n## Related Symbols\n\n");
        for node in &context.related_nodes {
            output.push_str(&format!(
                "- **{}** `{}` - {}:{}\n",
                node.kind.as_str(),
                node.name,
                node.file_path,
                node.start_line
            ));
        }
    }

    if !context.code_blocks.is_empty() {
        output.push_str("\n## Code\n\n");
        for block in &context.code_blocks {
            output.push_str(&format!(
                "### {} ({}:{})\n\n```{}\n{}\n```\n\n",
                block.node.name,
                block.node.file_path,
                block.node.start_line,
                block.node.language.as_str(),
                block.code
            ));
        }
    }

    output
}
