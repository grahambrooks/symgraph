# symgraph

[![CI](https://github.com/grahambrooks/symgraph/actions/workflows/ci.yml/badge.svg)](https://github.com/grahambrooks/symgraph/actions/workflows/ci.yml)
[![Release](https://github.com/grahambrooks/symgraph/actions/workflows/release.yml/badge.svg)](https://github.com/grahambrooks/symgraph/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-blue.svg)](https://www.rust-lang.org)
[![MCP](https://img.shields.io/badge/MCP-compatible-green.svg)](https://modelcontextprotocol.io)

Semantic code intelligence MCP server - build knowledge graphs of codebases to enhance AI-assisted code exploration.

symgraph is a rust implementation of https://github.com/colbymchenry/codegraph. Why? Ongoing exploration of compiled
binary deployment of MCP Servers.

## Features

- **Multi-language support**: Rust, TypeScript, JavaScript, Python, Go, Java, C, C++
- **Symbol extraction**: functions, classes, methods, structs, interfaces, traits, enums, constants
- **Relationship tracking**: calls, contains, imports, exports, extends, implements
- **Impact analysis**: trace the effect of changes through the codebase
- **Advanced code intelligence**:
    - Find call paths between functions
    - Detect unused/dead code
    - Explore class hierarchies
    - Locate all interface implementations
    - Analyze change impact by line range
- **Incremental indexing**: only re-indexes changed files using content hashing
- **Dual transport**: stdio (default) and HTTP server modes

## Installation

### macOS & Linux

```sh
curl -fsSL https://raw.githubusercontent.com/grahambrooks/symgraph/main/install.sh | bash
```

To also configure symgraph as an MCP server for Claude Code and Claude Desktop:

```sh
curl -fsSL https://raw.githubusercontent.com/grahambrooks/symgraph/main/install.sh | bash -s -- --mcp
```

Install a specific version:

```sh
SYMGRAPH_VERSION=2026.3.30 curl -fsSL https://raw.githubusercontent.com/grahambrooks/symgraph/main/install.sh | bash
```

### Windows (PowerShell)

```powershell
irm https://raw.githubusercontent.com/grahambrooks/symgraph/main/install.ps1 -OutFile install.ps1; .\install.ps1
```

To also configure symgraph as an MCP server for Claude Code and Claude Desktop:

```powershell
irm https://raw.githubusercontent.com/grahambrooks/symgraph/main/install.ps1 -OutFile install.ps1; .\install.ps1 -Mcp
```

Install a specific version:

```powershell
$env:SYMGRAPH_VERSION="2026.3.30"; irm https://raw.githubusercontent.com/grahambrooks/symgraph/main/install.ps1 -OutFile install.ps1; .\install.ps1
```

Both scripts install to `~/.symgraph/bin/` by default. Override with `SYMGRAPH_INSTALL_DIR`.

### Claude Desktop (MCPB Bundle)

Download and install the MCPB bundle for your platform:

| Platform              | Download                                                                                   |
|-----------------------|--------------------------------------------------------------------------------------------|
| macOS (Apple Silicon) | [symgraph-x.x.x-darwin-arm64.mcpb](https://github.com/grahambrooks/symgraph/releases/latest) |
| macOS (Intel)         | [symgraph-x.x.x-darwin-x64.mcpb](https://github.com/grahambrooks/symgraph/releases/latest)   |
| Windows               | [symgraph-x.x.x-windows-x64.mcpb](https://github.com/grahambrooks/symgraph/releases/latest)  |
| Linux                 | [symgraph-x.x.x-linux-x64.mcpb](https://github.com/grahambrooks/symgraph/releases/latest)    |

1. Download the `.mcpb` file for your platform from [Releases](https://github.com/grahambrooks/symgraph/releases/latest)
2. Open Claude Desktop
3. Drag and drop the `.mcpb` file onto Claude Desktop, or use **File > Install MCP Server**
4. Configure the project root when prompted

### From Source

```bash
git clone https://github.com/grahambrooks/symgraph
cd symgraph
make install
```

## Usage

### Index a Codebase

```bash
# Index current directory
symgraph index

# Index specific directory
symgraph index ~/projects/myapp
```

### Start MCP Server

```bash
# Start with stdio transport (for Claude Desktop)
symgraph serve

# Start with HTTP transport
symgraph serve --port 8080
```

### CLI Commands

```bash
symgraph index [path]           # Index a codebase
symgraph serve                  # Start MCP server (stdio)
symgraph serve --port <PORT>    # Start MCP server (HTTP)
symgraph status [path]          # Show index statistics
symgraph search <query>         # Search for symbols
symgraph context <task>         # Build context for a task
```

## MCP Tools

### Core Tools

| Tool                 | Description                                       |
|----------------------|---------------------------------------------------|
| `symgraph-context`    | Build focused code context for a specific task    |
| `symgraph-search`     | Quick symbol search by name                       |
| `symgraph-callers`    | Find all callers of a symbol                      |
| `symgraph-callees`    | Find all callees of a symbol                      |
| `symgraph-impact`     | Analyze the impact radius of changes              |
| `symgraph-node`       | Get detailed symbol information                   |
| `symgraph-definition` | Get the full source code of a symbol with context |
| `symgraph-file`       | List all symbols defined in a specific file       |
| `symgraph-references` | Find all references to a symbol                   |
| `symgraph-reindex`    | Trigger incremental reindexing of changed files   |
| `symgraph-status`     | Get index statistics                              |

### Advanced Tools

| Tool                      | Description                                             |
|---------------------------|---------------------------------------------------------|
| `symgraph-hierarchy`       | Get class/module hierarchy (parent/child relationships) |
| `symgraph-path`            | Find call paths between two symbols                     |
| `symgraph-unused`          | Find unused/dead code with no incoming references       |
| `symgraph-implementations` | Find all implementations of an interface/trait          |
| `symgraph-diff-impact`     | Analyze the impact of changing a specific code region   |

### Example Use Cases

**Find dead code for cleanup:**

```
Use symgraph-unused to find all unused functions and classes
```

**Understand function call chains:**

```
Use symgraph-path with from="main" and to="database_query" to see how data flows
```

**Assess change impact:**

```
Use symgraph-diff-impact with file_path="src/auth.rs" start_line=45 end_line=60
to see what would be affected by changes in that region
```

**Explore OOP hierarchies:**

```
Use symgraph-hierarchy with symbol="BaseHandler" to see all parent/child relationships
```

**Find all trait implementations:**

```
Use symgraph-implementations with symbol="Iterator" to find all structs implementing Iterator
```

## Project Setup

Add symgraph to your project in two steps: index your code, then configure your AI tool.

### Step 1: Index Your Project

```bash
cd /path/to/your/project
symgraph index
```

This creates a `.symgraph/` directory containing the SQLite knowledge graph. Add `.symgraph/` to your `.gitignore`.

Re-run `symgraph index` after significant code changes, or use the `symgraph-reindex` MCP tool to incrementally update from within your AI tool.

### Step 2: Configure Your AI Tool

#### Claude Code

Register symgraph as an MCP server for your project:

```bash
cd /path/to/your/project
claude mcp add symgraph -- symgraph serve
```

Or add `.mcp.json` to your project root:

```json
{
  "mcpServers": {
    "symgraph": {
      "type": "stdio",
      "command": "symgraph",
      "args": ["serve"]
    }
  }
}
```

**Optional: Add the `/explore-code` skill** for guided code exploration:

```bash
mkdir -p .claude/skills
cp -r /path/to/symgraph/.claude/skills/explore-code .claude/skills/
```

Then use it in Claude Code:

```
/explore-code how does the authentication middleware work?
/explore-code what would break if I changed the User struct?
```

#### Claude Desktop

**Via MCPB bundle (easiest):**

1. Download the `.mcpb` file for your platform from [Releases](https://github.com/grahambrooks/symgraph/releases/latest)
2. Drag and drop onto Claude Desktop, or use **File > Install MCP Server**
3. Set the project root when prompted

**Via manual config** — add to `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "symgraph": {
      "command": "symgraph",
      "args": ["serve"],
      "env": {
        "SYMGRAPH_ROOT": "/path/to/your/project"
      }
    }
  }
}
```

#### GitHub Copilot

**Per-repository** — add `.copilot/mcp.json` to your project:

```json
{
  "mcpServers": {
    "symgraph": {
      "command": "symgraph",
      "args": ["serve"]
    }
  }
}
```

**VS Code user settings** — add to `settings.json`:

```json
{
  "github.copilot.chat.mcp.servers": {
    "symgraph": {
      "command": "symgraph",
      "args": ["serve"],
      "env": {
        "SYMGRAPH_ROOT": "${workspaceFolder}"
      }
    }
  }
}
```

See [GitHub Copilot MCP documentation](https://docs.github.com/copilot/customizing-copilot/using-model-context-protocol/extending-copilot-chat-with-mcp) for more details.

#### OpenAI Codex

```bash
codex mcp add symgraph --command "symgraph" --args "serve"
```

Or add to `~/.codex/config.toml`:

```toml
[mcp_servers.symgraph]
command = "symgraph"
args = ["serve"]
```

See [OpenAI Codex MCP documentation](https://developers.openai.com/codex/mcp/) for more details.

#### HTTP Mode (Any MCP Client)

For shared or remote setups, run symgraph as an HTTP server:

```bash
symgraph serve --port 8080
```

Then point your MCP client at `http://localhost:8080/mcp`.

### Environment Variables

| Variable       | Description            | Default           |
|----------------|------------------------|-------------------|
| `SYMGRAPH_ROOT` | Project root directory | Current directory |

## Architecture

```
symgraph/
├── src/
│   ├── main.rs          # CLI entry point
│   ├── lib.rs           # Core indexing logic
│   ├── types.rs         # Type definitions (Node, Edge, etc.)
│   ├── db/              # SQLite database operations
│   ├── extraction/      # Tree-sitter code extraction
│   ├── graph/           # Graph traversal algorithms
│   ├── context/         # Context building for AI tasks
│   └── mcp/             # MCP protocol handlers
└── .symgraph/
    └── index.db         # SQLite database (per-project)
```

### Core Concepts

- **Node**: A code symbol (function, class, method, etc.)
- **Edge**: A relationship between nodes (calls, contains, imports, etc.)
- **Knowledge Graph**: The complete set of nodes and edges for a codebase

## Development

### Prerequisites

- Rust 1.70+
- SQLite (bundled via rusqlite)

### Building

```bash
make build           # Release build
make test            # Run tests
make check           # Format, lint, and test
make install         # Build and install to /usr/local/bin
```

### Project Structure

| Module       | Description                                                |
|--------------|------------------------------------------------------------|
| `types`      | Core type definitions (NodeKind, EdgeKind, Language, etc.) |
| `db`         | SQLite database schema and operations                      |
| `extraction` | Tree-sitter based code parsing and symbol extraction       |
| `graph`      | Graph algorithms (callers, callees, impact analysis)       |
| `context`    | Context builder for AI task assistance                     |
| `mcp`        | MCP protocol server implementation                         |

## License

MIT
