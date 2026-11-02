# loop – Command Line Utility

**loop** runs a command in several directories, one at a time or in parallel. It is the engine under `meta exec` and meta's plugins ([loop_lib](../loop_lib/src/lib.rs)), and a standalone CLI ([loop_cli](../loop_cli/README.md)) for any folder of packages or repos, with no `.meta.yaml`.

## Usage

By default loop runs in the current directory and each directory in it, except `.git`, one at a time:

```sh
loop git status
loop cargo test
loop --include pkg1,pkg2 npm install
loop --exclude legacy_pkg npm test
```

The command runs as a program with its arguments, never through a shell: `loop echo '$HOME'` prints `$HOME`. For a shell, ask for one: `loop sh -c 'npm install && npm test'`.

A failure in one directory never stops the others; loop lists the failures at the end and exits non-zero. Run `loop --help` for the options.

## Configuration

A `.looprc` (JSON) in the current directory, or the file `-c` names, sets defaults:

```json
{
  "directories": ["pkg1", "pkg2"],
  "ignore": ["legacy_pkg"],
  "parallel": true,
  "max_parallel": 4
}
```

`parallel` runs in every directory at once (there is no command-line flag for it), `max_parallel` caps how many, and `spawn_stagger_ms` spaces out the starts. `ignore` (or `--exclude`) takes a directory's name (`node_modules`) or the end of its path (`vendor/old`), matched by whole names, so `.git` doesn't exclude `.github`; it applies to directories listed in `directories` or given with `--include` too. [loop_cli/README.md](../loop_cli/README.md#fields) lists every key, and where settings come from.

## Contributing

loop's code is in `loop_lib/src/` (the library) and `loop_cli/src/` (the CLI); see the [contribution guide](contributing.md).
