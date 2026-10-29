# loop – Command Line Utility

**loop** is a command line utility for running commands in multiple repositories or directories in parallel. It is designed for monorepos and multi-package projects, making repetitive tasks like dependency management, testing, and code generation fast and easy. The utility consists of two main parts: [loop_cli](../loop_cli/README.md) (the CLI app) and [loop_lib](../loop_lib/src/lib.rs) (the Rust library for programmatic use).

[![Loop System](../docs/assets/meta-cli-screenshot.png)](../docs/assets/meta-cli-screenshot.png)

---

## Table of Contents

- [Overview](#overview)
- [Key Features](#key-features)
- [How It Works](#how-it-works)
- [Usage](#usage)
- [Architecture Summary](#architecture-summary)
- [FAQ](#faq)
- [Contribution Notes](#contribution-notes)
- [Advanced Guides](#advanced-guides)
- [Cross-References](#cross-references)

---

## Overview

**loop** lets you run a command in multiple repositories or directories at once. By default, it runs in all child repositories of the current directory, but you can use expressive options to control exactly where your command will run. You can also specify defaults in a `.looprc` config file. For developers, the [loop_lib](../loop_lib/src/lib.rs) library exposes these capabilities for programmatic use, including directory filtering and config parsing.

## Key Features

- Run any command in multiple directories in parallel (as a program with arguments, never through a shell; use `sh -c '...'` when you want one)
- Defaults to all child repositories, with flexible inclusion/exclusion via CLI or config
- Supports `.looprc` for persistent configuration
- Verbose and silent output modes
- Robust error handling and reporting
- Designed for Unix-like environments

## How It Works

1. **Configuration**: Specify target directories and options in `.looprc` (JSON).
2. **Execution**: Use the `loop` CLI utility to run commands in parallel.
3. **Library**: App developers can use `loop_lib` to leverage loop's power programmatically, including directory filtering and config support.

## Usage

To run a command in all child repositories:
```sh
loop <your-command>
```

Examples:
```sh
loop git status
loop npm install
loop cargo test
```

- To include or exclude specific directories, use CLI options (see [loop_cli/README.md](../loop_cli/README.md)).
- To set persistent defaults, create a `.looprc` file in your project root.

See [loop_cli/README.md](../loop_cli/README.md) for full CLI usage and [loop_lib/src/lib.rs](../loop_lib/src/lib.rs) for library integration.

## Architecture Summary

- **loop_cli**: CLI app, parses arguments, dispatches commands.
- **loop_lib**: Rust library for directory expansion, command execution, config parsing; enables programmatic use of loop's features in other apps.
- **.looprc**: JSON config file for directories and options.

See [Architecture Overview](loop_architecture_overview.md) for details.

## FAQ

See [FAQ / Troubleshooting](loop_faq_troubleshooting.md).

## Contribution Notes

- Written in Rust; see [Cargo.toml](../loop_lib/Cargo.toml).
- Contributions should follow the [contributing guidelines](../contributing.md).
- Add new features in `loop_lib/src/` (library) or CLI enhancements in `loop_cli/src/`.
- Include tests for all changes.

## Advanced Guides

- [Advanced Usage](loop_advanced_usage.md)
- [Architecture Overview](loop_architecture_overview.md)
- [FAQ / Troubleshooting](loop_faq_troubleshooting.md)
- [Visual Assets](loop_assets.md)

## Cross-References

- [loop_cli – CLI Usage](../loop_cli/README.md)
- [loop_lib – Rust Library](../loop_lib/src/lib.rs)
- [Meta Main Documentation](../../README.md)