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

# Release archives are named symgraph-v<version>-<rust target triple>.zip
# (release-kit v2). Releases cut before that used symgraph-<version>-windows-x64.zip,
# which is tried as a fallback so pinned older versions still install.
$ZipName = "symgraph-v$Version-x86_64-pc-windows-msvc.zip"
$LegacyZipName = "symgraph-$Version-windows-x64.zip"
$BaseUrl = "https://github.com/$Repo/releases/download/v$Version"
$ChecksumUrl = "$BaseUrl/SHA256SUMS"

Write-Host "Installing symgraph $Version for windows/$Arch..."

$TmpDir = Join-Path ([System.IO.Path]::GetTempPath()) "symgraph-install-$([System.Guid]::NewGuid())"
New-Item -ItemType Directory -Path $TmpDir -Force | Out-Null

try {
    # $Asset is the name the release actually carries, which is also how
    # SHA256SUMS lists it. The two naming schemes must not be confused at
    # verification time, or a legitimate download looks unlisted.
    $ZipPath = Join-Path $TmpDir $ZipName
    $Asset = $ZipName
    try {
        Invoke-WebRequest -Uri "$BaseUrl/$ZipName" -OutFile $ZipPath -UseBasicParsing
    } catch {
        $Asset = $LegacyZipName
        Invoke-WebRequest -Uri "$BaseUrl/$LegacyZipName" -OutFile $ZipPath -UseBasicParsing
    }

    # Verify the archive against the release's published checksums before
    # unpacking anything that is about to be run on this machine.
    $SumsPath = Join-Path $TmpDir "SHA256SUMS"
    $HaveSums = $true
    try {
        Invoke-WebRequest -Uri $ChecksumUrl -OutFile $SumsPath -UseBasicParsing
    } catch {
        # Releases published before checksums existed have nothing to check.
        Write-Warning "No SHA256SUMS published for v$Version; skipping verification."
        $HaveSums = $false
    }

    if ($HaveSums) {
        $Expected = $null
        foreach ($Line in Get-Content $SumsPath) {
            $Fields = $Line -split '\s+', 2
            if ($Fields.Count -eq 2 -and $Fields[1].TrimStart('*') -eq $Asset) {
                $Expected = $Fields[0]
                break
            }
        }
        if (-not $Expected) {
            throw "$Asset is not listed in SHA256SUMS."
        }

        $Actual = (Get-FileHash -Path $ZipPath -Algorithm SHA256).Hash
        if ($Actual -ne $Expected.ToUpperInvariant()) {
            throw ("Checksum mismatch for {0}.`n  expected: {1}`n  actual:   {2}`n" -f `
                   $Asset, $Expected.ToUpperInvariant(), $Actual)
        }
        Write-Host "Checksum verified."
    }

    Expand-Archive -Path $ZipPath -DestinationPath $TmpDir -Force

    # Install binaries and manifest. The archive ships both the full
    # `symgraph` (CLI + MCP server) and the lean `symgraph-cli`.
    $BinDir = Join-Path $InstallDir "bin"
    New-Item -ItemType Directory -Path $BinDir -Force | Out-Null

    Copy-Item -Path (Join-Path $TmpDir "symgraph.exe") -Destination (Join-Path $BinDir "symgraph.exe") -Force

    $CliPath = Join-Path $TmpDir "symgraph-cli.exe"
    if (Test-Path $CliPath) {
        Copy-Item -Path $CliPath -Destination (Join-Path $BinDir "symgraph-cli.exe") -Force
    }

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

    # Claude Code: prefer its own CLI, which owns the user-scope config and
    # keeps owning it if the file layout changes. Fall back to editing the
    # config only when the CLI is not installed.
    $ClaudeCli = Get-Command claude -ErrorAction SilentlyContinue
    if ($ClaudeCli) {
        & claude mcp add symgraph --scope user -- $SymgraphBin serve 2>&1 | Out-Null
        if ($LASTEXITCODE -eq 0) {
            Write-Host "  Configured Claude Code (via 'claude mcp add --scope user')"
        } else {
            Write-Host "  Claude Code: 'claude mcp add' failed - it may already be configured."
            Write-Host "               Check with: claude mcp list"
        }
    } else {
        $ClaudeCodeConfig = Join-Path $env:USERPROFILE ".claude.json"
        Configure-McpJson -FilePath $ClaudeCodeConfig -Label "Claude Code"
    }

    # Claude Desktop: %APPDATA%\Claude\claude_desktop_config.json
    $DesktopConfig = Join-Path $env:APPDATA "Claude\claude_desktop_config.json"
    Configure-McpJson -FilePath $DesktopConfig -Label "Claude Desktop"

    Write-Host ""
    Write-Host "Restart Claude Code / Claude Desktop to pick up the new MCP server."
}
