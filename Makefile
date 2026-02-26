PREFIX ?= $(HOME)/.local
BINDIR ?= $(PREFIX)/bin
BIN_NAME := buddy

.PHONY: build install

build:
	cargo build --release

install: build
	mkdir -p "$(BINDIR)"
	install -m 0755 "target/release/$(BIN_NAME)" "$(BINDIR)/$(BIN_NAME)"
