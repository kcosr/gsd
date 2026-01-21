mod config;
mod git;
mod logging;
mod snapshot;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tracing::{error, info};

use config::{Config, ConfigError, ConfigPathKind, TargetConfig};
use logging::LoggingSettings;
use snapshot::SnapshotService;

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "Git snapshot daemon - automatic versioning of directories",
    long_about = "gsd monitors configured directories and automatically creates git commits \
                  at regular intervals. Uses a separate .gsd/ directory so it coexists with \
                  existing git repositories."
)]
struct Cli {
    /// Path to configuration file
    #[arg(long, global = true, env = "GSD_CONFIG")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the daemon (default)
    Run,

    /// Add a directory to monitoring (initializes .gsd and adds to config)
    Add {
        /// Directory path to add (defaults to current directory)
        path: Option<PathBuf>,

        /// Snapshot interval in seconds
        #[arg(short, long, default_value = "60")]
        interval: u64,

        /// Skip confirmation prompt
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Remove a directory from monitoring (removes from config and deletes .gsd)
    Remove {
        /// Directory path to remove (defaults to current directory)
        path: Option<PathBuf>,

        /// Skip confirmation prompt
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Enable monitoring for a directory
    Enable {
        /// Directory path to enable (defaults to current directory)
        path: Option<PathBuf>,
    },

    /// Disable monitoring for a directory
    Disable {
        /// Directory path to disable (defaults to current directory)
        path: Option<PathBuf>,
    },

    /// Take a snapshot of a directory
    Snapshot {
        /// Directory path to snapshot (defaults to current directory)
        path: Option<PathBuf>,

        /// Commit message (auto-generated if not provided)
        #[arg(short, long)]
        message: Option<String>,
    },

    /// Preview files that would be included in a snapshot
    Preview {
        /// Directory path to preview (defaults to current directory)
        path: Option<PathBuf>,
    },

    /// Check target directories
    Check,

    /// Configuration management
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Validate configuration file
    Validate,

    /// Initialize a new configuration file
    Init {
        /// Path to write config file (defaults to XDG config path)
        path: Option<PathBuf>,
    },

    /// Show current configuration file path
    Path,
}

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error(transparent)]
    Logging(#[from] logging::LoggingError),

    #[error(transparent)]
    Snapshot(#[from] snapshot::SnapshotError),

    #[error(transparent)]
    Git(#[from] git::GitError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match run(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("Error: {err}");
            ExitCode::from(1)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode, CliError> {
    match cli.command.unwrap_or(Command::Run) {
        Command::Run => run_daemon(cli.config.as_deref()),
        Command::Add {
            path,
            interval,
            yes,
        } => add_target(path, interval, yes, cli.config.as_deref()),
        Command::Remove { path, yes } => remove_target(path, yes, cli.config.as_deref()),
        Command::Enable { path } => set_target_enabled(path, true, cli.config.as_deref()),
        Command::Disable { path } => set_target_enabled(path, false, cli.config.as_deref()),
        Command::Snapshot { path, message } => take_snapshot(path, message),
        Command::Preview { path } => {
            let path = resolve_target_path(path)?;
            preview_path(&path, cli.config.as_deref())
        }
        Command::Check => check_targets(cli.config.as_deref()),
        Command::Config { command } => match command {
            ConfigCommand::Validate => validate_config(cli.config.as_deref()),
            ConfigCommand::Init { path } => init_config(path, cli.config.as_deref()),
            ConfigCommand::Path => show_config_path(cli.config.as_deref()),
        },
    }
}

/// Resolve target path, defaulting to current directory
fn resolve_target_path(path: Option<PathBuf>) -> Result<PathBuf, CliError> {
    let path = path.unwrap_or_else(|| PathBuf::from("."));
    path.canonicalize().map_err(|e| {
        CliError::Io(std::io::Error::new(
            e.kind(),
            format!("cannot access '{}': {}", path.display(), e),
        ))
    })
}

/// Prompt for confirmation
fn confirm(prompt: &str) -> bool {
    use std::io::{self, Write};
    print!("{} [y/N] ", prompt);
    io::stdout().flush().unwrap();
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

fn add_target(
    path: Option<PathBuf>,
    interval: u64,
    yes: bool,
    config_path: Option<&Path>,
) -> Result<ExitCode, CliError> {
    let path = resolve_target_path(path)?;

    // Load or create config
    let (mut config, config_file) = Config::load_or_create(config_path)?;

    // Check if already exists
    if config.find_target(&path).is_some() {
        eprintln!("Error: Target already exists: {}", path.display());
        return Ok(ExitCode::from(1));
    }

    // Confirm
    if !yes {
        println!("Add target: {}", path.display());
        println!("  interval: {}s", interval);
        println!("  config: {}", config_file.display());
        if !confirm("Proceed?") {
            println!("Cancelled.");
            return Ok(ExitCode::SUCCESS);
        }
    }

    // Initialize .gsd repo
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError::Io(std::io::Error::other(e)))?;

    runtime.block_on(async {
        git::ensure_repo_initialized(
            &path,
            &config.git.author_name,
            &config.git.author_email,
            &config.git.default_ignore_patterns,
        )
        .await
    })?;

    // Add to config
    let target = TargetConfig {
        path: path.clone(),
        interval_seconds: interval,
        ignore_patterns: Vec::new(),
        enabled: true,
    };
    config.add_target(target)?;
    config.save(&config_file)?;

    println!("Added: {}", path.display());
    Ok(ExitCode::SUCCESS)
}

fn remove_target(
    path: Option<PathBuf>,
    yes: bool,
    config_path: Option<&Path>,
) -> Result<ExitCode, CliError> {
    let path = resolve_target_path(path)?;

    // Load config
    let (mut config, config_file) = Config::load_or_create(config_path)?;

    // Check if exists
    if config.find_target(&path).is_none() {
        eprintln!("Error: Target not found: {}", path.display());
        return Ok(ExitCode::from(1));
    }

    let gsd_dir = path.join(git::GSD_DIR);
    let has_gsd = gsd_dir.exists();

    // Confirm
    if !yes {
        println!("Remove target: {}", path.display());
        if has_gsd {
            println!("  Will delete: {}", gsd_dir.display());
        }
        println!("  config: {}", config_file.display());
        if !confirm("Proceed?") {
            println!("Cancelled.");
            return Ok(ExitCode::SUCCESS);
        }
    }

    // Remove from config
    config.remove_target(&path)?;
    config.save(&config_file)?;

    // Delete .gsd directory
    if has_gsd {
        std::fs::remove_dir_all(&gsd_dir)?;
        println!("Deleted: {}", gsd_dir.display());
    }

    println!("Removed: {}", path.display());
    Ok(ExitCode::SUCCESS)
}

fn set_target_enabled(
    path: Option<PathBuf>,
    enabled: bool,
    config_path: Option<&Path>,
) -> Result<ExitCode, CliError> {
    let path = resolve_target_path(path)?;

    // Load config
    let (mut config, config_file) = Config::load_or_create(config_path)?;

    // Find and update target
    let target = config.find_target_mut(&path).ok_or_else(|| {
        CliError::Config(ConfigError::Invalid(format!(
            "target not found: {}",
            path.display()
        )))
    })?;

    if target.enabled == enabled {
        println!(
            "Target {} is already {}",
            path.display(),
            if enabled { "enabled" } else { "disabled" }
        );
        return Ok(ExitCode::SUCCESS);
    }

    target.enabled = enabled;
    config.save(&config_file)?;

    println!(
        "{}: {}",
        if enabled { "Enabled" } else { "Disabled" },
        path.display()
    );
    Ok(ExitCode::SUCCESS)
}

fn take_snapshot(path: Option<PathBuf>, message: Option<String>) -> Result<ExitCode, CliError> {
    let path = resolve_target_path(path)?;

    // Check if .gsd exists
    let gsd_dir = path.join(git::GSD_DIR);
    if !gsd_dir.exists() {
        eprintln!(
            "Error: No .gsd directory found in {}. Run 'gsd add' first.",
            path.display()
        );
        return Ok(ExitCode::from(1));
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError::Io(std::io::Error::other(e)))?;

    runtime.block_on(async {
        // Check for changes
        let has_changes = git::has_changes(&path).await?;
        if !has_changes {
            println!("No changes to snapshot.");
            return Ok(ExitCode::SUCCESS);
        }

        // Get changed files for auto-message
        let changed_files = git::list_changed_files(&path).await?;

        // Generate or use provided message
        let commit_message = message.unwrap_or_else(|| {
            let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            if changed_files.len() <= 3 {
                format!("Snapshot {}: {}", timestamp, changed_files.join(", "))
            } else {
                format!(
                    "Snapshot {}: {} and {} more",
                    timestamp,
                    changed_files[..2].join(", "),
                    changed_files.len() - 2
                )
            }
        });

        // Commit
        git::commit_all(&path, &commit_message).await?;

        println!("Snapshot created: {} file(s)", changed_files.len());
        for f in &changed_files {
            println!("  {}", f);
        }

        Ok(ExitCode::SUCCESS)
    })
}

fn show_config_path(config_path: Option<&Path>) -> Result<ExitCode, CliError> {
    let (path, kind) = Config::resolve_path(config_path);
    let exists = path.exists();

    println!("{}", path.display());
    println!(
        "  source: {}",
        match kind {
            ConfigPathKind::Explicit => "command line",
            ConfigPathKind::Env => "GSD_CONFIG environment variable",
            ConfigPathKind::Default => "default (XDG)",
        }
    );
    println!("  exists: {}", exists);

    Ok(ExitCode::SUCCESS)
}

fn run_daemon(config_path: Option<&std::path::Path>) -> Result<ExitCode, CliError> {
    let config = load_config(config_path)?;

    // Initialize logging
    let logging_settings = LoggingSettings::from_config(&config.logging)?;
    logging_settings.init_tracing()?;

    info!(
        version = env!("CARGO_PKG_VERSION"),
        targets = config.targets.len(),
        "Starting gsd"
    );

    // Build and run the runtime
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError::Io(std::io::Error::other(e)))?;

    runtime.block_on(async {
        let mut service = SnapshotService::new(config);

        service.initialize().await?;

        // Get shutdown sender before starting run loop
        let shutdown_tx = service.get_shutdown_sender();

        // Handle shutdown signals
        tokio::spawn(async move {
            if let Ok(()) = tokio::signal::ctrl_c().await {
                info!("Received SIGINT, initiating shutdown");
                let _ = shutdown_tx.send(()).await;
            }
        });

        service.run().await?;

        info!("gsd shutdown complete");
        Ok::<_, CliError>(())
    })?;

    Ok(ExitCode::SUCCESS)
}

fn validate_config(config_path: Option<&std::path::Path>) -> Result<ExitCode, CliError> {
    let config = load_config(config_path)?;

    println!("Configuration is valid");
    println!();
    println!("Schema version: {}", config.schema_version);
    println!("Targets: {}", config.targets.len());

    for target in &config.targets {
        println!(
            "  - {}: interval={}s, enabled={}",
            target.path.display(),
            target.interval_seconds,
            target.enabled
        );
    }

    Ok(ExitCode::SUCCESS)
}

fn init_config(path: Option<PathBuf>, config_path: Option<&Path>) -> Result<ExitCode, CliError> {
    // Use provided path, or resolve from config_path/default
    let path = path.unwrap_or_else(|| Config::resolve_path(config_path).0);

    if path.exists() {
        eprintln!(
            "Config file {} already exists; refusing to overwrite.",
            path.display()
        );
        return Ok(ExitCode::from(1));
    }

    // Create parent directories if needed
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }

    std::fs::write(&path, Config::default_config_toml())?;
    println!("Wrote default config to {}", path.display());

    Ok(ExitCode::SUCCESS)
}

fn check_targets(config_path: Option<&std::path::Path>) -> Result<ExitCode, CliError> {
    let config = load_config(config_path)?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError::Io(std::io::Error::other(e)))?;

    let mut has_issues = false;

    runtime.block_on(async {
        for target in &config.targets {
            let snapshot_dir = target.path.join(git::GSD_DIR);
            let has_snapshot_repo = snapshot_dir.exists();
            let has_regular_git = target.path.join(".git").exists();

            let status = match git::check_repo_ownership(&target.path).await {
                Ok(git::RepoOwnership::Ours) => "✓ Managed by gsd",
                Ok(git::RepoOwnership::NoRepo) => {
                    if target.path.exists() {
                        "✓ No .gsd repo (will initialize)"
                    } else {
                        "✓ Directory missing (will create)"
                    }
                }
                Err(e) => {
                    has_issues = true;
                    error!(path = %target.path.display(), error = %e, "Check failed");
                    "✗ Check failed"
                }
            };

            println!(
                "{}: {} - {}",
                if target.enabled {
                    "enabled "
                } else {
                    "disabled"
                },
                target.path.display(),
                status
            );
            if has_regular_git && !has_snapshot_repo {
                println!("  Note: Has .git (will coexist with .gsd)");
            }
        }
    });

    if has_issues {
        println!();
        println!("Some targets have issues.");
        Ok(ExitCode::from(1))
    } else {
        println!();
        println!("All targets OK");
        Ok(ExitCode::SUCCESS)
    }
}

/// Entry representing a file or directory in the preview
#[derive(Debug)]
struct PreviewEntry {
    path: PathBuf,
    is_dir: bool,
    size: u64,
    depth: usize,
}

/// Format bytes as human-readable size
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1}GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1}MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1}KB", bytes as f64 / KB as f64)
    } else {
        format!("{}B", bytes)
    }
}

