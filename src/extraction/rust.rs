//! Rust extraction on rust-analyzer's parser (`ra_ap_syntax`).
//!
//! rust-analyzer's hand-written parser reads real Rust better than
//! tree-sitter-rust did: over the Cargo registry's 122,558 files, 5,347 that
//! tree-sitter gave a syntax error (macro-heavy code, mostly) parse cleanly
//! here, against 51 the other way, at the same speed. Its tree is lossless,
//! and comments and attributes sit inside the item they belong to rather
//! than beside it, so docs and `#[test]` are read off the item itself.
//!
//! What is extracted is what the tree-sitter path extracts for any language
//! (symbols, `contains` edges, calls, field reads and writes, imports, enum
//! dispatch, `&mut` parameters, trait impls), through the same shared rules
//! in the parent module, so the two paths agree on everything but the parse.

use std::collections::HashSet;
use std::path::Path;

use ra_ap_syntax::{
    AstNode, Edition, NodeOrToken, Parse, SourceFile, SyntaxKind as K, SyntaxNode, WalkEvent,
    ast::{self, BinaryOp},
};

use super::{
    FILE_NODE_ID, FileStart, callee_name_from_text, clean_docstring, has_test_marker,
    import_target, push_unresolved, qualified_name, signature_of, test_name_heuristic,
    type_base_name, visibility_from_first_line,
};
use crate::types::{
    Edge, EdgeKind, ExtractionError, ExtractionResult, Language, Node, NodeKind, Visibility,
};

/// Extract `content`, the Rust source at `path`.
pub(super) fn extract(path: &Path, content: &str) -> ExtractionResult {
    let (root, first_error) = parse(content);
    let file = FileStart::new(path, content, Language::Rust);
    let mut ctx = RustExtraction {
        result: file.result,
        file_path: file.file_path,
        content,
        lines: LineIndex::new(content),
        node_stack: vec![FILE_NODE_ID],
        next_id: FILE_NODE_ID + 1,
        file_is_test: file.is_test,
        file_is_generated: file.is_generated,
        seen_refs: HashSet::new(),
    };
    ctx.traverse(root);

    // The parser recovers from syntax errors, so a file it could not fully
    // read still yields a tree, with symbols missing around the error. Say so,
    // with the first error's line, and keep what was recovered.
    if let Some(offset) = first_error {
        let (row, column) = ctx.lines.position(offset);
        ctx.result.parse_failed = true;
        ctx.result.errors.push(ExtractionError {
            message: "file contains syntax errors; symbols may be incomplete".to_string(),
            file_path: ctx.file_path.clone(),
            line: Some(row + 1),
            column: Some(column),
        });
    }
    ctx.result
}

/// `text` parsed, with the offset of its first syntax error if it has any.
///
/// A file's edition is its crate's, which its text does not say. It is
/// parsed as 2021, where `gen` is a name as in most code, and, when that
/// reads it with errors, as 2024 if that reads it with fewer: the keywords
/// are the same in both but for `gen`. (colab's Rust parser settled on the
/// same rule over the Cargo registry.)
fn parse(text: &str) -> (SyntaxNode, Option<usize>) {
    let parse = SourceFile::parse(text, Edition::Edition2021);
    let errors = syntax_errors(&parse);
    if !errors.is_empty() {
        let newer = SourceFile::parse(text, Edition::Edition2024);
        let newer_errors = syntax_errors(&newer);
        if newer_errors.len() < errors.len() {
            return (newer.syntax_node(), newer_errors.into_iter().min());
        }
    }
    (parse.syntax_node(), errors.into_iter().min())
}

/// Where a parse's syntax errors are. `Parse::errors` also carries
/// rust-analyzer's validation (an unknown escape, a raw `\r` in a CRLF file's
/// doc string), which is not a syntax error: the parser's own errors are
/// zero-width, where a token it expected is missing, and its recovery leaves
/// `ERROR` nodes.
fn syntax_errors(parse: &Parse<SourceFile>) -> Vec<usize> {
    let mut offsets: Vec<usize> = parse
        .errors()
        .iter()
        .filter(|e| e.range().is_empty())
        .map(|e| usize::from(e.range().start()))
        .collect();
    offsets.extend(
        parse
            .syntax_node()
            .descendants()
            .filter(|n| n.kind() == K::ERROR)
            .map(|n| usize::from(n.text_range().start())),
    );
    offsets
}

