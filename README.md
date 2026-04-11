# codemap

[![CI](https://github.com/grahambrooks/codemap/actions/workflows/ci.yml/badge.svg)](https://github.com/grahambrooks/codemap/actions/workflows/ci.yml)
[![Release](https://github.com/grahambrooks/codemap/actions/workflows/release.yml/badge.svg)](https://github.com/grahambrooks/codemap/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-blue.svg)](https://www.rust-lang.org)
[![MCP](https://img.shields.io/badge/MCP-compatible-green.svg)](https://modelcontextprotocol.io)

Semantic code intelligence MCP server - build knowledge graphs of codebases to enhance AI-assisted code exploration.

codemap is a rust implementation of https://github.com/colbymchenry/codegraph. Why? Ongoing exploration of compiled
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
curl -fsSL https://raw.githubusercontent.com/grahambrooks/codemap/main/install.sh | bash
```

To also configure codemap as an MCP server for Claude Code and Claude Desktop:

```sh
curl -fsSL https://raw.githubusercontent.com/grahambrooks/codemap/main/install.sh | bash -s -- --mcp
```

Install a specific version:

```sh
CODEMAP_VERSION=2026.3.30 curl -fsSL https://raw.githubusercontent.com/grahambrooks/codemap/main/install.sh | bash
```

### Windows (PowerShell)

```powershell
irm https://raw.githubusercontent.com/grahambrooks/codemap/main/install.ps1 -OutFile install.ps1; .\install.ps1
```

To also configure codemap as an MCP server for Claude Code and Claude Desktop:

```powershell
irm https://raw.githubusercontent.com/grahambrooks/codemap/main/install.ps1 -OutFile install.ps1; .\install.ps1 -Mcp
```

Install a specific version:

```powershell
$env:CODEMAP_VERSION="2026.3.30"; irm https://raw.githubusercontent.com/grahambrooks/codemap/main/install.ps1 -OutFile install.ps1; .\install.ps1
```

Both scripts install to `~/.codemap/bin/` by default. Override with `CODEMAP_INSTALL_DIR`.

### Claude Desktop (MCPB Bundle)

Download and install the MCPB bundle for your platform:

| Platform              | Download                                                                                   |
|-----------------------|--------------------------------------------------------------------------------------------|
| macOS (Apple Silicon) | [codemap-x.x.x-darwin-arm64.mcpb](https://github.com/grahambrooks/codemap/releases/latest) |
| macOS (Intel)         | [codemap-x.x.x-darwin-x64.mcpb](https://github.com/grahambrooks/codemap/releases/latest)   |
| Windows               | [codemap-x.x.x-windows-x64.mcpb](https://github.com/grahambrooks/codemap/releases/latest)  |
| Linux                 | [codemap-x.x.x-linux-x64.mcpb](https://github.com/grahambrooks/codemap/releases/latest)    |

1. Download the `.mcpb` file for your platform from [Releases](https://github.com/grahambrooks/codemap/releases/latest)
2. Open Claude Desktop
3. Drag and drop the `.mcpb` file onto Claude Desktop, or use **File > Install MCP Server**
4. Configure the project root when prompted

### From Source

```bash
git clone https://github.com/grahambrooks/codemap
cd codemap
make install
```

## Usage

### Index a Codebase

```bash
# Index current directory
codemap index

# Index specific directory
codemap index ~/projects/myapp
```

### Start MCP Server

```bash
# Start with stdio transport (for Claude Desktop)
codemap serve

# Start with HTTP transport
codemap serve --port 8080
```

### CLI Commands

```bash
codemap index [path]           # Index a codebase
codemap serve                  # Start MCP server (stdio)
codemap serve --port <PORT>    # Start MCP server (HTTP)
codemap status [path]          # Show index statistics
codemap search <query>         # Search for symbols
codemap context <task>         # Build context for a task
```

## MCP Tools

### Core Tools

| Tool                 | Description                                       |
|----------------------|---------------------------------------------------|
| `codemap-context`    | Build focused code context for a specific task    |
| `codemap-search`     | Quick symbol search by name                       |
| `codemap-callers`    | Find all callers of a symbol                      |
| `codemap-callees`    | Find all callees of a symbol                      |
| `codemap-impact`     | Analyze the impact radius of changes              |
| `codemap-node`       | Get detailed symbol information                   |
| `codemap-definition` | Get the full source code of a symbol with context |
| `codemap-file`       | List all symbols defined in a specific file       |
| `codemap-references` | Find all references to a symbol                   |
| `codemap-reindex`    | Trigger incremental reindexing of changed files   |
| `codemap-status`     | Get index statistics                              |

### Advanced Tools

| Tool                      | Description                                             |
|---------------------------|---------------------------------------------------------|
| `codemap-hierarchy`       | Get class/module hierarchy (parent/child relationships) |
| `codemap-path`            | Find call paths between two symbols                     |
| `codemap-unused`          | Find unused/dead code with no incoming references       |
| `codemap-implementations` | Find all implementations of an interface/trait          |
| `codemap-diff-impact`     | Analyze the impact of changing a specific code region   |

### Example Use Cases

**Find dead code for cleanup:**

```
Use codemap-unused to find all unused functions and classes
```

**Understand function call chains:**

```
Use codemap-path with from="main" and to="database_query" to see how data flows
```

**Assess change impact:**

```
Use codemap-diff-impact with file_path="src/auth.rs" start_line=45 end_line=60
to see what would be affected by changes in that region
```

**Explore OOP hierarchies:**

```
Use codemap-hierarchy with symbol="BaseHandler" to see all parent/child relationships
```

**Find all trait implementations:**

```
Use codemap-implementations with symbol="Iterator" to find all structs implementing Iterator
```

## Project Setup

Add codemap to your project in two steps: index your code, then configure your AI tool.

### Step 1: Index Your Project

```bash
cd /path/to/your/project
codemap index
```

This creates a `.codemap/` directory containing the SQLite knowledge graph. Add `.codemap/` to your `.gitignore`.

Re-run `codemap index` after significant code changes, or use the `codemap-reindex` MCP tool to incrementally update from within your AI tool.

### Step 2: Configure Your AI Tool

#### Claude Code

Register codemap as an MCP server for your project:

```bash
cd /path/to/your/project
claude mcp add codemap -- codemap serve
```

Or add `.mcp.json` to your project root:

```json
{
  "mcpServers": {
    "codemap": {
      "type": "stdio",
      "command": "codemap",
      "args": ["serve"]
    }
  }
}
```

**Optional: Add the `/explore-code` skill** for guided code exploration:

```bash
mkdir -p .claude/skills
cp -r /path/to/codemap/.claude/skills/explore-code .claude/skills/
```

Then use it in Claude Code:

```
/explore-code how does the authentication middleware work?
/explore-code what would break if I changed the User struct?
```

#### Claude Desktop

**Via MCPB bundle (easiest):**

1. Download the `.mcpb` file for your platform from [Releases](https://github.com/grahambrooks/codemap/releases/latest)
2. Drag and drop onto Claude Desktop, or use **File > Install MCP Server**
3. Set the project root when prompted

**Via manual config** — add to `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "codemap": {
      "command": "codemap",
      "args": ["serve"],
      "env": {
        "CODEMAP_ROOT": "/path/to/your/project"
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
    "codemap": {
      "command": "codemap",
      "args": ["serve"]
    }
  }
}
```

**VS Code user settings** — add to `settings.json`:

```json
{
  "github.copilot.chat.mcp.servers": {
    "codemap": {
      "command": "codemap",
      "args": ["serve"],
      "env": {
        "CODEMAP_ROOT": "${workspaceFolder}"
      }
    }
  }
}
```

See [GitHub Copilot MCP documentation](https://docs.github.com/copilot/customizing-copilot/using-model-context-protocol/extending-copilot-chat-with-mcp) for more details.

#### OpenAI Codex

```bash
codex mcp add codemap --command "codemap" --args "serve"
```

Or add to `~/.codex/config.toml`:

```toml
[mcp_servers.codemap]
command = "codemap"
args = ["serve"]
```

See [OpenAI Codex MCP documentation](https://developers.openai.com/codex/mcp/) for more details.

#### HTTP Mode (Any MCP Client)

For shared or remote setups, run codemap as an HTTP server:

```bash
codemap serve --port 8080
```

Then point your MCP client at `http://localhost:8080/mcp`.

### Environment Variables

| Variable       | Description            | Default           |
|----------------|------------------------|-------------------|
| `CODEMAP_ROOT` | Project root directory | Current directory |

## Architecture

```
codemap/
├── src/
│   ├── main.rs          # CLI entry point
│   ├── lib.rs           # Core indexing logic
│   ├── types.rs         # Type definitions (Node, Edge, etc.)
│   ├── db/              # SQLite database operations
│   ├── extraction/      # Tree-sitter code extraction
│   ├── graph/           # Graph traversal algorithms
│   ├── context/         # Context building for AI tasks
│   └── mcp/             # MCP protocol handlers
└── .codemap/
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
