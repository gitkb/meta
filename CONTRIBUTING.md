# Contributing

Thank you for your interest in contributing to `meta`!

## Commits and PR titles

Use Conventional Commit style for commit subjects and PR titles:

```text
feat: add workspace tag filtering
fix(meta_git_cli): preserve child repo commit type
ci: enforce semantic PR titles
docs: clarify plugin installation
```

If you are working from an internal GitKB task, include the task wikilink in
commit messages:

```text
fix: remove legacy meta agent workspace entry [[tasks/harmony-678]]
```

GitHub merge subjects are derived from PR titles, so the semantic PR title check
is the merge-blocking guard for release notes. Local hooks catch malformed commit
messages before push when installed with:

```sh
make install-hooks
```

## Development Environment

Build from source as the README's [From GitHub main](README.md#from-github-main-recommended) section says. All of meta's crates are in this one repo and Cargo workspace, so a change across crates is one commit and one PR.

## Before Opening a PR

Run what CI runs:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --workspace --bins --examples --locked   # the tests below run these
cargo test --workspace --locked
make bats    # the integration tests in tests/*.bats; needs bats
```

`cargo test -p meta --test docs` (part of `cargo test`) runs every `meta …` command the README, docs and skills show; when you change a command, update the docs it breaks. `make install-hooks` installs git hooks that check commit messages and, before a push, run the cargo checks above (not `make bats`, which you run yourself).

## Reporting Issues

Use [GitHub Issues](https://github.com/gitkb/meta/issues) for bugs, feature requests, and questions, with steps to reproduce a bug.

## See Also

- [Plugin Development Guide](docs/plugin_development.md)
- [Architecture Overview](docs/architecture_overview.md)
- [Claude Code Skills](docs/claude_code_skills.md): editing the skills
- [FAQ / Troubleshooting Guide](docs/faq_troubleshooting.md)
