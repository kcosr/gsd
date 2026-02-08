# Changelog

## [Unreleased]

### Fixed

- Fixed `gsd preview` to honor `.gsdignore` allowlist negation patterns (`!`) reliably, including hidden paths and runs before `.gsd/` exists ([#1](https://github.com/kcosr/gsd/pull/1)).
- Synced `.gsdignore`/`.gitignore` to `.gsd/info/exclude` before snapshot commits and made exclude syncing authoritative so removed patterns are applied immediately ([#1](https://github.com/kcosr/gsd/pull/1)).

## [0.0.1] - 2026-01-21

Initial release.