/// Byte offsets as the 0-based row and byte column spans are given in.
struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    fn new(text: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Self { starts }
    }

    fn position(&self, offset: usize) -> (u32, u32) {
        let row = self.starts.partition_point(|&s| s <= offset) - 1;
        (row as u32, (offset - self.starts[row]) as u32)
    }
}

/// The symbol a node declares, if it is one symgraph indexes, and its name.
///
/// Declarations without a body are left out, as tree-sitter left them out: a
/// trait's required `fn` and associated `type` declare nothing callable or
/// aliased.
fn symbol_of(node: &SyntaxNode) -> Option<(NodeKind, String)> {
    let kind = match node.kind() {
        K::FN if node.children().any(|c| c.kind() == K::BLOCK_EXPR) => NodeKind::Function,
        K::STRUCT => NodeKind::Struct,
        K::ENUM => NodeKind::Enum,
        K::TRAIT => NodeKind::Interface,
        K::TYPE_ALIAS if ast::TypeAlias::cast(node.clone())?.ty().is_some() => NodeKind::TypeAlias,
        K::CONST | K::STATIC => NodeKind::Constant,
        K::MODULE => NodeKind::Module,
        K::RECORD_FIELD => NodeKind::Field,
        K::VARIANT => NodeKind::EnumMember,
        _ => return None,
    };
    let name = node
        .children()
        .find(|c| c.kind() == K::NAME)?
        .text()
        .to_string();
    (!name.is_empty()).then_some((kind, name))
}

/// Where a node's own syntax starts: after the comments and attributes
/// rust-analyzer attaches to an item, which tree-sitter kept outside it.
fn own_start(node: &SyntaxNode) -> usize {
    node.children_with_tokens()
        .find(|e| !matches!(e.kind(), K::ATTR | K::WHITESPACE) && !is_comment(e.kind()))
        .map_or(node.text_range().start(), |e| e.text_range().start())
        .into()
}

/// A plain comment (a token) or a doc comment (a `DOC_COMMENT` node holding
/// its `///` or `//!` token).
fn is_comment(kind: K) -> bool {
    matches!(kind, K::COMMENT | K::DOC_COMMENT)
}

/// The comments attached before an item's own syntax, as a docstring.
fn docstring(node: &SyntaxNode, own_start: usize) -> Option<String> {
    let comments: Vec<String> = node
        .children_with_tokens()
        .take_while(|e| usize::from(e.text_range().start()) < own_start)
        .filter(|e| is_comment(e.kind()))
        .map(|e| match e {
            NodeOrToken::Node(n) => n.text().to_string(),
            NodeOrToken::Token(t) => t.text().to_string(),
        })
        .collect();
    (!comments.is_empty()).then(|| clean_docstring(&comments.join("\n")))
}

/// The name a call's callee calls: the last segment of a path, or the field
/// of `(self.callback)(x)`. A callee that is neither (a closure called in
/// place, `f()(x)`, `handlers[i](x)`) names nothing a reference could
/// resolve to; tree-sitter's text rule made a name of its source text.
fn callee_name(mut callee: ast::Expr) -> Option<String> {
    loop {
        match callee {
            ast::Expr::ParenExpr(paren) => callee = paren.expr()?,
            ast::Expr::PathExpr(ref path) => {
                let path = path.path()?;
                return Some(match path.segment().and_then(|s| s.name_ref()) {
                    Some(name) => name.syntax().text().to_string(),
                    None => callee_name_from_text(&path.syntax().text().to_string()),
                });
            }
            ast::Expr::FieldExpr(field) => {
                return field.name_ref().map(|n| n.syntax().text().to_string());
            }
            _ => return None,
        }
    }
}

/// The last segment's name of a path: `Variant` of `Kind::Variant`.
fn last_segment(path: &ast::Path) -> Option<String> {
    Some(path.segment()?.name_ref()?.syntax().text().to_string())
}

