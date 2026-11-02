# Changelog

## [0.3.0](https://github.com/gitkb/meta/compare/v0.2.22...v0.3.0) (2026-11-02)


### ⚠ BREAKING CHANGES

* one owner for output, colour, warnings and exit codes; lints that make stray prints and panics impossible [[tasks/harmony-2996]] ([#153](https://github.com/gitkb/meta/issues/153))
* meta git update pulls --ff-only and recurses only with -r; the clone queue runs on rayon [[tasks/harmony-2986]] ([#151](https://github.com/gitkb/meta/issues/151))
* meta context reads every repo on every call and says what git says ([#150](https://github.com/gitkb/meta/issues/150))
* worktrees resolve one plan, read one record, and say what they did [[tasks/harmony-2973]] ([#149](https://github.com/gitkb/meta/issues/149))
* a meta run owns its children and the worktree sets it was making [[tasks/harmony-2972]] ([#147](https://github.com/gitkb/meta/issues/147))
* act only on project folders that are repositories of their own; stop refusing where projects live [[tasks/harmony-2968]] ([#146](https://github.com/gitkb/meta/issues/146))
* one selection, clap renders the help, meta's options before the command ([#148](https://github.com/gitkb/meta/issues/148))
* **loop:** one .looprc format; alias import keeps your settings; paths stay paths ([#145](https://github.com/gitkb/meta/issues/145))
* only the called plugin runs; plugin installs can't escape the plugins folder ([#144](https://github.com/gitkb/meta/issues/144))
* hide repo URL credentials wherever meta prints a URL; remote helpers never read as ssh ([#142](https://github.com/gitkb/meta/issues/142))
* **meta-mcp:** act only on the project asked about, never outside the workspace ([#143](https://github.com/gitkb/meta/issues/143))
* init claude outside a meta workspace is an error.
* one config load, one git wrapper, one parser per thing ([#139](https://github.com/gitkb/meta/issues/139))
* turn on the first clippy lint group and delete dead code ([#137](https://github.com/gitkb/meta/issues/137))
* existing checkouts must move their child clones aside before pulling; see the PR description.
* rename a .meta or .meta.json config to .meta.yaml (same content).
* **plugins:** rename a .meta.yml config to .meta.yaml.
* **worktree:** keep worktree records beside git, drop the global store ([#126](https://github.com/gitkb/meta/issues/126))
* drop the permission, ownership and symlink trust checks ([#125](https://github.com/gitkb/meta/issues/125))

### Features

* **meta_git_cli:** sync 5492231 ([#103](https://github.com/gitkb/meta/issues/103)) ([d57b145](https://github.com/gitkb/meta/commit/d57b14576dbf575975b6dcf5948971360c8e7a78))
* **meta_git_cli:** sync 6ff549a ([#99](https://github.com/gitkb/meta/issues/99)) ([51c6bd6](https://github.com/gitkb/meta/commit/51c6bd6f8ebd4b0d2f668a9b272892dce326cbd2))
* **meta_git_cli:** sync d5e05ca ([#101](https://github.com/gitkb/meta/issues/101)) ([79b4b7c](https://github.com/gitkb/meta/commit/79b4b7c0e105df49931c3a5f7c864c5d9d3f1f8d))
* **meta_project_cli:** sync 93f40bf ([#104](https://github.com/gitkb/meta/issues/104)) ([5bef81e](https://github.com/gitkb/meta/commit/5bef81ede61503c7e73510727aab21c33035d083))
* org rename ([#94](https://github.com/gitkb/meta/issues/94)) ([a44337c](https://github.com/gitkb/meta/commit/a44337ce03373e3a7cf892c4cc451b1f6d88b08b))
* **skills:** keep meta's skills in .meta/skills, linked for Claude a… ([ed594b9](https://github.com/gitkb/meta/commit/ed594b94ce69e0002103957a411ef51585fcf69e))
* **skills:** keep meta's skills in .meta/skills, linked for Claude and Codex ([#105](https://github.com/gitkb/meta/issues/105)) ([ed594b9](https://github.com/gitkb/meta/commit/ed594b94ce69e0002103957a411ef51585fcf69e))
* **worktree:** keep worktree records beside git, drop the global store ([#126](https://github.com/gitkb/meta/issues/126)) ([6bc56f7](https://github.com/gitkb/meta/commit/6bc56f75b89f2095dcd8887918303fc671243ca9))


### Bug Fixes

* --all refuses one repo given two branches, as without --all [[ta… ([07559ab](https://github.com/gitkb/meta/commit/07559ab52fe7c0429835db7dfc28effac7965c0a))
* --from-pr matches the URL git fetches from (insteadOf applied) [… ([9d16e22](https://github.com/gitkb/meta/commit/9d16e2299e8fbee053f9fee508001e1849055a1f))
* a .git file names its checkout, not a bundle's clone [[tasks/har… ([9d16e22](https://github.com/gitkb/meta/commit/9d16e2299e8fbee053f9fee508001e1849055a1f))
* a bare project's gone upstream is named and gone, not none; the … ([fb07953](https://github.com/gitkb/meta/commit/fb07953658a31ed077e118cf8a6e2a82aab18747))
* a bracketed URL host only at the start or after @, as git reads … ([9d16e22](https://github.com/gitkb/meta/commit/9d16e2299e8fbee053f9fee508001e1849055a1f))
* a branch or tag named MERGE_HEAD, CHERRY_PICK_HEAD or REVERT_HEA… ([fb07953](https://github.com/gitkb/meta/commit/fb07953658a31ed077e118cf8a6e2a82aab18747))
* a bundle clones into its name without .bundle; refs need a repos… ([9d16e22](https://github.com/gitkb/meta/commit/9d16e2299e8fbee053f9fee508001e1849055a1f))
* a bundle file's clone keeps its own name less .bundle [[tasks/ha… ([9d16e22](https://github.com/gitkb/meta/commit/9d16e2299e8fbee053f9fee508001e1849055a1f))
* a checkout git won't read keeps its ahead/behind counts in the t… ([fb07953](https://github.com/gitkb/meta/commit/fb07953658a31ed077e118cf8a6e2a82aab18747))
* a clone's output hides its known URL; cut lines lose an open URL ([41a4673](https://github.com/gitkb/meta/commit/41a467352eaf16c7510e0c3c05b620ed4f12d38d))
* a lock that can't be tested is an error, and a hangup ends meta'… ([4b51030](https://github.com/gitkb/meta/commit/4b51030ab39196535e43d808b1d348e7e3cf6177))
* a meta run owns its children and the worktree sets it was making [[tasks/harmony-2972]] ([#147](https://github.com/gitkb/meta/issues/147)) ([4b51030](https://github.com/gitkb/meta/commit/4b51030ab39196535e43d808b1d348e7e3cf6177))
* a stop always ends; a program meta started kills its children fi… ([4b51030](https://github.com/gitkb/meta/commit/4b51030ab39196535e43d808b1d348e7e3cf6177))
* a terminal is put in virtual-terminal mode for forced colour too… ([bd0faab](https://github.com/gitkb/meta/commit/bd0faab233adf2c9888eec8ce90060927f66c051))
* act only on project folders that are repositories of their own; stop refusing where projects live [[tasks/harmony-2968]] ([#146](https://github.com/gitkb/meta/issues/146)) ([4c08ffc](https://github.com/gitkb/meta/commit/4c08ffca62a8204400234fce9452520efd6f8de5))
* after loop's summary of failed commands, meta exits 1 without an… ([bd0faab](https://github.com/gitkb/meta/commit/bd0faab233adf2c9888eec8ce90060927f66c051))
* an empty plan fails the preview; simpler ephemeral preview ([8ec7c7c](https://github.com/gitkb/meta/commit/8ec7c7c88ed649aad8664c1da92c6ceb2ad2dbb6))
* bare projects keep their branch; isolate the list fixture; clone… ([9d16e22](https://github.com/gitkb/meta/commit/9d16e2299e8fbee053f9fee508001e1849055a1f))
* bugs from the 2026-10-29 audit and the meta[#135](https://github.com/gitkb/meta/issues/135) Ubuntu test ([#140](https://github.com/gitkb/meta/issues/140)) ([4b0f44e](https://github.com/gitkb/meta/commit/4b0f44eccc0f3fac78cfae8b968d48aa8a71f864))
* bugs from the 2026-10-29 audit and the meta[#135](https://github.com/gitkb/meta/issues/135) Ubuntu test [[ta… ([4b0f44e](https://github.com/gitkb/meta/commit/4b0f44eccc0f3fac78cfae8b968d48aa8a71f864))
* **build:** install and ship meta-rust again ([f591502](https://github.com/gitkb/meta/commit/f5915022c9212f28d05f6712cc0ec18effd04075))
* **build:** install meta-mcp and loop; keep Cargo.lock in step at rel… ([d1803e5](https://github.com/gitkb/meta/commit/d1803e5b99e8a0e57ac31668c5c5c343ff79467a))
* **build:** install meta-mcp and loop; keep Cargo.lock in step at release ([#109](https://github.com/gitkb/meta/issues/109)) ([d1803e5](https://github.com/gitkb/meta/commit/d1803e5b99e8a0e57ac31668c5c5c343ff79467a))
* **build:** make install overwrites untracked binaries; drop meta-rust ([f591502](https://github.com/gitkb/meta/commit/f5915022c9212f28d05f6712cc0ec18effd04075))
* **build:** make install overwrites untracked binaries; ship meta-rust ([#110](https://github.com/gitkb/meta/issues/110)) ([f591502](https://github.com/gitkb/meta/commit/f5915022c9212f28d05f6712cc0ec18effd04075))
* **build:** make uninstall still removes meta-rust ([f591502](https://github.com/gitkb/meta/commit/f5915022c9212f28d05f6712cc0ec18effd04075))
* **build:** uninstall by package name so a version bump can't hide an… ([d1803e5](https://github.com/gitkb/meta/commit/d1803e5b99e8a0e57ac31668c5c5c343ff79467a))
* **ci:** refresh Cargo.lock in child sync PRs; leave release builds u… ([d1803e5](https://github.com/gitkb/meta/commit/d1803e5b99e8a0e57ac31668c5c5c343ff79467a))
* clone passes the URL after --; isolate and add tests from review… ([9d16e22](https://github.com/gitkb/meta/commit/9d16e2299e8fbee053f9fee508001e1849055a1f))
* **clone:** a link's target that can't be inspected is an error [[tas… ([07559ab](https://github.com/gitkb/meta/commit/07559ab52fe7c0429835db7dfc28effac7965c0a))
* CodeRabbit findings [[tasks/harmony-2947]] ([9d16e22](https://github.com/gitkb/meta/commit/9d16e2299e8fbee053f9fee508001e1849055a1f))
* CodeRabbit round: loop -e ., stricter HEAD link, unreadable HEAD… ([4b0f44e](https://github.com/gitkb/meta/commit/4b0f44eccc0f3fac78cfae8b968d48aa8a71f864))
* CodeRabbit: each GIT_CONFIG entry needs its value, a usage error… ([bd0faab](https://github.com/gitkb/meta/commit/bd0faab233adf2c9888eec8ce90060927f66c051))
* CodeRabbit: loop reads JSON mode before it prints; a Windows con… ([bd0faab](https://github.com/gitkb/meta/commit/bd0faab233adf2c9888eec8ce90060927f66c051))
* decide a cut git line on its length as written ([41a4673](https://github.com/gitkb/meta/commit/41a467352eaf16c7510e0c3c05b620ed4f12d38d))
* **deps:** update rustls, rustls-webpki and tar for security advisori… ([69d26f0](https://github.com/gitkb/meta/commit/69d26f08bbec069a26597e7e4d01cde67f95664e))
* **deps:** update rustls, rustls-webpki, tar, webpki-roots and crossbeam-epoch; turn on Renovate lock maintenance ([#136](https://github.com/gitkb/meta/issues/136)) ([69d26f0](https://github.com/gitkb/meta/commit/69d26f08bbec069a26597e7e4d01cde67f95664e))
* **deps:** update webpki-roots and crossbeam-epoch for RustSec adviso… ([69d26f0](https://github.com/gitkb/meta/commit/69d26f08bbec069a26597e7e4d01cde67f95664e))
* drop the last --from-ref calls and stale upgrade docs [[tasks/ha… ([145f2f3](https://github.com/gitkb/meta/commit/145f2f3c681356fe1104a02c5738be9a68b913c0))
* drop the permission, ownership and symlink trust checks ([039b252](https://github.com/gitkb/meta/commit/039b252ac45376d1df31e323e64490aa611d3406))
* drop the permission, ownership and symlink trust checks ([#125](https://github.com/gitkb/meta/issues/125)) ([039b252](https://github.com/gitkb/meta/commit/039b252ac45376d1df31e323e64490aa611d3406))
* each command's stderr stays on stderr, and its bytes as it wrote… ([bd0faab](https://github.com/gitkb/meta/commit/bd0faab233adf2c9888eec8ce90060927f66c051))
* each repo's name before its output; clone progress in order, num… ([bd0faab](https://github.com/gitkb/meta/commit/bd0faab233adf2c9888eec8ce90060927f66c051))
* ephemeral preview refuses filters; a ref no repo has fails the p… ([8ec7c7c](https://github.com/gitkb/meta/commit/8ec7c7c88ed649aad8664c1da92c6ceb2ad2dbb6))
* forced colour stays forced on a Windows console; only the automa… ([bd0faab](https://github.com/gitkb/meta/commit/bd0faab233adf2c9888eec8ce90060927f66c051))
* git launches the commit editor; exact ephemeral and create previews ([8ec7c7c](https://github.com/gitkb/meta/commit/8ec7c7c88ed649aad8664c1da92c6ceb2ad2dbb6))
* git's errors keep their line breaks, each line under meta's mess… ([bd0faab](https://github.com/gitkb/meta/commit/bd0faab233adf2c9888eec8ce90060927f66c051))
* **git:** add git's colour after the user's own GIT_CONFIG_* entries … ([bd0faab](https://github.com/gitkb/meta/commit/bd0faab233adf2c9888eec8ce90060927f66c051))
* hide a clone's known URL after the cut check, not before ([41a4673](https://github.com/gitkb/meta/commit/41a467352eaf16c7510e0c3c05b620ed4f12d38d))
* hide repo URL credentials wherever meta prints a URL; remote helpers never read as ssh ([#142](https://github.com/gitkb/meta/issues/142)) ([41a4673](https://github.com/gitkb/meta/commit/41a467352eaf16c7510e0c3c05b620ed4f12d38d))
* hooks must be strings; one-line git plugin usage [[tasks/harmony… ([9d16e22](https://github.com/gitkb/meta/commit/9d16e2299e8fbee053f9fee508001e1849055a1f))
* **hooks:** isolate Git-local test environment ([#98](https://github.com/gitkb/meta/issues/98)) ([ae5818b](https://github.com/gitkb/meta/commit/ae5818b2dc4f93e61af30dd42c776cf781e23daa))
* in a terminal too, a signal another process sends ends meta with… ([4b51030](https://github.com/gitkb/meta/commit/4b51030ab39196535e43d808b1d348e7e3cf6177))
* **init:** don't read a settings.json reached through a link [[tasks/… ([145f2f3](https://github.com/gitkb/meta/commit/145f2f3c681356fe1104a02c5738be9a68b913c0))
* **install:** stop on unsupported platforms, explain broken transfers… ([91883f3](https://github.com/gitkb/meta/commit/91883f3570b8392bb6137ea62dc58ba8c6429940))
* **install:** verify release checksums and report download errors ([91883f3](https://github.com/gitkb/meta/commit/91883f3570b8392bb6137ea62dc58ba8c6429940))
* **install:** verify release checksums and report download errors ([#121](https://github.com/gitkb/meta/issues/121)) ([91883f3](https://github.com/gitkb/meta/commit/91883f3570b8392bb6137ea62dc58ba8c6429940))
* keep record errors visible; hook errors name the event only [[ta… ([07559ab](https://github.com/gitkb/meta/commit/07559ab52fe7c0429835db7dfc28effac7965c0a))
* keep the gitdir wording in the source-repo error [[tasks/harmony… ([9d16e22](https://github.com/gitkb/meta/commit/9d16e2299e8fbee053f9fee508001e1849055a1f))
* keep the public API: loop_lib::StdoutError, git::clear_repo_env … ([bd0faab](https://github.com/gitkb/meta/commit/bd0faab233adf2c9888eec8ce90060927f66c051))
* look up each of git's operation refs by its exact name [[tasks/h… ([fb07953](https://github.com/gitkb/meta/commit/fb07953658a31ed077e118cf8a6e2a82aab18747))
* **loop_cli:** sync 2a1b17a ([#120](https://github.com/gitkb/meta/issues/120)) ([10003a2](https://github.com/gitkb/meta/commit/10003a27dde5378092ecda9c82726d4223231330))
* **loop_cli:** sync 2b51829 ([#123](https://github.com/gitkb/meta/issues/123)) ([b352712](https://github.com/gitkb/meta/commit/b352712530488e341a1716aff63ae1f158d0a288))
* **loop_cli:** sync 5910ed3 ([#134](https://github.com/gitkb/meta/issues/134)) ([c2afaeb](https://github.com/gitkb/meta/commit/c2afaeb96c8fb567f325e0d498da4da0e365dc3a))
* **loop_cli:** sync d551fc8 ([#130](https://github.com/gitkb/meta/issues/130)) ([894fc0b](https://github.com/gitkb/meta/commit/894fc0b81afb4903694f9855894108b2acfcf872))
* **loop:** a command that never started keeps exit_code -1; alias imp… ([065358b](https://github.com/gitkb/meta/commit/065358b44833483b43a4594bd09a399143dde440))
* **loop:** escape the .looprc text serde quotes in a parse error ([065358b](https://github.com/gitkb/meta/commit/065358b44833483b43a4594bd09a399143dde440))
* **loop:** one .looprc format; alias import keeps your settings; paths stay paths ([#145](https://github.com/gitkb/meta/issues/145)) ([065358b](https://github.com/gitkb/meta/commit/065358b44833483b43a4594bd09a399143dde440))
* meta context reads every repo on every call and says what git says ([#150](https://github.com/gitkb/meta/issues/150)) ([fb07953](https://github.com/gitkb/meta/commit/fb07953658a31ed077e118cf8a6e2a82aab18747))
* meta git update pulls --ff-only and recurses only with -r; the clone queue runs on rayon [[tasks/harmony-2986]] ([#151](https://github.com/gitkb/meta/issues/151)) ([184f165](https://github.com/gitkb/meta/commit/184f16579fa0bf8ab5ee049e1eabf21f4ad22caf))
* **meta_core:** remote_url reads every config git reads, the worktree… ([4c08ffc](https://github.com/gitkb/meta/commit/4c08ffca62a8204400234fce9452520efd6f8de5))
* meta_get_config scans a config's text line by line ([41a4673](https://github.com/gitkb/meta/commit/41a467352eaf16c7510e0c3c05b620ed4f12d38d))
* **meta_git_cli:** sync 396a60a ([#133](https://github.com/gitkb/meta/issues/133)) ([ffb1c22](https://github.com/gitkb/meta/commit/ffb1c227a939e236299a57b8c71d4100c193ce72))
* **meta_git_cli:** sync 79af652 ([#119](https://github.com/gitkb/meta/issues/119)) ([5a63ddc](https://github.com/gitkb/meta/commit/5a63ddc30f7c904fb79182d660b412d43fc14ead))
* **meta_git_cli:** sync bfa2aa8 ([#129](https://github.com/gitkb/meta/issues/129)) ([1f0ae60](https://github.com/gitkb/meta/commit/1f0ae6002ac9e761143eecad983b38ef8c32823e))
* **meta_git_cli:** sync c24c7a5 ([#127](https://github.com/gitkb/meta/issues/127)) ([954d9ae](https://github.com/gitkb/meta/commit/954d9aef52aeee4772f44932e5aefeb0fd87b538))
* **meta_mcp:** sync 16ea505 ([#106](https://github.com/gitkb/meta/issues/106)) ([d023c63](https://github.com/gitkb/meta/commit/d023c639c0e978c7b389064604299e4f0f6ae62f))
* **meta_mcp:** sync 4045c82 ([#108](https://github.com/gitkb/meta/issues/108)) ([9c034a8](https://github.com/gitkb/meta/commit/9c034a838c6c5b36b70ff285d7850c6f27ba6281))
* **meta_mcp:** sync 7a07f39 ([#124](https://github.com/gitkb/meta/issues/124)) ([3df134e](https://github.com/gitkb/meta/commit/3df134e56b0e20eb9bd6914c6061b733635b7dd4))
* **meta_mcp:** sync b91929b ([#112](https://github.com/gitkb/meta/issues/112)) ([d54e450](https://github.com/gitkb/meta/commit/d54e4506a66868a642f0773fd2fca7a0b667e7f1))
* **meta_mcp:** sync c586106 ([#117](https://github.com/gitkb/meta/issues/117)) ([3d5689b](https://github.com/gitkb/meta/commit/3d5689b396b07ef2543dc947354cb9ae7a8c141e))
* **meta_project_cli:** sync a1e85ae ([#118](https://github.com/gitkb/meta/issues/118)) ([17cd9a3](https://github.com/gitkb/meta/commit/17cd9a33cd41f5ee66e1de3558a31630c8c071ab))
* **meta_rust_cli:** sync 35eac6e ([#111](https://github.com/gitkb/meta/issues/111)) ([4dc5fa3](https://github.com/gitkb/meta/commit/4dc5fa318f3e4c4e87a85273005d5bf6b755ab05))
* **meta-mcp:** a cancellation acts only with params as an object ([404bc89](https://github.com/gitkb/meta/commit/404bc897b12d47122475bcbef63aeb26c67ababa))
* **meta-mcp:** a cancellation with an id cancels nothing ([404bc89](https://github.com/gitkb/meta/commit/404bc897b12d47122475bcbef63aeb26c67ababa))
* **meta-mcp:** act only on the project asked about, never outside the workspace ([#143](https://github.com/gitkb/meta/issues/143)) ([404bc89](https://github.com/gitkb/meta/commit/404bc897b12d47122475bcbef63aeb26c67ababa))
* **meta-mcp:** review round 1: whole-group cancel, rollback on cancel… ([404bc89](https://github.com/gitkb/meta/commit/404bc897b12d47122475bcbef63aeb26c67ababa))
* **meta-mcp:** review round 11: only a well-formed cancellation acts … ([404bc89](https://github.com/gitkb/meta/commit/404bc897b12d47122475bcbef63aeb26c67ababa))
* **meta-mcp:** review round 2: own repository only, raw config, bound… ([404bc89](https://github.com/gitkb/meta/commit/404bc897b12d47122475bcbef63aeb26c67ababa))
* **meta-mcp:** review round 3: keep git's reason, read no path back ([404bc89](https://github.com/gitkb/meta/commit/404bc897b12d47122475bcbef63aeb26c67ababa))
* **meta-mcp:** review round 4: stop the tool with the server, work-tr… ([404bc89](https://github.com/gitkb/meta/commit/404bc897b12d47122475bcbef63aeb26c67ababa))
* **meta-mcp:** review round 5: a stopping server starts nothing, fini… ([404bc89](https://github.com/gitkb/meta/commit/404bc897b12d47122475bcbef63aeb26c67ababa))
* **meta-mcp:** review round 6: a rollback outlives a group signal ([404bc89](https://github.com/gitkb/meta/commit/404bc897b12d47122475bcbef63aeb26c67ababa))
* **meta-mcp:** review round 8: end when output is gone, keep git's re… ([404bc89](https://github.com/gitkb/meta/commit/404bc897b12d47122475bcbef63aeb26c67ababa))
* **meta-mcp:** review round 9: a broken output stops the running tool ([404bc89](https://github.com/gitkb/meta/commit/404bc897b12d47122475bcbef63aeb26c67ababa))
* meta's --dry-run never changes anything; init claude installs once per workspace ([#141](https://github.com/gitkb/meta/issues/141)) ([8ec7c7c](https://github.com/gitkb/meta/commit/8ec7c7c88ed649aad8664c1da92c6ceb2ad2dbb6))
* no raw argument log; meta_get_config refuses any credential URL;… ([41a4673](https://github.com/gitkb/meta/commit/41a467352eaf16c7510e0c3c05b620ed4f12d38d))
* one lease per set, in one place per workspace; deleting a set ho… ([4b51030](https://github.com/gitkb/meta/commit/4b51030ab39196535e43d808b1d348e7e3cf6177))
* one owner for output, colour, warnings and exit codes; lints that make stray prints and panics impossible [[tasks/harmony-2996]] ([#153](https://github.com/gitkb/meta/issues/153)) ([bd0faab](https://github.com/gitkb/meta/commit/bd0faab233adf2c9888eec8ce90060927f66c051))
* one pass over git's output; helper rule for any scheme git hands… ([41a4673](https://github.com/gitkb/meta/commit/41a467352eaf16c7510e0c3c05b620ed4f12d38d))
* one repo named twice is refused only on two branches; ephemeral … ([07559ab](https://github.com/gitkb/meta/commit/07559ab52fe7c0429835db7dfc28effac7965c0a))
* one selection, clap renders the help, meta's options before the command ([#148](https://github.com/gitkb/meta/issues/148)) ([93a437c](https://github.com/gitkb/meta/commit/93a437c7f46a555d099a043d792da1ea5f801e09))
* only a real bare repository takes RepoStatus's bare path [[tasks… ([9d16e22](https://github.com/gitkb/meta/commit/9d16e2299e8fbee053f9fee508001e1849055a1f))
* only curl's schemes end a URL at a space in text ([41a4673](https://github.com/gitkb/meta/commit/41a467352eaf16c7510e0c3c05b620ed4f12d38d))
* only the called plugin runs; plugin installs can't escape the plugins folder ([#144](https://github.com/gitkb/meta/issues/144)) ([1e857c6](https://github.com/gitkb/meta/commit/1e857c6c06abdb4de0c151c4d5e111d0e384d64e))
* places stay places under -r; a root git can't use doesn't fail a… ([07559ab](https://github.com/gitkb/meta/commit/07559ab52fe7c0429835db7dfc28effac7965c0a))
* plugin fallback lists every repo of the workspace above cwd; scp… ([9d16e22](https://github.com/gitkb/meta/commit/9d16e2299e8fbee053f9fee508001e1849055a1f))
* plugin update skips manifest entries whose file is gone; exchang… ([4ead80f](https://github.com/gitkb/meta/commit/4ead80fbadcb3ec171a70d448936c0dca2b7f182))
* **plugin:** refuse only plugin entries that aren't files; read tar o… ([1e857c6](https://github.com/gitkb/meta/commit/1e857c6c06abdb4de0c151c4d5e111d0e384d64e))
* plugins and hooks exchange data without deadlocking; discovery a… ([4ead80f](https://github.com/gitkb/meta/commit/4ead80fbadcb3ec171a70d448936c0dca2b7f182))
* plugins and hooks exchange data without deadlocking; plugin discovery and `plugin update` report what they couldn't do [[tasks/harmony-2988]] ([#152](https://github.com/gitkb/meta/issues/152)) ([4ead80f](https://github.com/gitkb/meta/commit/4ead80fbadcb3ec171a70d448936c0dca2b7f182))
* **plugin:** stream tar twice, one word rule for lookup and listing, … ([1e857c6](https://github.com/gitkb/meta/commit/1e857c6c06abdb4de0c151c4d5e111d0e384d64e))
* previews and runs share worktree_start for meta's checks before … ([8ec7c7c](https://github.com/gitkb/meta/commit/8ec7c7c88ed649aad8664c1da92c6ceb2ad2dbb6))
* previews honour --json and exec filters; review fixes ([8ec7c7c](https://github.com/gitkb/meta/commit/8ec7c7c88ed649aad8664c1da92c6ceb2ad2dbb6))
* previews make the checks a run makes for commit --edit and --fro… ([8ec7c7c](https://github.com/gitkb/meta/commit/8ec7c7c88ed649aad8664c1da92c6ceb2ad2dbb6))
* previews run meta's own checks for prune --strict, core.bare and… ([8ec7c7c](https://github.com/gitkb/meta/commit/8ec7c7c88ed649aad8664c1da92c6ceb2ad2dbb6))
* prune and remove claim an abandoned set's lease and hold it thro… ([4b51030](https://github.com/gitkb/meta/commit/4b51030ab39196535e43d808b1d348e7e3cf6177))
* prune drops git's entry of a deleted worktree only when nothing … ([4b51030](https://github.com/gitkb/meta/commit/4b51030ab39196535e43d808b1d348e7e3cf6177))
* prune keeps entries with an operation under way; a lease is only… ([4b51030](https://github.com/gitkb/meta/commit/4b51030ab39196535e43d808b1d348e7e3cf6177))
* prune leaves git's entries alone; create and add take back a del… ([4b51030](https://github.com/gitkb/meta/commit/4b51030ab39196535e43d808b1d348e7e3cf6177))
* prune removes an ephemeral set only beside the lease its run left ([4b51030](https://github.com/gitkb/meta/commit/4b51030ab39196535e43d808b1d348e7e3cf6177))
* prune tests a lease's lock through a writable handle, as NFS needs ([4b51030](https://github.com/gitkb/meta/commit/4b51030ab39196535e43d808b1d348e7e3cf6177))
* read a bare repository's branch, commit and upstream in RepoStat… ([9d16e22](https://github.com/gitkb/meta/commit/9d16e2299e8fbee053f9fee508001e1849055a1f))
* read git's operation refs by name, as files or with show-ref --e… ([fb07953](https://github.com/gitkb/meta/commit/fb07953658a31ed077e118cf8a6e2a82aab18747))
* remove agent plugins ([#96](https://github.com/gitkb/meta/issues/96)) ([4d8adcc](https://github.com/gitkb/meta/commit/4d8adcc7e2b3a684e344180f6698a9a56c81bcc4))
* remove and prune see a set's lease wherever its creator kept it ([4b51030](https://github.com/gitkb/meta/commit/4b51030ab39196535e43d808b1d348e7e3cf6177))
* review round 1 (CodeRabbit): no tokens in registry warnings, com… ([bd0faab](https://github.com/gitkb/meta/commit/bd0faab233adf2c9888eec8ce90060927f66c051))
* review round 1: escaped names in warnings, usage hints kept, clo… ([bd0faab](https://github.com/gitkb/meta/commit/bd0faab233adf2c9888eec8ce90060927f66c051))
* review round 1: nested search, strict skip, stricter git dirs, p… ([4b0f44e](https://github.com/gitkb/meta/commit/4b0f44eccc0f3fac78cfae8b968d48aa8a71f864))
* review round 2: a command's own --json and a .looprc's json_outp… ([bd0faab](https://github.com/gitkb/meta/commit/bd0faab233adf2c9888eec8ce90060927f66c051))
* review round 2: duplicate providers fail under --strict, a GIT_C… ([bd0faab](https://github.com/gitkb/meta/commit/bd0faab233adf2c9888eec8ce90060927f66c051))
* review round 2: removal stays conservative, search errors and co… ([4b0f44e](https://github.com/gitkb/meta/commit/4b0f44eccc0f3fac78cfae8b968d48aa8a71f864))
* review round 3 (platform): loop's closed-pipe test without head … ([bd0faab](https://github.com/gitkb/meta/commit/bd0faab233adf2c9888eec8ce90060927f66c051))
* review round 3 (user-facing): dependency warnings only where a s… ([bd0faab](https://github.com/gitkb/meta/commit/bd0faab233adf2c9888eec8ce90060927f66c051))
* review round 3: every --verbose diagnostic on stderr; stdout flu… ([bd0faab](https://github.com/gitkb/meta/commit/bd0faab233adf2c9888eec8ce90060927f66c051))
* review round 3: unreadable HEAD as git sees it, each project sea… ([4b0f44e](https://github.com/gitkb/meta/commit/4b0f44eccc0f3fac78cfae8b968d48aa8a71f864))
* review round 4 nice-to-haves: multi-line errors in git update an… ([bd0faab](https://github.com/gitkb/meta/commit/bd0faab233adf2c9888eec8ce90060927f66c051))
* review round 4: git dirs git can enter, per-level search errors,… ([4b0f44e](https://github.com/gitkb/meta/commit/4b0f44eccc0f3fac78cfae8b968d48aa8a71f864))
* review round 5: an ambiguous depends_on names every other provid… ([bd0faab](https://github.com/gitkb/meta/commit/bd0faab233adf2c9888eec8ce90060927f66c051))
* review round 5: loop's help as clap prints it; one providers map… ([bd0faab](https://github.com/gitkb/meta/commit/bd0faab233adf2c9888eec8ce90060927f66c051))
* review round 5: the enterable-objects test asserts, HEAD read la… ([4b0f44e](https://github.com/gitkb/meta/commit/4b0f44eccc0f3fac78cfae8b968d48aa8a71f864))
* review round 5: worktree create checks the depends_on of every p… ([bd0faab](https://github.com/gitkb/meta/commit/bd0faab233adf2c9888eec8ce90060927f66c051))
* review round 6: real-path search ceiling, worktree add refuses n… ([4b0f44e](https://github.com/gitkb/meta/commit/4b0f44eccc0f3fac78cfae8b968d48aa8a71f864))
* round 1 review findings [[tasks/harmony-2947]] ([9d16e22](https://github.com/gitkb/meta/commit/9d16e2299e8fbee053f9fee508001e1849055a1f))
* round 2 review: help never runs the command, a blank --tag is an… ([93a437c](https://github.com/gitkb/meta/commit/93a437c7f46a555d099a043d792da1ea5f801e09))
* round 3 review and CodeRabbit: --primary's own error, skills and… ([93a437c](https://github.com/gitkb/meta/commit/93a437c7f46a555d099a043d792da1ea5f801e09))
* round 5 review: meta --help before a command prints meta's help … ([93a437c](https://github.com/gitkb/meta/commit/93a437c7f46a555d099a043d792da1ea5f801e09))
* round 6 review: meta --help lists plugins whatever name meta was… ([93a437c](https://github.com/gitkb/meta/commit/93a437c7f46a555d099a043d792da1ea5f801e09))
* run commands as argv, never through a shell; --include/--exclude name whole repos ([#113](https://github.com/gitkb/meta/issues/113)) ([47eeca3](https://github.com/gitkb/meta/commit/47eeca361965690b3d26c37802596b6fe37acffc))
* run commands as argv, never through a shell; whole-repo filters ([47eeca3](https://github.com/gitkb/meta/commit/47eeca361965690b3d26c37802596b6fe37acffc))
* say when a mismatch differs only in hidden credentials; test pro… ([41a4673](https://github.com/gitkb/meta/commit/41a467352eaf16c7510e0c3c05b620ed4f12d38d))
* say why a plugin request can't be sent [[tasks/harmony-2947]] ([9d16e22](https://github.com/gitkb/meta/commit/9d16e2299e8fbee053f9fee508001e1849055a1f))
* show a core.bare checkout's refs in query, context and meta-mcp;… ([9d16e22](https://github.com/gitkb/meta/commit/9d16e2299e8fbee053f9fee508001e1849055a1f))
* **skills:** copy Claude skills from meta_cli and test the new layout ([586c985](https://github.com/gitkb/meta/commit/586c985347c863ea06777fbf6e20d1ce27011da6))
* **skills:** copy Claude skills from meta_cli and test the new layout ([#102](https://github.com/gitkb/meta/issues/102)) ([586c985](https://github.com/gitkb/meta/commit/586c985347c863ea06777fbf6e20d1ce27011da6))
* **skills:** make skills drops stale copies; sync from meta_cli ([586c985](https://github.com/gitkb/meta/commit/586c985347c863ea06777fbf6e20d1ce27011da6))
* **skills:** make skills stages copies before replacing them ([586c985](https://github.com/gitkb/meta/commit/586c985347c863ea06777fbf6e20d1ce27011da6))
* **skills:** make skills stops on a failed copy; document all six skills ([586c985](https://github.com/gitkb/meta/commit/586c985347c863ea06777fbf6e20d1ce27011da6))
* **skills:** sync skills from meta_cli ([586c985](https://github.com/gitkb/meta/commit/586c985347c863ea06777fbf6e20d1ce27011da6))
* **skills:** sync skills from meta_cli ([586c985](https://github.com/gitkb/meta/commit/586c985347c863ea06777fbf6e20d1ce27011da6))
* **skills:** sync skills from meta_cli ([586c985](https://github.com/gitkb/meta/commit/586c985347c863ea06777fbf6e20d1ce27011da6))
* **skills:** sync skills from meta_cli, bump the plugin version ([586c985](https://github.com/gitkb/meta/commit/586c985347c863ea06777fbf6e20d1ce27011da6))
* ssh's refusal, git's errors in worktree commands, and newlines i… ([41a4673](https://github.com/gitkb/meta/commit/41a467352eaf16c7510e0c3c05b620ed4f12d38d))
* stable .meta handling, workspace-scoped worktree cleanup, init keeps settings ([#128](https://github.com/gitkb/meta/issues/128)) ([3644067](https://github.com/gitkb/meta/commit/36440671a2a6b8d20a2733c9802681aa71c4f458))
* the lease locks a file of its own, so it can be read while held … ([4b51030](https://github.com/gitkb/meta/commit/4b51030ab39196535e43d808b1d348e7e3cf6177))
* the leftover-entry command is escaped for the terminal; document… ([4b51030](https://github.com/gitkb/meta/commit/4b51030ab39196535e43d808b1d348e7e3cf6177))
* the Windows colour check looks at stdout's console only [[tasks/… ([bd0faab](https://github.com/gitkb/meta/commit/bd0faab233adf2c9888eec8ce90060927f66c051))
* **update:** a file:///C:/ URL is a local path on Windows [[tasks/har… ([184f165](https://github.com/gitkb/meta/commit/184f16579fa0bf8ab5ee049e1eabf21f4ad22caf))
* **update:** a plain git worktree is no set; an empty clone has an up… ([184f165](https://github.com/gitkb/meta/commit/184f16579fa0bf8ab5ee049e1eabf21f4ad22caf))
* **update:** a remote that can't be reached stays a failure; only one… ([184f165](https://github.com/gitkb/meta/commit/184f16579fa0bf8ab5ee049e1eabf21f4ad22caf))
* **update:** an upstream not on the remote yet is nothing to pull fro… ([184f165](https://github.com/gitkb/meta/commit/184f16579fa0bf8ab5ee049e1eabf21f4ad22caf))
* **update:** list worktrees on git before 2.36; Windows-safe cycle fi… ([184f165](https://github.com/gitkb/meta/commit/184f16579fa0bf8ab5ee049e1eabf21f4ad22caf))
* **update:** say exactly what the unlisted-repo check doesn't read [[… ([184f165](https://github.com/gitkb/meta/commit/184f16579fa0bf8ab5ee049e1eabf21f4ad22caf))
* **update:** say what an unlisted repo with no commits yet holds [[ta… ([184f165](https://github.com/gitkb/meta/commit/184f16579fa0bf8ab5ee049e1eabf21f4ad22caf))
* **update:** say what failed as it is; fuller help [[tasks/harmony-29… ([184f165](https://github.com/gitkb/meta/commit/184f16579fa0bf8ab5ee049e1eabf21f4ad22caf))
* **update:** set up SSH for projects cloned into empty folders too [[… ([184f165](https://github.com/gitkb/meta/commit/184f16579fa0bf8ab5ee049e1eabf21f4ad22caf))
* **update:** set up SSH only for the remotes the run contacts; tests … ([184f165](https://github.com/gitkb/meta/commit/184f16579fa0bf8ab5ee049e1eabf21f4ad22caf))
* **update:** the remote alone says a branch has nothing to pull from … ([184f165](https://github.com/gitkb/meta/commit/184f16579fa0bf8ab5ee049e1eabf21f4ad22caf))
* validate .meta config, recover store locks, escape untrusted output ([#114](https://github.com/gitkb/meta/issues/114)) ([7cac339](https://github.com/gitkb/meta/commit/7cac339f5fc77e5c73d34b2aeeb4821bfa99dfeb))
* with --json, stdout is one JSON document, a plugin's text includ… ([bd0faab](https://github.com/gitkb/meta/commit/bd0faab233adf2c9888eec8ce90060927f66c051))
* worktree list uses the request's directory; round 2 tests and cl… ([9d16e22](https://github.com/gitkb/meta/commit/9d16e2299e8fbee053f9fee508001e1849055a1f))
* worktree status reads a member's refs when its status can't be r… ([9d16e22](https://github.com/gitkb/meta/commit/9d16e2299e8fbee053f9fee508001e1849055a1f))
* **worktree:** a create rolls back even when it could not write a record ([6bc56f7](https://github.com/gitkb/meta/commit/6bc56f75b89f2095dcd8887918303fc671243ca9))
* **worktree:** a hook's stderr shows, its stdout doesn't; a hook kill… ([07559ab](https://github.com/gitkb/meta/commit/07559ab52fe7c0429835db7dfc28effac7965c0a))
* **worktree:** prune checks a TTL when a set proves not orphaned; rep… ([6bc56f7](https://github.com/gitkb/meta/commit/6bc56f75b89f2095dcd8887918303fc671243ca9))
* worktrees resolve one plan, read one record, and say what they did [[tasks/harmony-2973]] ([#149](https://github.com/gitkb/meta/issues/149)) ([07559ab](https://github.com/gitkb/meta/commit/07559ab52fe7c0429835db7dfc28effac7965c0a))
* **worktree:** trust only records in git's layout, judge prune by the… ([6bc56f7](https://github.com/gitkb/meta/commit/6bc56f75b89f2095dcd8887918303fc671243ca9))
* **worktree:** write records with git's permissions; prune handles mo… ([6bc56f7](https://github.com/gitkb/meta/commit/6bc56f75b89f2095dcd8887918303fc671243ca9))


### Documentation

* **plugins:** META_DATA_DIR moves the global plugins folder ([1eacb24](https://github.com/gitkb/meta/commit/1eacb24cceb18bd9f75a9d253b871876d3245aab))


### Code Refactoring

* meta reads only .meta.yaml ([#132](https://github.com/gitkb/meta/issues/132)) ([1eacb24](https://github.com/gitkb/meta/commit/1eacb24cceb18bd9f75a9d253b871876d3245aab))
* move the child crates into this repo [[tasks/harmony-2901]] ([#135](https://github.com/gitkb/meta/issues/135)) ([2a303c7](https://github.com/gitkb/meta/commit/2a303c77e12196b3382b2aeef3a282ca5d390eb1))
* one config load, one git wrapper, one parser per thing ([#139](https://github.com/gitkb/meta/issues/139)) ([9d16e22](https://github.com/gitkb/meta/commit/9d16e2299e8fbee053f9fee508001e1849055a1f))
* turn on the first clippy lint group and delete dead code ([#137](https://github.com/gitkb/meta/issues/137)) ([145f2f3](https://github.com/gitkb/meta/commit/145f2f3c681356fe1104a02c5738be9a68b913c0))

## [0.2.22](https://github.com/gitkb/meta/compare/v0.2.21...v0.2.22) (2026-05-19)


### Features

* **meta_project_cli:** sync 5016ebf ([#88](https://github.com/gitkb/meta/issues/88)) ([553dfa5](https://github.com/gitkb/meta/commit/553dfa553d1942aea8d7b24c64064e834a7e5043))

## [0.2.21](https://github.com/gitkb/meta/compare/v0.2.20...v0.2.21) (2026-05-19)


### Features

* adding meta worker agent ([#73](https://github.com/gitkb/meta/issues/73)) ([7f7bbdc](https://github.com/gitkb/meta/commit/7f7bbdc3a10077e939547611b2b1948cb861b8aa))


### Bug Fixes

* enforce semantic titles after meta agent removal ([#83](https://github.com/gitkb/meta/issues/83)) ([344e814](https://github.com/gitkb/meta/commit/344e814b84773c7cb83e9f3d8c4687dfec177dbc))
* install/uninstall all plugins in Makefile ([#72](https://github.com/gitkb/meta/issues/72)) ([58423e7](https://github.com/gitkb/meta/commit/58423e7952d6f0219a63c9a7cb450c68060405a1))
* **loop_cli:** sync ed04ea6 ([#75](https://github.com/gitkb/meta/issues/75)) ([24c6d6a](https://github.com/gitkb/meta/commit/24c6d6aa6da6d0b8fa901ab64dc1c5ec97b26057))
* **meta_git_cli:** sync ac7ab64 ([#84](https://github.com/gitkb/meta/issues/84)) ([8b6f3a0](https://github.com/gitkb/meta/commit/8b6f3a0605b9fd42661c275b71084ca2bda48426))
* **meta_mcp:** sync 4d40a4d ([#77](https://github.com/gitkb/meta/issues/77)) ([5243522](https://github.com/gitkb/meta/commit/52435221bbd62b46a03f9d62e4e7a192ab9a3ba1))
* **meta_project_cli:** sync 9b3e9c8 ([#85](https://github.com/gitkb/meta/issues/85)) ([4b69d5f](https://github.com/gitkb/meta/commit/4b69d5f8b2ee2d328fe9a9c1661c181c88707980))
* **meta_project_cli:** sync ea1efff ([#76](https://github.com/gitkb/meta/issues/76)) ([e1fce96](https://github.com/gitkb/meta/commit/e1fce96aeb8a02fd4420a4b3c993f25ce894c53f))
* **meta_rust_cli:** sync 1f62fc2 ([#86](https://github.com/gitkb/meta/issues/86)) ([5c44f54](https://github.com/gitkb/meta/commit/5c44f54e2a526c7fabceed3f8c2c27254b3fd349))
* stricter regex and better error logging for sync PR commit type ([#70](https://github.com/gitkb/meta/issues/70)) ([74fa296](https://github.com/gitkb/meta/commit/74fa2965ef848e4a5d779f4d8825875c4ae4f0da))
* updating harmony-labs GitHub org to gitkb ([#74](https://github.com/gitkb/meta/issues/74)) ([b5c1969](https://github.com/gitkb/meta/commit/b5c196912044cfe386608665b4e57d700f41af83))
* use PAT for release-please PR updates ([#87](https://github.com/gitkb/meta/issues/87)) ([6149471](https://github.com/gitkb/meta/commit/61494718af93fc4a909de75cb3c49628588db415))

## [0.2.20](https://github.com/gitkb/meta/compare/v0.2.19...v0.2.20) (2026-03-29)


### Bug Fixes

* propagate conventional commit type from child repo to sync PR ([#69](https://github.com/gitkb/meta/issues/69)) ([a0f1cb2](https://github.com/gitkb/meta/commit/a0f1cb21f03ce82e6edf4fa439a52f4849624ce2))
* use feat: prefix in sync PR titles for release-please ([#67](https://github.com/gitkb/meta/issues/67)) ([1518fc4](https://github.com/gitkb/meta/commit/1518fc43954d29caa3952acf46830be206c8d852))

## [0.2.19](https://github.com/gitkb/meta/compare/v0.2.18...v0.2.19) (2026-03-26)


### Bug Fixes

* add concurrency group to child-repo-sync workflow ([#57](https://github.com/gitkb/meta/issues/57)) ([d978541](https://github.com/gitkb/meta/commit/d9785417b30800d75da8e3f84e94ed65beab9f91))
* **loop_lib:** simplify notify-downstream workflow ([#8](https://github.com/gitkb/meta/issues/8)) ([#51](https://github.com/gitkb/meta/issues/51)) ([4d61e39](https://github.com/gitkb/meta/commit/4d61e39fd4344fb0688d229b0fb3fa8577c9f419))
* **meta_cli:** simplify notify-downstream workflow ([#19](https://github.com/gitkb/meta/issues/19)) ([#53](https://github.com/gitkb/meta/issues/53)) ([931eed0](https://github.com/gitkb/meta/commit/931eed0d42eaf964633666ec265d15d6136a0e10))
* **meta_git_lib:** simplify notify-downstream workflow ([#5](https://github.com/gitkb/meta/issues/5)) ([#52](https://github.com/gitkb/meta/issues/52)) ([ce05395](https://github.com/gitkb/meta/commit/ce053955ccf26a6d8fb5b2408b4a01d7a165331b))
* **meta_plugin_protocol:** simplify notify-downstream workflow ([#3](https://github.com/gitkb/meta/issues/3)) ([#50](https://github.com/gitkb/meta/issues/50)) ([c9d1481](https://github.com/gitkb/meta/commit/c9d1481ac2bbd54717f2d8601e844200560f6acc))
* **meta_plugin_protocol:** simplify notify-downstream workflow ([#3](https://github.com/gitkb/meta/issues/3)) ([#56](https://github.com/gitkb/meta/issues/56)) ([edf502a](https://github.com/gitkb/meta/commit/edf502aa44e6d85e1cc19f69b77ebbd1da10b97a))

## [0.2.18](https://github.com/gitkb/meta/compare/v0.2.17...v0.2.18) (2026-03-25)


### Features

* **meta_git_cli:** all SSH operations just work, remove meta git setup-ssh ([#20](https://github.com/gitkb/meta/issues/20)) ([#48](https://github.com/gitkb/meta/issues/48)) ([2b9b64a](https://github.com/gitkb/meta/commit/2b9b64aa73135c440083865d71f54a838a76ae97))

## [0.2.17](https://github.com/gitkb/meta/compare/v0.2.16...v0.2.17) (2026-03-10)


### Features

* **meta_project_cli:** add absolute root field to --json output ([#9](https://github.com/gitkb/meta/issues/9)) ([#46](https://github.com/gitkb/meta/issues/46)) ([c80637d](https://github.com/gitkb/meta/commit/c80637d22ba31c74c05a96b4127be68579685a13))

## [0.2.16](https://github.com/gitkb/meta/compare/v0.2.15...v0.2.16) (2026-03-05)


### Bug Fixes

* use PR-based sync instead of direct push to protected main ([#44](https://github.com/gitkb/meta/issues/44)) ([e65ce03](https://github.com/gitkb/meta/commit/e65ce03487a2598250eabb6fd2887f82b2da61ff))

## [0.2.15](https://github.com/gitkb/meta/compare/v0.2.14...v0.2.15) (2026-03-05)


### Bug Fixes

* use org-level PARENT_REPO_PAT for sync commits to protected main ([#42](https://github.com/gitkb/meta/issues/42)) ([dc5c190](https://github.com/gitkb/meta/commit/dc5c1901a28a36ea23504274957995a64b00b1fb))

## [0.2.14](https://github.com/gitkb/meta/compare/v0.2.13...v0.2.14) (2026-02-21)


### Bug Fixes

* **release:** configure git credentials for homebrew tap push ([#38](https://github.com/gitkb/meta/issues/38)) ([5d68815](https://github.com/gitkb/meta/commit/5d68815e4647e7bcfa2e1f859155d315857fd9be))

## [0.2.13](https://github.com/gitkb/meta/compare/v0.2.12...v0.2.13) (2026-02-21)


### Bug Fixes

* **ci:** always create sync commit for child repo updates ([#35](https://github.com/gitkb/meta/issues/35)) ([1f0e06c](https://github.com/gitkb/meta/commit/1f0e06cb8a943ce5562f0e631673c2eb157ab5c5))
* use PR workflow for homebrew tap updates ([#29](https://github.com/gitkb/meta/issues/29)) ([c5f7327](https://github.com/gitkb/meta/commit/c5f7327110b424041bd9ea0521c431763ed72f2f))
* use version-file for VERSION instead of generic extra-file ([#33](https://github.com/gitkb/meta/issues/33)) ([949db93](https://github.com/gitkb/meta/commit/949db93e00084427dd65953b0ac4d6c25af088b2))

## [0.2.12](https://github.com/gitkb/meta/compare/v0.2.11...v0.2.12) (2026-02-07)


### Bug Fixes

* **meta_cli:** pass --recursive and --depth options to plugin ([2f8680e](https://github.com/gitkb/meta/commit/2f8680ef68eb8492a2141c74e3db94edb6df6cad))

## [0.2.11](https://github.com/gitkb/meta/compare/v0.2.10...v0.2.11) (2026-02-07)


### Bug Fixes

* **meta_cli:** create VERSION file for standalone builds ([36b5515](https://github.com/gitkb/meta/commit/36b5515ed151c7d91dce40329b4acc352b45833e))
* **meta_cli:** set up workspace for auto-format workflow ([98da90e](https://github.com/gitkb/meta/commit/98da90ebc940db26be409d292456b0c2e8aca341))
* **meta_git_cli:** add workspace root for format job ([811376a](https://github.com/gitkb/meta/commit/811376af17c25fc29279f9657c89fcc067b14c45))
* **meta_git_cli:** use rustfmt directly to avoid dependency resolution ([43da75b](https://github.com/gitkb/meta/commit/43da75b72d64f17a8dbf547fe9da40b97044e7d9))
* **meta_git_cli:** use TTY input for remote URL mismatch prompt ([ea27fea](https://github.com/gitkb/meta/commit/ea27fea87af46ff7b2e1f91231e7f7750b5eaf4d))
* **tests:** set default branch for bare repos in git clone tests ([e42d10f](https://github.com/gitkb/meta/commit/e42d10f130ee3c59b9fa9ac9774e19866cbe5839))
* **tests:** use explicit branch for git clone tests ([4fd35aa](https://github.com/gitkb/meta/commit/4fd35aaf1c0887cfe528e1b6738a7497ef8d53c0))

## [0.2.10](https://github.com/gitkb/meta/compare/v0.2.9...v0.2.10) (2026-02-05)


### Bug Fixes

* **meta_git_cli:** create workspace root for workspace inheritance ([7213ffa](https://github.com/gitkb/meta/commit/7213ffa3a039d6eed09448c4dece9711531a510c))

## [0.2.9](https://github.com/gitkb/meta/compare/v0.2.8...v0.2.9) (2026-02-04)


### Bug Fixes

* sync all version sources to 0.2.8 ([13895f9](https://github.com/gitkb/meta/commit/13895f98e7aab3b168ac571d259b31810f78cf27))

## [0.2.8](https://github.com/gitkb/meta/compare/v0.2.7...v0.2.8) (2026-02-04)


### Features

* **meta_cli:** pass spawn_stagger_ms from ExecutionPlan to LoopConfig ([55d103e](https://github.com/gitkb/meta/commit/55d103e431d0b278b11b75b6ff5cb778715d97c7))


### Bug Fixes

* **meta_cli:** resolve clippy print_literal warnings ([491a845](https://github.com/gitkb/meta/commit/491a84592edf25381545d579eb97e04082885633))
* pull before writing VERSION file in sync workflow ([00de90b](https://github.com/gitkb/meta/commit/00de90b7635d9a7e9d290e0432fc5c7836e341ef))

## [0.2.7](https://github.com/gitkb/meta/compare/v0.2.6...v0.2.7) (2026-02-03)


### Bug Fixes

* improve VERSION sync comment formatting ([4357697](https://github.com/gitkb/meta/commit/435769711d39d09dd903b2e8b56063248f3c3e81))

## [0.2.6](https://github.com/gitkb/meta/compare/v0.2.5...v0.2.6) (2026-02-03)


### Bug Fixes

* pull before pushing VERSION sync to avoid race condition ([acfca5f](https://github.com/gitkb/meta/commit/acfca5ffc2cd7fb4cef865249506163bb62cc197))

## [0.2.5](https://github.com/gitkb/meta/compare/v0.2.4...v0.2.5) (2026-02-03)


### Bug Fixes

* ensure VERSION file ends with newline ([8f80d55](https://github.com/gitkb/meta/commit/8f80d5585433f7de2e0f66e0b6028ba446ad48af))
* trigger release build after VERSION sync completes ([90948fc](https://github.com/gitkb/meta/commit/90948fcfe3aacc779d37146822c7df8affe3f171))

## [0.2.4](https://github.com/gitkb/meta/compare/v0.2.3...v0.2.4) (2026-02-03)


### Features

* automate VERSION file sync after release creation ([9b02426](https://github.com/gitkb/meta/commit/9b02426897a1669c35f51eb2b67d967da0d4a912))


### Bug Fixes

* add checkout step before release-please ([b66d78a](https://github.com/gitkb/meta/commit/b66d78ad39cae89963dff70354a085d5a046af73))
* add root package to Cargo.toml for rust release type ([6e3bdfa](https://github.com/gitkb/meta/commit/6e3bdfac8a4c73215b03ea0f0969b373ac7e75db))
* create Cargo.toml dynamically in CI instead of tracking in repo ([0c85eb9](https://github.com/gitkb/meta/commit/0c85eb99572c6c7431f90740388a45349b8bf2eb))
* remove workspace Cargo.toml and use meta exec in CI ([91b39a0](https://github.com/gitkb/meta/commit/91b39a03e289a99d718fb239fed374f30a430805))
* restore workspace Cargo.toml and CI to working state ([4f539c9](https://github.com/gitkb/meta/commit/4f539c903e71851284ecc3b59a034aa2b00dad68))
* restore workspace Cargo.toml and revert to simple release type ([f989639](https://github.com/gitkb/meta/commit/f989639d534acf25ad6b262313c53ccecd5585eb))
* use explicit type for VERSION in extra-files ([c3e479a](https://github.com/gitkb/meta/commit/c3e479ac4f15a96d1aefc837a9730be5c3f27b1a))

## [0.2.3](https://github.com/gitkb/meta/compare/v0.2.2...v0.2.3) (2026-02-03)


### Bug Fixes

* **ci:** use PAT for sync commits to trigger downstream workflows ([50982c3](https://github.com/gitkb/meta/commit/50982c32eb8fa8608d85dfe11b086dd3d980556d))
* **meta_cli:** remove dead code wrapper function ([046a663](https://github.com/gitkb/meta/commit/046a663680c6f30702c19dc6d593e5ed7519414f))
* **meta_cli:** remove needless borrow in test ([a391dbf](https://github.com/gitkb/meta/commit/a391dbf7f8740d5a2d59a76a67e0b41dd36d8da7))
* **meta_cli:** support .meta.json and skip directories in config discovery ([0b0e71d](https://github.com/gitkb/meta/commit/0b0e71d47be1320613909311b41074ad9cea3cee))
* plugin discovery and config file resolution ([c20598b](https://github.com/gitkb/meta/commit/c20598bffd747c9b12b118a17599de4bfabf485b))
* **tests:** use .meta/plugins/ and .meta.json in bats tests ([81b2b4e](https://github.com/gitkb/meta/commit/81b2b4e26edce919b1a73f83f5f36eb121dcbd7b))

## [0.2.2](https://github.com/gitkb/meta/compare/v0.2.1...v0.2.2) (2026-02-02)


### Features

* add bats integration tests to CI ([8daf83d](https://github.com/gitkb/meta/commit/8daf83d6d67fc0613450559b5db74225696197b8))

## [0.2.1](https://github.com/gitkb/meta/compare/v0.2.0...v0.2.1) (2026-02-01)


### Features

* add handler for child repo update notifications ([059a338](https://github.com/gitkb/meta/commit/059a33841ef56dee8eedcdb79c3d5da86b598322))
* **meta_cli:** test dispatch ([bd5bb56](https://github.com/gitkb/meta/commit/bd5bb5609e2efc46929f2f3d19898fb526c0274f))


### Bug Fixes

* add workflow_dispatch for testing and improve payload handling ([4a9e55f](https://github.com/gitkb/meta/commit/4a9e55fc371c1d81196b37ca49ffac1876593028))
* configure release-please to update VERSION file ([3fd4d58](https://github.com/gitkb/meta/commit/3fd4d58a4fb5688917e8ee04ca15678333d57f2f))
* handle repository_dispatch in release workflow ([3c48892](https://github.com/gitkb/meta/commit/3c48892c1fda9ff0e398a476e902a4ec75c56882))
* **meta_cli:** repository dispatch test ([188ad78](https://github.com/gitkb/meta/commit/188ad78a37cc7dd0c34564d61717eea9be3dec53))

## [0.2.0](https://github.com/gitkb/meta/compare/v0.1.3...v0.2.0) (2026-02-01)


### Features

* switch to release-please for automated versioning ([acae04f](https://github.com/gitkb/meta/commit/acae04f2a6e3522b86f05e4d9f792395d6d12044))
* test automated versioning ([9ae4cef](https://github.com/gitkb/meta/commit/9ae4ceffedca4f51b1e3fd7c02f33779dc047322))


### Bug Fixes

* explicitly pass DEPLOY_KEY to external workflow ([4f59d4b](https://github.com/gitkb/meta/commit/4f59d4b80c2464aa3edf0da981f414dab655f3d6))
