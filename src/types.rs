//! Core type definitions for symgraph
//!
//! Defines the fundamental types for representing code structure:
//! - Nodes: code symbols (functions, classes, methods, etc.)
//! - Edges: relationships between nodes (calls, contains, imports, etc.)
//! - Languages: supported programming languages

use serde::{Deserialize, Serialize};

/// Represents the kind of code symbol
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    File,
    Module,
    Class,
    Struct,
    Interface,
    Trait,
    Protocol,
    Function,
    Method,
    Property,
    Field,
    Variable,
    Constant,
    Enum,
    EnumMember,
    TypeAlias,
    Namespace,
    Parameter,
    Import,
    Export,
    Route,
    Component,
}

impl NodeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeKind::File => "file",
            NodeKind::Module => "module",
            NodeKind::Class => "class",
            NodeKind::Struct => "struct",
            NodeKind::Interface => "interface",
            NodeKind::Trait => "trait",
            NodeKind::Protocol => "protocol",
            NodeKind::Function => "function",
            NodeKind::Method => "method",
            NodeKind::Property => "property",
            NodeKind::Field => "field",
            NodeKind::Variable => "variable",
            NodeKind::Constant => "constant",
            NodeKind::Enum => "enum",
            NodeKind::EnumMember => "enum_member",
            NodeKind::TypeAlias => "type_alias",
            NodeKind::Namespace => "namespace",
            NodeKind::Parameter => "parameter",
            NodeKind::Import => "import",
            NodeKind::Export => "export",
            NodeKind::Route => "route",
            NodeKind::Component => "component",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "file" => Some(NodeKind::File),
            "module" => Some(NodeKind::Module),
            "class" => Some(NodeKind::Class),
            "struct" => Some(NodeKind::Struct),
            "interface" => Some(NodeKind::Interface),
            "trait" => Some(NodeKind::Trait),
            "protocol" => Some(NodeKind::Protocol),
            "function" => Some(NodeKind::Function),
            "method" => Some(NodeKind::Method),
            "property" => Some(NodeKind::Property),
            "field" => Some(NodeKind::Field),
            "variable" => Some(NodeKind::Variable),
            "constant" => Some(NodeKind::Constant),
            "enum" => Some(NodeKind::Enum),
            "enum_member" => Some(NodeKind::EnumMember),
            "type_alias" => Some(NodeKind::TypeAlias),
            "namespace" => Some(NodeKind::Namespace),
            "parameter" => Some(NodeKind::Parameter),
            "import" => Some(NodeKind::Import),
            "export" => Some(NodeKind::Export),
            "route" => Some(NodeKind::Route),
            "component" => Some(NodeKind::Component),
            _ => None,
        }
    }
}

/// Represents the kind of relationship between nodes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// Parent contains child (e.g., class contains method)
    #[default]
    Contains,
    /// Source calls target function/method
    Calls,
    /// Source imports target module/symbol
    Imports,
    /// Source exports target symbol
    Exports,
    /// Source extends target (inheritance)
    Extends,
    /// Source implements target interface/trait
    Implements,
    /// Source references target symbol
    References,
    /// Source has type target
    TypeOf,
    /// Source returns target type
    Returns,
    /// Source instantiates target class
    Instantiates,
    /// Source overrides target method
    Overrides,
    /// Source is decorated by target
    Decorates,
    /// Source is a test that exercises target
    Tests,
}

