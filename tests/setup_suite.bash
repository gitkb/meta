# Runs once before any test: the binaries under test are built once, here,
# and every test finds them in META_BIN_DIR. With META_BIN_DIR set (CI builds
# first), they are only checked.
setup_suite() {
    if [ -z "${META_BIN_DIR:-}" ]; then
        local manifest target
        manifest="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)/Cargo.toml"
        echo "# building the workspace once: cargo build --workspace" >&3
        cargo build --workspace --quiet --manifest-path "$manifest" || return 1
        # wherever cargo put it: CARGO_TARGET_DIR, a config's target-dir, ...
        target="$(cargo metadata --format-version 1 --no-deps --manifest-path "$manifest" |
            python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')" || return 1
        META_BIN_DIR="$target/debug"
    fi
    # every test cds first, so the path must not be relative
    META_BIN_DIR="$(cd "$META_BIN_DIR" && pwd -P)" || return 1
    export META_BIN_DIR
    local bin missing=""
    for bin in meta meta-git meta-project meta-cargo meta-mcp loop; do
        [ -x "$META_BIN_DIR/$bin" ] || missing="$missing $bin"
    done
    if [ -n "$missing" ]; then
        echo "not in META_BIN_DIR ($META_BIN_DIR):$missing; build them with cargo build --workspace" >&2
        return 1
    fi
}
