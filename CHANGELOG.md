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
- **`--format json` on every tool.** `context`, `search`, `blame`, `churn`,
  `diff-impact`, `status` and `reindex` were markdown-only; all 14 request
  types now carry `format`. Blame output is parsed into commit/author/date/line
  fields rather than one opaque string.
- **`symgraph reindex --files a.rs b.rs`** — the targeted mode the MCP tool
  always had, now on the CLI too.
- **`ignore_test_callers`** on `symgraph-unused` (CLI:
  `--ignore-test-callers`), which stops a test-only caller counting as a use
  and so finds production code kept alive by nothing but its own tests.
- **Inheritance extraction.** `symgraph-implementations` had never worked:
  extraction emitted no `implements`/`extends` edges for any language, so the
  tool always answered "none found". It now reads inheritance clauses across
  Rust, Java, TypeScript, JavaScript, Python, C++, C#, Kotlin, Scala, Ruby and
  Groovy. (Go has no declared inheritance to read.) Indexes built before this
  are recognised as stale and rebuild automatically.
- **Import-scoped symbol resolution.** A call now prefers a definition in a
  file the caller imports from over an arbitrary one. On symgraph's own
  codebase this cut arbitrary resolutions by a third.
- **Benchmarks** (`cargo bench`) over full indexing, the no-op incremental
  pass, and graph folding.
- `symgraph-cli` is now installed by the install scripts, which previously
  discarded it despite shipping it in the archive.

### Fixed

- **Targeted reindex always failed.** `delete_file` dropped `nodes` before
  `unresolved_refs`, which holds a foreign key to it, so every
  `symgraph-reindex` with a file list returned "FOREIGN KEY constraint failed"
  in a warnings field nothing surfaced. Masked until unresolvable references
  began to be kept rather than deleted on every pass.
- **The CLI and the MCP server had drifted.** `search`, `context` and `status`
  had separate CLI implementations, so fixes to a handler did not reach the
  command users run — `context` supported `--format json` the MCP tool did not
  have, and `status` needed its health block written twice. All three now call
  the same functions, and a test runs the binary against the handler to keep
  it that way.
- **Calls resolved to things that cannot be called.** Name-based resolution
  matched a reference to a definition by name alone, with no check that the
  two were compatible, so a call could land on a *field*, module, import or
  enum member that happened to share the callee's name. On symgraph's own
  index that was 603 of 3777 `calls` edges (16%), and they fed fan-in/fan-out,
  the coupling score and cycle detection as real dependencies. A reference now
  only resolves to a kind it could denote; with no compatible candidate it
  stays unresolved, which the health report counts, rather than resolving to
  something wrong. On symgraph's own index every `calls` edge now points at a
  function — and there are *more* of them (3563 vs 3174), because the filter
  re-points a reference at the right callable instead of dropping it.
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
- **The server held a lock that was unsound to share.** The database was
  wrapped in an `RwLock` behind an `unsafe impl Sync`; two concurrent readers
  calling into rusqlite's statement cache is a data race. It is a `Mutex` now,
  and the crate contains no `unsafe` code.
- **Tool calls blocked the async runtime.** Handlers ran their synchronous
  SQLite and git work directly on a tokio worker thread, so one slow call
  stalled every other HTTP session. They run on the blocking pool now.
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

- **Breaking: one binary instead of two.** `symgraph-cli` is gone; everything
  it did is in `symgraph`, and `symgraph serve` runs the MCP server. The two
  had drifted — `serve` existed only in `symgraph`, while `reindex`, `watch`,
  `completions` and `man` existed only in `symgraph-cli`, and `index` meant a
  full rebuild in one and an incremental pass in the other. A CLI-only build
  is now the same binary without the `server` feature, which is what the split
  was supposed to provide and did not: the two binaries were the same size
  either way, and every release shipped both.
  - `symgraph reindex`, `watch`, `completions` and `man` now exist — they were
    missing from the binary the MCP bundle and installers actually deliver.
  - `symgraph index` is now incremental (it was a full rebuild); `symgraph
    reindex` is the full rebuild. On a fresh checkout both do the same work.
  - Update any script calling `symgraph-cli` to call `symgraph`. The
    installers remove a stale `symgraph-cli` from their install directory.
- **Breaking: the coupling tools exclude test code by default.**
  `symgraph-module-graph`, `symgraph-coupling-score` and `symgraph-god-struct`
  folded test code into the architecture graph — a third of the edges on
  symgraph's own index — so a test file read as a module every production
  module depended on, and fan-in, the coupling ranking and cycle membership
  were all computed over it. They now report production code; `include_tests`
  (CLI: `--include-tests`) restores the old behaviour, and each report states
  which scope it used. Every other tool is unchanged: `symgraph-callers` and
  `symgraph-references` still show test callers, which is usually the point.
- **Breaking (JSON output):** `count` on `callers`, `callees` and `unused` is
  replaced by `total`, `shown` and `truncated`. `count` meant "how many we
  returned" but read as "how many exist".
- Incremental indexing refuses a stale index rather than mixing old and new
  extraction semantics; `symgraph index` detects this and rebuilds.
- Release workflow permissions are scoped per job instead of granting
  `contents: write` to everything.

### Performance

- Indexing streams in chunks rather than holding the whole repository in
  memory.
- A no-op incremental pass skips reading and hashing files whose size and mtime
  are unchanged.
- Reference resolution is one set-based statement per phase instead of several
  queries per reference.
- Coupling analysis aggregates edges in SQL — 559 rows instead of 7246 on
  symgraph's own index.
- Reindexing named files no longer walks the whole tree.

### Known limitations

- Symbol resolution is name-based. It is now deterministic and reports its own
  ambiguity, but it does not yet use imports to narrow candidates — on
  symgraph's own codebase 11.5% of names are shared by more than one
  definition. `status` reports this figure for your codebase. Import scoping
  narrows the choice but cannot rescue an import that is itself ambiguous.
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
