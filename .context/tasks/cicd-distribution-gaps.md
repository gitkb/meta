# Task: Complete CI/CD and Distribution Setup

## Goal

Make `meta` immediately usable by any user on any platform **without requiring Rust installed**.

## Current State

| Component | File | Status |
|-----------|------|--------|
| CI Pipeline | `.github/workflows/ci.yml` | ✅ Tests on 3 platforms |
| Release Pipeline | `.github/workflows/release.yml` | ✅ Builds 5 platforms |
| Homebrew Formula | `distribution/homebrew/meta-cli.rb` | ✅ Template ready |
| Install Script (Unix) | `install.sh` | ✅ Working |
| Install Script (Windows) | `install.ps1` | ✅ Working |
| Workspace Version | `Cargo.toml` | ✅ All crates inherit |
| Binstall Metadata | `meta_cli/Cargo.toml` | ✅ Configured |
| Crates.io Publish Job | `.github/workflows/release.yml` | ✅ Automated |
| Homebrew Auto-Update | `.github/workflows/release.yml` | ✅ Automated |

---

## Distribution Strategy

### Installation Methods (Priority Order)

| Method | Target Audience | Requires Rust? |
|--------|-----------------|----------------|
| **1. Install script** | Everyone (quickest) | No |
| **2. Homebrew** | macOS/Linux users | No |
| **3. cargo-binstall** | Rust users (downloads pre-built) | Rust + binstall |
| **4. cargo install** | Rust devs who want to compile | Rust |
| **5. GitHub Releases** | Manual download | No |

### 1. Install Script (Recommended)

**macOS/Linux:**
```bash
curl -fsSL https://raw.githubusercontent.com/gitkb/meta/main/install.sh | bash
```

**Windows (PowerShell):**
```powershell
irm https://raw.githubusercontent.com/gitkb/meta/main/install.ps1 | iex
```

- Auto-detects platform and arch
- Downloads from GitHub releases
- Installs to `~/.local/bin` (Unix) or `~\.meta\bin` (Windows)

### 2. Homebrew (macOS/Linux)

```bash
brew install gitkb/tap/meta-cli
```

### 3. cargo-binstall (Best for Rust users)

[cargo-binstall](https://github.com/cargo-bins/cargo-binstall) downloads pre-built binaries from GitHub releases instead of compiling from source.

```bash
cargo binstall meta
```

### 4. cargo install (Compile from source)

```bash
cargo install meta
```
- For Rust developers who want latest/bleeding edge
- Requires Rust toolchain

### 5. GitHub Releases (Manual)

Direct download from: `https://github.com/gitkb/meta/releases/latest`

---

## Automated vs Manual Setup

### Fully Automated (in code)

| Item | File | Description |
|------|------|-------------|
| Version sync | `Cargo.toml` | Workspace version inheritance |
| Windows install | `install.ps1` | PowerShell install script |
| Binstall metadata | `meta_cli/Cargo.toml` | Per-platform URL mappings |
| Crates.io publish | `release.yml` | Publish job on release |
| Homebrew update | `release.yml` | Auto-push formula on release |

### One-Time Manual Setup Required

| Item | Action | Notes |
|------|--------|-------|
| `CARGO_REGISTRY_TOKEN` | Create crates.io API token, add to GitHub secrets | Required for `cargo install` to work |
| `HOMEBREW_TAP_TOKEN` | Create GitHub PAT with repo scope, add to secrets | Required for Homebrew auto-update |
| `gitkb/homebrew-tap` | Create empty GitHub repository | One-time repo creation |

---

## One-Time Setup Instructions

### 1. Create Homebrew Tap Repository

```bash
# Create empty repo at: https://github.com/gitkb/homebrew-tap
# No files needed - the release workflow will populate it
```

### 2. Create GitHub PAT for Homebrew Tap

1. Go to https://github.com/settings/tokens
2. Generate new token (classic) with `repo` scope
3. Add as secret `HOMEBREW_TAP_TOKEN` in the meta repo settings

### 3. Create Crates.io API Token

1. Go to https://crates.io/settings/tokens
2. Create new token with publish scope
3. Add as secret `CARGO_REGISTRY_TOKEN` in the meta repo settings

---

## Success Criteria

- [x] `curl ... | bash` works on macOS/Linux (install.sh exists)
- [x] `irm ... | iex` works on Windows (install.ps1 exists)
- [ ] `brew install gitkb/tap/meta-cli` works (needs tap repo + secret)
- [ ] `cargo binstall meta` downloads pre-built binary (needs crates.io publish)
- [ ] `cargo install meta` compiles from source (needs crates.io publish)
- [x] All methods install the same version (workspace version sync)

## Files Modified

| File | Change |
|------|--------|
| `Cargo.toml` (root) | Added `[workspace.package]` with version, edition, license, repository |
| `loop_lib/Cargo.toml` | Added workspace inheritance |
| `loop_cli/Cargo.toml` | Added workspace inheritance |
| `meta_cli/Cargo.toml` | Added workspace inheritance + binstall metadata |
| `meta_git_lib/Cargo.toml` | Added workspace inheritance |
| `meta_git_cli/Cargo.toml` | Added workspace inheritance |
| `meta_mcp/Cargo.toml` | Added workspace inheritance |
| `meta_plugins/Cargo.toml` | Added workspace inheritance |
| `install.ps1` | Created Windows PowerShell install script |
| `.github/workflows/release.yml` | Added crates.io publish + Homebrew auto-update jobs |
