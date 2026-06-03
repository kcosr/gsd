# Changelog

## [Unreleased]

### Changed

- Release automation now creates normal GitHub releases.
- Release version bumping is now handled inside the single release script, matching sibling Rust release tooling.
- Release script now supports `current` and explicit stable version arguments, with clean-main, origin/main sync, authenticated GitHub CLI, and free-tag preconditions.
- Documented release download/install guidance and Linux x86_64 plus macOS ARM64 archive packaging, with source builds moved to the development workflow.

### Fixed

- Hardened release version validation, local and remote tag checks, release recovery instructions, and release-script cleanup paths.
- Fixed the `Cargo.lock` package version for `gsd` to match `Cargo.toml`.
- Fixed `gsd preview` to honor `.gsdignore` allowlist negation patterns (`!`) reliably, including hidden paths and runs before `.gsd/` exists ([#1](https://github.com/kcosr/gsd/pull/1)).
- Synced `.gsdignore`/`.gitignore` to `.gsd/info/exclude` before snapshot commits and made exclude syncing authoritative so removed patterns are applied immediately ([#1](https://github.com/kcosr/gsd/pull/1)).

## [0.0.1] - 2026-01-21

Initial release.
