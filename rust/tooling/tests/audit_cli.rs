use std::path::Path;
use std::process::Command;

fn git(root: &Path, arguments: &[&str]) {
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(arguments)
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn rust_cli_builds_a_complete_audit_queue_without_python() {
    let directory = tempfile::tempdir().unwrap();
    let repo = directory.path().join("repo");
    let output = directory.path().join("audit");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::create_dir_all(repo.join("src/components")).unwrap();
    std::fs::write(
        repo.join("src/components/SaveButton.tsx"),
        "export function SaveButton() { return <button>Save</button>; }\n",
    )
    .unwrap();
    git(&repo, &["add", "src/components/SaveButton.tsx"]);
    let result = Command::new(env!("CARGO_BIN_EXE_devcoordinator2-tooling"))
        .args(["audit", "build-full-repo", "--repo"])
        .arg(&repo)
        .arg("--out")
        .arg(&output)
        .args(["--run-id", "run-cli-1234"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["source_file_count"], 1);
    assert_eq!(manifest["run_id"], "run-cli-1234");
    assert_eq!(
        manifest["coverage_invariants"]["all_source_files_queued_exactly_once"],
        true
    );
    let args = manifest["verifier_args"].as_array().unwrap();
    assert_eq!(args[1], "audit");
    assert_eq!(args[2], "verify-full-repo");
    assert!(
        args.iter()
            .all(|argument| !argument.as_str().unwrap_or("").contains("python"))
    );
    assert!(output.join("queue_complete.json").is_file());
    assert!(output.join("batch_001.md").is_file());
    assert!(output.join("journey_audit.md").is_file());
}
