# Loop CLI

Loop is a command-line tool that executes commands across multiple directories simultaneously. It's the underlying execution engine for `meta` and can also be used as a standalone tool.

## Installation

Loop is installed automatically with `meta`. For standalone use:

```bash
cargo install --git https://github.com/gitkb/meta --locked loop_cli
```

## Usage

```
loop [OPTIONS] [COMMAND]...
```

### Basic Examples

```bash
# Run git status in all child directories
loop git status

# Run npm install in all child directories
loop npm install

# Run cargo test in all child directories
loop cargo test
```

### Options

| Option | Description |
|--------|-------------|
| `-c, --config <FILE>` | Config file (default: `.looprc`, if there is one); the file must exist |
| `-i, --include <DIR>` | Only run in these directories (comma list, or repeat) |
| `-e, --exclude <DIR>` | Skip these directories, by name or path end (comma list, or repeat) |
| `-v, --verbose` | Enable verbose output |
| `-s, --silent` | Suppress command output |
| `--add-aliases-to-global-looprc` | Import your shell's aliases into `~/.looprc` (runs no command) |

A flag only turns a setting on: `"silent": true` in the file stays without `-s`.

### Filtering Directories

```bash
# Only specific directories
loop -i api,web git status

# Exclude directories
loop -e legacy -e deprecated npm test

# Combine filters
loop -i api -i web -i worker -e web git pull
```

## Configuration

Loop uses a `.looprc` file (JSON format) for persistent configuration:

```json
{
  "directories": ["api", "web", "worker"],
  "ignore": ["node_modules", "target", ".git"],
  "parallel": true,
  "aliases": {"gs": "git status"}
}
```

Every key is optional; one left out takes its default (`{}` is the same as no
file). A key loop doesn't know is an error, as is a `-c` file that doesn't
exist.

### Where settings come from

1. Command-line flags, when given
2. The config file: `-c FILE`, else `./.looprc`
3. Defaults: the current directory and its child directories, without `.git`

Aliases also come from `~/.looprc`: its `aliases`, then the config file's,
which win for the same name. Only its aliases are used, but the whole file
must be valid.

### Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `directories` | `string[]` | `[]`: `.` and its child directories | Directories to run in |
| `ignore` | `string[]` | `[".git"]` | Directories to exclude, by name (`node_modules`) or path end (`vendor/old`), whole names only: `.git` doesn't exclude `.github`. Setting it replaces the default |
| `verbose` | `bool` | `false` | Enable verbose output |
| `silent` | `bool` | `false` | Suppress command output |
| `parallel` | `bool` | `false` | Run in every directory at once, showing each output when all are done |
| `max_parallel` | `number` | one per CPU core | With `parallel`, at most this many at once (1 or more) |
| `spawn_stagger_ms` | `number` | `0` | With `parallel`, milliseconds between two starts |
| `dry_run` | `bool` | `false` | Print what would run, run nothing |
| `json_output` | `bool` | `false` | Print the results as JSON |
| `env` | `object` | none | Environment variables for every command |
| `aliases` | `object` | `{}` | Programs that stand for a command: `{"gs": "git status"}` runs `gs -s` as `git status -s` |
| `root_dir` | `string` | none | Show this directory as `.` and others by their path from it |

## How It Works

1. **Directory Discovery**: Expands target directories based on config and CLI options
2. **Filtering**: Applies include/exclude rules
3. **Execution**: Runs the command in each directory
4. **Output**: Aggregates and displays results with status

### Execution Flow

```
loop npm test
     │
     ▼
┌─────────────────┐
│ Load Config     │
│ (.looprc)       │
└─────────────────┘
     │
     ▼
┌─────────────────┐
│ Expand          │
│ Directories     │
└─────────────────┘
     │
     ▼
┌─────────────────┐
│ Apply Filters   │
│ (include/exclude)│
└─────────────────┘
     │
     ▼
┌─────────────────┐
│ Execute Command │
│ (per directory) │
└─────────────────┘
     │
     ▼
┌─────────────────┐
│ Collect Results │
│ & Display       │
└─────────────────┘
```

## Output Modes

### Default Output

Shows command execution with directory prefixes:

```
> api
Tests passed

> web
Tests passed

> worker
Tests failed: 2 errors

Summary: 2 succeeded, 1 failed
```

### Verbose Mode

```bash
loop -v npm test
```

Shows additional execution details.

### Silent Mode

```bash
loop -s npm test
```

Prints no command output and no summary; a directory that doesn't exist is
still reported. If a command fails, loop ends with
`Error: At least one command failed` and a non-zero exit status.

## Error Handling

Loop continues execution even if a command fails in one directory. After all directories are processed, it shows:

- Success/failure count
- Failed directory names
- Exit codes from failed commands

Exit code is non-zero if any command failed.

## Integration with Meta

Loop is the execution engine underlying `meta`. When you run:

```bash
meta npm install
```

Meta processes configuration, applies filters, and delegates execution to loop_lib.

### Key Differences

| Feature | `loop` | `meta` |
|---------|--------|--------|
| Config format | `.looprc` (JSON) | `.meta.yaml` |
| Project tags | No | Yes |
| Plugin system | No | Yes |
| Git-aware clone | No | Yes (via plugin) |
| MCP server | No | Yes |

Use `loop` for simple multi-directory execution. Use `meta` for full multi-repository management.

## Advanced Usage

### Shell Aliases

Import your interactive shell's aliases into `~/.looprc`:

```bash
loop --add-aliases-to-global-looprc
```

It shows the change and asks before writing. Only `aliases` changes (the
shell's are added to those already there); the file's other keys stay. A
`~/.looprc` loop can't read is reported, not replaced.

loop runs commands without a shell, so an alias value is split into words
(POSIX quoting, nothing expanded or run) that replace the program. An alias
`ll="ls -la | less"` therefore runs `ls` with the arguments `-la`, `|` and
`less`. For pipes and `&&`, ask for a shell:

```bash
loop sh -c 'ls -la | less'
```

### Combining with Other Tools

```bash
# Pipe output
loop git log --oneline -1 | grep -v "^>"

# With xargs
loop ls | xargs -I{} echo "Dir: {}"

# Conditional execution
loop npm test && echo "All tests passed"
```

## Library Usage

Loop is available as a Rust library (`loop_lib`) for programmatic use:

```rust
use loop_lib::{run, LoopConfig};

let config = LoopConfig {
    directories: vec!["api".into(), "web".into()],
    ignore: vec!["node_modules".into()],
    ..Default::default()
};

// The program and its arguments, run as given (no shell)
run(&config, &["npm".to_string(), "test".to_string()])?;
```

See [loop_lib documentation](../loop_lib/) for details.

## See Also

- [Meta CLI Documentation](../README.md)
- [Loop System Overview](../docs/loop.md)
- [Architecture Overview](../docs/architecture_overview.md)