impl EdgeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EdgeKind::Contains => "contains",
            EdgeKind::Calls => "calls",
            EdgeKind::Imports => "imports",
            EdgeKind::Exports => "exports",
            EdgeKind::Extends => "extends",
            EdgeKind::Implements => "implements",
            EdgeKind::References => "references",
            EdgeKind::TypeOf => "type_of",
            EdgeKind::Returns => "returns",
            EdgeKind::Instantiates => "instantiates",
            EdgeKind::Overrides => "overrides",
            EdgeKind::Decorates => "decorates",
            EdgeKind::Tests => "tests",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "contains" => Some(EdgeKind::Contains),
            "calls" => Some(EdgeKind::Calls),
            "imports" => Some(EdgeKind::Imports),
            "exports" => Some(EdgeKind::Exports),
            "extends" => Some(EdgeKind::Extends),
            "implements" => Some(EdgeKind::Implements),
            "references" => Some(EdgeKind::References),
            "type_of" => Some(EdgeKind::TypeOf),
            "returns" => Some(EdgeKind::Returns),
            "instantiates" => Some(EdgeKind::Instantiates),
            "overrides" => Some(EdgeKind::Overrides),
            "decorates" => Some(EdgeKind::Decorates),
            "tests" => Some(EdgeKind::Tests),
            _ => None,
        }
    }
}

/// Supported programming languages
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
    TypeScript,
    JavaScript,
    Tsx,
    Jsx,
    Python,
    Go,
    Java,
    C,
    Cpp,
    CSharp,
    Php,
    Ruby,
    Swift,
    Kotlin,
    Scala,
    Groovy,
    Unknown,
}

impl Language {
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            "rs" => Language::Rust,
            "ts" => Language::TypeScript,
            "tsx" => Language::Tsx,
            "js" | "mjs" | "cjs" => Language::JavaScript,
            "jsx" => Language::Jsx,
            "py" | "pyi" => Language::Python,
            "go" => Language::Go,
            "java" => Language::Java,
            "c" | "h" => Language::C,
            "cpp" | "cc" | "cxx" | "hpp" | "hxx" => Language::Cpp,
            "cs" => Language::CSharp,
            "php" => Language::Php,
            "rb" => Language::Ruby,
            "swift" => Language::Swift,
            "kt" | "kts" => Language::Kotlin,
            "scala" | "sc" => Language::Scala,
            "groovy" => Language::Groovy,
            _ => Language::Unknown,
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "rust" => Language::Rust,
            "typescript" => Language::TypeScript,
            "javascript" => Language::JavaScript,
            "tsx" => Language::Tsx,
            "jsx" => Language::Jsx,
            "python" => Language::Python,
            "go" => Language::Go,
            "java" => Language::Java,
            "c" => Language::C,
            "cpp" => Language::Cpp,
            "csharp" => Language::CSharp,
            "php" => Language::Php,
            "ruby" => Language::Ruby,
            "swift" => Language::Swift,
            "kotlin" => Language::Kotlin,
            "scala" => Language::Scala,
            "groovy" => Language::Groovy,
            _ => Language::Unknown,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::TypeScript => "typescript",
            Language::JavaScript => "javascript",
            Language::Tsx => "tsx",
            Language::Jsx => "jsx",
            Language::Python => "python",
            Language::Go => "go",
            Language::Java => "java",
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::CSharp => "csharp",
            Language::Php => "php",
            Language::Ruby => "ruby",
            Language::Swift => "swift",
            Language::Kotlin => "kotlin",
            Language::Scala => "scala",
            Language::Groovy => "groovy",
            Language::Unknown => "unknown",
        }
    }
}

/// A location in source code
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub file_path: String,
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
}

/// Visibility modifier for symbols
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Public,
    Private,
    Protected,
    Internal,
    Unknown,
}

impl Visibility {
    pub fn as_str(&self) -> &'static str {
        match self {
            Visibility::Public => "public",
            Visibility::Private => "private",
            Visibility::Protected => "protected",
            Visibility::Internal => "internal",
            Visibility::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "public" | "pub" => Visibility::Public,
            "private" | "priv" => Visibility::Private,
            "protected" => Visibility::Protected,
            "internal" => Visibility::Internal,
            _ => Visibility::Unknown,
        }
    }
}

