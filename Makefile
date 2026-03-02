SHELL := /bin/bash

PREFIX ?= $(HOME)/.local
BINDIR ?= $(PREFIX)/bin
BIN_NAME := buddy
DIST_DIR ?= dist
RELEASE_REMOTE ?= origin

CRATE_VERSION := $(shell awk -F'"' '/^version = / { print $$2; exit }' Cargo.toml)
HOST_TRIPLE := $(shell rustc -vV | awk '/^host:/ { print $$2; exit }')
ARTIFACT_STEM := $(BIN_NAME)-v$(CRATE_VERSION)-$(HOST_TRIPLE)
RELEASE_TAG := v$(CRATE_VERSION)
SHA256_CMD := $(shell if command -v shasum >/dev/null 2>&1; then echo "shasum -a 256"; elif command -v sha256sum >/dev/null 2>&1; then echo "sha256sum"; else echo ""; fi)

.PHONY: help build build-debug run run-exec install clean \
	test test-ui-regression test-model-regression test-installer-smoke \
	fmt fmt-check clippy check release release-artifacts version \
	bump-patch bump-minor bump-major bump-set install-from-release release-tag

help:
	@echo "buddy make targets:"
	@echo "  make build               Build release binary"
	@echo "  make build-debug         Build debug binary"
	@echo "  make test                Run cargo test"
	@echo "  make test-installer-smoke Run offline installer smoke test"
	@echo "  make fmt                 Format sources"
	@echo "  make fmt-check           Check formatting"
	@echo "  make clippy              Run clippy with warnings as errors"
	@echo "  make check               Run fmt-check + clippy + test"
	@echo "  make release             Run checks and create release artifact"
	@echo "  make release-artifacts   Package release tarball + checksum"
	@echo "  make install-from-release Install from latest GitHub release (curl-style script)"
	@echo "  make install             Install binary to ~/.local/bin"
	@echo "  make version             Print Cargo.toml version"
	@echo "  make bump-patch          Bump patch version in Cargo.toml"
	@echo "  make bump-minor          Bump minor version in Cargo.toml"
	@echo "  make bump-major          Bump major version in Cargo.toml"
	@echo "  make bump-set VERSION=x.y.z  Set explicit semver version"
	@echo "  make release-tag         Auto-commit version files, then create + push v<version> tag"

build:
	cargo build --release

build-debug:
	cargo build

run:
	cargo run

run-exec:
	cargo run -- exec "$(PROMPT)"

install: build
	mkdir -p "$(BINDIR)"
	install -m 0755 "target/release/$(BIN_NAME)" "$(BINDIR)/$(BIN_NAME)"

clean:
	cargo clean
	rm -rf "$(DIST_DIR)"

test:
	cargo test

test-ui-regression:
	cargo test --test ui_tmux_regression -- --ignored --nocapture

test-model-regression:
	env -u BUDDY_API_KEY -u AGENT_API_KEY -u BUDDY_BASE_URL -u AGENT_BASE_URL -u BUDDY_MODEL -u AGENT_MODEL cargo test --test model_regression -- --ignored --nocapture

test-installer-smoke: release-artifacts
	@tmp="$$(mktemp -d)"; \
		./scripts/install.sh --from-dist "$(DIST_DIR)" --version "v$(CRATE_VERSION)" --install-dir "$$tmp/bin" --skip-init; \
		./scripts/install.sh --from-dist "$(DIST_DIR)" --version "v$(CRATE_VERSION)" --install-dir "$$tmp/bin" --skip-init; \
		"$$tmp/bin/$(BIN_NAME)" --version >/dev/null

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

clippy:
	cargo clippy --all-targets -- -D warnings

check: fmt-check clippy test

release: check release-artifacts

release-artifacts: build
	@if [[ -z "$(HOST_TRIPLE)" ]]; then echo "error: unable to resolve host triple"; exit 1; fi
	mkdir -p "$(DIST_DIR)"
	cp "target/release/$(BIN_NAME)" "$(DIST_DIR)/$(BIN_NAME)"
	tar -C "$(DIST_DIR)" -czf "$(DIST_DIR)/$(ARTIFACT_STEM).tar.gz" "$(BIN_NAME)"
	rm -f "$(DIST_DIR)/$(BIN_NAME)"
	@if [[ -z "$(SHA256_CMD)" ]]; then \
		echo "warning: no sha256 tool found (shasum/sha256sum); skipping checksum"; \
	else \
		cd "$(DIST_DIR)" && $(SHA256_CMD) "$(ARTIFACT_STEM).tar.gz" > "$(ARTIFACT_STEM).tar.gz.sha256"; \
	fi
	@echo "wrote $(DIST_DIR)/$(ARTIFACT_STEM).tar.gz"

install-from-release:
	curl -fsSL https://raw.githubusercontent.com/0xfe/buddy/main/scripts/install.sh | bash

version:
	@echo "$(CRATE_VERSION)"

bump-patch:
	./scripts/bump-version.sh patch

bump-minor:
	./scripts/bump-version.sh minor

bump-major:
	./scripts/bump-version.sh major

bump-set:
	@if [[ -z "$(VERSION)" ]]; then echo "usage: make bump-set VERSION=x.y.z"; exit 1; fi
	./scripts/bump-version.sh set "$(VERSION)"

release-tag:
	@if ! git rev-parse --verify HEAD >/dev/null 2>&1; then \
		echo "error: no git HEAD found"; \
		exit 1; \
	fi
	@if ! git diff --quiet -- . ':(exclude)Cargo.toml' ':(exclude)Cargo.lock' || ! git diff --cached --quiet -- . ':(exclude)Cargo.toml' ':(exclude)Cargo.lock'; then \
		echo "error: only Cargo.toml/Cargo.lock may be dirty when running release-tag"; \
		exit 1; \
	fi
	@if git rev-parse -q --verify "refs/tags/$(RELEASE_TAG)" >/dev/null 2>&1; then \
		echo "error: tag $(RELEASE_TAG) already exists"; \
		exit 1; \
	fi
	@current_branch="$$(git branch --show-current)"; \
	if [[ -z "$$current_branch" ]]; then \
		echo "error: cannot release from detached HEAD"; \
		exit 1; \
	fi; \
	git add Cargo.toml; \
	if [[ -f Cargo.lock ]]; then git add Cargo.lock; fi; \
	if ! git diff --cached --quiet; then \
		git commit -m "release: $(RELEASE_TAG)"; \
	fi; \
	git push "$(RELEASE_REMOTE)" "$$current_branch"
	git tag -a "$(RELEASE_TAG)" -m "Release $(RELEASE_TAG)"
	git push "$(RELEASE_REMOTE)" "$(RELEASE_TAG)"
	@echo "pushed $(RELEASE_TAG) to $(RELEASE_REMOTE)"
