use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::fs;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

const DEFAULT_MAX_OUTPUT_BYTES: usize = 1024 * 1024;

/// The directory name we use for our git repo (instead of .git)
pub const GSD_DIR: &str = ".gsd";

/// Optional ignore file that users can create
pub const GSD_IGNORE_FILE: &str = ".gsdignore";

#[derive(Debug)]
#[allow(dead_code)]
pub struct GitCommandResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub truncated: bool,
}

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum GitError {
    #[error("git command failed: {message}")]
    CommandFailed { message: String },

    #[error("git is not available")]
    NotAvailable,

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("detached HEAD in {path}")]
    DetachedHead { path: PathBuf },
}

/// Run a git command (standard, not using our snapshot dir)
#[allow(dead_code)]
pub async fn run_git(
    cwd: &Path,
    args: &[&str],
    max_output_bytes: Option<usize>,
) -> Result<GitCommandResult, GitError> {
    run_git_with_options(cwd, args, max_output_bytes, false).await
}

/// Run a git command using our snapshot git directory (.git-snapshotd)
pub async fn run_snapshot_git(
    cwd: &Path,
    args: &[&str],
    max_output_bytes: Option<usize>,
) -> Result<GitCommandResult, GitError> {
    run_git_with_options(cwd, args, max_output_bytes, true).await
}

async fn run_git_with_options(
    cwd: &Path,
    args: &[&str],
    max_output_bytes: Option<usize>,
    use_snapshot_dir: bool,
) -> Result<GitCommandResult, GitError> {
    let max_bytes = max_output_bytes.unwrap_or(DEFAULT_MAX_OUTPUT_BYTES);

    let mut cmd = Command::new("git");

    if use_snapshot_dir {
        // Use our custom git directory, separate from any existing .git
        cmd.arg(format!("--git-dir={}", GSD_DIR));
        cmd.arg("--work-tree=.");
    }

    cmd.args(args);
    cmd.current_dir(cwd);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn()?;

    let mut stdout_handle = child.stdout.take().expect("stdout piped");
    let mut stderr_handle = child.stderr.take().expect("stderr piped");

    let mut stdout_buf = vec![0u8; max_bytes];
    let mut stderr_buf = Vec::new();

    let stdout_read = stdout_handle.read(&mut stdout_buf);
    let stderr_read = stderr_handle.read_to_end(&mut stderr_buf);

    let (stdout_len, _) = tokio::join!(stdout_read, stderr_read);
    let stdout_len = stdout_len?;

    let truncated = stdout_len >= max_bytes;
    stdout_buf.truncate(stdout_len);

    let status = child.wait().await?;

    Ok(GitCommandResult {
        stdout: String::from_utf8_lossy(&stdout_buf).to_string(),
        stderr: String::from_utf8_lossy(&stderr_buf).to_string(),
        exit_code: status.code().unwrap_or(-1),
        truncated,
    })
}

pub async fn is_git_available() -> bool {
    match Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
    {
        Ok(status) => status.success(),
        Err(_) => false,
    }
}

/// Check if a directory has our snapshot git directory (.git-snapshotd).
///
/// We use a separate git directory from .git, so we never conflict with
/// existing repositories. If .git-snapshotd exists, it's ours.
pub async fn check_repo_ownership(dir: &Path) -> Result<RepoOwnership, GitError> {
    let snapshot_git_path = dir.join(GSD_DIR);

    let exists = fs::try_exists(&snapshot_git_path).await.unwrap_or(false);
    if exists {
        Ok(RepoOwnership::Ours)
    } else {
        Ok(RepoOwnership::NoRepo)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoOwnership {
    /// No .gsd directory exists
    NoRepo,
    /// .gsd exists - it's ours
    Ours,
}

/// Reads .gsdignore if it exists and copies patterns to .gsd/info/exclude
async fn setup_gsd_excludes(dir: &Path) -> Result<(), GitError> {
    let gitignore_path = dir.join(".gitignore");
    let gsdignore_path = dir.join(GSD_IGNORE_FILE);

    // Read .gitignore if it exists
    let gitignore_patterns = match fs::read_to_string(&gitignore_path).await {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(GitError::Io(e)),
    };

    // Read .gsdignore if it exists
    let gsdignore_patterns = match fs::read_to_string(&gsdignore_path).await {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(GitError::Io(e)),
    };

    // Combine both
    let patterns = format!("{}\n{}", gitignore_patterns, gsdignore_patterns);

    if patterns.trim().is_empty() {
        return Ok(());
    }

    // Ensure .gsd/info directory exists
    let info_dir = dir.join(GSD_DIR).join("info");
    fs::create_dir_all(&info_dir).await?;

    // Write to .gsd/info/exclude
    let exclude_path = info_dir.join("exclude");
    let existing = match fs::read_to_string(&exclude_path).await {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(GitError::Io(e)),
    };

    // Parse existing patterns
    let known: std::collections::HashSet<&str> = existing
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();

    // Find new patterns from .gsdignore
    let to_add: Vec<&str> = patterns
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter(|pattern| !known.contains(pattern))
        .collect();

    if to_add.is_empty() {
        return Ok(());
    }

    let suffix = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };

    let next = format!(
        "{}{}# From {}\n{}\n",
        existing,
        suffix,
        GSD_IGNORE_FILE,
        to_add.join("\n")
    );
    fs::write(&exclude_path, next).await?;

    Ok(())
}

pub async fn ensure_gitignore(dir: &Path, patterns: &[String]) -> Result<bool, GitError> {
    if patterns.is_empty() {
        return Ok(false);
    }

    let gitignore_path = dir.join(".gitignore");
    let existing = match fs::read_to_string(&gitignore_path).await {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(GitError::Io(e)),
    };

    let known: std::collections::HashSet<&str> = existing
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect();

    let to_add: Vec<&str> = patterns
        .iter()
        .map(|s| s.as_str())
        .filter(|pattern| !known.contains(pattern))
        .collect();

    if to_add.is_empty() {
        return Ok(false);
    }

    let suffix = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };

    let next = format!("{}{}{}\n", existing, suffix, to_add.join("\n"));
    fs::write(&gitignore_path, next).await?;

    Ok(true)
}

