#!/usr/bin/env bats

setup() {
    REPO_ROOT="$BATS_TEST_DIRNAME/.."
}

@test "release artifacts include the Rust plugin on every platform" {
    local workflow="$REPO_ROOT/.github/workflows/release.yml"

    grep -F -q 'cargo build --release --target ${{ matrix.target }} -p meta_rust_cli' "$workflow"
    grep -F -q 'cp target/${{ matrix.target }}/release/meta-rust dist/' "$workflow"
    grep -F -q 'copy target\${{ matrix.target }}\release\meta-rust.exe dist\' "$workflow"
    grep -F -q 'cargo publish -p meta_rust_cli' "$workflow"
    grep -F -q 'bin.install "meta-rust"' "$workflow"
}

@test "supported installers install the Rust plugin" {
    grep -F -q '"meta-rust"' "$REPO_ROOT/install.sh"
    grep -F -q '"meta-rust.exe"' "$REPO_ROOT/install.ps1"
    grep -F -q 'bin.install "meta-rust"' "$REPO_ROOT/distribution/homebrew/meta-cli.rb"
    grep -F -q './target/debug/meta-rust --meta-plugin-info' "$REPO_ROOT/.github/workflows/ci.yml"
}
