//! Graph traversal and query operations
//!
//! Provides algorithms for:
//! - Finding callers/callees
//! - Impact analysis
//! - Subgraph extraction

use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::Result;

use crate::db::Database;
use crate::types::{Edge, Node, TraversalOptions};

/// Graph operations on the code database
pub struct Graph<'a> {
    db: &'a Database,
}

impl<'a> Graph<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Find all callers of a symbol (functions that call this function)
    pub fn find_callers(&self, symbol_name: &str, limit: u32) -> Result<Vec<Node>> {
        // First, find the node by name
        let target = match self.db.find_node_by_name(symbol_name)? {
            Some(node) => node,
            None => return Ok(Vec::new()),
        };

        self.db.get_callers(target.id, limit)
    }

    /// Find all callees of a symbol (functions that this function calls)
    pub fn find_callees(&self, symbol_name: &str, limit: u32) -> Result<Vec<Node>> {
        let source = match self.db.find_node_by_name(symbol_name)? {
            Some(node) => node,
            None => return Ok(Vec::new()),
        };

        self.db.get_callees(source.id, limit)
    }

    /// Analyze the impact of changing a symbol
    /// Returns all symbols that could be affected by the change
    pub fn analyze_impact(&self, symbol_name: &str, depth: u32) -> Result<ImpactAnalysis> {
        let root = match self.db.find_node_by_name(symbol_name)? {
            Some(node) => node,
            None => {
                return Ok(ImpactAnalysis {
                    root: None,
                    direct_callers: Vec::new(),
                    indirect_callers: Vec::new(),
                    total_impact: 0,
                })
            }
        };

        let mut visited: HashSet<i64> = HashSet::new();
        let mut direct_callers = Vec::new();
        let mut indirect_callers = Vec::new();

        visited.insert(root.id);

        // BFS to find all callers up to depth
        let mut queue: VecDeque<(i64, u32)> = VecDeque::new();
        queue.push_back((root.id, 0));

        while let Some((node_id, current_depth)) = queue.pop_front() {
            if current_depth >= depth {
                continue;
            }

            // Get callers of this node
            let callers = self.db.get_callers(node_id, 100)?;

            for caller in callers {
                if visited.contains(&caller.id) {
                    continue;
                }
                visited.insert(caller.id);

                if current_depth == 0 {
                    direct_callers.push(caller.clone());
                } else {
                    indirect_callers.push(caller.clone());
                }

                queue.push_back((caller.id, current_depth + 1));
            }
        }

        let total_impact = direct_callers.len() + indirect_callers.len();

        Ok(ImpactAnalysis {
            root: Some(root),
            direct_callers,
            indirect_callers,
            total_impact,
        })
    }

    /// Extract a subgraph around a set of nodes
    pub fn extract_subgraph(
        &self,
        node_ids: &[i64],
        options: &TraversalOptions,
    ) -> Result<Subgraph> {
        let mut nodes: HashMap<i64, Node> = HashMap::new();
        let mut edges: Vec<Edge> = Vec::new();
        let mut visited: HashSet<i64> = HashSet::new();

        // Start with the seed nodes
        for &id in node_ids {
            if let Some(node) = self.db.get_node(id)? {
                nodes.insert(id, node);
                visited.insert(id);
            }
        }

        // BFS expansion
        let mut queue: VecDeque<(i64, u32)> = node_ids.iter().map(|&id| (id, 0)).collect();

        while let Some((node_id, depth)) = queue.pop_front() {
            if depth >= options.max_depth {
                continue;
            }

            // Get outgoing edges
            let out_edges = self.db.get_outgoing_edges(node_id)?;
            for edge in out_edges {
                // Filter by edge kind if specified
                if let Some(ref kinds) = options.edge_kinds {
                    if !kinds.contains(&edge.kind) {
                        continue;
                    }
                }

                edges.push(edge.clone());

                if !visited.contains(&edge.target_id) {
                    if let Some(target) = self.db.get_node(edge.target_id)? {
                        // Filter by node kind if specified
                        if let Some(ref kinds) = options.node_kinds {
                            if !kinds.contains(&target.kind) {
                                continue;
                            }
                        }

                        visited.insert(edge.target_id);
                        nodes.insert(edge.target_id, target);
                        queue.push_back((edge.target_id, depth + 1));

                        if nodes.len() >= options.limit as usize {
                            break;
                        }
                    }
                }
            }

            // Get incoming edges
            let in_edges = self.db.get_incoming_edges(node_id)?;
            for edge in in_edges {
                if let Some(ref kinds) = options.edge_kinds {
                    if !kinds.contains(&edge.kind) {
                        continue;
                    }
                }

                edges.push(edge.clone());

                if !visited.contains(&edge.source_id) {
                    if let Some(source) = self.db.get_node(edge.source_id)? {
                        if let Some(ref kinds) = options.node_kinds {
                            if !kinds.contains(&source.kind) {
                                continue;
                            }
                        }

                        visited.insert(edge.source_id);
                        nodes.insert(edge.source_id, source);
                        queue.push_back((edge.source_id, depth + 1));

                        if nodes.len() >= options.limit as usize {
                            break;
                        }
                    }
                }
            }

            if nodes.len() >= options.limit as usize {
                break;
            }
        }

        Ok(Subgraph {
            nodes: nodes.into_values().collect(),
            edges,
        })
    }

    /// Find related symbols given a set of entry points
    pub fn find_related(&self, entry_points: &[Node], max_nodes: u32) -> Result<Vec<Node>> {
        let mut related: HashMap<i64, (Node, f64)> = HashMap::new();
        let mut visited: HashSet<i64> = HashSet::new();

        for entry in entry_points {
            visited.insert(entry.id);
        }

        // For each entry point, find its neighbors
        for entry in entry_points {
            // Callees (what this function calls)
            let callees = self.db.get_callees(entry.id, 10)?;
            for (idx, callee) in callees.into_iter().enumerate() {
                if !visited.contains(&callee.id) {
                    let score = 1.0 / (idx as f64 + 1.0);
                    related
                        .entry(callee.id)
                        .and_modify(|(_, s)| *s += score)
                        .or_insert((callee, score));
                }
            }

            // Callers (what calls this function)
            let callers = self.db.get_callers(entry.id, 10)?;
            for (idx, caller) in callers.into_iter().enumerate() {
                if !visited.contains(&caller.id) {
                    let score = 0.8 / (idx as f64 + 1.0);
                    related
                        .entry(caller.id)
                        .and_modify(|(_, s)| *s += score)
                        .or_insert((caller, score));
                }
            }
        }

        // Sort by score and return top N
        let mut sorted: Vec<_> = related.into_values().collect();
        sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        sorted.truncate(max_nodes as usize);

        Ok(sorted.into_iter().map(|(node, _)| node).collect())
    }
}