async fn ensure_local_git_config(
    dir: &Path,
    author_name: &str,
    author_email: &str,
) -> Result<(), GitError> {
    run_snapshot_git(dir, &["config", "user.name", author_name], None).await?;
    run_snapshot_git(dir, &["config", "user.email", author_email], None).await?;
    run_snapshot_git(dir, &["config", "commit.gpgsign", "false"], None).await?;
    Ok(())
}

pub async fn ensure_repo_initialized(
    dir: &Path,
    author_name: &str,
    author_email: &str,
    ignore_patterns: &[String],
) -> Result<(), GitError> {
    // Create directory if it doesn't exist
    fs::create_dir_all(dir).await?;

    // Check if we already have a snapshot repo
    let ownership = check_repo_ownership(dir).await?;
    if ownership == RepoOwnership::Ours {
        // Already initialized by us, just ensure config and excludes
        ensure_local_git_config(dir, author_name, author_email).await?;
        ensure_gitignore(dir, ignore_patterns).await?;
        setup_gsd_excludes(dir).await?;
        return Ok(());
    }

    // Initialize new repo with custom git dir
    let init_result = run_snapshot_git(dir, &["init"], None).await?;
    if init_result.exit_code != 0 {
        return Err(GitError::CommandFailed {
            message: init_result.stderr.trim().to_string(),
        });
    }

    // Configure git
    ensure_local_git_config(dir, author_name, author_email).await?;

    // Set up gitignore - always include our own git directory
    let mut all_patterns = vec![format!("{}/", GSD_DIR)];
    all_patterns.extend(ignore_patterns.iter().cloned());
    ensure_gitignore(dir, &all_patterns).await?;

    // Set up .gsdignore -> .gsd/info/exclude
    setup_gsd_excludes(dir).await?;

    // Initial commit
    let add_result = run_snapshot_git(dir, &["add", "-A"], None).await?;
    if add_result.exit_code != 0 {
        return Err(GitError::CommandFailed {
            message: add_result.stderr.trim().to_string(),
        });
    }

    let commit_result = run_snapshot_git(
        dir,
        &["commit", "-m", "Initial commit", "--allow-empty"],
        None,
    )
    .await?;
    if commit_result.exit_code != 0 {
        return Err(GitError::CommandFailed {
            message: commit_result.stderr.trim().to_string(),
        });
    }

    Ok(())
}

pub async fn is_detached_head(dir: &Path) -> Result<bool, GitError> {
    let result = run_snapshot_git(dir, &["rev-parse", "--abbrev-ref", "HEAD"], None).await?;
    if result.exit_code != 0 {
        return Err(GitError::CommandFailed {
            message: result.stderr.trim().to_string(),
        });
    }
    Ok(result.stdout.trim() == "HEAD")
}

