use std::path::{Path, PathBuf};
use std::process::Stdio;

use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

const DEFAULT_MAX_OUTPUT_BYTES: usize = 1024 * 1024;

/// The directory name we use for our git repo (instead of .git)
pub const GSD_DIR: &str = ".gsd";

/// Optional ignore file that users can create
pub const GSD_IGNORE_FILE: &str = ".gsdignore";

const ARCHIVE_HASH_HEX_LEN: usize = 12;
const MAX_ARCHIVE_NAME_BYTES: usize = 255;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotRepo {
    pub work_tree: PathBuf,
    pub git_dir: PathBuf,
}

impl SnapshotRepo {
    pub fn is_colocated(&self) -> bool {
        self.git_dir == self.work_tree.join(GSD_DIR)
    }
}

pub fn resolve_snapshot_repo(
    work_tree: &Path,
    archive_root: Option<&Path>,
) -> Result<SnapshotRepo, GitError> {
    let canonical_work_tree = canonical_or_absolute(work_tree)?;
    let git_dir = if let Some(root) = archive_root {
        root.join(snapshot_archive_name(&canonical_work_tree))
    } else {
        canonical_work_tree.join(GSD_DIR)
    };

    Ok(SnapshotRepo {
        work_tree: canonical_work_tree,
        git_dir,
    })
}

fn canonical_or_absolute(path: &Path) -> Result<PathBuf, GitError> {
    match std::fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && path.is_absolute() => {
            Ok(path.to_path_buf())
        }
        Err(e) => Err(GitError::Io(e)),
    }
}

pub fn snapshot_archive_name(canonical_work_tree: &Path) -> String {
    let path = canonical_work_tree.to_string_lossy();
    let hash = stable_path_hash(&path);
    let encoded_path = encode_path_for_archive_name(&path);
    let suffix = format!(".{hash}.git");
    let max_prefix_bytes = MAX_ARCHIVE_NAME_BYTES.saturating_sub(suffix.len());
    format!(
        "{}{}",
        truncate_utf8(&encoded_path, max_prefix_bytes),
        suffix
    )
}

fn stable_path_hash(path: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.as_bytes());
    let digest = hasher.finalize();
    let hex = format!("{digest:x}");
    hex[..ARCHIVE_HASH_HEX_LEN].to_string()
}

fn encode_path_for_archive_name(path: &str) -> String {
    let without_leading_separator = path
        .strip_prefix('/')
        .or_else(|| path.strip_prefix('\\'))
        .unwrap_or(path);
    let encoded = without_leading_separator.replace(['/', '\\', ':'], "-");
    let encoded = if encoded.is_empty() { "root" } else { &encoded };
    format!("--{encoded}--")
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }

    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

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
    run_git_with_options(cwd, args, max_output_bytes).await
}

/// Run a git command using our snapshot git directory (.gsd)
pub async fn run_snapshot_git(
    repo: &SnapshotRepo,
    args: &[&str],
    max_output_bytes: Option<usize>,
) -> Result<GitCommandResult, GitError> {
    run_snapshot_git_with_options(repo, args, max_output_bytes).await
}

async fn run_git_with_options(
    cwd: &Path,
    args: &[&str],
    max_output_bytes: Option<usize>,
) -> Result<GitCommandResult, GitError> {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(cwd);
    run_prepared_git_command(cmd, max_output_bytes).await
}

async fn run_snapshot_git_with_options(
    repo: &SnapshotRepo,
    args: &[&str],
    max_output_bytes: Option<usize>,
) -> Result<GitCommandResult, GitError> {
    let mut cmd = Command::new("git");
    cmd.arg("--git-dir")
        .arg(&repo.git_dir)
        .arg("--work-tree")
        .arg(&repo.work_tree)
        .args(args)
        .current_dir(&repo.work_tree);
    run_prepared_git_command(cmd, max_output_bytes).await
}

