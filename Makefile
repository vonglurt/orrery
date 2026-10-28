# orrery's front door. No dependencies: cargo builds it, and on a Mac every
# build runs under tools/mem-guard.sh, which stops it before it takes the
# machine's memory.
#
#   make install                 ~/.local/bin/orrery; on a Mac also
#                                ~/Applications/Orrery.app, with the options
#                                it was last made with
#   make install OPTIONS='--operator paul --fleet grove --queue DIR ...'
#                                the same, with these options from now on
#   make uninstall               both
#   make build / make test

PREFIX  ?= $(HOME)/.local
CARGO   ?= cargo
OPTIONS ?=
GUARD    = $(shell [ "$$(uname -s)" = Darwin ] && echo "sh tools/mem-guard.sh")

.PHONY: all build test install uninstall help

all: build

help:
	@sed -n '1,13p' Makefile

build:
	$(GUARD) $(CARGO) build --release

test:
	$(GUARD) $(CARGO) test --release

install:
	@if [ "$$(uname -s)" = Darwin ]; then \
		PREFIX="$(PREFIX)" sh tools/mac-app.sh $(OPTIONS); \
	else \
		$(CARGO) build --release && mkdir -p "$(PREFIX)/bin" && \
		install -m 755 target/release/orrery "$(PREFIX)/bin/orrery" && \
		echo "installed $(PREFIX)/bin/orrery"; \
	fi

uninstall:
	@if [ "$$(uname -s)" = Darwin ]; then \
		PREFIX="$(PREFIX)" sh tools/mac-app.sh --uninstall; \
	else \
		rm -f "$(PREFIX)/bin/orrery" && echo "removed $(PREFIX)/bin/orrery"; \
	fi