pub async fn list_changed_files(dir: &Path) -> Result<Vec<String>, GitError> {
    let result = run_snapshot_git(dir, &["status", "--porcelain", "-z"], None).await?;
    if result.exit_code != 0 {
        return Err(GitError::CommandFailed {
            message: result.stderr.trim().to_string(),
        });
    }

    let entries: Vec<&str> = result
        .stdout
        .split('\0')
        .filter(|s| !s.is_empty())
        .collect();
    let mut files = Vec::new();
    let mut i = 0;

    while i < entries.len() {
        let entry = entries[i];
        if entry.len() < 4 {
            i += 1;
            continue;
        }

        let status = &entry[..2];
        let path_value = &entry[3..];

        // Handle renames and copies which have an extra path entry
        if status.starts_with('R') || status.starts_with('C') {
            if let Some(next) = entries.get(i + 1) {
                files.push(next.to_string());
                i += 2;
                continue;
            }
        }

        files.push(path_value.to_string());
        i += 1;
    }

    // Deduplicate and sort
    files.sort();
    files.dedup();

    Ok(files)
}

pub async fn has_changes(dir: &Path) -> Result<bool, GitError> {
    let files = list_changed_files(dir).await?;
    Ok(!files.is_empty())
}

pub async fn commit_all(dir: &Path, message: &str) -> Result<(), GitError> {
    let add_result = run_snapshot_git(dir, &["add", "-A"], None).await?;
    if add_result.exit_code != 0 {
        return Err(GitError::CommandFailed {
            message: add_result.stderr.trim().to_string(),
        });
    }

    let commit_result = run_snapshot_git(dir, &["commit", "-m", message], None).await?;
    if commit_result.exit_code != 0 {
        return Err(GitError::CommandFailed {
            message: commit_result.stderr.trim().to_string(),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_is_git_available() {
        assert!(is_git_available().await);
    }

    #[tokio::test]
    async fn test_repo_initialization() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path();

        // Should start with no repo
        let ownership = check_repo_ownership(dir).await.unwrap();
        assert_eq!(ownership, RepoOwnership::NoRepo);

        // Initialize
        ensure_repo_initialized(dir, "Test", "test@test.com", &["*.tmp".to_string()])
            .await
            .unwrap();

        // Should now be ours
        let ownership = check_repo_ownership(dir).await.unwrap();
        assert_eq!(ownership, RepoOwnership::Ours);

        // .gsd directory should exist (not .git)
        assert!(dir.join(GSD_DIR).exists());
        assert!(!dir.join(".git").exists());

        // .gitignore should have pattern and .gsd/
        let gitignore = fs::read_to_string(dir.join(".gitignore")).await.unwrap();
        assert!(gitignore.contains("*.tmp"));
        assert!(gitignore.contains(".gsd/"));
    }

    #[tokio::test]
    async fn test_gsdignore_support() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path();

        // Create .gsdignore before init
        fs::write(dir.join(GSD_IGNORE_FILE), "*.log\nsecrets/\n")
            .await
            .unwrap();

        // Initialize
        ensure_repo_initialized(dir, "Test", "test@test.com", &[])
            .await
            .unwrap();

        // Check that .gsd/info/exclude has our patterns
        let exclude_path = dir.join(GSD_DIR).join("info").join("exclude");
        let exclude_content = fs::read_to_string(&exclude_path).await.unwrap();
        assert!(exclude_content.contains("*.log"));
        assert!(exclude_content.contains("secrets/"));
    }

    #[tokio::test]
    async fn test_coexists_with_regular_git() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path();

        // Initialize a regular git repo first
        run_git(dir, &["init"], None).await.unwrap();
        assert!(dir.join(".git").exists());

        // Now initialize our snapshot repo - should work alongside
        ensure_repo_initialized(dir, "Snapshot", "snapshot@local", &[])
            .await
            .unwrap();

        // Both should exist
        assert!(dir.join(".git").exists());
        assert!(dir.join(GSD_DIR).exists());

        // Our ownership check should say it's ours
        let ownership = check_repo_ownership(dir).await.unwrap();
        assert_eq!(ownership, RepoOwnership::Ours);
    }

    #[tokio::test]
    async fn test_changes_and_commit() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path();

        ensure_repo_initialized(dir, "Test", "test@test.com", &[])
            .await
            .unwrap();

        // No changes initially
        assert!(!has_changes(dir).await.unwrap());

        // Create a file
        fs::write(dir.join("test.txt"), "hello").await.unwrap();

        // Should have changes now
        assert!(has_changes(dir).await.unwrap());

        let files = list_changed_files(dir).await.unwrap();
        assert!(files.contains(&"test.txt".to_string()));

        // Commit
        commit_all(dir, "Test commit").await.unwrap();

        // No changes after commit
        assert!(!has_changes(dir).await.unwrap());
    }

    #[tokio::test]
    async fn test_detached_head() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path();

        ensure_repo_initialized(dir, "Test", "test@test.com", &[])
            .await
            .unwrap();

        assert!(!is_detached_head(dir).await.unwrap());

        // Use run_snapshot_git to checkout detached in our repo
        run_snapshot_git(dir, &["checkout", "--detach"], None)
            .await
            .unwrap();

        assert!(is_detached_head(dir).await.unwrap());
    }
}
