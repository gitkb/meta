# Contribution Guide

Thank you for your interest in contributing to `meta`! The commit and PR title rules are in the main [CONTRIBUTING.md](../CONTRIBUTING.md); this page covers the rest.

## Development Environment

Build from source as the README's [From Source](../README.md#from-source) section says. All of meta's crates are in this one repo and Cargo workspace, so a change across crates is one commit and one PR.

## Before Opening a PR

Run what CI runs:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
make bats    # the integration tests in tests/*.bats; needs bats
```

`cargo test -p meta --test docs` (part of `cargo test`) runs every `meta …` command the README, docs and skills show; when you change a command, update the docs it breaks. `make install-hooks` installs git hooks that check commit messages and run these checks before a push.

## Reporting Issues

Use [GitHub Issues](https://github.com/gitkb/meta/issues) for bugs, feature requests, and questions, with steps to reproduce a bug.

## See Also

- [Plugin Development Guide](plugin_development.md)
- [Architecture Overview](architecture_overview.md)
- [Claude Code Skills](claude_code_skills.md): editing the skills
- [FAQ / Troubleshooting Guide](faq_troubleshooting.md)
