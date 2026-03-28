---
name: explore-code
description: Use codemap MCP tools to explore and understand code. Use when the user asks about code structure, symbol relationships, call graphs, impact analysis, or needs context for a coding task.
argument-hint: "<question or task description>"
allowed-tools: mcp__codemap__codemap-context, mcp__codemap__codemap-search, mcp__codemap__codemap-callers, mcp__codemap__codemap-callees, mcp__codemap__codemap-impact, mcp__codemap__codemap-definition, mcp__codemap__codemap-file, mcp__codemap__codemap-references, mcp__codemap__codemap-node, mcp__codemap__codemap-hierarchy, mcp__codemap__codemap-path, mcp__codemap__codemap-unused, mcp__codemap__codemap-implementations, mcp__codemap__codemap-diff-impact, mcp__codemap__codemap-status, mcp__codemap__codemap-reindex, Read, Grep, Glob
---

You have access to codemap, a semantic code intelligence tool that maintains a knowledge graph of the codebase. Use it to answer the user's question: $ARGUMENTS

## Strategy

1. Start with `codemap-search` to find relevant symbols by name
2. Use `codemap-definition` to read the actual source code of symbols
3. Use `codemap-callers` and `codemap-callees` to trace call relationships
4. Use `codemap-impact` to understand what would be affected by changes
5. Use `codemap-context` for broad task-oriented exploration
6. Use `codemap-hierarchy` for class/module inheritance trees
7. Use `codemap-path` to find how two functions are connected
8. Use `codemap-references` to find all usages of a symbol
9. Use `codemap-unused` to find dead code
10. Use `codemap-implementations` to find trait/interface implementations
11. Use `codemap-diff-impact` to assess impact of changes to a specific code region
12. Use `codemap-file` to list all symbols in a file
13. If the index seems stale, run `codemap-reindex` first

## Guidelines

- Prefer targeted tool calls over broad searches
- When tracing a call chain, follow it step by step using callers/callees rather than guessing
- Always show the user the relevant source code when explaining behavior
- If a symbol search returns multiple matches, clarify which one is relevant before diving deeper
- Combine codemap results with direct file reads when you need surrounding context beyond what codemap provides
