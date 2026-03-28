# Agent Development Guide

Quick reference for AI agents working in this Rust MCP server repository.

## Quick Commands

**Build/Test**: `make build` | `make test` | `make check` | `make fmt` | `make lint`
**Run**: `cargo run -- index .` | `cargo run -- serve` | `cargo run -- serve --port 8080`
**Test**: `cargo test --lib` | `cargo test module::` | `cargo test --test integration_test`
**Release**: `make release` (date-based version YYYY.M.D) | `make release-dry-run` (preview)
**Maintenance**: `make update` | `make outdated` | `make clean`

## Project Overview

**codemap**: Semantic code intelligence MCP server building knowledge graphs for AI-assisted development.

**Core Concepts**:
- **Node**: Code symbol (function, class, method, struct, etc.)
- **Edge**: Relationship (calls, contains, imports, extends, implements)
- **Graph**: Stored in SQLite `.codemap/index.db`

**Key Features**: Fast symbol search, impact analysis, call graph navigation, AI context building

**Module Structure**:
- `main.rs`: CLI entry, command routing (~126 lines)
- `server.rs`: MCP server startup (stdio/HTTP)
- `lib.rs`: Core indexing, file walking
- `types.rs`: Node, Edge, Language enums
- `cli/`: Command implementations, DB utilities
- `db/`: SQLite CRUD, queries, transactions
- `extraction/`: Tree-sitter parsing, symbol detection
- `graph/`: Traversal algorithms (callers, callees, impact)
- `context/`: AI context building
- `mcp/`: Protocol, tool definitions, handlers

**Data Flow**: Source Files → Extraction (tree-sitter) → Database (SQLite) → Resolution (link refs) → Graph Queries → MCP Tools

**Key Dependencies**: `rmcp` (MCP), `tree-sitter` (parsing), `rusqlite` (DB), `axum` (HTTP), `ignore` (file walking)

**Database**: `.codemap/index.db` with tables: `files`, `nodes`, `edges`, `unresolved_refs`
Indexes on: `nodes(name)`, `edges(source_id)`, `edges(target_id)`, `files(path)`

**Languages**: Rust, TypeScript/JavaScript, Python, Go, Java, C, C++
Add new: Edit `src/extraction/languages.rs`

**MCP Tools**:
- `codemap-context {task}`: AI coding tasks
- `codemap-search {query}`: Find symbols
- `codemap-callers {symbol}`: Who calls this?
- `codemap-callees {symbol}`: What does this call?
- `codemap-impact {symbol}`: What breaks if changed?
- `codemap-definition {symbol, context_lines?}`: View source
- `codemap-file {path}`: List symbols in file
- `codemap-references {symbol}`: All usages
- `codemap-node {symbol}`: Symbol metadata
- `codemap-status`: Index health
- `codemap-reindex {files?}`: Refresh index

## Development Guidelines

**Workflow**: Test → Change → `make check` (format, lint, test) → Commit

**Code Style**:
- Descriptive names, small functions (<100 lines)
- Document public APIs with `///`
- Use `Result<T>` + `?` operator, avoid panics
- Prefer iterators over loops

**Modularity**: Split files >500 lines. Pattern: `mod.rs` (API), `types.rs` (data), `constants.rs` (config), `handlers/` (impl)

**Testing**: Unit tests in `#[cfg(test)]`, integration in `tests/`, name as `test_<function>_<scenario>`

**Add Language**: Add grammar to `Cargo.toml` → Add to `Language` enum in `types.rs` → Config in `extraction/languages.rs` → Test

**Add MCP Tool**:
1. Request type in `mcp/types.rs`
2. Handler in `mcp/handlers/mytool.rs`
3. Export in `handlers/mod.rs`
4. Tool definition with `#[tool]` in `mcp/mod.rs`
5. Test in `tests/integration_test.rs`

**Fix Bug**: Add failing test → Fix → Verify → Check similar code → Add regression test

**Performance**: Use indexes, batch in transactions, lazy load, prefer `&str`, reuse parser

**Debug**:
- `RUST_LOG=debug cargo run -- index .`
- `cargo test test_name -- --nocapture`
- `sqlite3 .codemap/index.db "SELECT COUNT(*) FROM nodes"`

**Release**: `make release` bumps version to today's date (YYYY.M.D), updates Cargo.toml and manifest.json, commits, tags, and pushes. GitHub Actions builds and publishes artifacts. Use `make release-dry-run` to preview.

**Problems**:
- Test fails: `make clean && make test`
- Build errors: `make update && make build`
- DB locked: `pkill codemap`
- Slow indexing: Check .gitignore, file count
