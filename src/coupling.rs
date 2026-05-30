//! Coupling analysis.
//!
//! Folds the resolved symbol graph to a chosen boundary (file / dir / module),
//! ranks hubs by fan-in/fan-out, detects dependency cycles (SCCs), and scores
//! each module-pair edge on the strength × distance × volatility framework.
//!
//! All functions are pure over [`EdgeEndpoint`] lists plus an optional churn
//! map, so they are unit-testable without a database.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

use crate::db::EdgeEndpoint;
use crate::types::EdgeKind;

/// Boundary to fold the graph to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Granularity {
    File,
    Dir,
    Module,
}

impl Granularity {
    pub fn parse(s: &str) -> Granularity {
        match s.to_ascii_lowercase().as_str() {
            "file" => Granularity::File,
            "dir" | "directory" => Granularity::Dir,
            _ => Granularity::Module,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Granularity::File => "file",
            Granularity::Dir => "dir",
            Granularity::Module => "module",
        }
    }
}

/// Reduce a file path to its boundary id at the chosen granularity.
///
/// - `File`: the path itself (normalised to `/`).
/// - `Dir`: the parent directory.
/// - `Module`: the path without extension, with module-root files
///   (`mod`, `lib`, `main`, `index`, `__init__`) collapsed into their dir.
pub fn boundary_of(path: &str, g: Granularity) -> String {
    let p = path.replace('\\', "/");
    match g {
        Granularity::File => p,
        Granularity::Dir => parent_dir(&p),
        Granularity::Module => {
            let (dir, stem) = split_stem(&p);
            if matches!(stem, "mod" | "lib" | "main" | "index" | "__init__") {
                if dir.is_empty() {
                    stem.to_string()
                } else {
                    dir
                }
            } else if dir.is_empty() {
                stem.to_string()
            } else {
                format!("{}/{}", dir, stem)
            }
        }
    }
}

fn parent_dir(p: &str) -> String {
    match p.rfind('/') {
        Some(i) => p[..i].to_string(),
        None => ".".to_string(),
    }
}

/// Split a path into (dir, file-stem-without-extension).
fn split_stem(p: &str) -> (String, &str) {
    let (dir, file) = match p.rfind('/') {
        Some(i) => (p[..i].to_string(), &p[i + 1..]),
        None => (String::new(), p),
    };
    let stem = match file.rfind('.') {
        Some(i) if i > 0 => &file[..i],
        _ => file,
    };
    (dir, stem)
}

/// A folded module-to-module dependency edge.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleEdge {
    pub from: String,
    pub to: String,
    /// Total underlying symbol edges folded into this module edge.
    pub count: u32,
    /// Breakdown by underlying edge kind.
    pub by_kind: BTreeMap<String, u32>,
    /// Integration strength (1 contract, 2 model, 3 intrusive) — the max over
    /// the underlying edge kinds.
    pub strength: u8,
}

/// A node (module/file/dir) in the folded graph.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleNode {
    pub id: String,
    /// Number of distinct modules that depend on this one.
    pub fan_in: u32,
    /// Number of distinct modules this one depends on.
    pub fan_out: u32,
    /// Commits touching this module's files in the churn window, if computed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub churn: Option<u32>,
}

/// The folded module graph plus detected cycles.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleGraph {
    pub granularity: String,
    pub nodes: Vec<ModuleNode>,
    pub edges: Vec<ModuleEdge>,
    /// Strongly-connected components with more than one member (dependency
    /// cycles). Empty ⇒ the module graph is acyclic.
    pub cycles: Vec<Vec<String>>,
}

