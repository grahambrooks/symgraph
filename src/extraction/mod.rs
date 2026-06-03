//! Code extraction module
//!
//! Uses tree-sitter to parse source code and extract:
//! - Symbols (functions, classes, methods, etc.)
//! - Relationships (calls, contains, imports, etc.)

mod languages;
pub mod manifest;

use std::path::Path;
use tree_sitter::Parser;

use crate::types::{
    Edge, EdgeKind, ExtractionError, ExtractionResult, Language, Node, NodeKind,
    UnresolvedReference, Visibility,
};

use languages::LanguageConfig;

/// Extracts code symbols from source files using tree-sitter
pub struct Extractor {
    parser: Parser,
}

impl Extractor {
    pub fn new() -> Self {
        Self {
            parser: Parser::new(),
        }
    }

    /// Extract symbols from a source file
    pub fn extract_file<P: AsRef<Path>>(&mut self, path: P, content: &str) -> ExtractionResult {
        let path = path.as_ref();

        // Check for package manager manifest files first (detected by filename)
        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if manifest::is_manifest_file(filename) {
            return manifest::extract_manifest(path, content);
        }

        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let language = Language::from_extension(ext);

        if language == Language::Unknown {
            return ExtractionResult {
                errors: vec![ExtractionError {
                    message: format!("Unsupported file extension: {}", ext),
                    file_path: path.display().to_string(),
                    line: None,
                    column: None,
                }],
                ..Default::default()
            };
        }

        let Some(ts_lang) = languages::get_language(language) else {
            return ExtractionResult {
                errors: vec![ExtractionError {
                    message: format!("No tree-sitter grammar for {:?}", language),
                    file_path: path.display().to_string(),
                    line: None,
                    column: None,
                }],
                ..Default::default()
            };
        };

        if self.parser.set_language(&ts_lang).is_err() {
            return ExtractionResult {
                errors: vec![ExtractionError {
                    message: "Failed to set parser language".to_string(),
                    file_path: path.display().to_string(),
                    line: None,
                    column: None,
                }],
                ..Default::default()
            };
        }

        let Some(tree) = self.parser.parse(content, None) else {
            return ExtractionResult {
                errors: vec![ExtractionError {
                    message: "Failed to parse file".to_string(),
                    file_path: path.display().to_string(),
                    line: None,
                    column: None,
                }],
                ..Default::default()
            };
        };

        let config = languages::get_config(language);
        let file_path = path.display().to_string();

        let mut ctx = ExtractionContext {
            result: ExtractionResult::default(),
            file_path: file_path.clone(),
            content,
            language,
            config,
            node_stack: Vec::new(),
            next_id: 1,
            file_is_test: false,
            file_is_generated: false,
            seen_refs: std::collections::HashSet::new(),
        };

        let file_is_test = is_test_path(&file_path);
        let file_is_generated = is_generated_path(&file_path) || is_generated_content(content);

        // Create file node
        let file_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let file_node = Node::builder(NodeKind::File, file_name, file_path.clone(), language)
            .id(ctx.next_id)
            .span(0, content.lines().count() as u32, 0, 0)
            .qualified_name(Some(file_path.clone()))
            .visibility(Visibility::Public)
            .is_exported(true)
            .is_test(file_is_test)
            .is_generated(file_is_generated)
            .build();
        ctx.file_is_test = file_is_test;
        ctx.file_is_generated = file_is_generated;
        ctx.next_id += 1;
        ctx.result.nodes.push(file_node);
        ctx.node_stack.push(1); // file node ID

        // Traverse the tree
        ctx.traverse_node(tree.root_node());

        ctx.result
    }
}

struct ExtractionContext<'a> {
    result: ExtractionResult,
    file_path: String,
    content: &'a str,
    language: Language,
    config: &'static LanguageConfig,
    node_stack: Vec<i64>, // Stack of parent node IDs
    next_id: i64,
    file_is_test: bool,
    file_is_generated: bool,
    /// Dedup set for structural edges (accesses/mutates/imports/dispatch),
    /// keyed by (source node, edge kind, target name). Calls are not deduped
    /// to preserve existing call-graph semantics.
    seen_refs: std::collections::HashSet<(i64, EdgeKind, String)>,
}

