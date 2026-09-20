# Homebrew formula for symgraph.
#
# The `version` line is bumped by `make release` as part of the release commit.
# The four `sha256` values (and `version`, idempotently) are then rewritten by
# .github/workflows/release.yml once the tagged build publishes the tarballs.
# Edit the surrounding structure by hand, but let those two own version/sha256.
class Symgraph < Formula
  desc "Semantic code intelligence: symbol graph + MCP server for codebases"
  homepage "https://github.com/grahambrooks/symgraph"
  version "2026.7.21"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/grahambrooks/symgraph/releases/download/v#{version}/symgraph-#{version}-darwin-arm64.tar.gz"
      sha256 "d786e5e2dcce35a7e23550108df5aab9465a443226e299dc9bb9b6b59006338f"
    end
    on_intel do
      url "https://github.com/grahambrooks/symgraph/releases/download/v#{version}/symgraph-#{version}-darwin-x64.tar.gz"
      sha256 "df1a379aef2bfcd3785978375c9f7bf64d11d20de9336441734ddcb51393c08d"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/grahambrooks/symgraph/releases/download/v#{version}/symgraph-#{version}-linux-arm64.tar.gz"
      sha256 "42b64f9f9ffd9aeef64445ebe50fd17552e0a06ce40ead989d915da0de1987f9"
    end
    on_intel do
      url "https://github.com/grahambrooks/symgraph/releases/download/v#{version}/symgraph-#{version}-linux-x64.tar.gz"
      sha256 "5ba55a1612ee1d8f22d75985f5146b09edba0a64180c8bd8588a1bc4b41161fd"
    end
  end

  def install
    # The release tarball ships one binary — CLI and MCP server in the same
    # executable — plus manifest.json (not installed).
    bin.install "symgraph"
  end

  def caveats
    <<~EOS
      symgraph is now a single binary: `symgraph serve` runs the MCP server.
      Releases up to 2026.9.20 also installed a separate `symgraph-cli`;
      if one is still on your PATH from an older install, remove it.
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/symgraph --version")
    assert_match "serve", shell_output("#{bin}/symgraph help")
  end
end
