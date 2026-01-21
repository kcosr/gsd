# Agent Onboarding (gsd)

Git snapshot daemon - automatic versioning of monitored directories using a separate `.gsd/` git directory.

## Start Here

- Read `README.md` for project overview and usage.
- Core code lives in `src/`.

## Development

- Run `cargo fmt` before committing
- Run `cargo clippy` and fix warnings
- Run `cargo test` for all changes
- Run `cargo build --release` to verify release build

## Testing

- Unit tests: inline in source files using `#[cfg(test)]` modules
- Integration tests: in `tests/` directory
- Run `cargo test` to execute all tests
- Run `cargo test <name>` to run specific tests

### Writing Tests

- Use descriptive test function names with underscores: `test_parser_handles_empty_input`
- Keep tests focused - one concept per test
- Tests must be deterministic and not depend on external services
- Use `#[ignore]` for slow or flaky tests that shouldn't run by default

### What to Test

- Public API functions and their edge cases
- Error handling paths (`Result` and `Option` unwrapping)
- Edge cases for parsing/validation logic

## Changelog

Location: `CHANGELOG.md` (root)

### Format

Use these sections under `## [Unreleased]`:
- `### Breaking Changes` - API changes requiring migration
- `### Added` - New features
- `### Changed` - Changes to existing functionality
- `### Fixed` - Bug fixes
- `### Removed` - Removed features

### Rules

- New entries ALWAYS go under `## [Unreleased]`
- Append to existing subsections (e.g., `### Fixed`), do not create duplicates
- NEVER modify already-released version sections
- Use inline PR links: `([#123](https://github.com/kcosr/gsd/pull/123))`

### Attribution

- Internal changes: `Fixed foo bar ([#123](https://github.com/kcosr/gsd/pull/123))`
- External contributions: `Added feature X ([#456](https://github.com/kcosr/gsd/pull/456) by [@user](https://github.com/user))`
