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
      sha256 "3e51a04e7bbe352736fec658b5c0426bc3d7c937747928c3a62ab188d025099d"
    end
    on_intel do
      url "https://github.com/grahambrooks/symgraph/releases/download/v#{version}/symgraph-#{version}-darwin-x64.tar.gz"
      sha256 "66dcf561f9f2791d5c6dfaa4a857f664d3065190881045be38f20f0f971d90b0"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/grahambrooks/symgraph/releases/download/v#{version}/symgraph-#{version}-linux-arm64.tar.gz"
      sha256 "da8aed0258f1e06f782987ac54c41984d7e3dbb15b85fa94c0a1242fd3a2e651"
    end
    on_intel do
      url "https://github.com/grahambrooks/symgraph/releases/download/v#{version}/symgraph-#{version}-linux-x64.tar.gz"
      sha256 "f07f68d13ff9865c1aed20ac6f1b492042297ce0f02b78a1953c32da109e7603"
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
