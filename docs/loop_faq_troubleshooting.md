# FAQ / Troubleshooting Guide (loop)

This guide addresses common questions and issues encountered when using the **loop** system (loop_cli and loop_lib).

## Table of Contents

- [General Usage](#general-usage)
- [Configuration](#configuration)
- [Parallel Execution](#parallel-execution)
- [Debugging & Troubleshooting](#debugging--troubleshooting)
- [Getting Help](#getting-help)
- [See Also](#see-also)

---

## General Usage

**Q: `loop` command not found?**
A: Ensure the binary is installed and in your PATH, or use `cargo run --` from the loop_cli directory.

**Q: How do I see available options?**
A: Run `loop --help` for a full list of commands and options.

## Configuration

**Q: Why are some directories skipped?**
A: Check your `.looprc` for correct `directories` and `ignore` patterns.

**Q: How do I specify which directories to include or exclude?**
A: Use the `directories` and `ignore` fields in `.looprc`, or CLI flags like `--include` and `--exclude`.

## Parallel Execution

**Q: How do I run in parallel, and how many at once?**
A: Set `"parallel": true` in `.looprc`, and `max_parallel` to cap it. Without it, loop runs one directory at a time.

**Q: What happens if a command fails in one directory?**
A: loop runs it in the others too, lists the failures at the end and exits non-zero.

## Debugging & Troubleshooting

- Use `loop --verbose` for detailed logs.
- For library integration, set `RUST_BACKTRACE=1` to get stack traces.
- Check for correct permissions and executable paths.

## Getting Help

- See [Loop Overview](loop.md) for an overview.
- For advanced usage, see [Advanced Usage](loop_advanced_usage.md).
- For architecture, see [Architecture Overview](loop_architecture_overview.md).

## See Also

- [loop_cli/README.md](../loop_cli/README.md)
- [loop_lib/src/lib.rs](../loop_lib/src/lib.rs)