/// Fold `endpoints` to `g`, dropping internal (same-boundary) edges. If
/// `churn` is given (file path → commits) it is aggregated per boundary and
/// attached to each node.
pub fn build_module_graph(
    endpoints: &[EdgeEndpoint],
    g: Granularity,
    churn: Option<&HashMap<String, u32>>,
) -> ModuleGraph {
    // Aggregate edges keyed by (from, to).
    let mut agg: HashMap<(String, String), ModuleEdge> = HashMap::new();
    for e in endpoints {
        // `contains` is structural hierarchy and `tests` is synthetic test→prod
        // linkage — neither is an architectural dependency, so skip both.
        if matches!(e.kind, EdgeKind::Contains | EdgeKind::Tests) {
            continue;
        }
        let from = boundary_of(&e.source_file, g);
        let to = boundary_of(&e.target_file, g);
        if from == to {
            continue;
        }
        let entry = agg
            .entry((from.clone(), to.clone()))
            .or_insert_with(|| ModuleEdge {
                from,
                to,
                count: 0,
                by_kind: BTreeMap::new(),
                strength: 0,
            });
        entry.count += 1;
        *entry
            .by_kind
            .entry(e.kind.as_str().to_string())
            .or_insert(0) += 1;
        entry.strength = entry.strength.max(e.kind.strength());
    }

    let mut edges: Vec<ModuleEdge> = agg.into_values().collect();
    edges.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.from.cmp(&b.from)));

    // Per-node fan-in / fan-out (distinct neighbours).
    let mut fan_in: HashMap<String, u32> = HashMap::new();
    let mut fan_out: HashMap<String, u32> = HashMap::new();
    let mut ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for e in &edges {
        *fan_out.entry(e.from.clone()).or_insert(0) += 1;
        *fan_in.entry(e.to.clone()).or_insert(0) += 1;
        ids.insert(e.from.clone());
        ids.insert(e.to.clone());
    }

    let boundary_churn = churn.map(|c| aggregate_churn(c, g));

    let mut nodes: Vec<ModuleNode> = ids
        .into_iter()
        .map(|id| {
            let churn = boundary_churn.as_ref().and_then(|m| m.get(&id).copied());
            ModuleNode {
                fan_in: fan_in.get(&id).copied().unwrap_or(0),
                fan_out: fan_out.get(&id).copied().unwrap_or(0),
                churn,
                id,
            }
        })
        .collect();
    // Rank hubs first: highest fan-in, then fan-out.
    nodes.sort_by(|a, b| {
        b.fan_in
            .cmp(&a.fan_in)
            .then_with(|| b.fan_out.cmp(&a.fan_out))
            .then_with(|| a.id.cmp(&b.id))
    });

    let cycles = strongly_connected_components(&edges)
        .into_iter()
        .filter(|c| c.len() > 1)
        .collect();

    ModuleGraph {
        granularity: g.as_str().to_string(),
        nodes,
        edges,
        cycles,
    }
}

/// Aggregate a file-path churn map into per-boundary totals.
fn aggregate_churn(churn: &HashMap<String, u32>, g: Granularity) -> HashMap<String, u32> {
    let mut out: HashMap<String, u32> = HashMap::new();
    for (path, n) in churn {
        *out.entry(boundary_of(path, g)).or_insert(0) += *n;
    }
    out
}

/// Tarjan's algorithm — returns all strongly-connected components.
fn strongly_connected_components(edges: &[ModuleEdge]) -> Vec<Vec<String>> {
    // Build adjacency over integer ids for speed.
    let mut index_of: HashMap<&str, usize> = HashMap::new();
    let mut names: Vec<&str> = Vec::new();
    for e in edges {
        for id in [e.from.as_str(), e.to.as_str()] {
            if !index_of.contains_key(id) {
                index_of.insert(id, names.len());
                names.push(id);
            }
        }
    }
    let n = names.len();
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for e in edges {
        let u = index_of[e.from.as_str()];
        let v = index_of[e.to.as_str()];
        adj[u].push(v);
    }

    struct Tarjan {
        adj: Vec<Vec<usize>>,
        index: Vec<i64>,
        lowlink: Vec<i64>,
        on_stack: Vec<bool>,
        stack: Vec<usize>,
        next_index: i64,
        components: Vec<Vec<usize>>,
    }
    impl Tarjan {
        fn strongconnect(&mut self, v: usize) {
            self.index[v] = self.next_index;
            self.lowlink[v] = self.next_index;
            self.next_index += 1;
            self.stack.push(v);
            self.on_stack[v] = true;

            for i in 0..self.adj[v].len() {
                let w = self.adj[v][i];
                if self.index[w] < 0 {
                    self.strongconnect(w);
                    self.lowlink[v] = self.lowlink[v].min(self.lowlink[w]);
                } else if self.on_stack[w] {
                    self.lowlink[v] = self.lowlink[v].min(self.index[w]);
                }
            }

            if self.lowlink[v] == self.index[v] {
                let mut comp = Vec::new();
                loop {
                    let w = self.stack.pop().unwrap();
                    self.on_stack[w] = false;
                    comp.push(w);
                    if w == v {
                        break;
                    }
                }
                self.components.push(comp);
            }
        }
    }

    let mut t = Tarjan {
        adj,
        index: vec![-1; n],
        lowlink: vec![-1; n],
        on_stack: vec![false; n],
        stack: Vec::new(),
        next_index: 0,
        components: Vec::new(),
    };
    for v in 0..n {
        if t.index[v] < 0 {
            t.strongconnect(v);
        }
    }

    t.components
        .into_iter()
        .map(|comp| {
            let mut c: Vec<String> = comp.into_iter().map(|i| names[i].to_string()).collect();
            c.sort();
            c
        })
        .collect()
}

/// A scored module-pair coupling, on strength × distance × volatility.
#[derive(Debug, Clone, Serialize)]
pub struct CouplingScore {
    pub from: String,
    pub to: String,
    pub strength: u8,
    pub distance: u8,
    pub volatility: u32,
    /// strength × distance × volatility — higher is a worse hotspot.
    pub impact: u32,
    pub edge_count: u32,
    pub by_kind: BTreeMap<String, u32>,
}

