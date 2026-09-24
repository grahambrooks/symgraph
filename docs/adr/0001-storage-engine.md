# ADR 0001 — Storage engine for the symbol index

- **Status:** accepted
- **Date:** 2026-09-23
- **Decision:** keep SQLite; fix the algorithms that are actually quadratic.
- **Follow-up:** work item 1 is done — see
  [ADR 0002](0002-tiered-reference-resolution.md). Indexing went from
  O(n^2.02) to O(n^1.41), 12.9× faster at 8,000 files, with a byte-identical
  edge set. Items 2–6 remain open.

## Question

symgraph stores a property graph — symbols and the relationships between them —
in SQLite. Indexing large repositories is slow. Would a different storage
technology handle the data model better, scale further, or run faster?

The answer turned out to depend on a prior question: *what is actually slow?*
That was measured before any engine was compared, and it changed the
conclusion.

## What was measured

A corpus of real Rust source sampled evenly across the local crates.io registry
cache (75,033 files, deterministic 1-in-N stride, so no single crate's style
dominates), copied into trees of increasing size and indexed with
`symgraph reindex`. Apple Silicon, release build, warm page cache.

| files | index time | db size | nodes | edges | unresolved refs |
|---:|---:|---:|---:|---:|---:|
| 500 | 1.9 s | 11 MB | 11,937 | 50,249 | 22,026 |
| 1,000 | 7.1 s | 43 MB | 70,053 | 140,265 | 45,453 |
| 2,000 | 24.7 s | 68 MB | 96,281 | 279,361 | 82,348 |
| 4,000 | 97.3 s | 114 MB | 140,321 | 594,099 | 104,783 |
| 8,000 | 511.1 s | 347 MB | 571,677 | 1,385,119 | 154,858 |

Each doubling costs 3.74×, 3.48×, 3.94×, then **5.25×** — the cost per doubling
is itself rising. Fitted over the full range that is **O(n^2.02)**: quadratic,
and if anything drifting worse as hot-name candidate lists grow. Extrapolating
from the 8,000 anchor:

| files | projected index time |
|---:|---:|
| 16,000 | ~35 min |
| 50,000 | ~5.7 h |
| 100,000 | ~23 h |

A 100k-file monorepo is not indexable, and the wall arrives well before that.
Index size grows in step: 347 MB at 8,000 files is ~43 KB per file, which
projects to several GB at monorepo scale.

Read latency at 4,000 files (140k symbols, 594k edges), including process
start:

| command | latency |
|---|---:|
| `node`, `impact`, `references`, `hierarchy`, `path` | 20–40 ms |
| `search`, `callers` | ~55 ms |
| `unused` | 255 ms |
| `module-graph`, `coupling-score` | ~400 ms |
| **`god-struct`** | **98,603 ms** |

So reads are healthy except for one outlier, and the outlier is not a storage
problem (see below).

## Why indexing is quadratic

Reference resolution matches a name to a definition. The picker is a correlated
subquery: for one reference it scans every node sharing the name, evaluates a
preference ladder over each candidate — including an `EXISTS` against the
import-scope table — sorts them, and takes the first.

Its cost is therefore **O(candidates for that name)**, and in a growing
codebase both the number of references *and* the number of same-named
definitions grow linearly. That product is the quadratic.

The effect is not subtle. At 4,000 files, `new` has **1,482 definitions**
(`default` 1,246, `fmt` 1,087). Timing 2,000 references each way:

| reference target | per reference |
|---|---:|
| a name with 1,482 candidates | **1,050 µs** |
| a name with 1 candidate | **1.6 µs** |

A **660× spread**, tracking candidate count exactly.

Worse, that subquery is written out three times in `resolve_phase` — once in the
`INSERT`'s select list, once in the same statement's `WHERE`, and once in the
`DELETE` that clears the refs that resolved. SQLite evaluates it three times.

Measured on the true resolution workload at 4,000 files (489,370 references,
126,687 distinct `(name, kind, file)` triples):

| approach | time |
|---|---:|
| **A — current**: correlated subquery, ×3 | 41.7 s each, **~125 s total** |
| **B — deduplicate, one window-function pass**, result reused by insert and delete | **23.1 s** |
| **C — resolve the three tiers separately** (prototype) | **~6.5 s** |

C splits the ladder instead of sorting every candidate per reference: the
global preference winner per `(name, kind)` is computed once for the whole
index (a small table), same-file hits come from an indexed join, and the
import-scope tier is bounded by the imports a file actually has rather than by
hot-name fan-out. It is the only one of the three whose cost is not driven by
candidate count, so it is the one that changes the asymptote.

**Caveat:** C was a prototype measured in `sqlite3`, not a port of the
production semantics — it resolved 88,937 references against B's 94,241, so it
was not equivalent. ~19× was the shape of the available win, not a promise.

*Outcome:* the ported version is equivalent — a byte-identical edge set — and
delivered 7.6× at this corpus size and 12.9× at 8,000 files. See
[ADR 0002](0002-tiered-reference-resolution.md).

