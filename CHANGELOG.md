# Changelog

## [0.0.1] - 2026-01-21

### Added
- Added rotating file logging with size and retention limits (`max_bytes`, `max_files`) ([#0](https://github.com/kcosr/gsd/pull/0))

### Changed
- Updated file watching backends to support Linux and macOS (inotify/fsevent) ([#0](https://github.com/kcosr/gsd/pull/0))

### Fixed
- Fixed potential deadlock during initial snapshot commits ([#0](https://github.com/kcosr/gsd/pull/0))
- Fixed git command output draining to avoid hangs and partial reads ([#0](https://github.com/kcosr/gsd/pull/0))
- Fixed preview ignore handling to respect gitignore semantics and `.gsd/info/exclude` ([#0](https://github.com/kcosr/gsd/pull/0))
- Fixed `.gsd/` ignore pattern not being re-applied for existing targets ([#0](https://github.com/kcosr/gsd/pull/0))
- Fixed `gsd add` accepting an interval of 0 seconds ([#0](https://github.com/kcosr/gsd/pull/0))

## [0.1.0] - 2026-01-20

Initial release.
