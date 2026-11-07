# Plugin Development Guide

A plugin adds a command to `meta`. It is a program named `meta-<word>` that serves `meta <word> ...`: meta starts it, sends it the resolved request, and does what it answers (runs its plan, shows its results). Any language works.

The whole of a working plugin in Rust is [`meta_plugin/examples/meta-hello.rs`](../meta_plugin/examples/meta-hello.rs), about 60 lines: `meta hello NAME` runs `echo hello NAME` in each selected repo. It is built and tested with the workspace, so it matches the protocol below.

```bash
cargo build -p meta_plugin --example meta-hello
mkdir -p ~/.meta/plugins && cp target/debug/examples/meta-hello ~/.meta/plugins/
meta hello world
meta --tag backend hello team    # meta's options, selection included, go before the command
```

## Discovery and naming

meta looks for plugins in, in order:

1. `.meta/plugins/` next to the nearest meta config at or above the current directory, then next to each parent meta config that lists that directory as a project (the same workspace), nearest first. Inside a worktree set meta made, a checkout's own `.meta/plugins/` comes first, then those of the checkout it was made from and its workspace. `meta plugin install <name> --local` installs into the nearest one, and writes `.meta/plugins/.gitignore` (`*`) so the binaries stay out of git.
2. `~/.meta/plugins/` (moved by `META_DATA_DIR`), where `meta plugin install <name>` installs.
3. Executables named `meta-*` on PATH.

The first executable file with the name wins. The file name decides which command reaches a plugin (`meta-hello`, or `meta-hello.exe` on Windows; `.cmd` and `.bat` files aren't plugins), not the `name` or `commands` it reports: a plugin serving two top-level words ships two files. A word meta has as a built-in command (`exec`, `plugin`, `help`, ...) never reaches a plugin.

For `meta <word> ...` meta starts only `meta-<word>`; if it doesn't answer `--meta-plugin-info` within 5 seconds, the command fails naming it. `meta`, `meta help`, `meta --help` and `meta plugin list` start every plugin, and skip with a warning one that doesn't answer in time or answers with anything but its info (a `meta-*` program on PATH that isn't a plugin is just left out).

## The protocol

The protocol is the SDK's types, and its documentation is theirs: [`meta_plugin`](../meta_plugin/src/lib.rs) (`cargo doc -p meta_plugin --open`). In short: meta asks a plugin for its info (`PluginInfo`), then calls it with a `Request` (the invocation meta resolved: the workspace, the worktree set it runs in, the selected projects, meta's options a plugin may act on) in the file `META_REQUEST` names. The plugin writes one `Response` to the file `META_RESPONSE` names: a `Plan` of steps for meta to run in each project, `Results` (records meta shows, or that the plugin already showed in its own words), a message or an error, each with its warnings and skipped projects. The plugin keeps the terminal: its stdin, stdout and stderr are meta's (under `--json` its stdout is meta's stderr, and its stderr passes through meta, so the error of a plugin that ends without an answer carries its last lines). meta prints the outcome; under `--json`, the one `Envelope` every command prints (see [the --json envelope](advanced_usage.md#json-output-mode)). A plugin never prints JSON itself.

A command that finds things one at a time can show them as it goes: it writes events (`meta_plugin::Event`: a record, progress or a warning, one JSON object per line) to the file `META_EVENTS` names, and meta shows them as they arrive (`Ctx::{record, progress, emit}` in Rust, `printf '%s\n' "$event" >> "$META_EVENTS"` in a script). The call still ends with its response.

A Rust plugin uses the types and `run_plugin`, which reads the request, hands the handler a `Ctx` (for warnings and `--strict`) and writes the response. The SDK isn't published: depend on it by git (`meta_plugin = { git = "https://github.com/gitkb/meta", features = ["parse"] }`). Besides the protocol it gives a plugin meta's exit codes, output and escaping, the process owner (`meta_plugin::process`, on by default: a plugin's children stop when it is told to), and, with the feature `testing`, a test kit (`meta_plugin::testing`: a sandbox, a workspace builder with `testing-git`, and `call`, which sends a plugin a request as meta does).

Worktree sets are meta's, for any provider (feature `sets`): a set is a folder in the workspace's worktrees folder, its members are the projects whose folders are in it, and meta sends the set a command runs in (or names) as the request's `set`. A plugin that makes sets records itself as the set's owner with `meta_plugin::sets::create`; meta refuses a command on a named set from any other provider. The same module has the set's lease, `validate_name` and `fire` for the workspace's `worktree.hooks`.

A plugin may call meta back (feature `client`): meta sets `META_EXE` (itself) and `META_DEPTH` for every program it starts, and `meta_plugin::client::Meta::from_env().cwd(dir).args(["exec", "--", "pwd"]).json()?.wait()?` runs `meta --json exec -- pwd` there and returns its `Envelope`. Calls nest at most 8 deep. A program that holds a set's lease passes it on with `.lease(token)` (`META_LEASE`); meta strips `META_LEASE` and the call's own files from everything else it starts.

meta's options go before the command and reach a plugin resolved: the selection as the request's projects, the rest as its `meta` options. Never define an option of meta's of your own (an option of the tool you adapt that only shares a spelling and means something else stays the tool's: `git clone --depth N` is history depth). Parse your arguments with `meta_plugin::parse_args`, which gives meta's options typed after your command the tip with the line that has them before it.

`commands` in a plugin's info start with its word. For `meta git worktree create x`, the longest listed command that matches the typed words whole (`git worktree create`) is the request's command and the rest (`x`) its arguments; with none matching, the command is the word alone. `meta <word>` and `meta <word> --help` print the plugin's help, from its info. `runs_as_typed` names the tool that every other command runs as typed, in each selected repo (`git` for meta-git: `meta git log` runs `git log`); for its help meta answers itself.

## Other languages

The same plugin in Python, as `meta-hello` (executable) on PATH or in a plugins folder:

```python
#!/usr/bin/env python3
import json, os, sys

if sys.argv[1:] == ["--meta-plugin-info"]:
    print(json.dumps({"name": "hello", "version": "0.1.0", "commands": ["hello"],
                      "help": {"usage": "meta hello NAME"}}))
    sys.exit(0)
request = json.load(open(os.environ["META_REQUEST"]))
if len(request["args"]) != 1:
    response = {"error": {"code": "usage", "message": "meta hello takes one name"}}
else:
    steps = [{"project": p["path"], "dir": p["abs"], "argv": ["echo", "hello " + request["args"][0]]}
             for p in request["projects"]]
    response = {"plan": {"steps": steps}}
json.dump(response, open(os.environ["META_RESPONSE"], "w"))
```

## Testing a plugin

```bash
~/.meta/plugins/meta-hello --meta-plugin-info | jq .
meta --dry-run hello team        # what meta would run, run nowhere
meta --json hello team | jq .    # the envelope: one row per step
```

A Rust plugin's tests send it requests with `meta_plugin::testing::call`, through the same files meta uses.

If `meta hello` says the command is unrecognized, the file isn't where meta looks, isn't executable, or isn't named `meta-hello`; `meta --verbose plugin list` shows the folders meta looked in and what it found there.

## Publishing

`meta plugin install <name>` downloads a plugin from the registries listed in `$META_DATA_DIR/config.yaml` (default `~/.meta/config.yaml`, `registries:`; without one, the public registry) into `~/.meta/plugins/`; `meta plugin search <query>` searches them. Or ship the binary and have users put it in a plugins folder or on PATH.
