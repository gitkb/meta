RUST_BACKTRACE ?= full
RUST_LOG ?= trace

include Makefile.context.mk

.DEFAULT_GOAL := build-all

build-all: build

build:
	cargo build

# Every program the workspace declares as a [[bin]] (scripts/programs.py
# lists them), installed globally via cargo install. --force replaces any
# binary of the same name, e.g. one from a release download
install:
	@dirs=$$(python3 scripts/programs.py packages --dirs) && for dir in $$dirs; do \
		cargo install --locked --force --path "$$dir" || exit 1; \
	done
	@$(call remove_old_copies,$$HOME/.meta/plugins,$(CARGO_BIN))

install-all: install

clean:
	@$(call remove_old_copies,.meta/plugins,$$HOME/.meta/plugins)
	cargo clean

# The copies the old build-plugins and install-plugins targets put in this
# checkout's .meta/plugins and in ~/.meta/plugins, where meta looks before
# PATH. One list, install.sh's.
OLD_PLUGIN_COPIES := $(shell sed -n 's/^OLD_PLUGIN_COPIES="\(.*\)"$$/\1/p' install.sh)
ifeq ($(OLD_PLUGIN_COPIES),)
$(error install.sh has no OLD_PLUGIN_COPIES="..." line)
endif

# Where cargo install puts the programs (an install.root in a cargo config
# is not read)
CARGO_BIN := $(or $(CARGO_INSTALL_ROOT),$(CARGO_HOME),$(HOME)/.cargo)/bin

# $(call remove_old_copies,DIR,KEEP): remove each old copy from DIR (a file
# or a link, never a folder), saying so on stderr; nothing when DIR is KEEP,
# compared after following links
remove_old_copies = dir="$(1)"; keep="$(2)"; \
	if [ -d "$$dir" ] && { [ ! -d "$$keep" ] || [ "$$(cd "$$dir" && pwd -P)" != "$$(cd "$$keep" && pwd -P)" ]; }; then \
		for name in $(OLD_PLUGIN_COPIES); do \
			if [ -L "$$dir/$$name" ] || [ -f "$$dir/$$name" ]; then \
				rm -f "$$dir/$$name" && echo "Removed the old copy $$dir/$$name" >&2 || exit 1; \
			fi; \
		done; \
	fi

# Install git hooks (commit-msg validates release-safe subjects; pre-push runs CI checks locally)
# Uses git rev-parse to handle worktrees and submodules correctly
install-hooks:
	@echo "Installing git hooks..."
	@chmod +x .githooks/commit-msg
	@chmod +x .githooks/pre-commit
	@chmod +x .githooks/pre-push
	@mkdir -p "$$(git rev-parse --git-path hooks)"
	@hooks_dir="$$(git rev-parse --git-path hooks)" && root="$$(git rev-parse --show-toplevel)" && rel="$$(python3 -c 'import os, sys; print(os.path.relpath(sys.argv[1], sys.argv[2]))' "$$root/.githooks/commit-msg" "$$hooks_dir")" && ln -sf "$$rel" "$$hooks_dir/commit-msg"
	@hooks_dir="$$(git rev-parse --git-path hooks)" && root="$$(git rev-parse --show-toplevel)" && rel="$$(python3 -c 'import os, sys; print(os.path.relpath(sys.argv[1], sys.argv[2]))' "$$root/.githooks/pre-commit" "$$hooks_dir")" && ln -sf "$$rel" "$$hooks_dir/pre-commit"
	@hooks_dir="$$(git rev-parse --git-path hooks)" && root="$$(git rev-parse --show-toplevel)" && rel="$$(python3 -c 'import os, sys; print(os.path.relpath(sys.argv[1], sys.argv[2]))' "$$root/.githooks/pre-push" "$$hooks_dir")" && ln -sf "$$rel" "$$hooks_dir/pre-push"
	@echo "Commit-msg, pre-commit, and pre-push hooks installed."

release: 
	cargo build --release

rm-meta:
	rm -rf meta

run: 
	cargo run

# The docs and hints tests run the plugins and the example plugin, which
# nextest builds only for their own packages' tests
test:
	cargo build --workspace --bins --examples
	RUST_BACKTRACE=$(RUST_BACKTRACE) RUST_LOG=$(RUST_LOG) cargo nextest run --workspace

test-meta-git-clone: rm-meta
	RUST_BACKTRACE=$(RUST_BACKTRACE) RUST_LOG=$(RUST_LOG) cargo run --release --bin meta -- git clone git@github.com:mateodelnorte/meta.git

test-meta-git-clone-depth-1-recursive: rm-meta
	RUST_BACKTRACE=$(RUST_BACKTRACE) RUST_LOG=$(RUST_LOG) cargo run --release --bin meta -- -r git clone git@github.com:mateodelnorte/meta.git --depth 1

test-meta-git-clone-parallel-recursive: rm-meta
	RUST_BACKTRACE=$(RUST_BACKTRACE) RUST_LOG=$(RUST_LOG) cargo run --release --bin meta -- -r git clone git@github.com:mateodelnorte/meta.git --parallel 4

# Parallel jobs - defaults to CPU count
# Override with JOBS=N, or disable with JOBS=1
CPU_COUNT := $(shell sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 1)
JOBS ?= $(CPU_COUNT)
BATS_FLAGS :=
ifneq ($(JOBS),1)
  BATS_FLAGS += --jobs $(JOBS)
endif
ifdef FILTER
  BATS_FLAGS += --filter "$(FILTER)"
endif

# Run BATS integration tests with parallel jobs
# Optional: FILE="pattern" to filter by test file
# Optional: FILTER="pattern" to filter by test name
# Optional: JOBS=N to set parallel jobs (default: CPU count)
# Examples:
#   make bats                    - Run all tests (parallel)
#   make bats JOBS=1             - Run all tests (serial)
#   make bats FILE="cloud"       - Run worktree_cloud.bats
#   make bats FILTER="prune"     - Run tests with "prune" in name
# tests/setup_suite.bash builds the workspace first
bats:
	bats $(BATS_FLAGS) $(if $(FILE),$(wildcard tests/*$(FILE)*.bats),tests/)

integration-test:
	cargo build -p meta
	RUST_BACKTRACE=$(RUST_BACKTRACE) RUST_LOG=$(RUST_LOG) META_CLI_PATH=target/debug/meta CARGO_BIN_EXE_meta=target/debug/meta cargo nextest run --workspace

# By package name, so it still matches after a version bump since the install
uninstall:
	@packages=$$(python3 scripts/programs.py packages) && for package in $$packages; do \
		cargo uninstall "$$package" || true; \
	done
	@$(call remove_old_copies,$$HOME/.meta/plugins,$(CARGO_BIN))

.PHONY: build-all install install-all uninstall clean install-hooks build run test bats release integration-test