struct RustExtraction<'a> {
    result: ExtractionResult,
    file_path: String,
    content: &'a str,
    lines: LineIndex,
    /// Ids of the symbols enclosing the current node, the file outermost.
    node_stack: Vec<i64>,
    next_id: i64,
    file_is_test: bool,
    file_is_generated: bool,
    seen_refs: HashSet<(i64, EdgeKind, String)>,
}

impl RustExtraction<'_> {
    /// Walk the tree iteratively (stack-safe on deeply nested files),
    /// emitting each symbol inside the one enclosing it.
    fn traverse(&mut self, root: SyntaxNode) {
        enum Work {
            Visit(SyntaxNode),
            PopStack,
        }
        let mut work = vec![Work::Visit(root)];
        while let Some(item) = work.pop() {
            let node = match item {
                Work::PopStack => {
                    self.node_stack.pop();
                    continue;
                }
                Work::Visit(node) => node,
            };
            match node.kind() {
                K::USE => self.import(&node),
                // Rust declares what a type implements on the impl block,
                // not on the type.
                K::IMPL => self.trait_impl(&node),
                _ => {}
            }
            if let Some((kind, name)) = symbol_of(&node) {
                self.emit_symbol(&node, kind, name);
                work.push(Work::PopStack);
            }
            let children: Vec<_> = node.children().collect();
            work.extend(children.into_iter().rev().map(Work::Visit));
        }
    }

    fn emit_symbol(&mut self, node: &SyntaxNode, kind: NodeKind, name: String) {
        let start = own_start(node);
        let end = usize::from(node.text_range().end());
        let text = &self.content[start..end];
        let (start_row, start_col) = self.lines.position(start);
        let (end_row, end_col) = self.lines.position(end);

        let attributes: String = node
            .children()
            .filter(|c| c.kind() == K::ATTR)
            .map(|a| a.text().to_string())
            .collect();
        let is_test = self.file_is_test
            || (kind == NodeKind::Function
                && (test_name_heuristic(&name) || has_test_marker(&attributes)));
        let visibility = visibility_from_first_line(text)
            .or_else(|| {
                let modifier = node.children().find(|c| c.kind() == K::VISIBILITY)?;
                Some(Visibility::parse(&modifier.text().to_string()))
            })
            .unwrap_or(Visibility::Private);
        let is_async = ast::Fn::cast(node.clone()).is_some_and(|f| f.async_token().is_some());

        let symbol_id = self.next_id;
        self.next_id += 1;
        let symbol = Node::builder(kind, name.clone(), self.file_path.clone(), Language::Rust)
            .id(symbol_id)
            .qualified_name(qualified_name(&self.result.nodes, &self.node_stack, &name))
            .span(start_row + 1, end_row + 1, start_col, end_col)
            .signature(signature_of(text, kind))
            .visibility(visibility)
            .docstring(docstring(node, start))
            .is_async(is_async)
            .is_static(node.kind() == K::STATIC)
            .is_exported(text.starts_with("pub "))
            .is_test(is_test)
            .is_generated(self.file_is_generated)
            .build();
        self.result.nodes.push(symbol);

        if let Some(&parent_id) = self.node_stack.last() {
            let edge = Edge::new(parent_id, symbol_id, EdgeKind::Contains).at(
                self.file_path.clone(),
                start_row + 1,
                start_col,
            );
            self.result.edges.push(edge);
        }

        if kind == NodeKind::Function {
            self.mut_params(node, symbol_id);
        }
        self.node_stack.push(symbol_id);
        self.references(node, symbol_id);
    }

    /// Record a reference from `source_id`, positioned at `node`.
    fn push_ref(
        &mut self,
        source_id: i64,
        name: String,
        kind: EdgeKind,
        node: &SyntaxNode,
        detail: Option<&str>,
    ) {
        let (row, column) = self.lines.position(own_start(node));
        push_unresolved(
            &mut self.result,
            &mut self.seen_refs,
            &self.file_path,
            (source_id, name, kind, detail.map(str::to_string)),
            (row + 1, column),
        );
    }

    /// Record what a symbol's body refers to: `Calls` for calls, method
    /// calls and macro calls; `Accesses` / `Mutates` for field reads and
    /// writes; `References` ("dispatch") for enum variants a match names.
    /// Attributes are passed over: they hold no code that runs here.
    fn references(&mut self, symbol: &SyntaxNode, source_id: i64) {
        let mut walk = symbol.preorder();
        while let Some(event) = walk.next() {
            let WalkEvent::Enter(node) = event else {
                continue;
            };
            match node.kind() {
                K::ATTR => walk.skip_subtree(),
                K::CALL_EXPR => {
                    let name = ast::CallExpr::cast(node.clone())
                        .and_then(|c| c.expr())
                        .and_then(callee_name);
                    if let Some(name) = name {
                        self.push_ref(source_id, name, EdgeKind::Calls, &node, None);
                    }
                }
                K::METHOD_CALL_EXPR => {
                    let name = ast::MethodCallExpr::cast(node.clone())
                        .and_then(|m| m.name_ref())
                        .map(|n| n.syntax().text().to_string());
                    if let Some(name) = name {
                        self.push_ref(source_id, name, EdgeKind::Calls, &node, None);
                    }
                }
                K::MACRO_CALL => {
                    let name = ast::MacroCall::cast(node.clone())
                        .and_then(|m| m.path())
                        .and_then(|p| last_segment(&p));
                    if let Some(name) = name {
                        self.push_ref(source_id, name, EdgeKind::Calls, &node, None);
                    }
                }
                K::FIELD_EXPR => {
                    let name = ast::FieldExpr::cast(node.clone())
                        .and_then(|f| f.name_ref())
                        .map(|n| n.syntax().text().to_string());
                    if let Some(name) = name {
                        let kind = if is_write_target(&node) {
                            EdgeKind::Mutates
                        } else {
                            EdgeKind::Accesses
                        };
                        self.push_ref(source_id, name, kind, &node, None);
                    }
                }
                K::MATCH_EXPR => self.enum_dispatch(&node, source_id),
                _ => {}
            }
        }
    }

    /// Scattered enum dispatch: the qualified variants (`ViewKind::List`) a
    /// match names, in its patterns and its arms.
    fn enum_dispatch(&mut self, match_expr: &SyntaxNode, source_id: i64) {
        for node in match_expr.descendants() {
            let in_position = node.parent().is_some_and(|p| {
                matches!(
                    p.kind(),
                    K::PATH_EXPR | K::PATH_PAT | K::TUPLE_STRUCT_PAT | K::RECORD_PAT
                )
            });
            if !in_position {
                continue;
            }
            let Some(path) = ast::Path::cast(node.clone()) else {
                continue;
            };
            if path.qualifier().is_none() {
                continue;
            }
            if let Some(variant) = last_segment(&path) {
                let kind = EdgeKind::References;
                self.push_ref(source_id, variant, kind, &node, Some("dispatch"));
            }
        }
    }

    /// An `Imports` edge from the enclosing file, module or function to what
    /// a `use` names.
    fn import(&mut self, node: &SyntaxNode) {
        let container_id = self.node_stack.last().copied().unwrap_or(FILE_NODE_ID);
        let text = &self.content[own_start(node)..usize::from(node.text_range().end())];
        if let Some((name, detail)) = import_target(text) {
            let (row, column) = self.lines.position(own_start(node));
            push_unresolved(
                &mut self.result,
                &mut self.seen_refs,
                &self.file_path,
                (container_id, name, EdgeKind::Imports, detail),
                (row + 1, column),
            );
        }
    }

    /// `impl Render for Engine`: an `Implements` edge from `Engine` to
    /// `Render`.
    ///
    /// The type is looked up among the symbols already extracted from this
    /// file. An impl for a type declared elsewhere is skipped: there is no
    /// node here to hang the edge on, and guessing would attribute it to
    /// whatever else shares the name.
    fn trait_impl(&mut self, node: &SyntaxNode) {
        let Some(imp) = ast::Impl::cast(node.clone()) else {
            return;
        };
        let (Some(trait_ty), Some(self_ty)) = (imp.trait_(), imp.self_ty()) else {
            return; // an inherent `impl Type`, which implements nothing
        };
        let (Some(trait_name), Some(type_name)) = (
            type_base_name(&trait_ty.syntax().text().to_string()),
            type_base_name(&self_ty.syntax().text().to_string()),
        ) else {
            return;
        };
        let implementor = self.result.nodes.iter().find(|n| {
            n.name == type_name
                && matches!(
                    n.kind,
                    NodeKind::Struct | NodeKind::Enum | NodeKind::Class | NodeKind::TypeAlias
                )
        });
        if let Some(id) = implementor.map(|n| n.id) {
            self.push_ref(
                id,
                trait_name,
                EdgeKind::Implements,
                trait_ty.syntax(),
                None,
            );
        }
    }

    /// `&mut T` parameters, an intrusive/common-coupling signal: a `Mutates`
    /// edge from the function to the borrowed type.
    fn mut_params(&mut self, node: &SyntaxNode, source_id: i64) {
        let Some(params) = ast::Fn::cast(node.clone()).and_then(|f| f.param_list()) else {
            return;
        };
        // `self: &mut Allocator` is a self parameter with a type, which
        // rust-analyzer keeps apart from the others; `&mut self` has none.
        let typed_self = params.self_param().and_then(|s| s.ty());
        for ty in typed_self
            .into_iter()
            .chain(params.params().filter_map(|p| p.ty()))
        {
            let ast::Type::RefType(reference) = ty else {
                continue;
            };
            if reference.mut_token().is_none() {
                continue;
            }
            let name = reference
                .ty()
                .and_then(|inner| type_base_name(&inner.syntax().text().to_string()));
            if let Some(name) = name {
                let kind = EdgeKind::Mutates;
                self.push_ref(source_id, name, kind, reference.syntax(), Some("mut_param"));
            }
        }
    }
}

