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

The loop system can execute commands in parallel across many directories. You can control concurrency and error handling via CLI flags or `.looprc`:

```json
{
  "directories": ["pkg1", "pkg2"],
  "concurrency": 4,
  "fail_fast": true
}
```

## Advanced Configuration

- **Directory Globs**: Use wildcards in `.looprc` to target groups of directories.
- **Ignore Patterns**: Exclude directories with the `ignore` field.
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