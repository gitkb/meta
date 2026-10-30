## Project Layout

All of meta's crates are folders in this one git repo and members of the root Cargo workspace; a change across crates is one PR. `.meta.yaml` lists the separate repos `agent` and `meta-plugins`, which are cloned into gitignored folders. A new crate is a new folder added to the workspace `members`.

READ .kb/AGENTS.md