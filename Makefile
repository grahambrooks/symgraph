.PHONY: build test lint fmt check clean install uninstall release release-dry-run update outdated doc help

# Date-based version: YYYY.M.D (semver-compatible)
VERSION := $(shell date +%Y.%-m.%-d)
TAG := v$(VERSION)

# Default target
all: help

# Show help
help:
	@echo "symgraph — Semantic code intelligence MCP server"
	@echo ""
	@echo "Usage: make [target]"
	@echo ""
	@echo "Development:"
	@echo "  build            Build release binary"
	@echo "  test             Run all tests"
	@echo "  lint             Run clippy lints"
	@echo "  fmt              Format code"
	@echo "  fmt-check        Check formatting without modifying"
	@echo "  check            Run all checks (format, lint, test)"
	@echo "  doc              Generate and open documentation"
	@echo ""
	@echo "Installation:"
	@echo "  install          Build and install to /usr/local/bin"
	@echo "  uninstall        Remove from /usr/local/bin"
	@echo ""
	@echo "Release:"
	@echo "  release          Create and push a date-based release ($(TAG))"
	@echo "  release-dry-run  Preview what a release would do"
	@echo ""
	@echo "Maintenance:"
	@echo "  update           Update dependencies"
	@echo "  outdated         Show outdated dependencies"
	@echo "  clean            Remove build artifacts"
	@echo ""
	@echo "  help             Show this help"

# Build the project
build: check
	cargo build --release

# Run tests
test:
	cargo test --all-features

# Run clippy lints
lint:
	cargo clippy --all-features -- -D warnings

# Format code
fmt:
	cargo fmt --all

# Check formatting without modifying
fmt-check:
	cargo fmt --all -- --check

# Run all checks (format, lint, test)
check: fmt-check lint test

# Install to /usr/local/bin
install: build
	install -d /usr/local/bin
	install -m 755 target/release/symgraph /usr/local/bin/symgraph
	@echo "Installed symgraph to /usr/local/bin/symgraph"

# Uninstall from /usr/local/bin
uninstall:
	rm -f /usr/local/bin/symgraph
	@echo "Removed /usr/local/bin/symgraph"

# Clean build artifacts
clean:
	cargo clean

# Show outdated dependencies
outdated:
	cargo outdated

# Update dependencies
update:
	cargo update

# Generate documentation
doc:
	cargo doc --no-deps --open

# Show what a release would do without making changes
release-dry-run:
	@echo "Version: $(VERSION)"
	@echo "Tag:     $(TAG)"
	@echo ""
	@echo "Files to update:"
	@echo "  Cargo.toml      (version = \"$(VERSION)\")"
	@echo "  manifest.json    (version: \"$(VERSION)\")"
	@echo ""
	@echo "Git operations:"
	@echo "  git commit -am 'release $(TAG)'"
	@echo "  git tag $(TAG)"
	@echo "  git push origin main --tags"

# Create and push a release with date-based versioning
release: check
	@if [ -n "$$(git status --porcelain)" ]; then \
		echo "Error: working directory is not clean. Commit or stash changes first."; \
		exit 1; \
	fi
	@echo "Releasing $(TAG)..."
	sed -i '' 's/^version = ".*"/version = "$(VERSION)"/' Cargo.toml
	cargo check --quiet 2>/dev/null || (echo "Cargo.toml version update failed"; exit 1)
	jq --arg v "$(VERSION)" '.version = $$v' manifest.json > manifest.json.tmp && mv manifest.json.tmp manifest.json
	git add Cargo.toml manifest.json
	git commit -m "release $(TAG)"
	git tag "$(TAG)"
	git push origin main --tags
	@echo "Released $(TAG) — GitHub Actions will build and publish artifacts."
