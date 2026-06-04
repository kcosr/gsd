# Changelog

## [0.1.0] - 2026-06-04

### Added

- Added optional central snapshot archive storage with `[git].archive_root`, including config-aware CLI, daemon, preview, and git operations. ([#3](https://github.com/kcosr/gsd/pull/3))

### Changed

- Snapshot repositories now reserve `.gsd/` in generated excludes so existing colocated archives are not captured when central archive storage is enabled. ([#3](https://github.com/kcosr/gsd/pull/3))

## [0.0.2] - 2026-06-03

### Changed

- Release automation now creates normal GitHub releases. ([#2](https://github.com/kcosr/gsd/pull/2))
- Release version bumping is now handled inside the single release script, matching sibling Rust release tooling. ([#2](https://github.com/kcosr/gsd/pull/2))
- Release script now supports `current` and explicit stable version arguments, with clean-main, origin/main sync, authenticated GitHub CLI, and free-tag preconditions. ([#2](https://github.com/kcosr/gsd/pull/2))
- Documented release download/install guidance and Linux x86_64 plus macOS ARM64 archive packaging, with source builds moved to the development workflow. ([#2](https://github.com/kcosr/gsd/pull/2))

### Fixed

- Hardened release version validation, local and remote tag checks, release recovery instructions, and release-script cleanup paths. ([#2](https://github.com/kcosr/gsd/pull/2))
- Improved release-script diagnostics and changelog validation edge cases. ([#2](https://github.com/kcosr/gsd/pull/2))
- Fixed the `Cargo.lock` package version for `gsd` to match `Cargo.toml`. ([#2](https://github.com/kcosr/gsd/pull/2))
- Fixed `gsd preview` to honor `.gsdignore` allowlist negation patterns (`!`) reliably, including hidden paths and runs before `.gsd/` exists ([#1](https://github.com/kcosr/gsd/pull/1)).
- Synced `.gsdignore`/`.gitignore` to `.gsd/info/exclude` before snapshot commits and made exclude syncing authoritative so removed patterns are applied immediately ([#1](https://github.com/kcosr/gsd/pull/1)).

## [0.0.1] - 2026-01-21

Initial release.
