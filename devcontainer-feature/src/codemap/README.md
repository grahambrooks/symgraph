# Codemap (devcontainer feature)

Installs the [codemap](https://github.com/grahambrooks/codemap) semantic code intelligence MCP server.

## Usage

Add to your `devcontainer.json`:

```json
{
  "features": {
    "ghcr.io/grahambrooks/codemap/codemap:1": {}
  }
}
```

### Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `version` | string | `latest` | Version to install (e.g. `2026.3.28` or `latest`) |

### Example with pinned version

```json
{
  "features": {
    "ghcr.io/grahambrooks/codemap/codemap:1": {
      "version": "2026.3.28"
    }
  }
}
```
