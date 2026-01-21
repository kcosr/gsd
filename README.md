# gsd - Git Snapshot Daemon

Automatic git snapshots of monitored directories.

## Overview

gsd monitors configured directories and automatically creates git commits at regular intervals. It's designed for snapshotting files that are modified regularly by agents—skill documents, plan files, notes, and other working documents that benefit from continuous versioning.

gsd uses a separate `.gsd/` directory, so it coexists peacefully with existing git repositories.

## Features

- **Multiple targets**: Monitor multiple directories with individual settings
- **Separate git directory**: Uses `.gsd/` instead of `.git/`, so it coexists with existing repos
- **No conflicts**: Your project's `.git` folder is completely untouched
- **Custom excludes**: Create a `.gsdignore` file for target-specific excludes
- **Configurable intervals**: Set per-target commit intervals
- **Gitignore support**: Configure ignore patterns globally and per-target
- **Structured logging**: Uses tracing for structured, configurable logging

## Installation

```bash
cargo install --path .
```

## Usage

### Run the daemon

```bash
gsd --config /path/to/config.toml
```

Or using environment variable:

```bash
export GSD_CONFIG=/path/to/config.toml
gsd
```

### Commands

```bash
# Add current directory to monitoring (initializes .gsd, adds to config)
gsd add
gsd add /path/to/dir
gsd add -y                    # Skip confirmation
gsd add -i 300                # Set interval to 5 minutes

# Remove directory from monitoring (deletes .gsd, removes from config)
gsd remove
gsd remove /path/to/dir
gsd remove -y                 # Skip confirmation

# Enable/disable monitoring
gsd enable
gsd disable

# Take a manual snapshot
gsd snapshot
gsd snapshot -m "My message"

# Preview files that would be included in a snapshot
gsd preview
gsd preview /path/to/dir

# Run the daemon
gsd run

# Check target directories
gsd check

# Configuration management
gsd config path               # Show config file location
gsd config init               # Create default config at XDG path
gsd config validate           # Validate configuration
```

## Configuration

Configuration is stored at `~/.config/gsd/config.toml` by default (XDG). Override with `--config` or `GSD_CONFIG` environment variable.

The config file is auto-created when you run `gsd add` for the first time.

### Example config.toml

```toml
schema_version = "1"

[logging]
level = "info"
# directory = "/var/log/gsd"
console = true

[git]
author_name = "gsd"
author_email = "gsd@local"
default_ignore_patterns = ["*.db-wal", "*.db-shm", "*.db-journal"]

[[targets]]
path = "/home/user/.agent/skills"
interval_seconds = 60
ignore_patterns = ["*.tmp", ".DS_Store"]
enabled = true

[[targets]]
path = "/home/user/.agent/plans"
interval_seconds = 60
enabled = true
```

### Configuration options

#### Logging

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `level` | string | `"info"` | Log level (trace, debug, info, warn, error) |
| `directory` | string | none | Directory for log files (optional) |
| `console` | bool | `true` | Output to console |

#### Git

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `author_name` | string | `"gsd"` | Git commit author name |
| `author_email` | string | `"gsd@local"` | Git commit author email |
| `default_ignore_patterns` | array | `["*.db-wal", ...]` | Default gitignore patterns |

#### Targets

| Option | Type | Required | Default | Description |
|--------|------|----------|---------|-------------|
| `path` | string | yes | - | Absolute path to monitor (also serves as unique identifier) |
| `interval_seconds` | int | no | `60` | Commit interval in seconds |
| `ignore_patterns` | array | no | `[]` | Additional gitignore patterns |
| `enabled` | bool | no | `true` | Whether this target is active |

## How It Works

gsd uses a **separate git directory** (`.gsd/`) instead of the standard `.git/`. This means:

- **Coexistence**: Your existing git repositories are completely unaffected
- **No conflicts**: You can snapshot a directory that's already a git repo
- **Clean separation**: Snapshot history is independent from your project history

The `.gsd/` directory is automatically added to `.gitignore` so it won't show up as untracked in your regular git workflow.

## Custom Excludes with .gsdignore

Create a `.gsdignore` file in any target directory to exclude files from snapshots:

```
# .gsdignore - works like .gitignore
*.log
tmp/
.cache/
node_modules/
```

These patterns are copied to `.gsd/info/exclude`, keeping them separate from your project's `.gitignore`.

## Environment Variables

| Variable | Description |
|----------|-------------|
| `GSD_CONFIG` | Path to configuration file |
| `GSD_LOG_LEVEL` | Override log level from config |

## License

MIT
