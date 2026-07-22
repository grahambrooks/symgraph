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
    # The release tarball ships the full binary (CLI + MCP server) and, for
    # releases built after v2026.7.5, the lean CLI-only `symgraph-cli` as well,
    # plus manifest.json (not installed).
    bin.install "symgraph"
    bin.install "symgraph-cli" if File.exist?("symgraph-cli")
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/symgraph --version")
    assert_match version.to_s, shell_output("#{bin}/symgraph-cli version") if (bin/"symgraph-cli").exist?
  end
end
