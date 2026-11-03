# symgraph — semantic code intelligence MCP server.
#
# `make check` is the pre-push gate and runs exactly what the CI `check` job runs
# (.github/workflows/ci.yml); `make hooks` wires it into git. `make check-all` adds
# CI's other Linux gates (the lean CLI-only build and cargo-deny). Releases come
# from release.mk (release-kit): `make release` tags origin/main and pushes the tag.
.DEFAULT_GOAL := help

.PHONY: help build test lint fmt fmt-check check check-all cli-only deny hooks clean install uninstall update outdated doc

help: ## List targets
	@grep -hE '^[a-zA-Z0-9_-]+:.*## ' $(MAKEFILE_LIST) | awk -F':.*## ' '{ printf "  %-16s %s\n", $$1, $$2 }'

check: fmt-check lint test ## The pre-push gate: the CI check job (fmt, clippy, test)

check-all: check cli-only deny ## check plus the lean build and supply-chain gates CI also runs

build: check ## Build the release binary
	cargo build --release

test: ## Tests, as CI runs them
	cargo test --all-features --locked

lint: ## Clippy with warnings denied, as CI runs it
	cargo clippy --all-targets --all-features --locked -- -D warnings

fmt: ## Format the code
	cargo fmt --all

fmt-check: ## Fail if the code is not formatted
	cargo fmt --all --check

# Lint and test the lean feature combo: the same `symgraph` binary without the
# MCP server (no rmcp/axum/tokio), which is every command except `serve`.
# Nothing else exercises the `#[cfg(feature = "server")]` gates, so a missing
# one would only surface here. CI's cli-only job runs exactly these.
cli-only: ## Lint + test the lean build (no MCP server)
	cargo clippy --bin symgraph --no-default-features --features sqlite --locked -- -D warnings
	cargo test --no-default-features --features sqlite --locked

# Supply-chain gate: advisories, licenses, sources, bans (see deny.toml).
deny: ## Supply-chain check (advisories, licenses, sources)
	cargo deny check

hooks: ## Install the prek git hooks (pre-commit and pre-push)
	prek install

# Install to /usr/local/bin. One binary: `symgraph serve` is the MCP server.
install: build ## Build and install symgraph to /usr/local/bin
	install -d /usr/local/bin
	install -m 755 target/release/symgraph /usr/local/bin/symgraph
	@# Builds before the binaries were merged also installed `symgraph-cli`.
	@# Remove a leftover so it cannot shadow the real binary on PATH.
	@rm -f /usr/local/bin/symgraph-cli
	@echo "Installed symgraph to /usr/local/bin/symgraph"

uninstall: ## Remove from /usr/local/bin
	rm -f /usr/local/bin/symgraph /usr/local/bin/symgraph-cli
	@echo "Removed /usr/local/bin/symgraph"

clean: ## Remove build artifacts
	cargo clean

outdated: ## Show outdated dependencies
	cargo outdated

update: ## Update dependencies
	cargo update

doc: ## Generate and open documentation
	cargo doc --no-deps --open

include release.mk
