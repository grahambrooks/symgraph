param(
    [switch]$Mcp,
    [switch]$Help
)

$ErrorActionPreference = "Stop"

if ($Help) {
    Write-Host "Usage: install.ps1 [OPTIONS]"
    Write-Host ""
    Write-Host "Install symgraph from GitHub releases."
    Write-Host ""
    Write-Host "Options:"
    Write-Host "  -Mcp     Configure symgraph as an MCP server for Claude Code and Claude Desktop"
    Write-Host "  -Help    Show this help message"
    Write-Host ""
    Write-Host "Environment variables:"
    Write-Host "  SYMGRAPH_VERSION       Version to install (default: latest)"
    Write-Host "  SYMGRAPH_INSTALL_DIR   Installation directory (default: ~/.symgraph)"
    exit 0
}

$Repo = "grahambrooks/symgraph"
$InstallDir = if ($env:SYMGRAPH_INSTALL_DIR) { $env:SYMGRAPH_INSTALL_DIR } else { Join-Path $env:USERPROFILE ".symgraph" }
$Version = if ($env:SYMGRAPH_VERSION) { $env:SYMGRAPH_VERSION } else { "latest" }
$Arch = "x64"

# Resolve version
if ($Version -eq "latest") {
    $ReleaseUrl = "https://api.github.com/repos/$Repo/releases/latest"
    try {
        $Release = Invoke-RestMethod -Uri $ReleaseUrl -UseBasicParsing
        $Version = $Release.tag_name -replace "^v", ""
    } catch {
        Write-Error "Failed to resolve latest version: $_"
        exit 1
    }
    Write-Host "Resolved latest version: $Version"
}

# Strip leading 'v' if present
$Version = $Version -replace "^v", ""

$ZipName = "symgraph-$Version-windows-x64.zip"
$DownloadUrl = "https://github.com/$Repo/releases/download/v$Version/$ZipName"

Write-Host "Installing symgraph $Version for windows/$Arch..."

$TmpDir = Join-Path ([System.IO.Path]::GetTempPath()) "symgraph-install-$([System.Guid]::NewGuid())"
New-Item -ItemType Directory -Path $TmpDir -Force | Out-Null

try {
    $ZipPath = Join-Path $TmpDir $ZipName
    Invoke-WebRequest -Uri $DownloadUrl -OutFile $ZipPath -UseBasicParsing

    Expand-Archive -Path $ZipPath -DestinationPath $TmpDir -Force

    # Install binary and manifest
    $BinDir = Join-Path $InstallDir "bin"
    New-Item -ItemType Directory -Path $BinDir -Force | Out-Null

    Copy-Item -Path (Join-Path $TmpDir "symgraph.exe") -Destination (Join-Path $BinDir "symgraph.exe") -Force

    $ManifestPath = Join-Path $TmpDir "manifest.json"
    if (Test-Path $ManifestPath) {
        Copy-Item -Path $ManifestPath -Destination (Join-Path $InstallDir "manifest.json") -Force
    }
} finally {
    Remove-Item -Recurse -Force $TmpDir -ErrorAction SilentlyContinue
}

# Add to PATH for current user if not already present
$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
$BinDir = Join-Path $InstallDir "bin"

if ($UserPath -notlike "*$BinDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$BinDir;$UserPath", "User")
    $env:Path = "$BinDir;$env:Path"
    Write-Host ""
    Write-Host "Added $BinDir to your user PATH."
    Write-Host "Restart your terminal for the change to take effect."
}

Write-Host ""
Write-Host "symgraph $Version installed to $BinDir\symgraph.exe"

# Configure as MCP server
if ($Mcp) {
    $SymgraphBin = Join-Path $BinDir "symgraph.exe"

    function Configure-McpJson {
        param([string]$FilePath, [string]$Label)

        $Dir = Split-Path $FilePath -Parent
        if (-not (Test-Path $Dir)) {
            New-Item -ItemType Directory -Path $Dir -Force | Out-Null
        }

        if (Test-Path $FilePath) {
            $Config = Get-Content $FilePath -Raw | ConvertFrom-Json
        } else {
            $Config = [PSCustomObject]@{}
        }

        if (-not ($Config | Get-Member -Name "mcpServers" -ErrorAction SilentlyContinue)) {
            $Config | Add-Member -NotePropertyName "mcpServers" -NotePropertyValue ([PSCustomObject]@{})
        }

        $ServerConfig = [PSCustomObject]@{
            command = $SymgraphBin
            args = @("serve")
        }

        if ($Config.mcpServers | Get-Member -Name "symgraph" -ErrorAction SilentlyContinue) {
            $Config.mcpServers.symgraph = $ServerConfig
        } else {
            $Config.mcpServers | Add-Member -NotePropertyName "symgraph" -NotePropertyValue $ServerConfig
        }

        $Config | ConvertTo-Json -Depth 10 | Set-Content $FilePath -Encoding UTF8
        Write-Host "  Configured ${Label}: $FilePath"
    }

    Write-Host ""
    Write-Host "Configuring MCP server..."

    # Claude Code: ~/.claude/settings.json
    $ClaudeCodeConfig = Join-Path $env:USERPROFILE ".claude\settings.json"
    Configure-McpJson -FilePath $ClaudeCodeConfig -Label "Claude Code"

    # Claude Desktop: %APPDATA%\Claude\claude_desktop_config.json
    $DesktopConfig = Join-Path $env:APPDATA "Claude\claude_desktop_config.json"
    Configure-McpJson -FilePath $DesktopConfig -Label "Claude Desktop"

    Write-Host ""
    Write-Host "Restart Claude Code / Claude Desktop to pick up the new MCP server."
}
