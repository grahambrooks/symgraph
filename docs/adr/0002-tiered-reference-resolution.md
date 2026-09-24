# ADR 0002 — Tiered reference resolution

- **Status:** accepted
- **Date:** 2026-09-23
- **Supersedes nothing. Implements work item 1 of [ADR 0001](0001-storage-engine.md).**
- **Decision:** resolve the preference ladder as three materialised tiers
  joined by `COALESCE`, instead of ranking every candidate per reference.

## Context

[ADR 0001](0001-storage-engine.md) measured indexing at **O(n^2.02)** and
traced it to one query. Reference resolution asked, for each reference, "scan
every definition sharing this name, rank them by the preference ladder, take
the first". Its cost was O(candidates for that name), and in a growing codebase
the number of references and the number of same-named definitions both grow
linearly. That product was the quadratic.

At 4,000 files of real Rust, `new` had 1,482 definitions. A reference to a name
with that many candidates cost **1,050 µs**; a reference to a unique name cost
**1.6 µs** — a 660× spread tracking candidate count exactly. The subquery was
also written out three times in `resolve_phase` (the `INSERT`'s select list,
the same statement's `WHERE`, and the `DELETE` that cleared resolved refs), so
SQLite evaluated it three times per reference.

## Decision

The picker reads only `(reference_name, kind, file_path)`. Nothing else in the
reference row influences the choice. Two consequences follow, and the
implementation uses both.

**Resolve once per distinct triple, not once per reference.** On real corpora
that is roughly three references per triple, and the answer is joined back to
every reference that shares it.

**Split the ladder into three tiers, materialised in turn:**

| tier | meaning | keyed on | bounded by |
|---|---|---|---|
| 1 | a definition in the referencing file | `(name, kind, file)` | one file's contents |
| 2 | a definition in a file the referencing file imports from | `(name, kind, file)` | that file's actual imports |
| 3 | the global preference winner for the name | **`(name, kind)`** | computed once per pair |

Tier 3 is the one that carried the quadratic, and it does not depend on the
referencing file at all. Computing it once per distinct `(name, kind)` rather
than once per reference is the entire fix: `new`/`calls` is now ranked once for
the whole index instead of thousands of times.

`COALESCE(tier1, tier2, tier3)` reproduces the ladder exactly. Every tier
carries the same tiebreak (`is_test`, `is_generated`, `file_path`,
`start_line`, `id`), extracted into `RESOLUTION_TIEBREAK` so it is stated once
and shared with `resolve_symbol`. `import_scope` never pairs a file with
itself, so tier 2 cannot re-offer a tier 1 candidate — which is why the
`COALESCE` is equivalent to the original ordered `CASE`.

The materialised result is then used by both the `INSERT` and the `DELETE`, so
the picker is evaluated once per phase rather than three times per reference.

## Results

Same corpus as ADR 0001 — real Rust sampled evenly across the crates.io
registry cache, Apple Silicon, release build.

| files | before | after | speedup |
|---:|---:|---:|---:|
| 500 | 1.9 s | 0.8 s | 2.4× |
| 1,000 | 7.1 s | 2.6 s | 2.7× |
| 2,000 | 24.7 s | 5.8 s | 4.3× |
| 4,000 | 97.3 s | 12.8 s | 7.6× |
| 8,000 | 511.1 s | **39.6 s** | **12.9×** |

The speedup grows with the corpus, which is what distinguishes an asymptotic
fix from a constant-factor one. Fitted exponent over the full range:

**O(n^2.02) → O(n^1.41)**

Projected from the 8,000-file anchor:

| files | before | after |
|---:|---:|---:|
| 16,000 | ~35 min | ~1.8 min |
| 50,000 | ~5.7 h | ~8.7 min |
| 100,000 | ~23 h | **~23 min** |

A 100k-file monorepo moves from "not indexable" to "a coffee break".

## Equivalence

This changes how the answer is computed, not what the answer is, so the claim
had to be checked rather than asserted.

The full edge set was dumped before and after as
`source_file|source_name|source_line|kind|target_file|target_name|target_line`,
sorted, and diffed:

| corpus | edges | result |
|---|---:|---|
| 2,000 files | 279,361 | **identical** |
| 4,000 files | 594,099 | **identical** |

Edge counts also match exactly at 500, 1,000 and 8,000 files (50,249 / 140,265
/ 1,385,119 — the same figures ADR 0001 recorded for the old implementation).

Three regression tests pin the behaviour the refactor could plausibly have
broken:

- `resolution_prefers_the_same_file_over_an_imported_one` — tier 1 outranks
  tier 2. The pre-existing `test_resolve_prefers_same_file` only covered tier 1
  against tier 3.
- `the_global_tier_is_shared_across_referencing_files` — two references from
  different files both reach the one global winner, pinning the `(name, kind)`
  keying that removed the quadratic.
- `every_reference_gets_an_edge_even_when_the_pick_is_deduplicated` — five
  references sharing a triple each still get an edge, pinning the join-back.

Tier 2 against tier 3 was already covered by
`test_resolution_prefers_an_imported_file_over_an_arbitrary_one`, by a more
realistic route (it resolves the import first), so no fourth test was added.

## Consequences

- Resolution now allocates five temporary tables per phase, dropped and rebuilt
  each time. Peak memory during resolution rises with the number of distinct
  triples rather than staying flat; at 8,000 files the index process stayed
  well within normal bounds, but this is the thing to watch if a much larger
  corpus misbehaves.
- The SQL is longer and there are four statements where there was one. The
  ladder is no longer readable as a single `ORDER BY`, which is why the tier
  table above and the comment in `resolve_phase` exist.
- `kind_compatible_sql` now takes the reference alias as a parameter, since the
  tiers alias the reference table as `r` rather than `u`.

## What this does not fix

Resolution is still name-based. ADR 0001's finding stands: a method call on a
receiver whose type cannot be inferred still matches any same-named
definition, 26% of production cross-file calls still land outside the caller's
imports, and `module-graph` SCC membership is still not trustworthy. This ADR
made resolution *fast*; it did not make it *right*.

Of the other items in ADR 0001, `god-struct` has since been fixed (98.6 s →
0.76 s at 4,000 files, and the name-collision misattribution with it). The
covering index is not added, traversal still uses per-node queries rather than
recursive CTEs, and `Mutex<Database>` still serialises reads.

ADR 0001's revisit trigger was "fitted exponent above ~1.3 after items 1–4
land". At 1.41 with item 4 still outstanding, the storage decision stands and
should be re-tested once it does.