impl<'a> ExtractionContext<'a> {
    fn traverse_node<'tree>(&mut self, root: tree_sitter::Node<'tree>) {
        enum Work<'t> {
            Visit(tree_sitter::Node<'t>),
            PopStack,
        }

        fn push_children<'t>(node: tree_sitter::Node<'t>, work: &mut Vec<Work<'t>>) {
            let mut cursor = node.walk();
            let children: Vec<_> = node.children(&mut cursor).collect();
            for child in children.into_iter().rev() {
                work.push(Work::Visit(child));
            }
        }

        let mut work: Vec<Work<'tree>> = vec![Work::Visit(root)];

        while let Some(item) = work.pop() {
            match item {
                Work::PopStack => {
                    self.node_stack.pop();
                }
                Work::Visit(node) => {
                    let node_type = node.kind();
                    // Import edge: container (file/module) imports the named
                    // target. Handled here because import statements often have
                    // no extractable "name" and would otherwise be skipped.
                    if self.config.is_import_node(node_type) && !self.config.is_call_node(node_type)
                    {
                        let container_id = self.node_stack.last().copied().unwrap_or(1);
                        self.find_import_target(&node, container_id);
                    }
                    if let Some(kind) = self.config.node_type_to_kind(node_type) {
                        let name = self.extract_name(&node, kind);
                        if name.is_empty() {
                            push_children(node, &mut work);
                        } else {
                            self.emit_symbol(node, kind, name);
                            work.push(Work::PopStack);
                            push_children(node, &mut work);
                        }
                    } else {
                        push_children(node, &mut work);
                    }
                }
            }
        }
    }

    fn emit_symbol(&mut self, node: tree_sitter::Node, kind: NodeKind, name: String) {
        let start = node.start_position();
        let end = node.end_position();

        let is_test = self.file_is_test || self.is_test_symbol(&node, &name, kind);
        let symbol = Node::builder(kind, name.clone(), self.file_path.clone(), self.language)
            .id(self.next_id)
            .qualified_name(self.build_qualified_name(&name))
            .span(
                start.row as u32 + 1,
                end.row as u32 + 1,
                start.column as u32,
                end.column as u32,
            )
            .signature(self.extract_signature(&node, kind))
            .visibility(self.extract_visibility(&node))
            .docstring(self.extract_docstring(&node))
            .is_async(self.check_async(&node))
            .is_static(self.check_static(&node))
            .is_exported(self.check_exported(&node))
            .is_test(is_test)
            .is_generated(self.file_is_generated)
            .build();

        let symbol_id = self.next_id;
        self.next_id += 1;
        self.result.nodes.push(symbol);

        if let Some(&parent_id) = self.node_stack.last() {
            let edge = Edge::new(parent_id, symbol_id, EdgeKind::Contains).at(
                self.file_path.clone(),
                start.row as u32 + 1,
                start.column as u32,
            );
            self.result.edges.push(edge);
        }

        // `&mut T` parameters are an intrusive/common-coupling signal.
        if matches!(kind, NodeKind::Function | NodeKind::Method) {
            self.find_mut_params(&node, symbol_id);
        }

        self.node_stack.push(symbol_id);
        self.find_references(node, symbol_id);
    }

    fn extract_name(&self, node: &tree_sitter::Node, _kind: NodeKind) -> String {
        // Try to find name child
        for field_name in &["name", "declarator", "identifier"] {
            if let Some(name_node) = node.child_by_field_name(field_name) {
                let name = self.get_node_text(&name_node);
                if !name.is_empty() {
                    // Handle pointer declarators in C/C++
                    if name_node.kind() == "pointer_declarator"
                        || name_node.kind() == "function_declarator"
                    {
                        if let Some(id) = name_node.child_by_field_name("declarator") {
                            return self.get_node_text(&id);
                        }
                    }
                    return name;
                }
            }
        }

        // For some languages, look at specific child positions
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "identifier" || child.kind() == "type_identifier" {
                return self.get_node_text(&child);
            }
        }

        String::new()
    }

    fn extract_signature(&self, node: &tree_sitter::Node, kind: NodeKind) -> Option<String> {
        match kind {
            NodeKind::Function | NodeKind::Method => {
                // Get the first line or until opening brace
                let text = self.get_node_text(node);
                let sig = text.lines().next().unwrap_or("");
                // Truncate at opening brace or newline
                let sig = sig.split('{').next().unwrap_or(sig).trim();
                if sig.len() > 200 {
                    let boundary = (0..=200)
                        .rev()
                        .find(|&i| sig.is_char_boundary(i))
                        .unwrap_or(0);
                    Some(format!("{}...", &sig[..boundary]))
                } else {
                    Some(sig.to_string())
                }
            }
            NodeKind::Class | NodeKind::Struct | NodeKind::Interface | NodeKind::Trait => {
                let text = self.get_node_text(node);
                let sig = text.lines().next().unwrap_or("");
                let sig = sig.split('{').next().unwrap_or(sig).trim();
                Some(sig.to_string())
            }
            _ => None,
        }
    }

    fn extract_visibility(&self, node: &tree_sitter::Node) -> Visibility {
        let text = self.get_node_text(node);
        let first_line = text.lines().next().unwrap_or("");

        if first_line.starts_with("pub ") || first_line.contains(" pub ") {
            return Visibility::Public;
        }
        if first_line.starts_with("public ") || first_line.contains(" public ") {
            return Visibility::Public;
        }
        if first_line.starts_with("private ") || first_line.contains(" private ") {
            return Visibility::Private;
        }
        if first_line.starts_with("protected ") || first_line.contains(" protected ") {
            return Visibility::Protected;
        }

        // Check for visibility modifier child
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            let kind = child.kind();
            if kind == "visibility_modifier" || kind == "access_specifier" {
                let vis_text = self.get_node_text(&child);
                return Visibility::parse(&vis_text);
            }
        }

        // Check for export keyword (JS/TS)
        if let Some(parent) = node.parent() {
            if parent.kind() == "export_statement" {
                return Visibility::Public;
            }
        }

        // Language-specific defaults
        match self.language {
            Language::Rust => Visibility::Private,
            Language::Go | Language::Python => Visibility::Public,
            Language::TypeScript | Language::JavaScript | Language::Tsx | Language::Jsx => {
                // In JS/TS, top-level functions without export are module-private
                Visibility::Private
            }
            _ => Visibility::Unknown,
        }
    }

    fn extract_docstring(&self, node: &tree_sitter::Node) -> Option<String> {
        // Walk backward through consecutive comment siblings so multi-line
        // doc blocks (`///`, `//!`, `#`, JSDoc) are captured as one docstring.
        let mut comments: Vec<String> = Vec::new();
        let mut current = node.prev_sibling();
        while let Some(prev) = current {
            let kind = prev.kind();
            let is_comment = kind.contains("comment")
                || kind == "doc_comment"
                || kind == "block_comment"
                || kind == "line_comment";
            if !is_comment {
                break;
            }
            comments.push(self.get_node_text(&prev));
            current = prev.prev_sibling();
        }
        if comments.is_empty() {
            // Python-style: docstring is the first string literal inside the body.
            if let Some(body) = node.child_by_field_name("body") {
                let mut cursor = body.walk();
                for child in body.children(&mut cursor) {
                    let k = child.kind();
                    if k == "expression_statement" || k == "string" {
                        let text = self.get_node_text(&child);
                        if text.starts_with("\"\"\"")
                            || text.starts_with("'''")
                            || text.starts_with("r\"\"\"")
                        {
                            return Some(self.clean_docstring(&text));
                        }
                        break;
                    }
                }
            }
            return None;
        }
        comments.reverse();
        Some(self.clean_docstring(&comments.join("\n")))
    }

    fn is_test_symbol(&self, node: &tree_sitter::Node, name: &str, kind: NodeKind) -> bool {
        if !matches!(
            kind,
            NodeKind::Function | NodeKind::Method | NodeKind::Class
        ) {
            return false;
        }
        if test_name_heuristic(name) {
            return true;
        }
        // Walk back through preceding sibling annotations/attributes/decorators.
        let mut prev = node.prev_sibling();
        while let Some(p) = prev {
            let text = self.get_node_text(&p);
            let trimmed = text.trim();
            if trimmed.contains("#[test]")
                || trimmed.contains("#[tokio::test]")
                || trimmed.contains("#[cfg(test)]")
                || trimmed.contains("@Test")
                || trimmed.contains("@pytest.")
            {
                return true;
            }
            // Stop walking once we leave attribute/decorator/annotation territory.
            let k = p.kind();
            if !(k.contains("attribute")
                || k.contains("decorator")
                || k.contains("annotation")
                || k.contains("comment"))
            {
                break;
            }
            prev = p.prev_sibling();
        }
        false
    }

    fn clean_docstring(&self, text: &str) -> String {
        text.lines()
            .map(|line| {
                line.trim()
                    .trim_start_matches("///")
                    .trim_start_matches("//!")
                    .trim_start_matches("//")
                    .trim_start_matches("/**")
                    .trim_start_matches("/*")
                    .trim_start_matches('*')
                    .trim_end_matches("*/")
                    .trim()
            })
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn check_async(&self, node: &tree_sitter::Node) -> bool {
        let text = self.get_node_text(node);
        text.starts_with("async ") || text.contains(" async ")
    }

    fn check_static(&self, node: &tree_sitter::Node) -> bool {
        let text = self.get_node_text(node);
        text.starts_with("static ") || text.contains(" static ")
    }

    fn check_exported(&self, node: &tree_sitter::Node) -> bool {
        let text = self.get_node_text(node);
        // Rust pub
        if text.starts_with("pub ") {
            return true;
        }
        // JS/TS export
        if text.starts_with("export ") {
            return true;
        }
        // Check for export default
        if let Some(parent) = node.parent() {
            if parent.kind() == "export_statement" {
                return true;
            }
        }
        false
    }

    fn build_qualified_name(&self, name: &str) -> Option<String> {
        let mut parts = Vec::new();
        for &parent_id in &self.node_stack {
            if let Some(parent) = self.result.nodes.iter().find(|n| n.id == parent_id) {
                if parent.kind != NodeKind::File {
                    parts.push(parent.name.clone());
                }
            }
        }
        parts.push(name.to_string());
        Some(parts.join("::"))
    }

    /// Walk a symbol's body iteratively (stack-safe on deeply nested ASTs) and
    /// record references:
    /// - `Calls` for call expressions,
    /// - `Accesses` / `Mutates` for field reads / writes,
    /// - `References` (detail "dispatch") for enum variants inside match/switch.
    fn find_references<'tree>(&mut self, root: tree_sitter::Node<'tree>, source_id: i64) {
        let mut stack: Vec<tree_sitter::Node<'tree>> = vec![root];
        while let Some(node) = stack.pop() {
            let kind = node.kind();

            if self.config.is_call_node(kind) {
                if let Some(func_name) = self.extract_call_name(&node) {
                    self.push_ref(source_id, func_name, EdgeKind::Calls, &node, None);
                }
            } else if self.config.is_field_access_node(kind) && !self.is_call_callee(&node) {
                if let Some(field_name) = self.extract_field_name(&node) {
                    let edge_kind = if self.is_write_target(&node) {
                        EdgeKind::Mutates
                    } else {
                        EdgeKind::Accesses
                    };
                    self.push_ref(source_id, field_name, edge_kind, &node, None);
                }
            }

            // Scattered enum dispatch: scan match/switch subtrees for qualified
            // variant references (e.g. `ViewKind::List`).
            if self.config.is_enum_match_node(kind) {
                self.find_enum_dispatch(&node, source_id);
            }

            let mut cursor = node.walk();
            let children: Vec<_> = node.children(&mut cursor).collect();
            for child in children.into_iter().rev() {
                stack.push(child);
            }
        }
    }

    /// Push an unresolved reference, deduping structural (non-call) edges per
    /// (source, kind, name) so common field reads don't explode the graph.
    fn push_ref(
        &mut self,
        source_id: i64,
        name: String,
        kind: EdgeKind,
        node: &tree_sitter::Node,
        detail: Option<String>,
    ) {
        if name.is_empty() {
            return;
        }
        if kind != EdgeKind::Calls && !self.seen_refs.insert((source_id, kind, name.clone())) {
            return;
        }
        let start = node.start_position();
        self.result.unresolved_refs.push(UnresolvedReference {
            source_node_id: source_id,
            reference_name: name,
            kind,
            file_path: self.file_path.clone(),
            line: start.row as u32 + 1,
            column: start.column as u32,
            detail,
        });
    }

    /// True if `node` is the callee (function position) of a call expression,
    /// so a method call `obj.method()` isn't also counted as a field read.
    fn is_call_callee(&self, node: &tree_sitter::Node) -> bool {
        if let Some(parent) = node.parent() {
            if self.config.is_call_node(parent.kind()) {
                if let Some(func) = parent.child_by_field_name("function") {
                    return func.id() == node.id();
                }
            }
        }
        false
    }

    /// True if `node` is the left-hand side of an assignment (a field write).
    fn is_write_target(&self, node: &tree_sitter::Node) -> bool {
        if let Some(parent) = node.parent() {
            if self.config.is_assignment_node(parent.kind()) {
                if let Some(left) = parent.child_by_field_name("left") {
                    return left.id() == node.id();
                }
            }
        }
        false
    }

    /// Extract the accessed member identifier from a field-access node.
    fn extract_field_name(&self, node: &tree_sitter::Node) -> Option<String> {
        let field = self.config.field_name_field;
        let child = node.child_by_field_name(field)?;
        let text = self.get_node_text(&child);
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    /// Scan a match/switch construct for qualified enum variant references
    /// (`Type::Variant`) and record them as `References` dispatch edges.
    fn find_enum_dispatch(&mut self, node: &tree_sitter::Node, source_id: i64) {
        if node.kind() == "scoped_identifier" {
            let text = self.get_node_text(node);
            if let Some(pos) = text.rfind("::") {
                let variant = text[pos + 2..].trim().to_string();
                self.push_ref(
                    source_id,
                    variant,
                    EdgeKind::References,
                    node,
                    Some("dispatch".to_string()),
                );
            }
            return;
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.find_enum_dispatch(&child, source_id);
        }
    }

    /// Emit an `Imports` edge from `container_id` to the imported target.
    /// Glob imports (`use x::*`, `from x import *`) carry detail "glob".
    fn find_import_target(&mut self, node: &tree_sitter::Node, container_id: i64) {
        let text = self.get_node_text(node);
        let is_glob = text.contains('*');
        let tokens: Vec<&str> = text
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .filter(|t| {
                !t.is_empty()
                    && !matches!(
                        *t,
                        "use" | "import" | "from" | "as" | "pub" | "crate" | "self" | "super"
                    )
            })
            .collect();
        if let Some(&name) = tokens.last() {
            let detail = if is_glob {
                Some("glob".to_string())
            } else {
                None
            };
            self.push_ref(
                container_id,
                name.to_string(),
                EdgeKind::Imports,
                node,
                detail,
            );
        }
    }

    /// Detect `&mut T` parameters (Rust) and record a `Mutates` edge from the
    /// function to the borrowed type — a common-coupling signal.
    fn find_mut_params(&mut self, node: &tree_sitter::Node, source_id: i64) {
        if self.language != Language::Rust {
            return;
        }
        let Some(params) = node.child_by_field_name("parameters") else {
            return;
        };
        let mut cursor = params.walk();
        for param in params.children(&mut cursor) {
            if let Some(ty) = param.child_by_field_name("type") {
                if ty.kind() == "reference_type" && self.has_mut_specifier(&ty) {
                    if let Some(inner) = ty.child_by_field_name("type") {
                        if let Some(name) = self.type_base_name(&inner) {
                            self.push_ref(
                                source_id,
                                name,
                                EdgeKind::Mutates,
                                &ty,
                                Some("mut_param".to_string()),
                            );
                        }
                    }
                }
            }
        }
    }

    fn has_mut_specifier(&self, node: &tree_sitter::Node) -> bool {
        let mut cursor = node.walk();
        let found = node
            .children(&mut cursor)
            .any(|c| c.kind() == "mutable_specifier");
        found
    }

    /// Base type name from a type node, dropping path prefix and generics:
    /// `crate::model::Model<T>` -> `Model`.
    fn type_base_name(&self, node: &tree_sitter::Node) -> Option<String> {
        let text = self.get_node_text(node);
        let base = text.split('<').next().unwrap_or(&text);
        let seg = base.rsplit("::").next().unwrap_or(base).trim();
        if seg.is_empty() {
            None
        } else {
            Some(seg.to_string())
        }
    }

    fn extract_call_name(&self, node: &tree_sitter::Node) -> Option<String> {
        // Look for function name in call expression
        if let Some(func) = node.child_by_field_name("function") {
            let text = self.get_node_text(&func);
            // Handle method calls: obj.method() -> method
            if let Some(dot_pos) = text.rfind('.') {
                return Some(text[dot_pos + 1..].to_string());
            }
            // Handle path calls: foo::bar() -> bar
            if let Some(colon_pos) = text.rfind("::") {
                return Some(text[colon_pos + 2..].to_string());
            }
            return Some(text);
        }

        // Try first child as fallback
        if let Some(first) = node.child(0) {
            if first.kind() == "identifier" || first.kind() == "field_expression" {
                let text = self.get_node_text(&first);
                if let Some(dot_pos) = text.rfind('.') {
                    return Some(text[dot_pos + 1..].to_string());
                }
                return Some(text);
            }
        }

        None
    }

    fn get_node_text(&self, node: &tree_sitter::Node) -> String {
        let start = node.start_byte();
        let end = node.end_byte();
        if start < self.content.len() && end <= self.content.len() {
            self.content[start..end].to_string()
        } else {
            String::new()
        }
    }
}