fn preview_path(path: &Path, config_path: Option<&Path>) -> Result<ExitCode, CliError> {
    use ignore::overrides::OverrideBuilder;
    use ignore::WalkBuilder;
    use std::collections::HashMap;

    // Canonicalize the path
    let path = match path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: Cannot access path {}: {}", path.display(), e);
            return Ok(ExitCode::from(1));
        }
    };

    if !path.is_dir() {
        eprintln!("Error: {} is not a directory", path.display());
        return Ok(ExitCode::from(1));
    }

    // Try to load config (optional)
    let config = match Config::load_from_sources(config_path) {
        Ok(cfg) => Some(cfg),
        Err(ConfigError::Io { ref source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            println!("Note: No config file found, using default patterns only");
            println!();
            None
        }
        Err(e) => {
            eprintln!("Warning: Failed to load config: {}", e);
            eprintln!();
            None
        }
    };

    // Find matching target and collect patterns
    let mut ignore_patterns: Vec<String> = vec![
        // Always ignore .gsd and .git directories
        ".gsd/".to_string(),
        ".git/".to_string(),
    ];

    let target_match = config
        .as_ref()
        .and_then(|cfg| cfg.targets.iter().find(|t| t.path == path));

    if let Some(cfg) = &config {
        // Add global default patterns
        ignore_patterns.extend(cfg.git.default_ignore_patterns.clone());

        if let Some(target) = target_match {
            // Add target-specific patterns
            ignore_patterns.extend(target.ignore_patterns.clone());
        }
    }

    // Read .gsdignore if present
    let gsdignore_path = path.join(git::GSD_IGNORE_FILE);
    if gsdignore_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&gsdignore_path) {
            for line in content.lines() {
                let line = line.trim();
                if !line.is_empty() && !line.starts_with('#') {
                    ignore_patterns.push(line.to_string());
                }
            }
        }
    }

    // Build overrides for ignore patterns (negate them to exclude)
    let mut override_builder = OverrideBuilder::new(&path);
    for pattern in &ignore_patterns {
        // Prefix with ! to negate (exclude) these patterns
        let exclude_pattern = format!("!{}", pattern);
        if let Err(e) = override_builder.add(&exclude_pattern) {
            eprintln!("Warning: invalid pattern '{}': {}", pattern, e);
        }
    }
    let overrides = override_builder
        .build()
        .unwrap_or_else(|_| OverrideBuilder::new(&path).build().unwrap());

    // Build the walker - sort to get consistent directory traversal order
    let mut builder = WalkBuilder::new(&path);
    builder
        .hidden(false) // Don't skip hidden files by default
        .git_ignore(false) // Don't use .gitignore (we use .gsdignore)
        .git_global(false)
        .git_exclude(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .overrides(overrides);

    // First pass: collect all files and their sizes
    let mut file_paths: Vec<PathBuf> = Vec::new();
    let mut file_sizes: HashMap<PathBuf, u64> = HashMap::new();

    for entry in builder.build() {
        match entry {
            Ok(entry) => {
                let entry_path = entry.path();
                // Skip the root directory itself
                if entry_path == path {
                    continue;
                }
                // Only collect files
                if entry_path.is_file() {
                    if let Ok(relative) = entry_path.strip_prefix(&path) {
                        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                        file_paths.push(relative.to_path_buf());
                        file_sizes.insert(relative.to_path_buf(), size);
                    }
                }
            }
            Err(_) => continue,
        }
    }

    // Build directory sizes by summing file sizes
    let mut dir_sizes: HashMap<PathBuf, u64> = HashMap::new();
    for (file_path, size) in &file_sizes {
        // Add size to all parent directories
        let mut current = file_path.parent();
        while let Some(parent) = current {
            if parent.as_os_str().is_empty() {
                break;
            }
            *dir_sizes.entry(parent.to_path_buf()).or_insert(0) += size;
            current = parent.parent();
        }
    }

    // Collect directories that contain files (non-empty)
    let mut dirs_with_files: Vec<PathBuf> = dir_sizes.keys().cloned().collect();
    dirs_with_files.sort();

    // Build the display entries in traversal order
    let mut entries: Vec<PreviewEntry> = Vec::new();
    let mut shown_dirs: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

    // Sort files for consistent output
    file_paths.sort();

    for file_path in &file_paths {
        // First, ensure all parent directories are shown
        let mut ancestors: Vec<PathBuf> = Vec::new();
        let mut current = file_path.parent();
        while let Some(parent) = current {
            if parent.as_os_str().is_empty() {
                break;
            }
            ancestors.push(parent.to_path_buf());
            current = parent.parent();
        }
        ancestors.reverse();

        for ancestor in ancestors {
            if !shown_dirs.contains(&ancestor) {
                shown_dirs.insert(ancestor.clone());
                let depth = ancestor.components().count();
                let size = dir_sizes.get(&ancestor).copied().unwrap_or(0);
                entries.push(PreviewEntry {
                    path: ancestor,
                    is_dir: true,
                    size,
                    depth,
                });
            }
        }

        // Add the file
        let depth = file_path.components().count();
        let size = file_sizes.get(file_path).copied().unwrap_or(0);
        entries.push(PreviewEntry {
            path: file_path.clone(),
            is_dir: false,
            size,
            depth,
        });
    }

    // Calculate totals
    let total_files = file_paths.len();
    let total_dirs = shown_dirs.len();
    let total_size: u64 = file_sizes.values().sum();

    // Print results
    println!("Preview: {}", path.display());
    println!();

    if let Some(target) = target_match {
        println!("Target: CONFIGURED");
        println!("  interval: {}s", target.interval_seconds);
        println!("  enabled: {}", target.enabled);
    } else {
        println!("Target: NOT CONFIGURED (showing with default patterns only)");
    }
    println!();

    println!("Ignore patterns:");
    for pattern in &ignore_patterns {
        println!("  {}", pattern);
    }
    println!();

    // Print table header
    println!("{:<4}  {:>8}  PATH", "TYPE", "SIZE");

    // Print entries
    for entry in &entries {
        let type_str = if entry.is_dir { "dir" } else { "file" };
        let size_str = format_size(entry.size);
        let indent = "  ".repeat(entry.depth.saturating_sub(1));
        let name = entry
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| entry.path.to_string_lossy().to_string());
        let display_name = if entry.is_dir {
            format!("{}{}/", indent, name)
        } else {
            format!("{}{}", indent, name)
        };
        println!("{:<4}  {:>8}  {}", type_str, size_str, display_name);
    }

    // Print summary
    println!("{}", "─".repeat(40));
    println!(
        "Total: {} files, {} dirs, {}",
        total_files,
        total_dirs,
        format_size(total_size)
    );

    Ok(ExitCode::SUCCESS)
}

fn load_config(path: Option<&std::path::Path>) -> Result<Config, CliError> {
    let (resolved_path, kind) = Config::resolve_path(path);

    match Config::load_from_sources(path) {
        Ok(cfg) => Ok(cfg),
        Err(ConfigError::Io { ref source, .. })
            if source.kind() == std::io::ErrorKind::NotFound
                && matches!(kind, ConfigPathKind::Default) =>
        {
            eprintln!(
                "No configuration file found.\n\
                 Searched: {}\n\n\
                 To fix:\n  \
                 - Pass --config <path>, or\n  \
                 - Set GSD_CONFIG env var, or\n  \
                 - Run: git-snapshotd config init {}",
                resolved_path.display(),
                resolved_path.display(),
            );
            Err(CliError::Config(ConfigError::Invalid(
                "configuration file not found".into(),
            )))
        }
        Err(e) => Err(e.into()),
    }
}
