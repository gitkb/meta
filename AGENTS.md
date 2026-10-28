## Project Architecture: Meta-Repo (NOT a monorepo)

This is a **meta-repo** — a workspace of independent git repositories managed by the `meta` CLI tool. Each directory listed in `.meta.yaml` (the project config) is a **separate git repo** with its own remote, commits, and history. NEVER treat this as a monorepo. New crates must be separate git repos added to `.meta.yaml` and `.gitignore`.

READ .kb/AGENTS.md