/// Score every module-pair edge of a folded graph. `churn` (file → commits)
/// supplies volatility; absent churn defaults volatility to 1 (neutral).
pub fn score_coupling(
    graph: &ModuleGraph,
    g: Granularity,
    churn: Option<&HashMap<String, u32>>,
) -> Vec<CouplingScore> {
    let boundary_churn = churn.map(|c| aggregate_churn(c, g));
    let mut scores: Vec<CouplingScore> = graph
        .edges
        .iter()
        .map(|e| {
            let distance = distance_between(&e.from, &e.to);
            let volatility = boundary_churn
                .as_ref()
                .map(|m| {
                    let a = m.get(&e.from).copied().unwrap_or(0);
                    let b = m.get(&e.to).copied().unwrap_or(0);
                    a.max(b).max(1)
                })
                .unwrap_or(1);
            CouplingScore {
                from: e.from.clone(),
                to: e.to.clone(),
                strength: e.strength,
                distance,
                volatility,
                impact: e.strength as u32 * distance as u32 * volatility,
                edge_count: e.count,
                by_kind: e.by_kind.clone(),
            }
        })
        .collect();
    scores.sort_by(|a, b| b.impact.cmp(&a.impact).then_with(|| a.from.cmp(&b.from)));
    scores
}

/// Distance heuristic: 1 if siblings (same parent dir), 2 if same top-level
/// segment, 3 otherwise.
fn distance_between(a: &str, b: &str) -> u8 {
    if parent_dir(a) == parent_dir(b) {
        1
    } else if top_segment(a) == top_segment(b) {
        2
    } else {
        3
    }
}

fn top_segment(p: &str) -> &str {
    p.split('/').next().unwrap_or(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(s: &str, t: &str, kind: EdgeKind) -> EdgeEndpoint {
        EdgeEndpoint {
            source_file: s.to_string(),
            target_file: t.to_string(),
            kind,
            detail: None,
        }
    }

    #[test]
    fn boundary_module_collapses_mod_files() {
        assert_eq!(boundary_of("src/db/mod.rs", Granularity::Module), "src/db");
        assert_eq!(
            boundary_of("src/mcp/handlers/graph.rs", Granularity::Module),
            "src/mcp/handlers/graph"
        );
        assert_eq!(boundary_of("src/db/mod.rs", Granularity::Dir), "src/db");
        assert_eq!(boundary_of("a.py", Granularity::Module), "a");
    }

    #[test]
    fn folds_edges_and_drops_internal() {
        let eps = vec![
            ep("src/a/mod.rs", "src/b/mod.rs", EdgeKind::Calls),
            ep("src/a/mod.rs", "src/b/mod.rs", EdgeKind::Mutates),
            ep("src/a/x.rs", "src/a/y.rs", EdgeKind::Calls), // internal to module src/a? no: a/x vs a/y
            ep("src/a/mod.rs", "src/a/mod.rs", EdgeKind::Calls), // self -> dropped
        ];
        let g = build_module_graph(&eps, Granularity::Module, None);
        // a->b edge folded (count 2, strength 3 from Mutates).
        let ab = g
            .edges
            .iter()
            .find(|e| e.from == "src/a" && e.to == "src/b");
        assert!(ab.is_some());
        let ab = ab.unwrap();
        assert_eq!(ab.count, 2);
        assert_eq!(ab.strength, 3);
        // src/a hub: a/x -> a/y are distinct modules so that's an a/x->a/y edge.
        assert!(g
            .edges
            .iter()
            .any(|e| e.from == "src/a/x" && e.to == "src/a/y"));
    }

    #[test]
    fn detects_cycles() {
        let eps = vec![
            ep("src/a/mod.rs", "src/b/mod.rs", EdgeKind::Calls),
            ep("src/b/mod.rs", "src/c/mod.rs", EdgeKind::Calls),
            ep("src/c/mod.rs", "src/a/mod.rs", EdgeKind::Calls),
        ];
        let g = build_module_graph(&eps, Granularity::Module, None);
        assert_eq!(g.cycles.len(), 1);
        assert_eq!(g.cycles[0], vec!["src/a", "src/b", "src/c"]);
    }

    #[test]
    fn acyclic_has_no_cycles() {
        let eps = vec![
            ep("src/a/mod.rs", "src/b/mod.rs", EdgeKind::Calls),
            ep("src/b/mod.rs", "src/c/mod.rs", EdgeKind::Calls),
        ];
        let g = build_module_graph(&eps, Granularity::Module, None);
        assert!(g.cycles.is_empty());
    }

    #[test]
    fn scores_weight_intrusive_higher() {
        let eps = vec![
            ep("src/a/mod.rs", "src/b/mod.rs", EdgeKind::Mutates),
            ep("src/a/mod.rs", "src/c/mod.rs", EdgeKind::Calls),
        ];
        let g = build_module_graph(&eps, Granularity::Module, None);
        let scores = score_coupling(&g, Granularity::Module, None);
        // a->b (strength 3) should outrank a->c (strength 1) at equal distance.
        assert_eq!(scores[0].to, "src/b");
        assert!(scores[0].impact > scores[1].impact);
    }
}
