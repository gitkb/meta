# Advanced Usage Guide (loop)

This guide covers advanced features of the **loop** system, including parallel execution strategies, configuration tips, scripting, and integration with CI pipelines.

## Table of Contents

- [Parallel Execution Strategies](#parallel-execution-strategies)
- [Advanced Configuration](#advanced-configuration)
- [Scripting with loop_cli](#scripting-with-loop_cli)
- [Integration with CI/CD](#integration-with-cicd)
- [Debugging & Troubleshooting](#debugging--troubleshooting)
- [See Also](#see-also)

---

## Parallel Execution Strategies

loop runs in one directory at a time. With `"parallel": true` in `.looprc` it runs in all at once, `max_parallel` caps how many, and `spawn_stagger_ms` spaces out the starts. A failure in one directory never stops the others; loop exits non-zero if any failed.

```json
{
  "directories": ["pkg1", "pkg2"],
  "parallel": true,
  "max_parallel": 4
}
```

Every key is listed in [loop_cli/README.md](../loop_cli/README.md#fields).

## Advanced Configuration

- **Ignore**: Exclude directories with the `ignore` field (or `--exclude`): each entry is a directory's name (`node_modules`) or the end of its path (`vendor/old`), matched by whole names, so `.git` doesn't exclude `.github`. It applies to directories listed in `directories` or given with `--include` too.
- **Verbose Output**: Enable detailed logs with `--verbose` or `"verbose": true` in config.

## Scripting with loop_cli

`loop_cli` is script-friendly and can be used in shell scripts or CI jobs:

```sh
loop --include pkg1,pkg2 cargo test
```

Chain commands for complex workflows:

```sh
loop npm install && loop npm test
```

## Integration with CI/CD

Integrate loop into CI pipelines for parallelized builds, tests, or deployments. Example (GitHub Actions):

```yaml
- name: Run tests in all packages
  run: loop cargo test
```

## Debugging & Troubleshooting

- Use `loop --verbose` for detailed output.
- Check `.looprc` for correct directory and ignore patterns.
- For library integration, enable Rust backtraces with `RUST_BACKTRACE=1`.

## See Also

- [loop_cli/README.md](../loop_cli/README.md)
- [FAQ / Troubleshooting](loop_faq_troubleshooting.md)
- [Architecture Overview](loop_architecture_overview.md)