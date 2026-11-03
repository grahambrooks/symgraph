# Agent Development Guide

Quick reference for AI agents working in this Rust MCP server repository.

## Quick Commands

**Build/Test**: `make build` | `make test` | `make check` | `make fmt` | `make lint` | `make cli-only` (lean, no MCP server) | `make check-all` (check + cli-only + deny) | `make hooks` (install prek git hooks)
**Run**: `cargo run -- index .` | `cargo run -- serve` | `cargo run -- serve --port 8080`
**Test**: `cargo test --lib` | `cargo test module::` | `cargo test --test integration_test`
**Release**: `make release` (tags origin/main with the next YYYY.M.D) | `make release-dry` (preview) | `make release-version`
**Maintenance**: `make update` | `make outdated` | `make clean`

## Project Overview

**symgraph**: Semantic code intelligence MCP server building knowledge graphs for AI-assisted development.

**Core Concepts**:
- **Node**: Code symbol (function, class, method, struct, etc.)
- **Edge**: Relationship (calls, contains, imports, extends, implements)
- **Graph**: Stored in SQLite `.symgraph/index.db`

**Key Features**: Fast symbol search, impact analysis, call graph navigation, AI context building

**Module Structure**:
- `main.rs`: the single binary — command catalog, routing, `serve`, help/completions/man
- `server.rs`: MCP server startup (stdio/HTTP), behind the `server` feature
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

**Database**: `.symgraph/index.db` with tables: `files`, `nodes`, `edges`, `unresolved_refs`
Indexes on: `nodes(name)`, `edges(source_id)`, `edges(target_id)`, `files(path)`

**Languages**: Rust, TypeScript/JavaScript, Python, Go, Java, C, C++
Add new: Edit `src/extraction/languages.rs`

**MCP Tools**:
- `symgraph-context {task}`: AI coding tasks
- `symgraph-search {query}`: Find symbols
- `symgraph-callers {symbol}`: Who calls this?
- `symgraph-callees {symbol}`: What does this call?
- `symgraph-impact {symbol}`: What breaks if changed?
- `symgraph-definition {symbol, context_lines?}`: View source
- `symgraph-file {path}`: List symbols in file
- `symgraph-references {symbol}`: All usages
- `symgraph-node {symbol}`: Symbol metadata
- `symgraph-status`: Index health
- `symgraph-reindex {files?}`: Refresh index

## Development Guidelines

**Workflow**: Test → Change → `make check` (format, lint, test — the CI check job; `make check-all` adds lean build and supply chain) → Commit

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
- `sqlite3 .symgraph/index.db "SELECT COUNT(*) FROM nodes"`

**Release**: `make release` tags origin/main with today's date (YYYY.M.D, incremented if taken) and pushes only the tag; nothing is bumped or committed locally. The release workflow stamps the version, builds and publishes artifacts, and merges a PR bumping Cargo.toml and the formula (release-extras stamps the bundle manifest.json). Use `make release-dry` to preview.

**crates.io is manual**: the release workflow does not publish (`PUBLISH_CRATE=false` in `.release.env`), because a crates.io version can never be deleted or reused. After the release workflow succeeds, publish from a clean checkout of the tag. The tag's `Cargo.toml` still has the previous version (the workflow stamps it at build time and its PR bumps `main` later), so stamp it the way the workflow does:

```sh
v=2026.11.3                                   # the released tag, without the "v"
git worktree add --detach "../symgraph-v$v" "v$v" && cd "../symgraph-v$v"
python3 scripts/release.py set-version "$v"   # Cargo.toml + symgraph's entry in Cargo.lock
cargo publish --locked --allow-dirty --dry-run
cargo publish --locked --allow-dirty          # only the stamped version differs
cd - && git worktree remove --force "../symgraph-v$v"
```

**Problems**:
- Test fails: `make clean && make test`
- Build errors: `make update && make build`
- DB locked: `pkill symgraph`
- Slow indexing: Check .gitignore, file count
