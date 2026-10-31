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
| `-c, --config <FILE>` | Custom config file path (default: `.looprc`) |
| `-i, --include <DIR>` | Only run in this directory (repeat for more) |
| `-e, --exclude <DIR>` | Skip this directory, by name or path end (repeat for more) |
| `-v, --verbose` | Enable verbose output |
| `-s, --silent` | Suppress all output except summary |
| `--add-aliases-to-global-looprc` | Add shell aliases to global config |

### Filtering Directories

```bash
# Only specific directories
loop -i api -i web git status

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
  "verbose": false,
  "silent": false
}
```

### Configuration Priority

1. Command-line arguments (highest priority)
2. Local `.looprc` in current directory
3. Global `~/.looprc`
4. Defaults (all child directories)

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `directories` | `string[]` | Directories to include |
| `ignore` | `string[]` | Directories to exclude |
| `verbose` | `bool` | Enable verbose output |
| `silent` | `bool` | Suppress output |
| `add_aliases_to_global_looprc` | `bool` | Enable alias support |

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

Only shows final summary.

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

Add aliases to your global config:

```bash
loop --add-aliases-to-global-looprc
```

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
