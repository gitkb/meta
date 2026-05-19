# Changelog

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
