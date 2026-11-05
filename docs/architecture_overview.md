# Architecture Overview

How the `meta` CLI is put together: its crates, its plugins, and what happens when you run a command.

## Crates

`meta` is one Cargo workspace (the root `Cargo.toml` lists its members):

```
meta/
├── meta_cli/              # The CLI (binary: meta)
├── meta_core/             # The workspace domain meta and its plugins share
├── meta_plugin/           # The plugin SDK: protocol, runtime, process owner, test kit
├── meta_git_cli/          # Git plugin (binary: meta-git)
├── meta_git_lib/          # Clone queue and worktree sets
├── meta_rust_cli/         # Cargo plugin (binary: meta-cargo)
├── meta_mcp/              # MCP server (binary: meta-mcp)
├── loop_cli/              # Standalone loop CLI (binary: loop)
└── loop_lib/              # Runs a command in many directories
```

- **meta_cli**: the `meta` binary. It parses the command line, finds and runs plugins (`subprocess_plugins.rs`), and holds `meta exec`, `meta context` (`context.rs`), `meta project list`, `check` and `dependents` (`project.rs`), `meta init claude` (`init.rs`) and the plugin registry (`registry.rs`).
- **meta_core**: reading `.meta.yaml` (`config.rs`), running git (`git/`), the `~/.meta/` data directory and atomic JSON writes (`data_dir.rs`, `store.rs`), the records of worktree sets (`set_record.rs`), worktree detection (`worktree.rs`), repo selection (`repo_filter.rs`), the dependency graph (`dependency_graph.rs`) and the query language the MCP server uses (`query.rs`).
- **meta_plugin**: the plugin SDK, which meta, loop and every plugin use. The protocol: `PluginInfo` and `PluginHelp` (what a plugin says about itself), `Request` (the invocation meta resolved, what it sends a plugin), `Response` (a plan, results, a message, a delegate or an error), `Envelope` (what `meta --json` prints for every command), and `run_plugin()`, which a Rust plugin's `main` calls. The runtime: everything printed and how a program exits, with its exit codes (`out.rs`, `exit.rs`), escaping untrusted text (`display.rs`), `RepoUrl` (`repo_url.rs`), and the one owner of the processes a program starts and how they are stopped (`process.rs`). Opt-in features: `parse` (a plugin's clap parsing and the tip for meta's options), `testing` and `testing-git` (the sandbox and workspace builder plugin tests use).
- **meta_git_cli**: `meta git clone`, `update`, `commit` and `worktree`; any other `meta git <cmd>` runs `git <cmd>` in each repo. SSH multiplexing for parallel remote commands is set up internally.
- **meta_git_lib**: the clone queue and the worktree set building blocks (plans, leases, TTLs, removal safety checks).
- **meta_rust_cli**: `meta cargo <args>` runs cargo, as given, in each selected repo with a `Cargo.toml`, and lists the repos it skips.
- **meta_mcp**: the MCP server; see [mcp_server.md](mcp_server.md).
- **loop_lib / loop_cli**: the engine that runs commands across directories, used by `meta exec` and plugin plans, and the standalone `loop` CLI with its `.looprc`; see [loop.md](loop.md).

## Plugins

A plugin is an executable named `meta-<word>` that serves `meta <word> ...`. Meta finds plugins in `.meta/plugins/` of the workspace, in `~/.meta/plugins/`, and on `PATH`; it asks one for its info, then calls it with the resolved request in a file. The plugin answers with one response in another file: a plan (steps per project), which meta runs through `loop_lib`, or its records, a message, a delegate or an error. Meta prints the outcome, and under `--json` one envelope for every command. [plugin_development.md](plugin_development.md) has the discovery order and the protocol.

## Running a command

`meta --tag backend git status`:

1. meta_cli reads the global options before the command word, finds the workspace's `.meta.yaml`, and selects the repos (`--include` and `--tag` add repos, `--exclude` removes them).
2. `exec`, `context`, `project`, `init`, `plugin` and `help` are meta's own. `meta exec -- <cmd>` runs the command, as typed and with no shell, in each selected repo through `loop_lib`, never through a plugin. So `meta exec -- git ...` is plain git, without meta-git's handling.
3. Any other first word goes to the plugin of that name (`meta-git` here), started with the selected projects and the options.
4. With no plugin of that name, meta says the command is unrecognized and suggests the nearest command (for the typo `gti status`, `meta git status`), or `meta exec -- <words>` when none is near.

## See Also

- [Plugin Development Guide](plugin_development.md)
- [Advanced Usage Guide](advanced_usage.md)
- [MCP Server Documentation](mcp_server.md)
- [FAQ / Troubleshooting](faq_troubleshooting.md)
