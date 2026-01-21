use std::fs;
use std::process::Command;

use tempfile::TempDir;

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

    let output = Command::new(env!("CARGO_BIN_EXE_gsd"))
        .arg("preview")
        .arg(root)
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join(".config"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("keep.txt"), "stdout:\n{stdout}");
    assert!(stdout.contains("reinclude.log"), "stdout:\n{stdout}");
    assert!(stdout.contains("secrets.txt"), "stdout:\n{stdout}");
    assert!(!stdout.contains("ignored.log"), "stdout:\n{stdout}");
}
