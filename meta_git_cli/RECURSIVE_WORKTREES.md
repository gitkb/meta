# Recursive worktree inventory

```sh
meta git worktree list --recursive --json
meta git worktree status my-feature --recursive --json
```

The global `--recursive` / `-r` option now reaches these two commands.
Without it, discovery still stops at the first repository in each subtree.
With it, discovery continues beneath repositories, retaining fully qualified
relative aliases, sorted deterministically. Only actual `.git` files count as
members; configured repositories without a realized checkout are absent.

For a set containing root, `open-source`, and `open-source/atc`, the default
member aliases remain `[".", "open-source"]`; recursive inventory returns
`[".", "open-source", "open-source/atc"]`. Existing JSON fields retain their
names and types.

Recursive discovery does not follow directory symlinks or enter hidden
directories. It stops with an error on traversal failures, malformed Git files,
failed Git status observations, or more than 2,000,000 scanned entries per set.
`status NAME` confines the scan to one set; these errors produce a nonzero exit
without an incomplete successful JSON payload.

`list` continues with healthy sets if an individual set cannot be read, and
exits successfully. Human output reports the unavailable sets on stderr. JSON
includes an additive `warnings` array with each unavailable set's `name`,
`root`, and `message`; those sets are omitted from `worktrees`, so warnings mean
the listing is incomplete. The array is omitted when there are no warnings.
Failure to enumerate the worktree root itself remains an error. Default
nonrecursive discovery and output are unchanged.

Build trees are scanned and can add latency. Consumers should bound subprocess
runtime and treat failure as unknown, not an empty or clean inventory.

This option does not change create, add, remove, prune, exec, diff, or automatic
worktree context detection. It performs no Git mutation. The accompanying
`meta_cli` discovery API must land before this plugin change is built/released.
