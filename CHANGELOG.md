# Changelog

Notable changes to symgraph. Versions use CalVer (`YYYY.M.D`).

## Unreleased

Work from the 2026-09-20 review (`docs/review-2026-09-20.md`). The through-line
is that several tools answered in a way that read as complete and exact when it
was neither; they now say what they know and what they do not.

### Added

- **Ambiguity reporting.** Every single-symbol result says how many definitions
  shared the name and where the others are. `file` / `qualified_name` (CLI:
  `--file` / `--qualified-name`) narrow the lookup.
- **Truncation reporting and paging.** Every list result carries `total`,
  `shown` and `truncated`, plus the `offset` that gets the next page. `limit` /
  `offset` on the symbol tools and `unused`; `--limit` on `search`.
- **Index provenance and health.** Indexes record the schema, extractor and
  crate version that built them. `status` reports whether the index is current,
  whether the full-text index is intact, how many references went unresolved,
  and what share of names more than one definition answers to.
- **Supply chain.** Releases publish `SHA256SUMS`; both installers verify the
  download before unpacking it. `cargo deny` runs in CI over advisories,
  licenses, sources and bans.
- **Declared MSRV** of 1.90, verified by a CI job that builds against exactly
  that toolchain.
- `symgraph-cli` is now installed by the install scripts, which previously
  discarded it despite shipping it in the archive.

### Fixed

- **Symbol resolution was non-deterministic.** A name shared by several
  definitions resolved to whichever row SQLite happened to return, and the
  choice could change between reindexes. Resolution now prefers production code
  over tests and generated code, with a stable tiebreak.
- **Callers and callees double-counted.** A function calling its target three
  times appeared three times and filled three slots of the page. The queries
  also had a `LIMIT` with no `ORDER BY`, so which subset came back was arbitrary.
- **The reindex guard did not work over HTTP.** Each session got its own flag
  over a shared database, so two clients could each start a full rebuild.
- **Git subprocesses had no timeout.** A stalled `git` hung the tool call
  indefinitely, in one case while holding the database read lock.
- **Paths were keyed on the native separator.** On Windows the index stored
  `src\foo.rs` while every caller passed `src/foo.rs`, so lookups by path found
  nothing.
- **Tool failures looked like successes.** Errors were returned as result text
  beginning `"Error: "`; they now set the MCP `isError` flag.
- **Partial parses were invisible.** tree-sitter recovers from syntax errors
  rather than failing, so half-parsed files were indexed with no signal. They
  are now flagged and counted. A size cap (2 MiB) skips files too large to be
  worth parsing.
- **The macOS "universal" bundle was arm64-only** — unrunnable on an Intel Mac.
  It is now a real `lipo` binary, and the build fails if either slice is missing.
- **`git log` mis-keyed non-ASCII paths**, so churn read zero for those files.
- `make release` used BSD-only `sed -i ''` and could only be run from macOS.
- The tag/`Cargo.toml` version check ran *after* the GitHub release was
  published; it is now a pre-flight gate.

### Changed

- **Breaking (JSON output):** `count` on `callers`, `callees` and `unused` is
  replaced by `total`, `shown` and `truncated`. `count` meant "how many we
  returned" but read as "how many exist".
- Incremental indexing refuses a stale index rather than mixing old and new
  extraction semantics; `symgraph index` detects this and rebuilds.
- Release workflow permissions are scoped per job instead of granting
  `contents: write` to everything.

### Known limitations

- Symbol resolution is name-based. It is now deterministic and reports its own
  ambiguity, but it does not yet use imports to narrow candidates — on
  symgraph's own codebase 11.5% of names are shared by more than one
  definition. `status` reports this figure for your codebase.
- Groovy parses partially: `tree-sitter-groovy` 0.1.2 rejects idiomatic
  semicolon-free statements. Symbols are still recovered.

## 2026.7.21

- Homebrew formula and release process.
- Lean `symgraph-cli` binary that builds without the MCP server stack.
- Improved pre-commit checks.

## 2026.7.5

- Dependency updates.

## 2026.6.20

- Earlier releases predate this changelog; see the git history.
