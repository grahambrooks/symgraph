# Standard release targets for grahambrooks Rust tools (release-kit v2).
#
# This file is byte-identical in every repo; re-sync with release-kit/stamp.sh.
# The Makefile pulls it in with:   include release.mk
#
# The tag is the version. `make release` tags the default branch on origin with the
# next CalVer and pushes only the tag. .github/workflows/release.yml then builds and
# publishes, and merges a PR that bumps Cargo.toml and the Homebrew formula. Nothing
# is bumped or committed here, so nothing races that PR, and local uncommitted work
# or the current branch don't matter.
#
# Version: today's UTC date, vYYYY.M.D (no leading zeros: Cargo's SemVer parser
# rejects 2026.04.1). If that tag already exists, the last component is incremented
# until it is free. auto-managed's monthly release job uses the same rule
# (scripts/release_repos.sh), so manual and scheduled releases share one series.

RELEASE_NAME   := $(or $(shell sed -n 's/^FORMULA_NAME=//p' .release.env 2>/dev/null),$(notdir $(CURDIR)))
RELEASE_BRANCH := $(or $(shell git symbolic-ref --short refs/remotes/origin/HEAD 2>/dev/null | sed 's|^origin/||'),main)

# Shell fragment that leaves the next free version in $v. Each target runs it inside
# a single recipe line, so the version is computed once per run, after the fetch.
_RELEASE_NEXT = v=$$(date -u +%Y.%-m.%-d); \
	while git rev-parse -q --verify "refs/tags/v$$v" >/dev/null \
	   || git rev-parse -q --verify "refs/tags/$$v" >/dev/null; do \
		v=$$(echo "$$v" | awk -F. '{ printf "%s.%s.%d", $$1, $$2, $$3 + 1 }'); \
	done

.PHONY: release release-dry release-version release-fetch

# Tags come from origin so the version reflects what is published, not this clone.
release-fetch:
	@git fetch --quiet origin $(RELEASE_BRANCH) --tags

release-version: release-fetch ## Print the version `make release` would tag next
	@$(_RELEASE_NEXT); echo "$$v"

release-dry: release-fetch ## Show what `make release` would do, without doing it
	@$(_RELEASE_NEXT); \
	echo "would tag origin/$(RELEASE_BRANCH) ($$(git rev-parse --short origin/$(RELEASE_BRANCH))) as v$$v and push the tag"; \
	echo "the tag push runs the release workflow: it builds the binaries, publishes a"; \
	echo "GitHub Release, and merges a PR that bumps Cargo.toml and the formula"

release: release-fetch ## Tag origin's default branch with the next CalVer and push the tag
	@set -e; $(_RELEASE_NEXT); \
	echo "--> tagging origin/$(RELEASE_BRANCH) as v$$v"; \
	git tag -a "v$$v" -m "$(RELEASE_NAME) $$v" "origin/$(RELEASE_BRANCH)"; \
	git push --quiet origin "v$$v"; \
	echo "pushed v$$v; the release workflow takes it from here"