/// Whether a field expression is the left-hand side of an assignment, `=` or
/// compound: a write rather than a read.
fn is_write_target(node: &SyntaxNode) -> bool {
    let Some(assignment) = node.parent().and_then(ast::BinExpr::cast) else {
        return false;
    };
    matches!(assignment.op_kind(), Some(BinaryOp::Assignment { .. }))
        && assignment.lhs().is_some_and(|lhs| lhs.syntax() == node)
}

#[cfg(test)]
mod tests {
    //! What reading Rust with rust-analyzer's parser changed from
    //! tree-sitter-rust. The cases shared with every language are in the
    //! parent module's tests.
    use super::*;

    fn extract_str(code: &str) -> ExtractionResult {
        extract(Path::new("src/lib.rs"), code)
    }

    fn symbol<'r>(result: &'r ExtractionResult, name: &str) -> &'r Node {
        result
            .nodes
            .iter()
            .find(|n| n.name == name)
            .unwrap_or_else(|| panic!("no symbol {name}"))
    }

    fn refs(result: &ExtractionResult, kind: EdgeKind) -> Vec<&str> {
        result
            .unresolved_refs
            .iter()
            .filter(|r| r.kind == kind)
            .map(|r| r.reference_name.as_str())
            .collect()
    }

    #[test]
    fn docs_above_an_attribute_belong_to_the_item() {
        let result = extract_str("/// A point.\n#[derive(Debug)]\npub struct Point;\n");
        let point = symbol(&result, "Point");
        assert_eq!(point.docstring.as_deref(), Some("A point."));
        // The span and signature start at the item, not its docs or attributes.
        assert_eq!(point.start_line, 3);
        assert_eq!(point.signature.as_deref(), Some("pub struct Point;"));
        assert_eq!(point.visibility, Visibility::Public);
    }

    #[test]
    fn module_docs_and_section_banners_are_not_an_items_docs() {
        let code = "//! The crate.\n\nfn first() {}\n\n// --- section ---\n\nfn second() {}\n";
        let result = extract_str(code);
        assert_eq!(symbol(&result, "first").docstring, None);
        assert_eq!(symbol(&result, "second").docstring, None);
    }

    #[test]
    fn async_and_static_come_from_the_declaration_not_its_body() {
        let code = "fn spawns() { run(async move {}); static N: u8 = 0; }\n\
                    async fn waits() {}\npub static TABLE: u8 = 0;\n";
        let result = extract_str(code);
        let spawns = symbol(&result, "spawns");
        assert!(!spawns.is_async && !spawns.is_static);
        assert!(symbol(&result, "waits").is_async);
        assert!(symbol(&result, "TABLE").is_static);
    }

    #[test]
    fn a_test_marker_on_the_previous_item_does_not_spread() {
        let code = "#[cfg(test)]\nimpl Fixture {}\nfn production() {}\n#[tokio::test]\nasync fn checks() {}\n";
        let result = extract_str(code);
        assert!(!symbol(&result, "production").is_test);
        assert!(symbol(&result, "checks").is_test);
    }

    #[test]
    fn turbofish_and_qualified_macro_calls_are_named() {
        let code = "fn f(xs: Vec<u8>) {\n    let v = xs.iter().collect::<Vec<_>>();\n    \
                    serde_json::json!({});\n    (self.callback)(1);\n    (|| 1)();\n}\n";
        let result = extract_str(code);
        let calls = refs(&result, EdgeKind::Calls);
        for name in ["iter", "collect", "json", "callback"] {
            assert!(calls.contains(&name), "{name} missing from {calls:?}");
        }
        // A closure called in place names nothing; tree-sitter made a name of
        // its text. A method's name is a call, never a field read (tree-sitter
        // read `collect` of a turbofish call as one); a called field is both.
        assert_eq!(calls.len(), 4, "{calls:?}");
        assert_eq!(refs(&result, EdgeKind::Accesses), ["callback"]);
    }

    #[test]
    fn dispatch_takes_the_variant_of_every_pattern_shape() {
        let code = "fn f(e: Event) {\n    match e {\n        egui::Event::Key { .. } => {}\n        \
                    Event::Text(_) => {}\n        Event::Copy => tracing::warn!(\"x\"),\n        \
                    _ => { let m: std::collections::HashMap<u8, u8> = Default::default(); }\n    }\n}\n";
        let result = extract_str(code);
        let mut dispatch: Vec<&str> = result
            .unresolved_refs
            .iter()
            .filter(|r| r.detail.as_deref() == Some("dispatch"))
            .map(|r| r.reference_name.as_str())
            .collect();
        dispatch.sort();
        // Not the enum of `egui::Event::Key`, a macro's path or a type's. A
        // qualified path in an arm's body counts, as it did with tree-sitter.
        assert_eq!(dispatch, ["Copy", "Key", "Text", "default"]);
    }

    #[test]
    fn a_syntax_error_is_reported_at_its_line() {
        let result = extract_str("fn fine() {}\n\nfn broken( {\n}\n\nfn after() {}\n");
        assert!(result.parse_failed);
        assert_eq!(result.errors[0].line, Some(3));
        assert!(result.nodes.iter().any(|n| n.name == "after"));
    }

    #[test]
    fn edition_2024_is_tried_when_2021_reads_errors() {
        // `gen` is a keyword only in 2024: a gen block reads cleanly there.
        let result = extract_str("fn numbers() -> impl Iterator<Item = u8> { gen { yield 1; } }\n");
        assert!(!result.parse_failed, "{:?}", result.errors);
        // ... and a name in 2021, where most code is.
        let result = extract_str("fn gen() {}\nfn caller() { gen(); }\n");
        assert!(!result.parse_failed);
        assert!(refs(&result, EdgeKind::Calls).contains(&"gen"));
    }

    #[test]
    fn trait_impls_and_mut_params_survive_attributes() {
        let code = "struct Engine;\n#[automatically_derived]\nimpl std::fmt::Display for Engine {}\n\
                    #[inline]\nfn tune(e: &mut Engine, n: &u8) {}\n\
                    impl Engine { fn take(self: &mut Pool) {} fn plain(&mut self) {} }\n";
        let result = extract_str(code);
        assert_eq!(refs(&result, EdgeKind::Implements), ["Display"]);
        let engine = symbol(&result, "Engine").id;
        let implements = &result.unresolved_refs[0];
        assert_eq!((implements.source_node_id, implements.line), (engine, 3));
        assert_eq!(refs(&result, EdgeKind::Mutates), ["Engine", "Pool"]);
    }

    #[test]
    fn positions_are_byte_columns_on_one_based_lines() {
        let result = extract_str("mod m {\n    pub fn é() { call(); }\n}\n");
        let f = symbol(&result, "é");
        assert_eq!((f.start_line, f.start_column), (2, 4));
        assert_eq!(f.qualified_name.as_deref(), Some("m::é"));
        let call = &result.unresolved_refs[0];
        assert_eq!((call.line, call.column), (2, 18));
    }
}