impl Default for Extractor {
    fn default() -> Self {
        Self::new()
    }
}

/// Heuristic: does the given symbol name look like a test?
///
/// Catches `test_*`, `*_test`, `test*Foo` (camelCase), and exact `test` /
/// `Test` class names. Intentionally permissive — false positives here just
/// exclude a symbol from `unused`/test-edge logic, which is the safe default.
fn test_name_heuristic(name: &str) -> bool {
    let lower = name.to_lowercase();
    if lower == "test" || lower == "tests" {
        return true;
    }
    if lower.starts_with("test_") || lower.ends_with("_test") || lower.ends_with("_tests") {
        return true;
    }
    let after_test = name
        .strip_prefix("test")
        .or_else(|| name.strip_prefix("Test"));
    if let Some(rest) = after_test {
        if rest.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            return true;
        }
    }
    if let Some(prefix) = name.strip_suffix("Test") {
        if !prefix.is_empty() {
            return true;
        }
    }
    false
}

/// Heuristic: does this file path look like test code?
fn is_test_path(file_path: &str) -> bool {
    let p = format!("/{}", file_path.replace('\\', "/").to_lowercase());
    p.contains("/tests/")
        || p.contains("/test/")
        || p.contains("/__tests__/")
        || p.contains("/spec/")
        || p.ends_with("_test.go")
        || p.ends_with(".test.ts")
        || p.ends_with(".test.tsx")
        || p.ends_with(".test.js")
        || p.ends_with(".test.jsx")
        || p.ends_with(".spec.ts")
        || p.ends_with(".spec.js")
        || p.ends_with("_spec.rb")
        || p.ends_with("_test.rb")
}

