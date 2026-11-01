# Architecture Overview (loop)

This document provides a high-level overview of the **loop** system's architecture, including its core components, execution flow, and extensibility.

## Table of Contents

- [System Overview](#system-overview)
- [Core Components](#core-components)
- [Configuration](#configuration)
- [Command Execution Flow](#command-execution-flow)
- [Extensibility Points](#extensibility-points)
- [Key Modules](#key-modules)
- [Visual Overview](#visual-overview)
- [See Also](#see-also)

---

## System Overview

The loop system is a command execution engine for the Meta platform, designed to automate tasks across multiple directories.

## Core Components

- **loop_cli**: CLI entry point, parses arguments, dispatches commands.
- **loop_lib**: Core Rust library for directory expansion, command execution, config parsing, and error handling.
- **.looprc**: JSON configuration file specifying directories, ignore patterns, and options.

## Configuration

The `.looprc` file controls which directories are targeted and how commands are executed:

```json
{
  "directories": ["pkg1", "pkg2"],
  "ignore": ["legacy_pkg"],
  "parallel": true
}
```

Every key is listed in [loop_cli/README.md](../loop_cli/README.md#fields).

## Command Execution Flow

1. User invokes a `loop` command via CLI.
2. CLI loads `.looprc` and parses arguments.
3. loop_lib expands directories and applies filters.
4. Commands run one at a time, or all at once with `"parallel": true`.
5. Results are aggregated and reported.

## Extensibility Points

- Add new CLI options in `loop_cli/src/`.
- Extend core logic in `loop_lib/src/`.
- Support for custom directory filters and output formats.

## Key Modules

- `config`: Loads and validates `.looprc`.
- `executor`: Handles parallel command execution.
- `filter`: Applies inclusion/exclusion logic.
- `report`: Aggregates and formats results.

## See Also

- [Loop Overview](loop.md)
- [Advanced Usage](loop_advanced_usage.md)
- [FAQ / Troubleshooting](loop_faq_troubleshooting.md)