async fn run_prepared_git_command(
    mut cmd: Command,
    max_output_bytes: Option<usize>,
) -> Result<GitCommandResult, GitError> {
    let max_bytes = max_output_bytes.unwrap_or(DEFAULT_MAX_OUTPUT_BYTES);

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn()?;

    let stdout_handle = child.stdout.take().expect("stdout piped");
    let stderr_handle = child.stderr.take().expect("stderr piped");

    let stdout_read = read_with_cap(stdout_handle, max_bytes);
    let stderr_read = read_with_cap(stderr_handle, max_bytes);

    let (stdout_result, stderr_result) = tokio::join!(stdout_read, stderr_read);
    let (stdout_buf, stdout_truncated) = stdout_result?;
    let (stderr_buf, stderr_truncated) = stderr_result?;

    let truncated = stdout_truncated || stderr_truncated;

    let status = child.wait().await?;

    Ok(GitCommandResult {
        stdout: String::from_utf8_lossy(&stdout_buf).to_string(),
        stderr: String::from_utf8_lossy(&stderr_buf).to_string(),
        exit_code: status.code().unwrap_or(-1),
        truncated,
    })
}

async fn read_with_cap<R: AsyncRead + Unpin>(
    mut reader: R,
    max_bytes: usize,
) -> Result<(Vec<u8>, bool), std::io::Error> {
    let mut buf = vec![0u8; 8192];
    let mut out = Vec::new();
    let mut truncated = false;

    loop {
        let read_len = reader.read(&mut buf).await?;
        if read_len == 0 {
            break;
        }

        if !truncated {
            let remaining = max_bytes.saturating_sub(out.len());
            if read_len <= remaining {
                out.extend_from_slice(&buf[..read_len]);
            } else {
                out.extend_from_slice(&buf[..remaining]);
                truncated = true;
            }
        }
    }

    Ok((out, truncated))
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

/// Check if a target has our snapshot git directory.
///
/// We use a separate git directory from .git, so we never conflict with
/// existing repositories.
pub async fn check_repo_ownership(repo: &SnapshotRepo) -> Result<RepoOwnership, GitError> {
    let exists = fs::try_exists(&repo.git_dir).await.unwrap_or(false);
    if exists {
        Ok(RepoOwnership::Ours)
    } else {
        Ok(RepoOwnership::NoRepo)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoOwnership {
    /// No snapshot git directory exists
    NoRepo,
    /// Snapshot git directory exists
    Ours,
}

/// Sync .gitignore + .gsdignore + configured patterns into the snapshot exclude file.
///
/// This file is generated state for the snapshot repository and is rewritten
/// on each sync so ignore removals are applied immediately.
pub async fn sync_snapshot_excludes(
    repo: &SnapshotRepo,
    configured_patterns: &[String],
) -> Result<(), GitError> {
    let dir = &repo.work_tree;
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

    // Always reserve .gsd/ for target-local snapshot archives, even when
    // central storage is configured and the target .gitignore has no entry.
    let reserved_patterns = [format!("{}/", GSD_DIR)];

    // Keep gitignore semantics and ordering after GSD's reserved entries.
    let patterns: Vec<String> = reserved_patterns
        .iter()
        .map(String::as_str)
        .chain(gitignore_patterns.lines())
        .chain(gsdignore_patterns.lines())
        .chain(configured_patterns.iter().map(String::as_str))
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect();

    // Ensure the snapshot repository info directory exists
    let info_dir = repo.git_dir.join("info");
    fs::create_dir_all(&info_dir).await?;

    let exclude_path = info_dir.join("exclude");
    let next = if patterns.is_empty() {
        String::new()
    } else {
        format!(
            "# Generated by gsd from .gitignore and {}\n{}\n",
            GSD_IGNORE_FILE,
            patterns.join("\n")
        )
    };
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
    repo: &SnapshotRepo,
    author_name: &str,
    author_email: &str,
) -> Result<(), GitError> {
    let work_tree = repo.work_tree.to_string_lossy();
    run_snapshot_git(repo, &["config", "user.name", author_name], None).await?;
    run_snapshot_git(repo, &["config", "user.email", author_email], None).await?;
    run_snapshot_git(repo, &["config", "commit.gpgsign", "false"], None).await?;
    run_snapshot_git(repo, &["config", "core.worktree", work_tree.as_ref()], None).await?;
    Ok(())
}

pub async fn ensure_repo_initialized(
    target_dir: &Path,
    archive_root: Option<&Path>,
    author_name: &str,
    author_email: &str,
    ignore_patterns: &[String],
) -> Result<SnapshotRepo, GitError> {
    // Create directory if it doesn't exist
    fs::create_dir_all(target_dir).await?;
    let repo = resolve_snapshot_repo(target_dir, archive_root)?;

    if !repo.is_colocated() {
        create_archive_root(&repo.git_dir).await?;
    }

    // Check if we already have a snapshot repo
    let ownership = check_repo_ownership(&repo).await?;
    if ownership == RepoOwnership::Ours {
        // Already initialized by us, just ensure config and excludes
        ensure_local_git_config(&repo, author_name, author_email).await?;
        ensure_snapshot_gitignore(&repo, ignore_patterns).await?;
        sync_snapshot_excludes(&repo, ignore_patterns).await?;
        return Ok(repo);
    }

    // Initialize new repo with custom git dir
    let init_result = run_snapshot_git(&repo, &["init"], None).await?;
    if init_result.exit_code != 0 {
        return Err(GitError::CommandFailed {
            message: init_result.stderr.trim().to_string(),
        });
    }

    // Configure git
    ensure_local_git_config(&repo, author_name, author_email).await?;

    // Set up gitignore for colocated storage
    ensure_snapshot_gitignore(&repo, ignore_patterns).await?;

    // Set up .gsdignore/configured patterns -> snapshot info/exclude
    sync_snapshot_excludes(&repo, ignore_patterns).await?;

    // Initial commit
    let add_result = run_snapshot_git(&repo, &["add", "-A"], None).await?;
    if add_result.exit_code != 0 {
        return Err(GitError::CommandFailed {
            message: add_result.stderr.trim().to_string(),
        });
    }

    let commit_result = run_snapshot_git(
        &repo,
        &["commit", "-m", "Initial commit", "--allow-empty"],
        None,
    )
    .await?;
    if commit_result.exit_code != 0 {
        return Err(GitError::CommandFailed {
            message: commit_result.stderr.trim().to_string(),
        });
    }

    Ok(repo)
}

async fn create_archive_root(git_dir: &Path) -> Result<(), GitError> {
    let Some(root) = git_dir.parent() else {
        return Ok(());
    };

    let existed = fs::try_exists(root).await.unwrap_or(false);
    fs::create_dir_all(root).await?;
    if !existed {
        set_private_dir_permissions(root).await?;
    }
    Ok(())
}

#[cfg(unix)]
async fn set_private_dir_permissions(path: &Path) -> Result<(), GitError> {
    use std::os::unix::fs::PermissionsExt;

    let permissions = std::fs::Permissions::from_mode(0o700);
    fs::set_permissions(path, permissions).await?;
    Ok(())
}

#[cfg(not(unix))]
async fn set_private_dir_permissions(_path: &Path) -> Result<(), GitError> {
    Ok(())
}

async fn ensure_snapshot_gitignore(
    repo: &SnapshotRepo,
    ignore_patterns: &[String],
) -> Result<(), GitError> {
    if !repo.is_colocated() {
        return Ok(());
    }

    let mut all_patterns = vec![format!("{}/", GSD_DIR)];
    all_patterns.extend(ignore_patterns.iter().cloned());
    ensure_gitignore(&repo.work_tree, &all_patterns).await?;
    Ok(())
}

pub async fn is_detached_head(repo: &SnapshotRepo) -> Result<bool, GitError> {
    let result = run_snapshot_git(repo, &["rev-parse", "--abbrev-ref", "HEAD"], None).await?;
    if result.exit_code != 0 {
        return Err(GitError::CommandFailed {
            message: result.stderr.trim().to_string(),
        });
    }
    Ok(result.stdout.trim() == "HEAD")
}

pub async fn list_changed_files(repo: &SnapshotRepo) -> Result<Vec<String>, GitError> {
    let result = run_snapshot_git(repo, &["status", "--porcelain", "-z"], None).await?;
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

pub async fn has_changes(repo: &SnapshotRepo) -> Result<bool, GitError> {
    let files = list_changed_files(repo).await?;
    Ok(!files.is_empty())
}

pub async fn commit_all(repo: &SnapshotRepo, message: &str) -> Result<(), GitError> {
    let add_result = run_snapshot_git(repo, &["add", "-A"], None).await?;
    if add_result.exit_code != 0 {
        return Err(GitError::CommandFailed {
            message: add_result.stderr.trim().to_string(),
        });
    }

    let commit_result = run_snapshot_git(repo, &["commit", "-m", message], None).await?;
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
        let repo = resolve_snapshot_repo(dir, None).unwrap();

        // Should start with no repo
        let ownership = check_repo_ownership(&repo).await.unwrap();
        assert_eq!(ownership, RepoOwnership::NoRepo);

        // Initialize
        let repo =
            ensure_repo_initialized(dir, None, "Test", "test@test.com", &["*.tmp".to_string()])
                .await
                .unwrap();

        // Should now be ours
        let ownership = check_repo_ownership(&repo).await.unwrap();
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
    async fn test_central_repo_initialization() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path().join("target");
        let archive_root = temp.path().join("archives");

        let repo = ensure_repo_initialized(
            &dir,
            Some(&archive_root),
            "Test",
            "test@test.com",
            &["*.tmp".to_string()],
        )
        .await
        .unwrap();

        assert!(!dir.join(GSD_DIR).exists());
        assert!(!dir.join(".gitignore").exists());
        assert!(repo.git_dir.starts_with(&archive_root));
        assert!(repo.git_dir.exists());
        assert_eq!(
            repo.git_dir.file_name().unwrap().to_string_lossy(),
            snapshot_archive_name(&std::fs::canonicalize(&dir).unwrap())
        );

        let exclude = fs::read_to_string(repo.git_dir.join("info").join("exclude"))
            .await
            .unwrap();
        assert!(exclude.contains(".gsd/"));
        assert!(exclude.contains("*.tmp"));
    }

    #[tokio::test]
    async fn test_central_repo_ignores_existing_gsd_directory() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path().join("target");
        let archive_root = temp.path().join("archives");
        fs::create_dir_all(dir.join(GSD_DIR)).await.unwrap();
        fs::write(dir.join(GSD_DIR).join("old-history"), "old")
            .await
            .unwrap();
        fs::write(dir.join("tracked.txt"), "tracked").await.unwrap();

        let repo = ensure_repo_initialized(&dir, Some(&archive_root), "Test", "test@test.com", &[])
            .await
            .unwrap();

        let tracked = run_snapshot_git(&repo, &["ls-files", "-z"], None)
            .await
            .unwrap();
        assert!(tracked.stdout.contains("tracked.txt"));
        assert!(!tracked.stdout.contains(".gsd/old-history"));
    }

    #[test]
    fn test_snapshot_archive_name_is_readable_and_hashed() {
        let name = snapshot_archive_name(Path::new("/etc/systemd/system"));
        assert!(name.starts_with("--etc-systemd-system--."));
        assert!(name.ends_with(".git"));
        assert!(name.len() <= MAX_ARCHIVE_NAME_BYTES);
    }

    #[test]
    fn test_snapshot_archive_name_for_root_path() {
        let name = snapshot_archive_name(Path::new("/"));
        assert!(name.starts_with("--root--."));
        assert!(name.ends_with(".git"));
    }

    #[test]
    fn test_snapshot_archive_name_truncates_before_hash_suffix() {
        let long_path = format!("/{}", "long-segment/".repeat(40));
        let name = snapshot_archive_name(Path::new(&long_path));

        assert!(name.len() <= MAX_ARCHIVE_NAME_BYTES);
        assert!(name.ends_with(".git"));
        assert_eq!(name.matches(".git").count(), 1);
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
        let repo = ensure_repo_initialized(dir, None, "Test", "test@test.com", &[])
            .await
            .unwrap();

        // Check that .gsd/info/exclude has our patterns
        let exclude_path = repo.git_dir.join("info").join("exclude");
        let exclude_content = fs::read_to_string(&exclude_path).await.unwrap();
        assert!(exclude_content.contains("*.log"));
        assert!(exclude_content.contains("secrets/"));
    }

    #[tokio::test]
    async fn test_sync_snapshot_excludes_rewrites_removed_patterns() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path();

        // Initialize with a .gsdignore pattern.
        fs::write(dir.join(GSD_IGNORE_FILE), "*.log\n")
            .await
            .unwrap();
        let repo = ensure_repo_initialized(dir, None, "Test", "test@test.com", &[])
            .await
            .unwrap();

        let exclude_path = repo.git_dir.join("info").join("exclude");
        let first = fs::read_to_string(&exclude_path).await.unwrap();
        assert!(first.contains("*.log"));

        // Remove the pattern and resync.
        fs::write(dir.join(GSD_IGNORE_FILE), "").await.unwrap();
        sync_snapshot_excludes(&repo, &[]).await.unwrap();

        let second = fs::read_to_string(&exclude_path).await.unwrap();
        assert!(!second.contains("*.log"));
    }

    #[tokio::test]
    async fn test_coexists_with_regular_git() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path();

        // Initialize a regular git repo first
        run_git(dir, &["init"], None).await.unwrap();
        assert!(dir.join(".git").exists());

        // Now initialize our snapshot repo - should work alongside
        let repo = ensure_repo_initialized(dir, None, "Snapshot", "snapshot@local", &[])
            .await
            .unwrap();

        // Both should exist
        assert!(dir.join(".git").exists());
        assert!(dir.join(GSD_DIR).exists());

        // Our ownership check should say it's ours
        let ownership = check_repo_ownership(&repo).await.unwrap();
        assert_eq!(ownership, RepoOwnership::Ours);
    }

    #[tokio::test]
    async fn test_changes_and_commit() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path();

        let repo = ensure_repo_initialized(dir, None, "Test", "test@test.com", &[])
            .await
            .unwrap();

        // No changes initially
        assert!(!has_changes(&repo).await.unwrap());

        // Create a file
        fs::write(dir.join("test.txt"), "hello").await.unwrap();

        // Should have changes now
        assert!(has_changes(&repo).await.unwrap());

        let files = list_changed_files(&repo).await.unwrap();
        assert!(files.contains(&"test.txt".to_string()));

        // Commit
        commit_all(&repo, "Test commit").await.unwrap();

        // No changes after commit
        assert!(!has_changes(&repo).await.unwrap());
    }

    #[tokio::test]
    async fn test_detached_head() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path();

        let repo = ensure_repo_initialized(dir, None, "Test", "test@test.com", &[])
            .await
            .unwrap();

        assert!(!is_detached_head(&repo).await.unwrap());

        // Use run_snapshot_git to checkout detached in our repo
        run_snapshot_git(&repo, &["checkout", "--detach"], None)
            .await
            .unwrap();

        assert!(is_detached_head(&repo).await.unwrap());
    }
}
