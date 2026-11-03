# Meta Workspace Discipline

You are working in a **meta-repo** — multiple independent git repositories managed together.

## Required Behaviors

1. **Use `meta git` for cross-repo operations** — NOT raw `git`
   - `meta git status` shows all repos at once
   - `meta git commit -m "msg"` commits what is staged, in each repo that has staged changes

2. **Use `meta exec` for cross-repo commands** — NOT `cd <repo> && cmd`
   - `meta exec -- npm install` runs in all repos
   - `meta --include X,Y exec -- cmd` targets specific repos

3. **Check scope before committing**
   - Run `meta git status` to see which repos have changes
   - Verify you intend to commit in all listed repos

4. **Target precisely with filters**
   - `--include repo1,repo2` — only these repos
   - `--exclude repo3` — skip this repo
   - `--tag backend` — only repos with this tag

5. **Move pinned repos with meta, not by hand**
   - A `depends_on` entry `- core: <sha or tag>` pins `core` for worktree sets
   - After changing a pin, run `meta git update` inside the set: it moves only checkouts that lose nothing
