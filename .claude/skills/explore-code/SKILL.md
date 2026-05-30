---
name: explore-code
description: Use symgraph MCP tools to explore and understand code — structure, symbol relationships, call graphs, impact analysis, coupling/architecture, git history, or building context for a coding task.
argument-hint: "<question or task description>"
allowed-tools: mcp__symgraph__symgraph-context, mcp__symgraph__symgraph-search, mcp__symgraph__symgraph-callers, mcp__symgraph__symgraph-callees, mcp__symgraph__symgraph-impact, mcp__symgraph__symgraph-definition, mcp__symgraph__symgraph-file, mcp__symgraph__symgraph-references, mcp__symgraph__symgraph-node, mcp__symgraph__symgraph-hierarchy, mcp__symgraph__symgraph-path, mcp__symgraph__symgraph-unused, mcp__symgraph__symgraph-implementations, mcp__symgraph__symgraph-diff-impact, mcp__symgraph__symgraph-blame, mcp__symgraph__symgraph-churn, mcp__symgraph__symgraph-module-graph, mcp__symgraph__symgraph-coupling-score, mcp__symgraph__symgraph-god-struct, mcp__symgraph__symgraph-dispatch-sites, mcp__symgraph__symgraph-status, mcp__symgraph__symgraph-reindex, Read, Grep, Glob
---

You have access to symgraph, a semantic code intelligence tool that maintains a knowledge graph of the codebase. Use it to answer the user's question: $ARGUMENTS

## Explore symbols and relationships

1. Start with `symgraph-search` to find relevant symbols by name
2. Use `symgraph-definition` to read the actual source code of symbols
3. Use `symgraph-callers` and `symgraph-callees` to trace call relationships
4. Use `symgraph-impact` to understand what would be affected by a change. It now
   also breaks inbound coupling down by edge kind — **method-call (contract),
   field-read (model), field-write/`&mut` (intrusive)** — plus inbound module
   count; pass `churn: true` to annotate each inbound module's volatility
5. Use `symgraph-context` for broad task-oriented exploration
6. Use `symgraph-hierarchy` for class/module inheritance trees
7. Use `symgraph-path` to find how two functions are connected
8. Use `symgraph-references` to find all usages of a symbol
9. Use `symgraph-unused` to find dead code
10. Use `symgraph-implementations` to find trait/interface implementations
11. Use `symgraph-diff-impact` to assess impact of changes to a specific region
12. Use `symgraph-file` to list all symbols in a file

## Coupling and architecture

Use these to reason about design quality (strength × distance × volatility):

- `symgraph-module-graph` — fold the graph to a `file`/`dir`/`module` boundary;
  returns the dependency adjacency list, fan-in/fan-out per node, and detected
  cycles (SCCs). Fan-in ranking surfaces hubs
- `symgraph-coupling-score` — rank module pairs by strength × distance ×
  volatility (churn); the hotspots table for "where is the risky coupling"
- `symgraph-god-struct` — structs/classes ranked by pub-field count × inbound
  references × churn; the "where is the architectural debt" entry point
- `symgraph-dispatch-sites` — every file that matches/switches on a given enum's
  members (control coupling); use to verify completeness before a trait refactor

All of the above accept `format: "json"` for machine-aggregable output.

## Git history (volatility)

- `symgraph-churn` — file change frequency over a recent window (the volatility
  dimension; hotspots most likely to harbor bugs)
- `symgraph-blame` — git blame over a symbol's definition lines

## Index maintenance

- The coupling/architecture tools rely on field-access, import, and enum-dispatch
  edges populated during indexing. If results look empty or stale, run
  `symgraph-reindex` first (and after code edits)
- Use `symgraph-status` to check how much is indexed

## Guidelines

- Prefer targeted tool calls over broad searches
- When tracing a call chain, follow it step by step using callers/callees rather than guessing
- Always show the user the relevant source code when explaining behavior
- If a symbol search returns multiple matches, clarify which one is relevant before diving deeper
- For coupling analysis, treat the strength/volatility *ranking* as reliable but
  cycle membership as approximate — edge resolution is name-based (heuristic)
- Combine symgraph results with direct file reads when you need surrounding context beyond what symgraph provides
