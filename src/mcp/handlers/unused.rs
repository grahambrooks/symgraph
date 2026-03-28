//! Handler for unused symbols tool

use crate::db::Database;

pub fn handle_unused(db: &Database) -> Result<String, String> {
    match db.find_unused_symbols() {
        Ok(nodes) => {
            if nodes.is_empty() {
                Ok("No unused symbols found (all symbols are referenced or exported)".to_string())
            } else {
                let mut output = format!(
                    "# Unused Symbols\n\nFound {} unused symbols:\n\n",
                    nodes.len()
                );

                // Group by file
                let mut by_file: std::collections::HashMap<String, Vec<_>> =
                    std::collections::HashMap::new();
                for node in nodes {
                    by_file
                        .entry(node.file_path.clone())
                        .or_default()
                        .push(node);
                }

                let mut files: Vec<_> = by_file.keys().collect();
                files.sort();

                for file_path in files {
                    let nodes = &by_file[file_path];
                    output.push_str(&format!("## {}\n\n", file_path));
                    for node in nodes {
                        output.push_str(&format!(
                            "- {} `{}` at line {}\n",
                            node.kind.as_str(),
                            node.name,
                            node.start_line
                        ));
                    }
                    output.push('\n');
                }
                Ok(output)
            }
        }
        Err(e) => Err(e.to_string()),
    }
}
