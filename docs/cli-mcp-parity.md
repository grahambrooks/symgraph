# Design: CLI ⇄ MCP feature parity

Status: proposed · Issue: [#2](https://github.com/grahambrooks/symgraph/issues/2)

## Context

The MCP server exposes 22 tools; the CLI exposes 7 commands (`index`, `status`,
`search`, `context`, `where`, `prune`, `serve`). Issue #2 asks for the full tool
surface on the command line — for testing without an MCP client, and because
agents are increasingly driven by **CLI skills** instead of MCP tools (lighter
on context, and not subject to per-session tool-count limits).

### Key enabler

Every MCP tool is already a thin, protocol-free function:

```rust
pub fn handle_<tool>(db: &Database, [project_root: &str,] req: &<Req>) -> Result<String, String>
```

It takes a `&Database` and a plain request struct and returns a `String`
(markdown, or JSON when the request's `format` field is `"json"`). The MCP layer
(`SymgraphHandler`) is a 1-line wrapper around each. **If the CLI calls the same
functions, CLI output is identical to MCP output by construction** — no second
implementation, no drift. The request structs are already public via
`pub use types::*` in `src/mcp/mod.rs`.

This makes the work mostly *plumbing*, not new feature code.

## Goals / non-goals

**Goals**
- One CLI subcommand per MCP tool, with the same semantics and output.
- A single source of truth for tool logic: a shared `ops` layer behind both
  the MCP server and the CLI.
- Ergonomic, discoverable args (`--help` per command) that scale to ~25 commands.
- Machine-readable `--format json` on **every** tool (CLI and MCP).
- Preserve the existing commands and the MCP wire schema.

**Non-goals (this iteration)**
- Changing what each tool computes (only where its logic lives + how it renders).
- An interactive/REPL mode.

## Architecture (decided)

A provider-neutral **ops layer** holds the tool logic and returns *typed*
results; a thin **presentation** step renders each result as markdown or JSON;
both the MCP server and the CLI are thin front-ends over it.

```
                ┌────────────── src/ops/ (neutral) ──────────────┐
   CLI  ──────► │  fn callers(db, args) -> Result<CallersResult>  │
   MCP  ──────► │  … one op per tool, each result: Serialize +    │
                │  impl Render (to_markdown)                      │
                └─────────────────────────────────────────────────┘
                          │ present(result, Format) │
                          ▼                          ▼
                      markdown                      JSON
```

### 1. `src/ops/` — the shared layer

Move the logic currently in `src/mcp/handlers/*` into `src/ops/*`, but change the
return type from `String` to a **typed result** per tool:

```rust
pub fn callers(db: &Database, symbol: &str, limit: u32) -> Result<CallersResult, OpError>;

#[derive(Serialize)]
pub struct CallersResult { pub symbol: String, pub callers: Vec<NodeRef> }
impl Render for CallersResult { fn to_markdown(&self) -> String { /* the current md */ } }
```

Many result types already exist and already derive `Serialize` — `ImpactBreakdown`,
`ModuleGraph`, `CouplingScore`, the god-struct/dispatch rows — so those tools are
mostly a re-home, not a rewrite. The older tools (callers/callees/node/…) compute
a `Vec<Node>` today; we wrap that in a named, `Serialize` result and move their
string-building into a `Render` impl.

### 2. Presentation split → JSON on every tool

A single tiny helper, used identically by MCP and CLI:

```rust
pub enum Format { Markdown, Json }
pub fn present<T: Serialize + Render>(value: &T, fmt: Format) -> Result<String> {
    match fmt {
        Format::Markdown => Ok(value.to_markdown()),
        Format::Json     => Ok(serde_json::to_string_pretty(value)?),
    }
}
```

This is how **all 22 tools gain `--format json`** (and the MCP tools gain a uniform
`format` arg) without per-handler special-casing. The ad-hoc `format` fields and
`wants_json` checks currently scattered in a few handlers collapse into this one
path.

### 3. MCP becomes a thin wrapper

`SymgraphHandler::symgraph_callers` becomes:

```rust
let r = ops::callers(db, &req.symbol, DEFAULT_GRAPH_LIMIT)?;
present(&r, req.format.into())
```

The MCP request structs stay (they define the wire schema) but only carry args +
`format`; all real work is in `ops`.

### 4. `cli::tools` module

One thin function per command: resolve project root + open DB (reuse `resolve_db`
and the existing "no index" guard), call the matching `ops` function, `present`
in the requested format, print to stdout, map `Err` to a non-zero exit.

```rust
fn run_callers(ctx: &CliCtx, symbol: String, fmt: Format) -> Result<()> {
    let db = ctx.open_db()?;
    let r = ops::callers(&db, &symbol, DEFAULT_GRAPH_LIMIT)?;
    println!("{}", present(&r, fmt)?);
    Ok(())
}
```

`CliCtx` carries the resolved `project_root` (from `--path`, default cwd) and DB
path, so commands needing `project_root` (context, definition, diff-impact, blame,
churn, impact, module-graph, coupling-score, god-struct) get it uniformly.

### 3. Adopt `clap` (derive)

The hand-rolled `match args[1]` parser does not scale to 25 commands with
per-command flags. Add `clap = { version = "4", features = ["derive"] }` and
model the CLI as an enum:

```rust
#[derive(Parser)]
#[command(name = "symgraph", version)]
struct Cli {
    /// Project root (default: current directory)
    #[arg(long, short = 'C', global = true, default_value = ".")]
    path: String,
    /// Explicit index DB path (overrides storage resolution)
    #[arg(long, global = true)]
    db: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    Callers { symbol: String },
    Impact  { symbol: String, #[arg(long)] churn: bool, #[arg(long)] days: Option<u32>,
              #[arg(long, value_enum)] format: Option<Format> },
    ModuleGraph { #[arg(long, default_value="module")] granularity: Granularity,
                  #[arg(long)] churn: bool, #[arg(long)] days: Option<u32>,
                  #[arg(long, value_enum)] format: Option<Format>, #[arg(long)] limit: Option<u32> },
    /* … one variant per tool … */
}
```

`--db` simply seeds `SYMGRAPH_DB` (as today). Existing commands map 1:1 onto new
variants, so behaviour is preserved while the parser gets real `--help`,
validation, and enums (`--granularity file|dir|module`, `--format md|json`).

> `clap` is the de-facto standard and the only new dependency. Manual parsing is
> the alternative but would re-implement help/validation for 25 commands.

## Command surface

Existing commands keep working; new ones in **bold**.

| MCP tool | CLI command | Positional / flags | Handler |
|---|---|---|---|
| symgraph-search | `search <query>` | `--semantic` | `search::handle_search` |
| symgraph-context | `context <task>` | — | `context::handle_context` |
| symgraph-status | `status` | — | `status::handle_status` |
| symgraph-callers | **`callers <symbol>`** | — | `graph::handle_callers` |
| symgraph-callees | **`callees <symbol>`** | — | `graph::handle_callees` |
| symgraph-impact | **`impact <symbol>`** | `--churn --days --format` | `graph::handle_impact` |
| symgraph-node | **`node <symbol>`** | — | `symbol::handle_node` |
| symgraph-definition | **`definition <symbol>`** | `--context-lines` | `symbol::handle_definition` |
| symgraph-references | **`references <symbol>`** | — | `symbol::handle_references` |
| symgraph-file | **`file <path>`** | — | `file::handle_file` |
| symgraph-hierarchy | **`hierarchy <symbol>`** | — | `hierarchy::handle_hierarchy` |
| symgraph-path | **`path <from> <to>`** | — | `path::handle_path` |
| symgraph-unused | **`unused`** | — | `unused::handle_unused` |
| symgraph-implementations | **`implementations <symbol>`** | — | `implementations::handle_implementations` |
| symgraph-diff-impact | **`diff-impact`** | `--file --start --end --git-ref` | `diff_impact::handle_diff_impact` |
| symgraph-blame | **`blame <symbol>`** | — | `blame::handle_blame` |
| symgraph-churn | **`churn`** | `--path --days` | `churn::handle_churn` |
| symgraph-module-graph | **`module-graph`** | `--granularity --churn --days --format --limit` | `module_graph::handle_module_graph` |
| symgraph-coupling-score | **`coupling-score`** | (same as module-graph) | `module_graph::handle_coupling_score` |
| symgraph-god-struct | **`god-struct`** | `--churn --days --format --limit` | `god_struct::handle_god_struct` |
| symgraph-dispatch-sites | **`dispatch-sites <symbol>`** | `--format` | `dispatch_sites::handle_dispatch_sites` |
| symgraph-reindex | **`reindex [files…]`** | — | `reindex::handle_reindex` |

Plus storage/server commands (unchanged): `index`, `where`, `prune`, `serve`.

Naming: subcommands drop the `symgraph-` prefix (it's the binary name). `node`
and `definition` are distinct, mirroring the tools.

## Cross-cutting concerns

- **Project root** — global `-C/--path` (default cwd), canonicalized once into
  `CliCtx`, matching `SYMGRAPH_ROOT` semantics used by the server.
- **DB resolution** — reuse `resolve_db`; query commands error cleanly with
  "No index found — run 'symgraph index'" (the existing guard) instead of
  silently creating an empty DB.
- **Output / `--format`** — every command gets `--format md|json` (default md)
  via the `present()` helper, since each op returns a `Serialize + Render` result.
  No "unsupported" cases.
- **Exit codes** — `0` ok; `1` on handler `Err` or DB-missing; `2` on arg-parse
  error (clap default). Errors print to stderr; tool output to stdout (so agents
  can pipe stdout to `jq`).
- **`reindex`** — the CLI already has synchronous `index`; `reindex [files…]`
  maps to the incremental handler for parity, but runs to completion (no
  background task) since there's no long-lived process.

## Agent-friendliness (the second half of the issue)

- Stable stdout = tool output, stderr = diagnostics → safe to pipe.
- `--format json` on every tool (once handlers support it) makes the CLI a
  drop-in for agents that prefer shelling out over MCP.
- Ship a **skill doc** (`.claude/skills/symgraph-cli/SKILL.md`) describing the
  subcommands, mirroring the existing `explore-code` MCP skill — this is exactly
  the "give agents a CLI skill" pattern the issue calls out.

## Testing

- A table-driven integration test indexes a small fixture, then runs each
  subcommand and asserts non-empty / well-formed output (and valid JSON for
  `--format json`). This doubles as the "even just for testing" ask — the CLI
  becomes the harness for exercising every tool without an MCP client.
- Snapshot one CLI command against the corresponding MCP handler output to lock
  in equivalence.

## Rollout

1. **Scaffolding** — create `src/ops/` with the `Render` trait + `present()` +
   `Format`; add `clap`; migrate the 7 existing commands to the clap enum (no
   behaviour change). Backward compatible.
2. **Migrate tools to ops, vertical slice per tool** — for each tool: define its
   typed `Serialize` result + `Render` impl in `ops`, point the MCP handler at
   `ops::… + present`, and add the CLI subcommand. Start with the tools whose
   result types already derive `Serialize` (impact, module-graph, coupling-score,
   god-struct, dispatch-sites), then the `Vec<Node>` tools (callers/callees/node/
   definition/references/file/hierarchy/path/unused/implementations), then the
   git ones (blame/churn/diff-impact/reindex).
3. **Agent-grade** — confirm `--format json` on all 22, add the skill doc, and
   the table-driven integration test.

Each tool in step 2 is an independent, shippable slice; MCP output is regression-
checked against the pre-refactor markdown as we go.

## Decisions (confirmed)

1. **Parser:** adopt `clap` (derive).
2. **Reuse:** extract a shared, provider-neutral `ops` layer (not just exposing
   the existing handlers) — MCP and CLI both front-end it.
3. **JSON:** add structured JSON to **all** tools via the typed-result +
   `present()` design, benefiting the MCP server as well as the CLI.

## Risks / notes

- This refactors all 22 handlers (logic re-homed to `ops`, formatting split into
  `Render`). Mitigation: per-tool vertical slices, each regression-checked
  against current markdown; the request/response *wire* schema for MCP is
  preserved.
- A few handlers already carry bespoke `format`/`wants_json` logic; those get
  *simpler* (deleted in favour of `present()`), not harder.
- `Render` output must match today's markdown verbatim where we want zero
  user-visible change; snapshot tests lock this in.