/// Heuristic: does this file path look like generated code?
fn is_generated_path(file_path: &str) -> bool {
    let p = file_path.replace('\\', "/").to_lowercase();
    p.contains("/generated/")
        || p.contains("/.generated/")
        || p.contains("/gen/")
        || p.ends_with(".pb.go")
        || p.ends_with(".pb.cc")
        || p.ends_with(".pb.h")
        || p.ends_with("_pb2.py")
        || p.ends_with(".g.dart")
        || p.ends_with(".freezed.dart")
}

/// Heuristic: does the file content begin with a generated-code marker?
fn is_generated_content(content: &str) -> bool {
    let head: String = content.chars().take(512).collect();
    let lower = head.to_lowercase();
    lower.contains("do not edit")
        || lower.contains("auto-generated")
        || lower.contains("autogenerated")
        || lower.contains("generated by")
        || lower.contains("@generated")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extractor_creation() {
        let extractor = Extractor::new();
        assert!(std::mem::size_of_val(&extractor) > 0);
    }

    #[test]
    fn test_extract_unsupported_extension() {
        let mut extractor = Extractor::new();
        let result = extractor.extract_file("test.xyz", "some content");
        assert!(!result.errors.is_empty());
        assert!(result.errors[0]
            .message
            .contains("Unsupported file extension"));
    }

    // Rust extraction tests
    #[test]
    fn test_extract_rust_function() {
        let mut extractor = Extractor::new();
        let code = r#"
fn hello_world() {
    println!("Hello, world!");
}
"#;
        let result = extractor.extract_file("test.rs", code);
        assert!(result.errors.is_empty());

        // Should have file node + function node
        assert!(result.nodes.len() >= 2);

        let func = result.nodes.iter().find(|n| n.name == "hello_world");
        assert!(func.is_some());
        let func = func.unwrap();
        assert_eq!(func.kind, NodeKind::Function);
        assert_eq!(func.language, Language::Rust);
    }

    #[test]
    fn test_extract_rust_pub_function() {
        let mut extractor = Extractor::new();
        let code = r#"
pub fn public_function() -> i32 {
    42
}
"#;
        let result = extractor.extract_file("test.rs", code);
        let func = result
            .nodes
            .iter()
            .find(|n| n.name == "public_function")
            .unwrap();
        assert_eq!(func.visibility, Visibility::Public);
        assert!(func.signature.is_some());
    }

    #[test]
    fn test_extract_rust_struct() {
        let mut extractor = Extractor::new();
        let code = r#"
pub struct MyStruct {
    field1: i32,
    field2: String,
}
"#;
        let result = extractor.extract_file("test.rs", code);
        let strukt = result.nodes.iter().find(|n| n.name == "MyStruct");
        assert!(strukt.is_some());
        assert_eq!(strukt.unwrap().kind, NodeKind::Struct);
    }

    #[test]
    fn test_extract_rust_impl_methods() {
        let mut extractor = Extractor::new();
        let code = r#"
struct Foo {}

impl Foo {
    fn bar(&self) {
    }

    pub fn baz() -> Self {
        Foo {}
    }
}
"#;
        let result = extractor.extract_file("test.rs", code);

        // Should find struct and methods
        assert!(result.nodes.iter().any(|n| n.name == "Foo"));
        assert!(result.nodes.iter().any(|n| n.name == "bar"));
        assert!(result.nodes.iter().any(|n| n.name == "baz"));
    }

    #[test]
    fn test_extract_rust_enum() {
        let mut extractor = Extractor::new();
        let code = r#"
enum Color {
    Red,
    Green,
    Blue,
}
"#;
        let result = extractor.extract_file("test.rs", code);
        let enum_node = result.nodes.iter().find(|n| n.name == "Color");
        assert!(enum_node.is_some());
        assert_eq!(enum_node.unwrap().kind, NodeKind::Enum);
    }

    #[test]
    fn test_extract_rust_function_calls() {
        let mut extractor = Extractor::new();
        let code = r#"
fn caller() {
    helper();
    other_func();
}

fn helper() {}
fn other_func() {}
"#;
        let result = extractor.extract_file("test.rs", code);

        // Should have unresolved references for the calls
        assert!(!result.unresolved_refs.is_empty());
        assert!(result
            .unresolved_refs
            .iter()
            .any(|r| r.reference_name == "helper"));
        assert!(result
            .unresolved_refs
            .iter()
            .any(|r| r.reference_name == "other_func"));
    }

    #[test]
    fn test_extract_rust_async_function() {
        let mut extractor = Extractor::new();
        let code = r#"
async fn async_handler() {
    do_something().await;
}
"#;
        let result = extractor.extract_file("test.rs", code);
        let func = result
            .nodes
            .iter()
            .find(|n| n.name == "async_handler")
            .unwrap();
        assert!(func.is_async);
    }

    // TypeScript extraction tests
    #[test]
    fn test_extract_typescript_function() {
        let mut extractor = Extractor::new();
        let code = r#"
function greet(name: string): string {
    return `Hello, ${name}!`;
}
"#;
        let result = extractor.extract_file("test.ts", code);
        assert!(result.errors.is_empty());

        let func = result.nodes.iter().find(|n| n.name == "greet");
        assert!(func.is_some());
        assert_eq!(func.unwrap().language, Language::TypeScript);
    }

    #[test]
    fn test_extract_typescript_class() {
        let mut extractor = Extractor::new();
        let code = r#"
class MyClass {
    private value: number;

    constructor(val: number) {
        this.value = val;
    }

    getValue(): number {
        return this.value;
    }
}
"#;
        let result = extractor.extract_file("test.ts", code);
        let class = result.nodes.iter().find(|n| n.name == "MyClass");
        assert!(class.is_some());
        assert_eq!(class.unwrap().kind, NodeKind::Class);
    }

    #[test]
    fn test_extract_typescript_interface() {
        let mut extractor = Extractor::new();
        let code = r#"
interface User {
    name: string;
    age: number;
}
"#;
        let result = extractor.extract_file("test.ts", code);
        let iface = result.nodes.iter().find(|n| n.name == "User");
        assert!(iface.is_some());
        assert_eq!(iface.unwrap().kind, NodeKind::Interface);
    }

    #[test]
    fn test_extract_typescript_arrow_function() {
        let mut extractor = Extractor::new();
        let code = r#"
const add = (a: number, b: number): number => a + b;
"#;
        let result = extractor.extract_file("test.ts", code);
        // Arrow functions are typically extracted as constants or variables
        assert!(!result.nodes.is_empty());
    }

    // Python extraction tests
    #[test]
    fn test_extract_python_function() {
        let mut extractor = Extractor::new();
        let code = r#"
def hello_world():
    print("Hello, world!")
"#;
        let result = extractor.extract_file("test.py", code);
        assert!(result.errors.is_empty());

        let func = result.nodes.iter().find(|n| n.name == "hello_world");
        assert!(func.is_some());
        assert_eq!(func.unwrap().language, Language::Python);
    }

    #[test]
    fn test_extract_python_class() {
        let mut extractor = Extractor::new();
        let code = r#"
class MyClass:
    def __init__(self, value):
        self.value = value

    def get_value(self):
        return self.value
"#;
        let result = extractor.extract_file("test.py", code);
        let class = result.nodes.iter().find(|n| n.name == "MyClass");
        assert!(class.is_some());
        assert_eq!(class.unwrap().kind, NodeKind::Class);

        // Should also find methods
        assert!(result.nodes.iter().any(|n| n.name == "__init__"));
        assert!(result.nodes.iter().any(|n| n.name == "get_value"));
    }

    #[test]
    fn test_extract_python_async_function() {
        let mut extractor = Extractor::new();
        let code = r#"
async def fetch_data():
    await some_async_call()
"#;
        let result = extractor.extract_file("test.py", code);
        let func = result
            .nodes
            .iter()
            .find(|n| n.name == "fetch_data")
            .unwrap();
        assert!(func.is_async);
    }

    // JavaScript extraction tests
    #[test]
    fn test_extract_javascript_function() {
        let mut extractor = Extractor::new();
        let code = r#"
function processData(data) {
    return data.map(x => x * 2);
}
"#;
        let result = extractor.extract_file("test.js", code);
        let func = result.nodes.iter().find(|n| n.name == "processData");
        assert!(func.is_some());
        assert_eq!(func.unwrap().language, Language::JavaScript);
    }

    // Go extraction tests
    #[test]
    fn test_extract_go_function() {
        let mut extractor = Extractor::new();
        let code = r#"
func main() {
    fmt.Println("Hello, World!")
}

func helper(x int) int {
    return x * 2
}
"#;
        let result = extractor.extract_file("test.go", code);
        assert!(result.nodes.iter().any(|n| n.name == "main"));
        assert!(result.nodes.iter().any(|n| n.name == "helper"));
    }

    #[test]
    fn test_extract_go_struct() {
        let mut extractor = Extractor::new();
        let code = r#"
type Person struct {
    Name string
    Age  int
}
"#;
        let result = extractor.extract_file("test.go", code);
        // Go type declarations may be extracted differently depending on grammar
        // At minimum we should have the file node and no errors
        assert!(result.errors.is_empty());
        assert!(result.nodes.iter().any(|n| n.kind == NodeKind::File));
    }

    // Contains edge tests
    #[test]
    fn test_contains_edges() {
        let mut extractor = Extractor::new();
        let code = r#"
fn outer() {
    fn inner() {}
}
"#;
        let result = extractor.extract_file("test.rs", code);

        // Should have contains edges
        let contains_edges: Vec<_> = result
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Contains)
            .collect();
        assert!(!contains_edges.is_empty());
    }

    // File node tests
    #[test]
    fn test_file_node_created() {
        let mut extractor = Extractor::new();
        let result = extractor.extract_file("mymodule.rs", "fn foo() {}");

        let file_node = result.nodes.iter().find(|n| n.kind == NodeKind::File);
        assert!(file_node.is_some());
        assert_eq!(file_node.unwrap().name, "mymodule.rs");
    }

    // Empty file test
    #[test]
    fn test_extract_empty_file() {
        let mut extractor = Extractor::new();
        let result = extractor.extract_file("empty.rs", "");
        assert!(result.errors.is_empty());
        // Should still have a file node
        assert!(result.nodes.iter().any(|n| n.kind == NodeKind::File));
    }

    // Line number tests
    #[test]
    fn test_line_numbers() {
        let mut extractor = Extractor::new();
        let code = r#"
fn first() {}

fn second() {}

fn third() {}
"#;
        let result = extractor.extract_file("test.rs", code);

        let first = result.nodes.iter().find(|n| n.name == "first").unwrap();
        let second = result.nodes.iter().find(|n| n.name == "second").unwrap();
        let third = result.nodes.iter().find(|n| n.name == "third").unwrap();

        assert!(first.start_line < second.start_line);
        assert!(second.start_line < third.start_line);
    }

    #[test]
    fn test_extract_csharp_class() {
        let mut extractor = Extractor::new();
        let code = r#"
namespace MyApp
{
    public class Program
    {
        public static void Main(string[] args)
        {
            Console.WriteLine("Hello, World!");
        }

        private int Add(int x, int y)
        {
            return x + y;
        }
    }
}
"#;
        let result = extractor.extract_file("test.cs", code);
        assert!(result.errors.is_empty());
        assert!(result
            .nodes
            .iter()
            .any(|n| n.name == "Program" && n.kind == NodeKind::Class));
        assert!(result
            .nodes
            .iter()
            .any(|n| n.name == "Main" && n.kind == NodeKind::Method));
        assert!(result
            .nodes
            .iter()
            .any(|n| n.name == "Add" && n.kind == NodeKind::Method));
        assert!(result
            .nodes
            .iter()
            .any(|n| n.name == "MyApp" && n.kind == NodeKind::Module));
    }

    #[test]
    fn test_extract_csharp_interface() {
        let mut extractor = Extractor::new();
        let code = r#"
public interface IRepository
{
    void Save();
    int GetCount();
}
"#;
        let result = extractor.extract_file("test.cs", code);
        assert!(result.errors.is_empty());
        assert!(result
            .nodes
            .iter()
            .any(|n| n.name == "IRepository" && n.kind == NodeKind::Interface));
    }

    // Kotlin extraction tests
    #[test]
    fn test_extract_kotlin_function() {
        let mut extractor = Extractor::new();
        let code = r#"
package com.example

fun greet(name: String): String {
    return "Hello, $name!"
}
"#;
        let result = extractor.extract_file("test.kt", code);
        assert!(result.errors.is_empty());
        let func = result.nodes.iter().find(|n| n.name == "greet");
        assert!(func.is_some());
        assert_eq!(func.unwrap().language, Language::Kotlin);
    }

    #[test]
    fn test_extract_kotlin_class() {
        let mut extractor = Extractor::new();
        let code = r#"
class Person(val name: String, val age: Int) {
    fun greet(): String {
        return "Hello, I'm $name"
    }
}
"#;
        let result = extractor.extract_file("test.kt", code);
        assert!(result.errors.is_empty());
        assert!(result
            .nodes
            .iter()
            .any(|n| n.name == "Person" && n.kind == NodeKind::Class));
        assert!(result.nodes.iter().any(|n| n.name == "greet"));
    }

    #[test]
    fn test_extract_kotlin_object() {
        let mut extractor = Extractor::new();
        let code = r#"
object Singleton {
    fun getInstance(): Singleton = this
}
"#;
        let result = extractor.extract_file("test.kt", code);
        assert!(result.errors.is_empty());
        assert!(result
            .nodes
            .iter()
            .any(|n| n.name == "Singleton" && n.kind == NodeKind::Class));
    }

    // Scala extraction tests
    #[test]
    fn test_extract_scala_function() {
        let mut extractor = Extractor::new();
        let code = r#"
object Main {
  def hello(name: String): String = {
    s"Hello, $name!"
  }
}
"#;
        let result = extractor.extract_file("test.scala", code);
        assert!(result.errors.is_empty());
        assert!(result.nodes.iter().any(|n| n.name == "hello"));
        assert!(result
            .nodes
            .iter()
            .any(|n| n.name == "Main" && n.kind == NodeKind::Class));
    }

    #[test]
    fn test_extract_scala_trait() {
        let mut extractor = Extractor::new();
        let code = r#"
trait Greeter {
  def greet(name: String): String
}
"#;
        let result = extractor.extract_file("test.scala", code);
        assert!(result.errors.is_empty());
        assert!(result
            .nodes
            .iter()
            .any(|n| n.name == "Greeter" && n.kind == NodeKind::Interface));
    }

    #[test]
    fn test_extract_scala_class() {
        let mut extractor = Extractor::new();
        let code = r#"
class Person(val name: String, val age: Int) {
  def greet(): String = s"Hello, I'm $name"
}
"#;
        let result = extractor.extract_file("test.scala", code);
        assert!(result.errors.is_empty());
        assert!(result
            .nodes
            .iter()
            .any(|n| n.name == "Person" && n.kind == NodeKind::Class));
    }

    // Groovy extraction tests
    #[test]
    fn test_extract_groovy_class() {
        let mut extractor = Extractor::new();
        let code = r#"
class Calculator {
    int add(int a, int b) {
        return a + b
    }

    static void main(String[] args) {
        println new Calculator().add(1, 2)
    }
}
"#;
        let result = extractor.extract_file("test.groovy", code);
        assert!(result.errors.is_empty());
        assert!(result
            .nodes
            .iter()
            .any(|n| n.name == "Calculator" && n.kind == NodeKind::Class));
        assert!(result
            .nodes
            .iter()
            .any(|n| n.name == "add" && n.kind == NodeKind::Method));
    }

    #[test]
    fn test_extract_groovy_interface() {
        let mut extractor = Extractor::new();
        let code = r#"
interface Greeter {
    String greet(String name)
}
"#;
        let result = extractor.extract_file("test.groovy", code);
        assert!(result.errors.is_empty());
        assert!(result
            .nodes
            .iter()
            .any(|n| n.name == "Greeter" && n.kind == NodeKind::Interface));
    }

    #[test]
    fn test_extract_ruby_class_and_method() {
        let mut extractor = Extractor::new();
        let code = r#"
class Greeter
  def greet(name)
    "Hello, #{name}"
  end
end
"#;
        let result = extractor.extract_file("greeter.rb", code);
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert!(result
            .nodes
            .iter()
            .any(|n| n.name == "Greeter" && n.kind == NodeKind::Class));
        assert!(result.nodes.iter().any(|n| n.name == "greet"));
    }

    #[test]
    fn test_is_test_path_detection() {
        assert!(is_test_path("src/tests/foo.rs"));
        assert!(is_test_path("foo/__tests__/bar.ts"));
        assert!(is_test_path("pkg/foo_test.go"));
        assert!(is_test_path("a/b.test.tsx"));
        assert!(!is_test_path("src/lib/foo.rs"));
    }

    #[test]
    fn test_is_generated_content_marker() {
        assert!(is_generated_content(
            "// Code generated by protoc. DO NOT EDIT.\nfn foo(){}"
        ));
        assert!(is_generated_content("# @generated\nclass Foo:\n    pass\n"));
        assert!(!is_generated_content("fn foo() { 1 + 1 }"));
    }

    #[test]
    fn test_deeply_nested_does_not_overflow() {
        // Regression test: the old recursive traverse_node would stack-overflow
        // on ASTs deeper than ~500 levels. The iterative rewrite must handle this.
        let mut extractor = Extractor::new();
        let depth = 500usize;
        let mut code = String::new();
        for i in 0..depth {
            code.push_str(&format!("fn f{}(){{", i));
        }
        code.push_str(&"}".repeat(depth));
        let result = extractor.extract_file("deep.rs", &code);
        assert!(result.errors.is_empty());
        assert_eq!(
            result
                .nodes
                .iter()
                .filter(|n| n.kind == NodeKind::Function)
                .count(),
            depth
        );
    }

    #[test]
    fn test_test_name_heuristic() {
        assert!(test_name_heuristic("test_user_login"));
        assert!(test_name_heuristic("TestUserLogin"));
        assert!(test_name_heuristic("UserLoginTest"));
        assert!(test_name_heuristic("user_login_test"));
        assert!(!test_name_heuristic("login_user"));
        assert!(!test_name_heuristic("attest"));
    }

    #[test]
    fn test_extract_rust_test_attribute_marks_is_test() {
        let mut extractor = Extractor::new();
        let code = r#"
#[test]
fn checks_addition() {
    assert_eq!(1 + 1, 2);
}
"#;
        let result = extractor.extract_file("src/lib.rs", code);
        let func = result
            .nodes
            .iter()
            .find(|n| n.name == "checks_addition")
            .expect("function");
        assert!(func.is_test, "should detect #[test]-attributed function");
    }

    #[test]
    fn test_test_path_propagates_to_symbols() {
        let mut extractor = Extractor::new();
        let result =
            extractor.extract_file("tests/integration.rs", "fn helper() {}\nfn other() {}\n");
        let helper = result.nodes.iter().find(|n| n.name == "helper").unwrap();
        assert!(helper.is_test, "symbols in tests/ should inherit is_test");
    }

    // Manifest file routing tests
    #[test]
    fn test_extract_manifest_via_extractor() {
        let mut extractor = Extractor::new();
        let content = r#"{"name": "test-pkg", "dependencies": {"express": "^4.0"}}"#;
        let result = extractor.extract_file("package.json", content);
        assert!(result.errors.is_empty());
        assert!(result
            .nodes
            .iter()
            .any(|n| n.name == "test-pkg" && n.kind == NodeKind::Module));
        assert!(result
            .nodes
            .iter()
            .any(|n| n.name == "express" && n.kind == NodeKind::Import));
    }

    // ---- New coupling edges (accesses / mutates / imports / dispatch) ----

    fn refs_of(result: &ExtractionResult, kind: EdgeKind) -> Vec<String> {
        result
            .unresolved_refs
            .iter()
            .filter(|r| r.kind == kind)
            .map(|r| r.reference_name.clone())
            .collect()
    }

    #[test]
    fn test_rust_field_read_and_write() {
        let mut extractor = Extractor::new();
        let code = r#"
struct Model { count: u32, name: String }
fn work(m: &Model) {
    let _ = m.name;
}
fn mutate(m: &mut Model) {
    m.count = 5;
}
"#;
        let result = extractor.extract_file("test.rs", code);
        let reads = refs_of(&result, EdgeKind::Accesses);
        let writes = refs_of(&result, EdgeKind::Mutates);
        assert!(reads.contains(&"name".to_string()), "reads={:?}", reads);
        assert!(writes.contains(&"count".to_string()), "writes={:?}", writes);
        // `count` is written, not read.
        assert!(!reads.contains(&"count".to_string()));
    }

    #[test]
    fn test_rust_mut_param_emits_mutates_on_type() {
        let mut extractor = Extractor::new();
        let code = r#"
struct Model;
fn apply(m: &mut Model) {}
"#;
        let result = extractor.extract_file("test.rs", code);
        let muts = result
            .unresolved_refs
            .iter()
            .filter(|r| r.kind == EdgeKind::Mutates && r.detail.as_deref() == Some("mut_param"))
            .map(|r| r.reference_name.clone())
            .collect::<Vec<_>>();
        assert!(muts.contains(&"Model".to_string()), "mut_params={:?}", muts);
    }

    #[test]
    fn test_rust_method_call_not_counted_as_field_read() {
        let mut extractor = Extractor::new();
        let code = r#"
fn caller(m: &Foo) {
    m.do_thing();
}
"#;
        let result = extractor.extract_file("test.rs", code);
        assert!(refs_of(&result, EdgeKind::Calls).contains(&"do_thing".to_string()));
        assert!(
            !refs_of(&result, EdgeKind::Accesses).contains(&"do_thing".to_string()),
            "method callee should not be a field access"
        );
    }

    #[test]
    fn test_rust_glob_import_flagged() {
        let mut extractor = Extractor::new();
        let code = "use crate::model::*;\nuse crate::db::Database;\n";
        let result = extractor.extract_file("test.rs", code);
        let glob = result
            .unresolved_refs
            .iter()
            .find(|r| r.kind == EdgeKind::Imports && r.detail.as_deref() == Some("glob"));
        assert!(glob.is_some(), "expected a glob import");
        assert_eq!(glob.unwrap().reference_name, "model");
        // Named import resolves to the symbol, no glob detail.
        assert!(result
            .unresolved_refs
            .iter()
            .any(|r| r.kind == EdgeKind::Imports
                && r.reference_name == "Database"
                && r.detail.is_none()));
    }

    #[test]
    fn test_rust_enum_dispatch_references() {
        let mut extractor = Extractor::new();
        let code = r#"
fn render(k: ViewKind) -> u8 {
    match k {
        ViewKind::List => 1,
        ViewKind::Grid => 2,
    }
}
"#;
        let result = extractor.extract_file("test.rs", code);
        let dispatch = result
            .unresolved_refs
            .iter()
            .filter(|r| r.kind == EdgeKind::References && r.detail.as_deref() == Some("dispatch"))
            .map(|r| r.reference_name.clone())
            .collect::<Vec<_>>();
        assert!(
            dispatch.contains(&"List".to_string()),
            "dispatch={:?}",
            dispatch
        );
        assert!(
            dispatch.contains(&"Grid".to_string()),
            "dispatch={:?}",
            dispatch
        );
    }

    #[test]
    fn test_python_attribute_access() {
        let mut extractor = Extractor::new();
        let code = "def work(m):\n    x = m.name\n    m.count = 5\n";
        let result = extractor.extract_file("test.py", code);
        assert!(refs_of(&result, EdgeKind::Accesses).contains(&"name".to_string()));
        assert!(refs_of(&result, EdgeKind::Mutates).contains(&"count".to_string()));
    }

    #[test]
    fn test_typescript_member_access() {
        let mut extractor = Extractor::new();
        let code = "function work(m: Model) {\n  const x = m.name;\n  m.count = 5;\n}\n";
        let result = extractor.extract_file("test.ts", code);
        assert!(refs_of(&result, EdgeKind::Accesses).contains(&"name".to_string()));
        assert!(refs_of(&result, EdgeKind::Mutates).contains(&"count".to_string()));
    }

    #[test]
    fn test_field_access_deduped() {
        let mut extractor = Extractor::new();
        let code = r#"
fn work(m: &Foo) {
    let _ = m.value;
    let _ = m.value;
    let _ = m.value;
}
"#;
        let result = extractor.extract_file("test.rs", code);
        let count = result
            .unresolved_refs
            .iter()
            .filter(|r| r.kind == EdgeKind::Accesses && r.reference_name == "value")
            .count();
        assert_eq!(count, 1, "repeated field reads should dedup to one edge");
    }
}