/// A code symbol (function, class, method, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: i64,
    pub kind: NodeKind,
    pub name: String,
    pub qualified_name: Option<String>,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub start_column: u32,
    pub end_column: u32,
    pub signature: Option<String>,
    pub visibility: Visibility,
    pub docstring: Option<String>,
    pub is_async: bool,
    pub is_static: bool,
    pub is_exported: bool,
    pub is_test: bool,
    pub is_generated: bool,
    pub language: Language,
}

/// Represents a relationship/edge between code symbols
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Edge {
    pub id: i64,
    pub source_id: i64,
    pub target_id: i64,
    #[serde(default)]
    pub kind: EdgeKind,
    pub file_path: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

/// Metadata about an indexed file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    pub path: String,
    pub content_hash: String,
    pub language: Language,
    pub size: u64,
    pub modified_at: i64,
    pub indexed_at: i64,
    pub node_count: u32,
}

/// An unresolved reference found during extraction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnresolvedReference {
    pub source_node_id: i64,
    pub reference_name: String,
    pub kind: EdgeKind,
    pub file_path: String,
    pub line: u32,
    pub column: u32,
}

/// Result of extracting symbols from a file
#[derive(Debug, Clone, Default)]
pub struct ExtractionResult {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub unresolved_refs: Vec<UnresolvedReference>,
    pub errors: Vec<ExtractionError>,
}

/// Error during extraction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionError {
    pub message: String,
    pub file_path: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

/// Search result from the code graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub node: Node,
    pub score: f64,
    pub code_snippet: Option<String>,
}

/// Options for graph traversal
#[derive(Debug, Clone)]
pub struct TraversalOptions {
    pub max_depth: u32,
    pub edge_kinds: Option<Vec<EdgeKind>>,
    pub node_kinds: Option<Vec<NodeKind>>,
    pub limit: u32,
}

impl Default for TraversalOptions {
    fn default() -> Self {
        Self {
            max_depth: 2,
            edge_kinds: None,
            node_kinds: None,
            limit: 50,
        }
    }
}

/// Index statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStats {
    pub total_files: u64,
    pub total_nodes: u64,
    pub total_edges: u64,
    pub db_size_bytes: u64,
    pub languages: Vec<(Language, u64)>,
    pub node_kinds: Vec<(NodeKind, u64)>,
}

/// Context built for a task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskContext {
    pub entry_points: Vec<Node>,
    pub related_nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub code_blocks: Vec<CodeBlock>,
}

