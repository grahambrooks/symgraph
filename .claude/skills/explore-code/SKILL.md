---
name: explore-code
description: Use symgraph MCP tools to explore and understand code. Use when the user asks about code structure, symbol relationships, call graphs, impact analysis, or needs context for a coding task.
argument-hint: "<question or task description>"
allowed-tools: mcp__symgraph__symgraph-context, mcp__symgraph__symgraph-search, mcp__symgraph__symgraph-callers, mcp__symgraph__symgraph-callees, mcp__symgraph__symgraph-impact, mcp__symgraph__symgraph-definition, mcp__symgraph__symgraph-file, mcp__symgraph__symgraph-references, mcp__symgraph__symgraph-node, mcp__symgraph__symgraph-hierarchy, mcp__symgraph__symgraph-path, mcp__symgraph__symgraph-unused, mcp__symgraph__symgraph-implementations, mcp__symgraph__symgraph-diff-impact, mcp__symgraph__symgraph-status, mcp__symgraph__symgraph-reindex, Read, Grep, Glob
---

You have access to symgraph, a semantic code intelligence tool that maintains a knowledge graph of the codebase. Use it to answer the user's question: $ARGUMENTS

## Strategy

1. Start with `symgraph-search` to find relevant symbols by name
2. Use `symgraph-definition` to read the actual source code of symbols
3. Use `symgraph-callers` and `symgraph-callees` to trace call relationships
4. Use `symgraph-impact` to understand what would be affected by changes
5. Use `symgraph-context` for broad task-oriented exploration
6. Use `symgraph-hierarchy` for class/module inheritance trees
7. Use `symgraph-path` to find how two functions are connected
8. Use `symgraph-references` to find all usages of a symbol
9. Use `symgraph-unused` to find dead code
10. Use `symgraph-implementations` to find trait/interface implementations
11. Use `symgraph-diff-impact` to assess impact of changes to a specific code region
12. Use `symgraph-file` to list all symbols in a file
13. If the index seems stale, run `symgraph-reindex` first

## Guidelines

- Prefer targeted tool calls over broad searches
- When tracing a call chain, follow it step by step using callers/callees rather than guessing
- Always show the user the relevant source code when explaining behavior
- If a symbol search returns multiple matches, clarify which one is relevant before diving deeper
- Combine symgraph results with direct file reads when you need surrounding context beyond what symgraph provides
