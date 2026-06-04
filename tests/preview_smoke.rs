use std::fs;
use std::process::Command;

use tempfile::TempDir;

fn command_env<'a>(command: &'a mut Command, root: &std::path::Path) -> &'a mut Command {
    command
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join(".config"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
}

#[test]
fn test_preview_respects_gitignore_and_gsd_exclude() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(root.join("keep.txt"), "ok").unwrap();
    fs::write(root.join("ignored.log"), "no").unwrap();
    fs::write(root.join("reinclude.log"), "yes").unwrap();
    fs::write(root.join("secrets.txt"), "secret").unwrap();

    fs::write(root.join(".gitignore"), "*.log\n!reinclude.log\n").unwrap();

    let gsd_info = root.join(".gsd").join("info");
    fs::create_dir_all(&gsd_info).unwrap();
    fs::write(gsd_info.join("exclude"), "secrets.txt\n!secrets.txt\n").unwrap();

    let mut preview = Command::new(env!("CARGO_BIN_EXE_gsd"));
    let output = command_env(preview.arg("preview").arg(root), root)
        .output()
        .unwrap();

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("keep.txt"), "stdout:\n{stdout}");
    assert!(stdout.contains("reinclude.log"), "stdout:\n{stdout}");
    assert!(stdout.contains("secrets.txt"), "stdout:\n{stdout}");
    assert!(!stdout.contains("ignored.log"), "stdout:\n{stdout}");
}

#[test]
fn test_preview_respects_gsdignore_allowlist_patterns() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::create_dir_all(root.join("test")).unwrap();
    fs::write(root.join("test").join("a.txt"), "ok").unwrap();
    fs::write(root.join("other.txt"), "no").unwrap();
    fs::write(root.join(".gsdignore"), "*\n!test/\n!test/**\n").unwrap();

    let mut preview = Command::new(env!("CARGO_BIN_EXE_gsd"));
    let output = command_env(preview.arg("preview").arg(root), root)
        .output()
        .unwrap();

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("a.txt"), "stdout:\n{stdout}");
    assert!(!stdout.contains("other.txt"), "stdout:\n{stdout}");
}

#[test]
fn test_preview_respects_gsdignore_allowlist_for_hidden_tree() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::create_dir_all(root.join(".codex").join("skills")).unwrap();
    fs::write(root.join(".codex").join("skills").join("note.md"), "ok").unwrap();
    fs::write(root.join("other.txt"), "no").unwrap();
    fs::write(
        root.join(".gsdignore"),
        "*\n!.codex/\n!.codex/skills/\n!.codex/skills/**\n",
    )
    .unwrap();

    let mut preview = Command::new(env!("CARGO_BIN_EXE_gsd"));
    let output = command_env(preview.arg("preview").arg(root), root)
        .output()
        .unwrap();

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("note.md"), "stdout:\n{stdout}");
    assert!(!stdout.contains("other.txt"), "stdout:\n{stdout}");
}

#[test]
fn test_add_snapshot_and_git_use_central_archive_root() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let target = root.join("project");
    let archive_root = root.join("archives");
    let config = root.join("config.toml");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("initial.txt"), "initial").unwrap();
    fs::write(
        &config,
        format!(
            r#"schema_version = "1"

[git]
author_name = "gsd"
author_email = "gsd@local"
archive_root = "{}"
default_ignore_patterns = ["*.db-wal", "*.db-shm", "*.db-journal"]
"#,
            archive_root.display()
        ),
    )
    .unwrap();

    let mut add = Command::new(env!("CARGO_BIN_EXE_gsd"));
    let add_output = command_env(
        add.arg("--config")
            .arg(&config)
            .arg("add")
            .arg("-y")
            .arg(&target),
        root,
    )
    .output()
    .unwrap();
    assert!(
        add_output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&add_output.stderr)
    );
    assert!(!target.join(".gsd").exists());
    assert!(!target.join(".gitignore").exists());

    let archives: Vec<_> = fs::read_dir(&archive_root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(archives.len(), 1);
    let archive = &archives[0];
    assert!(archive
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("--"));
    assert!(archive
        .file_name()
        .unwrap()
        .to_string_lossy()
        .ends_with(".git"));
    assert!(archive.join("info").join("exclude").exists());

    fs::write(target.join("changed.txt"), "changed").unwrap();

    let mut snapshot = Command::new(env!("CARGO_BIN_EXE_gsd"));
    let snapshot_output = command_env(
        snapshot
            .arg("--config")
            .arg(&config)
            .arg("snapshot")
            .arg(&target)
            .arg("-m")
            .arg("Manual central snapshot"),
        root,
    )
    .output()
    .unwrap();
    assert!(
        snapshot_output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&snapshot_output.stderr)
    );

    let mut git_log = Command::new(env!("CARGO_BIN_EXE_gsd"));
    let git_log_output = command_env(
        git_log
            .arg("--config")
            .arg(&config)
            .arg("git")
            .arg("-C")
            .arg(&target)
            .arg("log")
            .arg("--oneline")
            .arg("-1"),
        root,
    )
    .output()
    .unwrap();
    assert!(
        git_log_output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&git_log_output.stderr)
    );
    let stdout = String::from_utf8_lossy(&git_log_output.stdout);
    assert!(
        stdout.contains("Manual central snapshot"),
        "stdout:\n{stdout}"
    );
}

#[test]
fn test_preview_reads_central_snapshot_exclude() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let target = root.join("project");
    let archive_root = root.join("archives");
    let config = root.join("config.toml");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("keep.txt"), "ok").unwrap();
    fs::write(target.join("secret.txt"), "secret").unwrap();
    fs::write(
        &config,
        format!(
            r#"schema_version = "1"

[git]
author_name = "gsd"
author_email = "gsd@local"
archive_root = "{}"
default_ignore_patterns = ["secret.txt"]
"#,
            archive_root.display()
        ),
    )
    .unwrap();

    let mut add = Command::new(env!("CARGO_BIN_EXE_gsd"));
    let add_output = command_env(
        add.arg("--config")
            .arg(&config)
            .arg("add")
            .arg("-y")
            .arg(&target),
        root,
    )
    .output()
    .unwrap();
    assert!(
        add_output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&add_output.stderr)
    );

    let mut preview = Command::new(env!("CARGO_BIN_EXE_gsd"));
    let output = command_env(
        preview
            .arg("--config")
            .arg(&config)
            .arg("preview")
            .arg(&target),
        root,
    )
    .output()
    .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("keep.txt"), "stdout:\n{stdout}");
    assert!(!stdout.contains("secret.txt"), "stdout:\n{stdout}");
    assert!(
        stdout.contains("snapshot info/exclude"),
        "stdout:\n{stdout}"
    );
}
