use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use devcoordinator2_executor_core::artifact_receipts;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Repository(PathBuf);

impl Repository {
    fn new() -> Self {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "dc2-executor-cli-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create repository");
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(&root)
                .status()
                .expect("git init")
                .success()
        );
        fs::write(root.join("source.txt"), b"source\n").expect("source");
        assert!(
            Command::new("git")
                .args(["add", "source.txt"])
                .current_dir(&root)
                .status()
                .expect("git add")
                .success()
        );
        Self(root)
    }
}

impl Drop for Repository {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("cleanup");
    }
}

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_devcoordinator2-executor")
}

fn json_stdout(output: std::process::Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("JSON stdout")
}

#[test]
fn source_digest_command_returns_only_bounded_receipt() {
    let repository = Repository::new();
    let output = Command::new(binary())
        .args(["source-digest", "--worktree"])
        .arg(&repository.0)
        .output()
        .expect("source-digest");
    let receipt = json_stdout(output);
    assert_eq!(receipt["schema"], 2);
    assert_eq!(receipt["sha256"].as_str().expect("sha256").len(), 64);
    assert_eq!(receipt.as_object().expect("object").len(), 2);
}

#[test]
fn receipts_match_accepts_file_and_detects_changed_artifact() {
    let repository = Repository::new();
    fs::write(repository.0.join("artifact.bin"), b"artifact").expect("artifact");
    let receipts = artifact_receipts(&repository.0, &["artifact.bin".into()]).expect("receipts");
    let receipt_path = repository.0.join("receipts.json");
    fs::write(
        &receipt_path,
        serde_json::to_vec(&receipts).expect("serialize"),
    )
    .expect("receipt file");

    let matched = Command::new(binary())
        .args(["receipts-match", "--worktree"])
        .arg(&repository.0)
        .arg("--receipts")
        .arg(&receipt_path)
        .output()
        .expect("receipts-match");
    assert_eq!(json_stdout(matched)["matches"], true);

    fs::write(repository.0.join("artifact.bin"), b"changed").expect("change");
    let changed = Command::new(binary())
        .args(["receipts-match", "--worktree"])
        .arg(&repository.0)
        .arg("--receipts")
        .arg(&receipt_path)
        .stdin(Stdio::null())
        .output()
        .expect("receipts-match changed");
    assert_eq!(changed.status.code(), Some(1));
    let receipt: serde_json::Value = serde_json::from_slice(&changed.stdout).expect("changed JSON");
    assert_eq!(receipt["matches"], false);
}

#[test]
fn worktree_path_must_be_absolute() {
    let output = Command::new(binary())
        .args(["source-digest", "--worktree", "."])
        .output()
        .expect("relative source-digest");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("must be absolute"));
}