/// A code block with context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeBlock {
    pub node: Node,
    pub code: String,
    pub context_before: Option<String>,
    pub context_after: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // NodeKind tests
    #[test]
    fn test_node_kind_as_str() {
        assert_eq!(NodeKind::Function.as_str(), "function");
        assert_eq!(NodeKind::Class.as_str(), "class");
        assert_eq!(NodeKind::Method.as_str(), "method");
        assert_eq!(NodeKind::EnumMember.as_str(), "enum_member");
        assert_eq!(NodeKind::TypeAlias.as_str(), "type_alias");
    }

    #[test]
    fn test_node_kind_from_str() {
        assert_eq!(NodeKind::parse("function"), Some(NodeKind::Function));
        assert_eq!(NodeKind::parse("class"), Some(NodeKind::Class));
        assert_eq!(NodeKind::parse("enum_member"), Some(NodeKind::EnumMember));
        assert_eq!(NodeKind::parse("invalid"), None);
        assert_eq!(NodeKind::parse(""), None);
    }

    #[test]
    fn test_node_kind_roundtrip() {
        let kinds = [
            NodeKind::File,
            NodeKind::Module,
            NodeKind::Class,
            NodeKind::Struct,
            NodeKind::Interface,
            NodeKind::Trait,
            NodeKind::Protocol,
            NodeKind::Function,
            NodeKind::Method,
            NodeKind::Property,
            NodeKind::Field,
            NodeKind::Variable,
            NodeKind::Constant,
            NodeKind::Enum,
            NodeKind::EnumMember,
            NodeKind::TypeAlias,
            NodeKind::Namespace,
            NodeKind::Parameter,
            NodeKind::Import,
            NodeKind::Export,
            NodeKind::Route,
            NodeKind::Component,
        ];

        for kind in kinds {
            let s = kind.as_str();
            let parsed = NodeKind::parse(s);
            assert_eq!(parsed, Some(kind), "Roundtrip failed for {:?}", kind);
        }
    }

    // EdgeKind tests
    #[test]
    fn test_edge_kind_as_str() {
        assert_eq!(EdgeKind::Calls.as_str(), "calls");
        assert_eq!(EdgeKind::Contains.as_str(), "contains");
        assert_eq!(EdgeKind::TypeOf.as_str(), "type_of");
    }

    #[test]
    fn test_edge_kind_from_str() {
        assert_eq!(EdgeKind::parse("calls"), Some(EdgeKind::Calls));
        assert_eq!(EdgeKind::parse("contains"), Some(EdgeKind::Contains));
        assert_eq!(EdgeKind::parse("type_of"), Some(EdgeKind::TypeOf));
        assert_eq!(EdgeKind::parse("invalid"), None);
    }

    #[test]
    fn test_edge_kind_roundtrip() {
        let kinds = [
            EdgeKind::Contains,
            EdgeKind::Calls,
            EdgeKind::Imports,
            EdgeKind::Exports,
            EdgeKind::Extends,
            EdgeKind::Implements,
            EdgeKind::References,
            EdgeKind::TypeOf,
            EdgeKind::Returns,
            EdgeKind::Instantiates,
            EdgeKind::Overrides,
            EdgeKind::Decorates,
            EdgeKind::Tests,
        ];

        for kind in kinds {
            let s = kind.as_str();
            let parsed = EdgeKind::parse(s);
            assert_eq!(parsed, Some(kind), "Roundtrip failed for {:?}", kind);
        }
    }

    // Language tests
    #[test]
    fn test_language_from_extension() {
        assert_eq!(Language::from_extension("rs"), Language::Rust);
        assert_eq!(Language::from_extension("ts"), Language::TypeScript);
        assert_eq!(Language::from_extension("tsx"), Language::Tsx);
        assert_eq!(Language::from_extension("js"), Language::JavaScript);
        assert_eq!(Language::from_extension("mjs"), Language::JavaScript);
        assert_eq!(Language::from_extension("cjs"), Language::JavaScript);
        assert_eq!(Language::from_extension("jsx"), Language::Jsx);
        assert_eq!(Language::from_extension("py"), Language::Python);
        assert_eq!(Language::from_extension("pyi"), Language::Python);
        assert_eq!(Language::from_extension("go"), Language::Go);
        assert_eq!(Language::from_extension("java"), Language::Java);
        assert_eq!(Language::from_extension("c"), Language::C);
        assert_eq!(Language::from_extension("h"), Language::C);
        assert_eq!(Language::from_extension("cpp"), Language::Cpp);
        assert_eq!(Language::from_extension("cc"), Language::Cpp);
        assert_eq!(Language::from_extension("hpp"), Language::Cpp);
        assert_eq!(Language::from_extension("cs"), Language::CSharp);
        assert_eq!(Language::from_extension("php"), Language::Php);
        assert_eq!(Language::from_extension("rb"), Language::Ruby);
        assert_eq!(Language::from_extension("swift"), Language::Swift);
        assert_eq!(Language::from_extension("kt"), Language::Kotlin);
        assert_eq!(Language::from_extension("kts"), Language::Kotlin);
        assert_eq!(Language::from_extension("unknown"), Language::Unknown);
        assert_eq!(Language::from_extension(""), Language::Unknown);
    }

    #[test]
    fn test_language_from_extension_case_insensitive() {
        assert_eq!(Language::from_extension("RS"), Language::Rust);
        assert_eq!(Language::from_extension("Ts"), Language::TypeScript);
        assert_eq!(Language::from_extension("PY"), Language::Python);
    }

    #[test]
    fn test_language_as_str() {
        assert_eq!(Language::Rust.as_str(), "rust");
        assert_eq!(Language::TypeScript.as_str(), "typescript");
        assert_eq!(Language::Cpp.as_str(), "cpp");
        assert_eq!(Language::Unknown.as_str(), "unknown");
    }

    // Visibility tests
    #[test]
    fn test_visibility_from_str() {
        assert_eq!(Visibility::parse("public"), Visibility::Public);
        assert_eq!(Visibility::parse("pub"), Visibility::Public);
        assert_eq!(Visibility::parse("private"), Visibility::Private);
        assert_eq!(Visibility::parse("priv"), Visibility::Private);
        assert_eq!(Visibility::parse("protected"), Visibility::Protected);
        assert_eq!(Visibility::parse("internal"), Visibility::Internal);
        assert_eq!(Visibility::parse("unknown"), Visibility::Unknown);
        assert_eq!(Visibility::parse(""), Visibility::Unknown);
    }

    #[test]
    fn test_visibility_as_str() {
        assert_eq!(Visibility::Public.as_str(), "public");
        assert_eq!(Visibility::Private.as_str(), "private");
        assert_eq!(Visibility::Protected.as_str(), "protected");
        assert_eq!(Visibility::Internal.as_str(), "internal");
        assert_eq!(Visibility::Unknown.as_str(), "unknown");
    }

    // TraversalOptions tests
    #[test]
    fn test_traversal_options_default() {
        let opts = TraversalOptions::default();
        assert_eq!(opts.max_depth, 2);
        assert_eq!(opts.limit, 50);
        assert!(opts.edge_kinds.is_none());
        assert!(opts.node_kinds.is_none());
    }

    // Node construction test
    #[test]
    fn test_node_creation() {
        let node = Node {
            id: 1,
            kind: NodeKind::Function,
            name: "test_fn".to_string(),
            qualified_name: Some("module::test_fn".to_string()),
            file_path: "src/lib.rs".to_string(),
            start_line: 10,
            end_line: 20,
            start_column: 0,
            end_column: 1,
            signature: Some("fn test_fn() -> bool".to_string()),
            visibility: Visibility::Public,
            docstring: Some("A test function".to_string()),
            is_async: false,
            is_static: false,
            is_exported: true,
            is_test: false,
            is_generated: false,
            language: Language::Rust,
        };

        assert_eq!(node.name, "test_fn");
        assert_eq!(node.kind, NodeKind::Function);
        assert_eq!(node.language, Language::Rust);
    }

    // Edge construction test
    #[test]
    fn test_edge_creation() {
        let edge = Edge {
            id: 1,
            source_id: 10,
            target_id: 20,
            kind: EdgeKind::Calls,
            file_path: Some("src/lib.rs".to_string()),
            line: Some(15),
            column: Some(4),
        };

        assert_eq!(edge.source_id, 10);
        assert_eq!(edge.target_id, 20);
        assert_eq!(edge.kind, EdgeKind::Calls);
    }

    // Serialization tests
    #[test]
    fn test_node_kind_serialization() {
        let kind = NodeKind::Function;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, "\"function\"");

        let parsed: NodeKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, NodeKind::Function);
    }

    #[test]
    fn test_edge_kind_serialization() {
        let kind = EdgeKind::Calls;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, "\"calls\"");

        let parsed: EdgeKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, EdgeKind::Calls);
    }

    #[test]
    fn test_language_serialization() {
        let lang = Language::TypeScript;
        let json = serde_json::to_string(&lang).unwrap();
        assert_eq!(json, "\"typescript\"");

        let parsed: Language = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, Language::TypeScript);
    }
}
