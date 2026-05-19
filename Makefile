RUST_BACKTRACE ?= full
RUST_LOG ?= trace

include Makefile.context.mk

.DEFAULT_GOAL := build-all

# Build everything for development (plugins go to .meta/plugins/ for local discovery)
build-all: build build-plugins

build:
	cargo build

# Build plugins and install to project-local .meta/plugins/ (for development)
build-plugins:
	cargo build --release -p meta_git_cli
	cargo build --release -p meta_project_cli
	cargo build --release -p meta_rust_cli
	mkdir -p .meta/plugins
	cp target/release/meta-git .meta/plugins/meta-git
	cp target/release/meta-project .meta/plugins/meta-project
	cp target/release/meta-rust .meta/plugins/meta-rust

# Install meta binary and all plugins globally via cargo install
install:
	cargo install --path meta_cli
	cargo install --path meta_git_cli
	cargo install --path meta_project_cli
	cargo install --path meta_rust_cli

# Install plugins globally to ~/.meta/plugins/
install-plugins: build-plugins
	mkdir -p ~/.meta/plugins
	cp target/release/meta-git ~/.meta/plugins/meta-git
	cp target/release/meta-project ~/.meta/plugins/meta-project
	cp target/release/meta-rust ~/.meta/plugins/meta-rust

# Install everything globally (meta binary + plugins to both cargo bin and ~/.meta/plugins/)
install-all: install install-plugins

clean: clean-plugins
	cargo clean

clean-plugins:
	rm -rf .meta/plugins

rebuild-plugins: clean-plugins build-plugins

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

test:
	RUST_BACKTRACE=$(RUST_BACKTRACE) RUST_LOG=$(RUST_LOG) cargo nextest run --workspace

test-meta-git-clone: rm-meta
	RUST_BACKTRACE=$(RUST_BACKTRACE) RUST_LOG=$(RUST_LOG) cargo run --release --bin meta -- git clone git@github.com:mateodelnorte/meta.git

test-meta-git-clone-depth-1-recursive: rm-meta
	RUST_BACKTRACE=$(RUST_BACKTRACE) RUST_LOG=$(RUST_LOG) cargo run --release --bin meta -- git clone git@github.com:mateodelnorte/meta.git --depth 1 --recursive

test-meta-git-clone-parallel-recursive: rm-meta
	RUST_BACKTRACE=$(RUST_BACKTRACE) RUST_LOG=$(RUST_LOG) cargo run --release --bin meta -- git clone git@github.com:mateodelnorte/meta.git --parallel 4 --recursive

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
bats:
	cargo build --workspace
	bats $(BATS_FLAGS) $(if $(FILE),$(wildcard tests/*$(FILE)*.bats),tests/)

integration-test:
	cargo build -p meta
	RUST_BACKTRACE=$(RUST_BACKTRACE) RUST_LOG=$(RUST_LOG) META_CLI_PATH=target/debug/meta CARGO_BIN_EXE_meta=target/debug/meta cargo nextest run --workspace

uninstall:
	cargo uninstall meta 2>/dev/null || true
	cargo uninstall meta_git_cli 2>/dev/null || true
	cargo uninstall meta_project_cli 2>/dev/null || true
	cargo uninstall meta_rust_cli 2>/dev/null || true
	rm -f ~/.meta/plugins/meta-git ~/.meta/plugins/meta-project ~/.meta/plugins/meta-rust

.PHONY: install install-hooks build run test bats release integration-test
