---
name: symgraph
description: Explore and analyse a codebase with the symgraph CLI — find symbols, trace callers and callees, assess change impact, inspect coupling, and check whether the index can be trusted. Use when working in a repository that has a symgraph index, or when symbol-level questions ("who calls this?", "what breaks if I change it?") would otherwise need grep. Prefer this over the symgraph MCP tools when shelling out is cheaper than tool calls.
---

# symgraph

`symgraph` answers symbol-level questions about a codebase from a pre-built
index. Every command supports `--format json`, writes results to stdout and
diagnostics to stderr, so output is safe to pipe into `jq`.

One binary does both jobs: the commands below, and `symgraph serve` for the
MCP server. Releases up to 2026.9.20 also installed a second `symgraph-cli`
binary; if one is still on your PATH it is stale, and should be removed.

## Before anything else

```sh
symgraph index .      # incremental: only changed files
symgraph status .     # is there an index, and can it be trusted?
```

Query commands fail with "No index found" until `index` has run. Re-run it
after edits, or use `symgraph watch`.

## Read the trust signals

`status` reports two different things, and the second matters more:

```
Files: 46          Symbols: 1445         <- how much is indexed
Index health:
  Built by: symgraph 2026.7.21 (schema v1, extractor v2)
  Ambiguous names: 129 of 1145 (11.3%)   <- how much of it is a guess
  Unresolved references: 6545
```

- **Out of date** warning → run `symgraph reindex`; answers reflect older
  extraction semantics until you do.
- **Ambiguous names** is the share of names carried by more than one
  definition. Resolution picks the highest-preference one, so results for a
  common name (`new`, `run`, `handle`) are a best guess unless narrowed.

## Commands

### Finding things

| Command | Use it for |
|---|---|
| `search <query> [--semantic] [--limit N]` | Symbol by name; `--semantic` searches identifier fragments and docstrings instead, and is better for a description than a name |
| `context <task...> [--limit N]` | Entry points and code for a free-text task |
| `node <symbol>` | One symbol's metadata |
| `definition <symbol> [--context-lines N]` | Its source |
| `file <path>` | Everything defined in a file (repo-relative, forward slashes) |

### Relationships

| Command | Use it for |
|---|---|
| `callers <symbol>` | Who calls it |
| `callees <symbol>` | What it calls |
| `references <symbol>` | Every usage, grouped by kind |
| `hierarchy <symbol>` | Parent/child containment |
| `implementations <symbol>` | Types implementing a trait or interface |
| `path <from> <to>` | How one function reaches another |
| `unused [--limit N] [--offset N] [--ignore-test-callers]` | Symbols with no incoming references |

### Change and risk

| Command | Use it for |
|---|---|
| `impact <symbol> [--churn] [--days N]` | Blast radius, broken down by coupling kind |
| `diff-impact [--git-ref REF \| --file F --start N --end N]` | What a change touches |
| `blame <symbol>` | Who last changed each line of it |
| `churn [PATH] [--days N] [--limit N]` | Change frequency — the volatility signal |

### Architecture

| Command | Use it for |
|---|---|
| `module-graph [--granularity file\|dir\|module] [--churn] [--include-tests] [--limit N]` | Dependencies, fan-in/out, cycles |
| `coupling-score [same flags]` | Hotspots ranked by strength × distance × volatility |
| `god-struct [--churn] [--include-tests] [--limit N]` | Structs ranked by architectural debt |
| `dispatch-sites <enum>` | Every file matching on an enum's members |

## Two things the output tells you — read them

**Truncation.** Lists are paged. A result that says `Found 20 of 84 callers`
is a page, not an answer:

```
> **Truncated:** showing 20 of 84. Pass `offset=20` for the next page, or raise `limit`.
```

Use `--limit` / `--offset`. In JSON this is `total`, `shown`, `truncated` —
never read `shown` as the total.

**Ambiguity.** Resolution is name-based. When several definitions share a name:

```
> **Ambiguous:** 7 definitions share this name; showing the first.
> Narrow it with `file` or `qualified_name`.
```

Narrow it and re-run — the numbers change substantially:

```sh
symgraph callers new                          # 84 callers, across 7 definitions
symgraph callers new --file src/graph/mod.rs  # 22 — the ones that are real
```

`--file` and `--qualified-name` work on every symbol command.

## Recipes

```sh
# Is this change safe? Impact, weighted by how often the callers churn.
symgraph impact Database --churn --days 90

# What did my branch touch, symbol by symbol?
symgraph diff-impact --git-ref main --format json | jq '.regions[].direct[].name'

# Where is the architectural debt?
symgraph coupling-score --churn --limit 20
symgraph god-struct --churn --limit 10

# Can I delete this? Check nothing references it, then confirm it is not a
# false positive from an ambiguous name.
symgraph references helper --format json | jq '.total'
symgraph node helper --format json | jq '.resolution'

# Before replacing an enum with a trait, find every dispatch site.
symgraph dispatch-sites NodeKind
```

## Notes and limits

- Paths are repo-relative with forward slashes on every platform.
- Coupling tools read import, field and dispatch edges, so `reindex` after
  edits before trusting them.
- The three coupling tools (`module-graph`, `coupling-score`, `god-struct`)
  report on production code only. Test code depends on everything, so counting
  it inflates fan-in and merges unrelated modules; pass `--include-tests` when
  you want it. Every other tool — `callers` and `references` especially —
  still shows test callers, because "the tests call this" is usually the point.
- Cycle membership in `module-graph` is still approximate: a call whose target
  type cannot be inferred resolves by name alone, and on symgraph's own index
  26% of cross-file calls still land in a file the caller does not import.
  A handful of those merge modules that are not really cyclic. Fan-in/out and
  the coupling ranking are reliable; treat SCCs as a prompt to look, not a
  verdict.
- Go reports no `implementations`: interface satisfaction is implicit, so
  there is nothing in the syntax to index.
- `symgraph serve` runs the MCP server, exposing these same tools over MCP.
  Both front-ends call the same code, so the answers are identical.