`god-struct`'s 98 seconds is a separate, simpler fault: it loops over 7,239
structs issuing per-struct and per-field queries. `get_struct_fields` also
matches **by name**, so with 80 structs called `Struct` each iteration pulls
all 80 structs' fields — quadratic work *and* misattributed fields. One
`GROUP BY` replaces the whole loop.

## Options considered

### 1. Keep SQLite, fix the algorithms — *chosen*

Everything measured above is a query-plan or N+1 problem, not a storage
problem. None of it is caused by SQLite's data model, and none of it would be
fixed by moving the same algorithms onto a different engine: "for each
reference, scan and rank every same-named definition" is quadratic on a graph
database too.

SQLite also already provides what the workload needs — indexed point lookups,
grouped aggregation, window functions, recursive CTEs for multi-hop traversal,
and FTS5 for both name search and the bm25 semantic index — and it is the
healthiest dependency in the tree (`rusqlite` 0.40.2, released 2026-08-08,
110M downloads).

Work items, in the order they pay:

1. **Tiered resolution** (option C above), replacing the per-reference sort.
   The asymptote fix. *Done — [ADR 0002](0002-tiered-reference-resolution.md).*
2. **Evaluate the picker once**, materialised into a temp table used by both
   the insert and the delete. ~3× on its own; do it as part of 1.
   *Done, with item 1.*
3. **Rewrite `god-struct` as one aggregate query**, and fix `get_struct_fields`
   to key on the struct's node id rather than its name — a correctness fix as
   well as a 98-second one.
4. **Covering index on `nodes(name, kind, file_path)`** so candidate scans stay
   in the index.
5. **Recursive CTEs** for `traverse`/`path`, replacing the per-node query loop
   in `graph/mod.rs`. Not urgent — these read paths measure 20–40 ms — but it
   removes the N+1 before a deeper traversal is ever added.
6. **A connection pool** in place of `Mutex<Database>`, which currently
   serialises every read. A concurrency fix, independent of the above.

### 2. An embedded graph database

Structurally the closest fit, and the category is effectively empty:

- **Kuzu** — embedded property graph, Cypher, MIT, the strongest candidate on
  paper. Its vendor was acquired by Apple in October 2025; the project is
  **archived** and the last Rust release was 2025-10-10. Ruled out.
- **CozoDB** — embedded Datalog graph store in Rust. Last crates.io release
  **0.7.6, 2023-12-11** — nearly three years stale. Ruled out.

Beyond availability, a graph engine would help the multi-hop traversals, which
are already 20–40 ms, and would not help resolution, which is a ranked-lookup
problem rather than a traversal one. It would solve the part that is not
broken.

### 3. DuckDB

Healthy and fast-moving (1.10505.0, 2026-07-22, 4.1M downloads), and columnar
execution would genuinely suit the aggregation tools — `module-graph`,
`coupling-score` — which are the heaviest legitimate read paths.

Against it: those paths already run in ~400 ms; DuckDB is built for analytics
rather than the single-row point lookups and per-file incremental writes that
dominate this workload; it has no FTS5 equivalent in place, so name and
semantic search would have to be rebuilt; and it would add substantially to a
binary that is already 35 MB and delivered by `curl | bash`. Revisit if the
coupling tools become the bottleneck, not before.

### 4. A pure key-value store (redb, sled)

`redb` is healthy (4.3.0, 2026-09-15). But a KV store supplies no query
planner, no joins, no aggregation and no full-text search, so every query in
`db/mod.rs` would be hand-written traversal code. That is a large rewrite whose
payoff is bounded by how well the hand-written plans beat SQLite's — and the
current problem is precisely that the hand-written *algorithm* is bad. Moving
more logic into hand-written form is the wrong direction.

### 5. A server database (Postgres + AGE, Neo4j, Memgraph)

Breaks the product. symgraph is a CLI and an MCP server that must work offline,
install from one binary, and index a repository with no setup. Requiring a
server to be provisioned is a different product. Not considered further.

### 6. RDF / SPARQL (oxigraph)

Healthy (0.5.11, 2026-09-02), but the triple model discards the typed
node/edge attributes the tools rank on (`is_test`, `visibility`, line spans,
per-edge `detail`), and SPARQL is a poor fit for the aggregate reports. Worse
model fit than the status quo.

## Decision

Keep SQLite. Fix items 1–4 above, then re-measure the scaling curve on the same
corpus before considering any engine change.

## When to revisit

Concrete triggers, so this is re-opened on evidence rather than instinct:

- The scaling curve stays materially superlinear (fitted exponent above ~1.3,
  against the measured 2.02) *after* items 1–4 land.
- Aggregation reads — `module-graph`, `coupling-score` — exceed ~2 s on a
  50k-file index, which would make DuckDB's columnar execution worth its cost.
- Traversal depth grows beyond the current 1–2 hops such that recursive CTEs
  stop being enough.
- A maintained embedded property-graph engine appears with a healthy Rust
  binding. The category was viable a year ago and is not now; it may be again.

## Notes on the evidence

Corpus, timings and the three resolution prototypes were produced on
2026-09-23 against commit `8068d5a`. The fit uses all five points, 500–8,000;
the resolution head-to-head was run at 4,000. Third-party release data is from
the crates.io API on the same date and should be re-checked before acting on
any of the rejected options.
