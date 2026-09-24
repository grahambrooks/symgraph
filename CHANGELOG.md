# Changelog

Notable changes to symgraph. Versions use CalVer (`YYYY.M.N`).

## Unreleased

Mostly the 2026-09-20 review (`docs/review-2026-09-20.md`), whose through-line
is that several tools answered in a way that read as complete and exact when it
was neither; they now say what they know and what they do not. Plus the
release-pipeline follow-ups to the release-kit v2 adoption in 2026.9.2.

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
- **Supply chain.** Both installers now verify the downloaded archive against
  the release's published `SHA256SUMS` before unpacking it — it is about to be
  run on the machine, so the transport alone is not good enough. `cargo deny`
  runs in CI over advisories, licenses, sources and bans.
- **Declared MSRV** of 1.90, verified by a CI job that builds against exactly
  that toolchain.
- **`--format json` on every tool.** `context`, `search`, `blame`, `churn`,
  `diff-impact`, `status` and `reindex` were markdown-only; all 16 request
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
- **`symgraph-god-struct` credited every struct with its namesakes' fields.**
  The field lookup keyed on the struct's *name*, so in a codebase with 80
  types called `Struct` each of them was reported with all 80 types' fields.
  On a 2,000-file corpus this moved 2,857 of 4,548 rows — `Build` was listed
  with 467 fields against an actual 40 — which put whichever name was most
  duplicated at the top of a report meant to rank architectural debt. Fields
  are now reached through the `contains` edge from a specific struct, so every
  changed row is one that shares its name with another, and no uniquely-named
  struct changed at all.
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
- **Intel Macs got a binary they could not run.** The macOS archive was built
  only for arm64. The release now builds `x86_64-apple-darwin` natively on an
  Intel runner, so each macOS archive really is for the architecture it names.
  (The `darwin-universal` **MCPB bundle** is still arm64-only — see Known
  limitations.)
- **`git log` mis-keyed non-ASCII paths**, so churn read zero for those files.
- `make release` used BSD-only `sed -i ''` and could only be run from macOS.
- A tag whose version did not match `Cargo.toml` produced a GitHub release
  that then failed to publish to crates.io, leaving the two out of step. The
  tag is now the single source of the version: it is stamped into `Cargo.toml`
  during the build, so the two cannot disagree.
- **The next release would not have built.** `.release.env` still listed
  `symgraph-cli` in `BINS` after the binaries were merged, and that value is
  passed to the build as `--bin`; every leg of the five-target matrix would
  have failed on a target that no longer exists.

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

### Performance

- **Indexing was quadratic; it no longer is.** Reference resolution ranked
  every definition sharing a name for *every* reference to that name, so its
  cost grew with the product of the two — and the query was written out three
  times, so SQLite evaluated it three times per reference. At 4,000 files of
  real Rust, `new` had 1,482 definitions and a reference to it cost 1,050 µs
  against 1.6 µs for a unique name. The preference ladder is now resolved as
  three materialised tiers, the last of which is keyed on `(name, kind)` and
  computed once for the whole index rather than once per reference.

  | files | before | after |
  |---:|---:|---:|
  | 2,000 | 24.7 s | 5.8 s |
  | 4,000 | 97.3 s | 12.8 s |
  | 8,000 | 511.1 s | 39.6 s |

  O(n^2.02) → O(n^1.41). Projected at 100,000 files: ~23 hours → ~23 minutes.
  The resolved edge set is byte-identical, verified by diffing the full edge
  dump at 2,000 and 4,000 files. See
  [ADR 0002](docs/adr/0002-tiered-reference-resolution.md), and
  [ADR 0001](docs/adr/0001-storage-engine.md) for why the storage engine was
  not the problem.
- **`symgraph-god-struct` was quadratic too, for a different reason.** It
  looped over every struct issuing a field lookup plus one incoming-edge query
  per struct and per field. One aggregate query replaces the loop: **98.6 s →
  0.76 s** at 4,000 files (130×), and 8,000 files now answers in 1.0 s.
- Indexing streams in chunks rather than holding the whole repository in
  memory.
- A no-op incremental pass skips reading and hashing files whose size and mtime
  are unchanged.
- Reference resolution is one set-based statement per phase instead of several
  queries per reference.
- Coupling analysis aggregates edges in SQL — 581 rows instead of 7711 on
  symgraph's own index, and the ratio grows with the codebase.
- Reindexing named files no longer walks the whole tree.

### Known limitations

- **Symbol resolution is still name-based.** It is deterministic, it reports
  its own ambiguity, it prefers a file the caller imports from, and it no
  longer resolves a reference to a kind it could not denote — but a method
  call on a receiver whose type cannot be inferred still matches any
  same-named definition. On symgraph's own codebase 10.6% of names are shared
  by more than one definition (`status` reports this for yours), and 216 of
  837 cross-file calls between production files (26%) resolve to a file the
  caller does not import.
  Import scoping also cannot rescue an import that is itself ambiguous.
- **Treat `module-graph` cycles as a prompt, not a verdict.** Those remaining
  mis-resolved edges are enough to merge modules that are not really cyclic:
  symgraph's own graph still reports one cycle spanning 33 of 39 files. Fan-in,
  fan-out and the coupling ranking are reliable; SCC membership is not.
- **The `darwin-universal` MCPB bundle carries the arm64 binary only** and will
  not run on an Intel Mac. The per-architecture bundles (`darwin-x64`,
  `darwin-arm64`) and every `.tar.gz` archive are built for the architecture
  they name — prefer those.
- Groovy parses partially: `tree-sitter-groovy` 0.1.2 rejects idiomatic
  semicolon-free statements. Symbols are still recovered.

## 2026.9.2

- **Adopted release-kit v2** as the release pipeline: the tag is the version
  and is stamped into `Cargo.toml` during the build, `SHA256SUMS` is published
  for every archive, the Homebrew formula is generated rather than hand-edited,
  and each job asks for only the permissions it needs. Repo-specific settings
  live in `.release.env`; `release.yml` and `scripts/release.py` are shared
  across repositories and should not be edited here.
- **Archive names changed** to `symgraph-v<version>-<rust target triple>`
  (for example `symgraph-v2026.9.2-aarch64-apple-darwin.tar.gz`). The install
  scripts fall back to the old `symgraph-<version>-<os>-<arch>` names, so
  pinning an older version still works.

## 2026.9.1

- Dependency update: tree-sitter 0.27.

## 2026.8.1

- Re-release of 2026.7.21 with no source changes; its binaries report
  `2026.7.21`, which is the drift the tag-driven versioning above fixes.

## 2026.7.21

- Homebrew formula and release process.
- Lean `symgraph-cli` binary that builds without the MCP server stack.
- Improved pre-commit checks.

## 2026.7.5

- Dependency updates.

## 2026.6.20

- Earlier releases predate this changelog; see the git history.
