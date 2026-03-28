//! Handler for implementations tool

use crate::db::Database;
use crate::mcp::format;
use crate::mcp::types::SymbolRequest;

pub fn handle_implementations(db: &Database, req: &SymbolRequest) -> Result<String, String> {
    match db.find_implementations(&req.symbol) {
        Ok(nodes) => {
            if nodes.is_empty() {
                Ok(format!("No implementations found for '{}'", req.symbol))
            } else {
                let mut output = format!("# Implementations of '{}'\n\n", req.symbol);
                output.push_str(&format!("Found {} implementation(s):\n\n", nodes.len()));
                for node in nodes {
                    output.push_str(&format::format_node(&node));
                    output.push_str("\n\n");
                }
                Ok(output)
            }
        }
        Err(e) => Err(e.to_string()),
    }
}