/// Result of impact analysis
#[derive(Debug, Clone)]
pub struct ImpactAnalysis {
    pub root: Option<Node>,
    pub direct_callers: Vec<Node>,
    pub indirect_callers: Vec<Node>,
    pub total_impact: usize,
}

/// A subgraph extracted from the code graph
#[derive(Debug, Clone)]
pub struct Subgraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{EdgeKind, FileRecord, Language, NodeKind, Visibility};

    fn create_test_node(name: &str, kind: NodeKind) -> Node {
        Node {
            id: 0,
            kind,
            name: name.to_string(),
            qualified_name: Some(format!("test::{}", name)),
            file_path: "test.rs".to_string(),
            start_line: 1,
            end_line: 10,
            start_column: 0,
            end_column: 1,
            signature: Some(format!("fn {}()", name)),
            visibility: Visibility::Public,
            docstring: None,
            is_async: false,
            is_static: false,
            is_exported: true,
            is_test: false,
            is_generated: false,
            language: Language::Rust,
        }
    }

    fn setup_test_db() -> Database {
        let db = Database::in_memory().unwrap();
        let file = FileRecord {
            path: "test.rs".to_string(),
            content_hash: "abc123".to_string(),
            language: Language::Rust,
            size: 1000,
            modified_at: 0,
            indexed_at: 0,
            node_count: 0,
        };
        db.insert_or_update_file(&file).unwrap();
        db
    }

    #[test]
    fn test_graph_creation() {
        let db = setup_test_db();
        let graph = Graph::new(&db);
        assert!(std::mem::size_of_val(&graph) > 0);
    }

    #[test]
    fn test_find_callers_nonexistent_symbol() {
        let db = setup_test_db();
        let graph = Graph::new(&db);

        let callers = graph.find_callers("nonexistent", 10).unwrap();
        assert!(callers.is_empty());
    }

    #[test]
    fn test_find_callees_nonexistent_symbol() {
        let db = setup_test_db();
        let graph = Graph::new(&db);

        let callees = graph.find_callees("nonexistent", 10).unwrap();
        assert!(callees.is_empty());
    }

    #[test]
    fn test_find_callers() {
        let db = setup_test_db();

        // Create nodes: caller -> target
        let caller_id = db
            .insert_node(&create_test_node("caller_func", NodeKind::Function))
            .unwrap();
        let target_id = db
            .insert_node(&create_test_node("target_func", NodeKind::Function))
            .unwrap();

        // Create call edge
        let edge = Edge {
            id: 0,
            source_id: caller_id,
            target_id,
            kind: EdgeKind::Calls,
            file_path: None,
            line: None,
            column: None,
        };
        db.insert_edge(&edge).unwrap();

        let graph = Graph::new(&db);
        let callers = graph.find_callers("target_func", 10).unwrap();

        assert_eq!(callers.len(), 1);
        assert_eq!(callers[0].name, "caller_func");
    }

    #[test]
    fn test_find_callees() {
        let db = setup_test_db();

        // Create nodes: source -> callee
        let source_id = db
            .insert_node(&create_test_node("source_func", NodeKind::Function))
            .unwrap();
        let callee_id = db
            .insert_node(&create_test_node("callee_func", NodeKind::Function))
            .unwrap();

        // Create call edge
        let edge = Edge {
            id: 0,
            source_id,
            target_id: callee_id,
            kind: EdgeKind::Calls,
            file_path: None,
            line: None,
            column: None,
        };
        db.insert_edge(&edge).unwrap();

        let graph = Graph::new(&db);
        let callees = graph.find_callees("source_func", 10).unwrap();

        assert_eq!(callees.len(), 1);
        assert_eq!(callees[0].name, "callee_func");
    }

    #[test]
    fn test_find_multiple_callers() {
        let db = setup_test_db();

        let target_id = db
            .insert_node(&create_test_node("target", NodeKind::Function))
            .unwrap();
        let caller1_id = db
            .insert_node(&create_test_node("caller1", NodeKind::Function))
            .unwrap();
        let caller2_id = db
            .insert_node(&create_test_node("caller2", NodeKind::Function))
            .unwrap();
        let caller3_id = db
            .insert_node(&create_test_node("caller3", NodeKind::Function))
            .unwrap();

        for caller_id in [caller1_id, caller2_id, caller3_id] {
            let edge = Edge {
                id: 0,
                source_id: caller_id,
                target_id,
                kind: EdgeKind::Calls,
                file_path: None,
                line: None,
                column: None,
            };
            db.insert_edge(&edge).unwrap();
        }

        let graph = Graph::new(&db);
        let callers = graph.find_callers("target", 10).unwrap();

        assert_eq!(callers.len(), 3);
    }

    #[test]
    fn test_analyze_impact_nonexistent() {
        let db = setup_test_db();
        let graph = Graph::new(&db);

        let analysis = graph.analyze_impact("nonexistent", 2).unwrap();
        assert!(analysis.root.is_none());
        assert_eq!(analysis.total_impact, 0);
    }

    #[test]
    fn test_analyze_impact_no_callers() {
        let db = setup_test_db();

        db.insert_node(&create_test_node("isolated", NodeKind::Function))
            .unwrap();

        let graph = Graph::new(&db);
        let analysis = graph.analyze_impact("isolated", 2).unwrap();

        assert!(analysis.root.is_some());
        assert_eq!(analysis.root.unwrap().name, "isolated");
        assert!(analysis.direct_callers.is_empty());
        assert!(analysis.indirect_callers.is_empty());
        assert_eq!(analysis.total_impact, 0);
    }

    #[test]
    fn test_analyze_impact_direct_callers() {
        let db = setup_test_db();

        // Create chain: caller1 -> caller2 -> target
        let target_id = db
            .insert_node(&create_test_node("target", NodeKind::Function))
            .unwrap();
        let direct_id = db
            .insert_node(&create_test_node("direct_caller", NodeKind::Function))
            .unwrap();

        let edge = Edge {
            id: 0,
            source_id: direct_id,
            target_id,
            kind: EdgeKind::Calls,
            file_path: None,
            line: None,
            column: None,
        };
        db.insert_edge(&edge).unwrap();

        let graph = Graph::new(&db);
        let analysis = graph.analyze_impact("target", 2).unwrap();

        assert_eq!(analysis.direct_callers.len(), 1);
        assert_eq!(analysis.direct_callers[0].name, "direct_caller");
        assert_eq!(analysis.total_impact, 1);
    }

    #[test]
    fn test_analyze_impact_indirect_callers() {
        let db = setup_test_db();

        // Create chain: indirect -> direct -> target
        let target_id = db
            .insert_node(&create_test_node("target", NodeKind::Function))
            .unwrap();
        let direct_id = db
            .insert_node(&create_test_node("direct", NodeKind::Function))
            .unwrap();
        let indirect_id = db
            .insert_node(&create_test_node("indirect", NodeKind::Function))
            .unwrap();

        db.insert_edge(&Edge {
            id: 0,
            source_id: direct_id,
            target_id,
            kind: EdgeKind::Calls,
            file_path: None,
            line: None,
            column: None,
        })
        .unwrap();

        db.insert_edge(&Edge {
            id: 0,
            source_id: indirect_id,
            target_id: direct_id,
            kind: EdgeKind::Calls,
            file_path: None,
            line: None,
            column: None,
        })
        .unwrap();

        let graph = Graph::new(&db);
        let analysis = graph.analyze_impact("target", 3).unwrap();

        assert_eq!(analysis.direct_callers.len(), 1);
        assert_eq!(analysis.indirect_callers.len(), 1);
        assert_eq!(analysis.total_impact, 2);
    }

    #[test]
    fn test_analyze_impact_depth_limit() {
        let db = setup_test_db();

        // Create chain: c3 -> c2 -> c1 -> target
        let target_id = db
            .insert_node(&create_test_node("target", NodeKind::Function))
            .unwrap();
        let c1_id = db
            .insert_node(&create_test_node("c1", NodeKind::Function))
            .unwrap();
        let c2_id = db
            .insert_node(&create_test_node("c2", NodeKind::Function))
            .unwrap();
        let c3_id = db
            .insert_node(&create_test_node("c3", NodeKind::Function))
            .unwrap();

        db.insert_edge(&Edge {
            id: 0,
            source_id: c1_id,
            target_id,
            kind: EdgeKind::Calls,
            file_path: None,
            line: None,
            column: None,
        })
        .unwrap();
        db.insert_edge(&Edge {
            id: 0,
            source_id: c2_id,
            target_id: c1_id,
            kind: EdgeKind::Calls,
            file_path: None,
            line: None,
            column: None,
        })
        .unwrap();
        db.insert_edge(&Edge {
            id: 0,
            source_id: c3_id,
            target_id: c2_id,
            kind: EdgeKind::Calls,
            file_path: None,
            line: None,
            column: None,
        })
        .unwrap();

        let graph = Graph::new(&db);

        // With depth 1, only direct callers
        let analysis = graph.analyze_impact("target", 1).unwrap();
        assert_eq!(analysis.total_impact, 1);

        // With depth 2, direct + 1 level of indirect
        let analysis = graph.analyze_impact("target", 2).unwrap();
        assert_eq!(analysis.total_impact, 2);

        // With depth 3, all callers
        let analysis = graph.analyze_impact("target", 3).unwrap();
        assert_eq!(analysis.total_impact, 3);
    }

    #[test]
    fn test_extract_subgraph_empty() {
        let db = setup_test_db();
        let graph = Graph::new(&db);

        let options = TraversalOptions::default();
        let subgraph = graph.extract_subgraph(&[], &options).unwrap();

        assert!(subgraph.nodes.is_empty());
        assert!(subgraph.edges.is_empty());
    }

    #[test]
    fn test_extract_subgraph_single_node() {
        let db = setup_test_db();

        let node_id = db
            .insert_node(&create_test_node("single", NodeKind::Function))
            .unwrap();

        let graph = Graph::new(&db);
        let options = TraversalOptions::default();
        let subgraph = graph.extract_subgraph(&[node_id], &options).unwrap();

        assert_eq!(subgraph.nodes.len(), 1);
        assert_eq!(subgraph.nodes[0].name, "single");
    }

    #[test]
    fn test_extract_subgraph_with_edges() {
        let db = setup_test_db();

        let id1 = db
            .insert_node(&create_test_node("func1", NodeKind::Function))
            .unwrap();
        let id2 = db
            .insert_node(&create_test_node("func2", NodeKind::Function))
            .unwrap();

        db.insert_edge(&Edge {
            id: 0,
            source_id: id1,
            target_id: id2,
            kind: EdgeKind::Calls,
            file_path: None,
            line: None,
            column: None,
        })
        .unwrap();

        let graph = Graph::new(&db);
        let options = TraversalOptions {
            max_depth: 1,
            ..Default::default()
        };
        let subgraph = graph.extract_subgraph(&[id1], &options).unwrap();

        assert_eq!(subgraph.nodes.len(), 2);
        assert!(!subgraph.edges.is_empty());
    }

    #[test]
    fn test_find_related_empty() {
        let db = setup_test_db();
        let graph = Graph::new(&db);

        let related = graph.find_related(&[], 10).unwrap();
        assert!(related.is_empty());
    }

    #[test]
    fn test_find_related_with_callees() {
        let db = setup_test_db();

        let entry_id = db
            .insert_node(&create_test_node("entry", NodeKind::Function))
            .unwrap();
        let helper_id = db
            .insert_node(&create_test_node("helper", NodeKind::Function))
            .unwrap();

        db.insert_edge(&Edge {
            id: 0,
            source_id: entry_id,
            target_id: helper_id,
            kind: EdgeKind::Calls,
            file_path: None,
            line: None,
            column: None,
        })
        .unwrap();

        let entry = db.get_node(entry_id).unwrap().unwrap();

        let graph = Graph::new(&db);
        let related = graph.find_related(&[entry], 10).unwrap();

        assert_eq!(related.len(), 1);
        assert_eq!(related[0].name, "helper");
    }

    #[test]
    fn test_impact_analysis_prevents_cycles() {
        let db = setup_test_db();

        // Create cycle: a -> b -> c -> a
        let a_id = db
            .insert_node(&create_test_node("func_a", NodeKind::Function))
            .unwrap();
        let b_id = db
            .insert_node(&create_test_node("func_b", NodeKind::Function))
            .unwrap();
        let c_id = db
            .insert_node(&create_test_node("func_c", NodeKind::Function))
            .unwrap();

        db.insert_edge(&Edge {
            id: 0,
            source_id: a_id,
            target_id: b_id,
            kind: EdgeKind::Calls,
            file_path: None,
            line: None,
            column: None,
        })
        .unwrap();
        db.insert_edge(&Edge {
            id: 0,
            source_id: b_id,
            target_id: c_id,
            kind: EdgeKind::Calls,
            file_path: None,
            line: None,
            column: None,
        })
        .unwrap();
        db.insert_edge(&Edge {
            id: 0,
            source_id: c_id,
            target_id: a_id,
            kind: EdgeKind::Calls,
            file_path: None,
            line: None,
            column: None,
        })
        .unwrap();

        let graph = Graph::new(&db);

        // Should not infinite loop due to visited set
        let analysis = graph.analyze_impact("func_b", 10).unwrap();

        // func_a calls func_b, so func_a is a direct caller
        // func_c calls func_a (indirectly affects func_b through cycle)
        assert!(analysis.root.is_some());
        // The exact count depends on traversal, but should complete without hanging
        assert!(analysis.total_impact <= 2);
    }
}
