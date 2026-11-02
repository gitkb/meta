# Plugin Development Guide

A plugin adds a command to `meta`. It is a program named `meta-<word>` that serves `meta <word> ...`: meta starts it, sends it the request as JSON, and runs the plan it answers with (or prints what it prints). Any language works.

The whole of a working plugin in Rust is [`meta_plugin_protocol/examples/meta-hello.rs`](../meta_plugin_protocol/examples/meta-hello.rs), about 60 lines: `meta hello NAME` runs `echo hello NAME` in each selected repo. It is built and tested with the workspace, so it matches the protocol below.

```bash
cargo build -p meta_plugin_protocol --example meta-hello
cp target/debug/examples/meta-hello ~/.meta/plugins/
meta hello world
meta --tag backend hello team    # meta's options, selection included, go before the command
```

## Discovery and naming

meta looks for plugins in, in order:

1. `.meta/plugins/` next to the nearest meta config at or above the current directory, then next to each parent meta config that lists that directory as a project (the same workspace), nearest first. Inside a worktree set meta made, a checkout's own `.meta/plugins/` comes first, then those of the checkout it was made from and its workspace. `meta plugin install <name> --local` installs into the nearest one, and writes `.meta/plugins/.gitignore` (`*`) so the binaries stay out of git.
2. `~/.meta/plugins/` (moved by `META_DATA_DIR`), where `meta plugin install <name>` installs.
3. Executables named `meta-*` on PATH.

The first file with the name wins. The file name decides which command reaches a plugin (`meta-hello`, or `meta-hello.exe` on Windows; `.cmd` and `.bat` files aren't plugins), not the `name` or `commands` it reports: a plugin serving two top-level words ships two files. A word meta has as a built-in command (`exec`, `plugin`, `help`, ...) never reaches a plugin.

For `meta <word> ...` meta starts only `meta-<word>`; if it doesn't answer `--meta-plugin-info` within 5 seconds, the command fails naming it. `meta`, `meta help`, `meta --help` and `meta plugin list` start every plugin, and skip with a warning one that doesn't answer in time or answers with anything but its info (a `meta-*` program on PATH that isn't a plugin is just left out).

## The protocol

The types are in [`meta_plugin_protocol`](../meta_plugin_protocol/src/lib.rs); a Rust plugin uses them and `run_plugin`, which handles the flags below.

**`meta-<word> --meta-plugin-info`** prints a `PluginInfo`:

```json
{
  "name": "hello",
  "version": "0.1.0",
  "commands": ["hello"],
  "description": "Say hello in each repo",
  "help": {"usage": "meta hello NAME", "examples": ["meta hello world"]}
}
```

`commands` start with the plugin's word. For `meta git worktree create x`, the longest listed command that matches the typed words whole (`git worktree create`) is the request's `command` and the rest (`["x"]`) its `args`; with none matching, `command` is the word alone. `meta <word>` and `meta <word> --help` print the plugin's help, from its info: `help.usage` (required), then `commands` or `command_sections`, `examples` and `note` (all optional). A `--help` after more words (`meta hello world --help`) reaches the plugin as an argument, so it answers it itself (`has_help_token`, then `CommandResult::ShowHelp`).

**`meta-<word> --meta-plugin-exec`** reads a `PluginRequest` on stdin:

```json
{
  "command": "hello",
  "args": ["team"],
  "projects": ["/home/me/ws", "/home/me/ws/api", "/home/me/ws/apps/web"],
  "cwd": "/home/me/ws",
  "options": {"json_output": false, "dry_run": false, "parallel": true, "recursive": false,
              "verbose": false, "silent": false, "strict": false, "depth": null,
              "include_filters": null, "exclude_filters": null, "tag_filters": ["backend"]}
}
```

`projects` are the selected repos' directories, `--include`, `--exclude` and `--tag` already applied (the root included unless they left it out). The filters are passed too: a plugin that finds its repos itself must refuse filters it can't honour.

It answers on stdout with one of:

- a plan, which meta runs in each directory (in parallel unless the plan or the user says otherwise), honouring `--dry-run` itself:
  ```json
  {"plan": {"commands": [{"dir": "/home/me/ws/api", "argv": ["echo", "hello team"]}]}}
  ```
  Each `argv` runs as given, with no shell. A plan can also carry `pre_commands` and `post_commands`, which run one at a time before and after the main ones and **run under `--dry-run` too** (they are for setup such as an SSH connection, not for changes), and `parallel`, `max_parallel`, `spawn_stagger_ms` and a per-command `env` (`ExecutionPlan`).
- any other output, which meta prints as it is.

and ends with its exit code: 0 done, 1 failed, 2 the command line was wrong (`CommandResult::Error` and `CommandResult::Usage` print the message on stderr and exit 1 and 2). meta ends with the plugin's code. A plugin that changes something itself (instead of planning) honours `options.dry_run`, or refuses it with exit 2.

## Other languages

The same plugin in Python, as `meta-hello` (executable) on PATH or in a plugins folder:

```python
#!/usr/bin/env python3
import json, sys

if sys.argv[1:] == ["--meta-plugin-info"]:
    print(json.dumps({"name": "hello", "version": "0.1.0", "commands": ["hello"],
                      "help": {"usage": "meta hello NAME"}}))
elif sys.argv[1:] == ["--meta-plugin-exec"]:
    request = json.load(sys.stdin)
    if len(request["args"]) != 1:
        print("meta hello takes one name", file=sys.stderr)
        sys.exit(2)
    commands = [{"dir": d, "argv": ["echo", "hello " + request["args"][0]]}
                for d in request["projects"]]
    print(json.dumps({"plan": {"commands": commands}}))
else:
    sys.exit(2)
```

## Testing a plugin

```bash
meta-hello --meta-plugin-info | jq .
echo '{"command":"hello","args":["team"],"projects":["."]}' | meta-hello --meta-plugin-exec
meta --dry-run hello team    # what meta would run, run nowhere
```

If `meta hello` says the command is unrecognized, the file isn't where meta looks, isn't executable, or isn't named `meta-hello`; `meta --verbose plugin list` shows the folders meta looked in and what it found there.

## Publishing

`meta plugin install <name>` downloads a plugin from the registries listed in `$META_DATA_DIR/config.yaml` (default `~/.meta/config.yaml`, `registries:`; without one, the public registry) into `~/.meta/plugins/`; `meta plugin search <query>` searches them. Or ship the binary and have users put it in a plugins folder or on PATH.
