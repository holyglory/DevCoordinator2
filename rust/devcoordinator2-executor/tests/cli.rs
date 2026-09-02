use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use devcoordinator2_executor_core::{
    LeafLogMetadata, LeafSelector, RunLogLease, RunLogMetadata, artifact_receipts,
    protocol::{DiagnosticExit, LeafStatus, LogStream, RunStatus},
};

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

fn bridge(
    repository: &Repository,
    command: &str,
    request: serde_json::Value,
) -> (i32, serde_json::Value) {
    let mut child = Command::new(binary())
        .args([command, "--worktree"])
        .arg(&repository.0)
        .args(["--request", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn log bridge");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(&serde_json::to_vec(&request).expect("request JSON"))
        .expect("write request");
    let output = child.wait_with_output().expect("bridge output");
    let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid bridge JSON: {error}; stderr={}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.code().expect("exit code"), value)
}

#[test]
fn source_digest_command_returns_only_bounded_receipt() {
    let repository = Repository::new();
    let output = Command::new(binary())
        .args(["source-digest", "--worktree"])
        .arg(&repository.0)
        .env("PATH", "/nonexistent")
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

#[test]
fn governed_run_fails_closed_without_capacity_broker() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../executor-protocol/tests/fixtures/backend-plan.json");
    let output = Command::new(binary())
        .arg("run")
        .arg(fixture)
        .env_remove("DEVCOORDINATOR_CAPACITY_SOCKET")
        .output()
        .expect("governed run");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("DEVCOORDINATOR_CAPACITY_SOCKET is required")
    );
}

#[test]
fn log_query_and_prune_commands_use_real_bounded_json_bridge() {
    let repository = Repository::new();
    let run_id = "t20260902T120000Z-123abc";
    let run_dir = repository
        .0
        .join(".devcoordinator/test/logs/runs")
        .join(run_id);
    fs::create_dir_all(&run_dir).expect("run directory");
    let lease = RunLogLease::acquire(&run_dir, run_id).expect("run lease");
    lease
        .publish_run_metadata(&RunLogMetadata {
            schema: 2,
            run_id: run_id.into(),
            test: "unit".into(),
            started_at_epoch_ms: 1,
            finished_at_epoch_ms: Some(2),
            status: RunStatus::Passed,
            complete: true,
        })
        .expect("run metadata");
    let selector = LeafSelector::check("main").expect("selector");
    for (stream, payload) in [
        (LogStream::Stdout, b"first\nFINAL-SENTINEL\n".as_slice()),
        (LogStream::Stderr, b"".as_slice()),
    ] {
        let mut writer = lease
            .create_stream(selector.clone(), stream)
            .expect("stream");
        writer.write_all(payload).expect("complete stream");
        writer.seal().expect("seal stream");
    }
    lease
        .publish_leaf_metadata(&LeafLogMetadata {
            schema: 2,
            selector,
            status: LeafStatus::Passed,
            exit: DiagnosticExit {
                code: Some(0),
                signal: None,
            },
            started_at_epoch_ms: 1,
            finished_at_epoch_ms: Some(2),
            process_started: true,
            complete: true,
            structured_evidence_formats: Vec::new(),
            structured_evidence_count: 0,
        })
        .expect("leaf metadata");
    drop(lease);

    let repository_id = "r0123456789abcdef";
    let (code, catalog) = bridge(
        &repository,
        "log-query",
        serde_json::json!({
            "schema": 2,
            "operation": "catalog",
            "repository_id": repository_id,
            "selector": {"run_id": run_id, "check": "main", "phase": "check"},
            "options": {"limit": 100, "max_age_seconds": 86400, "case_depth": 3}
        }),
    );
    assert_eq!(code, 0);
    assert_eq!(catalog["schema"], 2);
    assert_eq!(catalog["ok"], true);
    assert_eq!(
        catalog["result"]["entries"]
            .as_array()
            .expect("entries")
            .len(),
        2
    );
    assert!(catalog.to_string().len() < 65_536);

    let (code, tail) = bridge(
        &repository,
        "log-query",
        serde_json::json!({
            "schema": 2,
            "operation": "tail",
            "repository_id": repository_id,
            "selector": {
                "run_id": run_id, "check": "main", "phase": "check",
                "stream": "stdout"
            },
            "options": {"lines": 50, "max_bytes": 32768}
        }),
    );
    assert_eq!(code, 0);
    assert_eq!(tail["ok"], true);
    assert!(tail.to_string().contains("FINAL-SENTINEL"));
    assert!(tail.to_string().len() < 65_536);

    let (code, pruned) = bridge(
        &repository,
        "log-prune",
        serde_json::json!({
            "schema": 2,
            "repository_id": repository_id,
            "max_age_seconds": 1,
            "case_depth": 3
        }),
    );
    assert_eq!(code, 0);
    assert_eq!(pruned["ok"], true);
    assert_eq!(pruned["result"]["removed_leaf_folders"], 1);
    assert!(!run_dir.join("checks/main/check").exists());
}
