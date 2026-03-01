PREFIX ?= $(HOME)/.local
BINDIR ?= $(PREFIX)/bin
BIN_NAME := buddy

.PHONY: build install test test-ui-regression test-model-regression

build:
	cargo build --release

install: build
	mkdir -p "$(BINDIR)"
	install -m 0755 "target/release/$(BIN_NAME)" "$(BINDIR)/$(BIN_NAME)"

test:
	cargo test

test-ui-regression:
	cargo test --test ui_tmux_regression -- --ignored --nocapture

test-model-regression:
	env -u BUDDY_API_KEY -u AGENT_API_KEY -u BUDDY_BASE_URL -u AGENT_BASE_URL -u BUDDY_MODEL -u AGENT_MODEL cargo test --test model_regression -- --ignored --nocapture
