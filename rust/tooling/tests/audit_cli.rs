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

    let unit = manifest["coverage_units"][0]["unit_id"].as_str().unwrap();
    let digest = manifest["coverage_units"][0]["sha256"].as_str().unwrap();
    let report = format!(
        "## Run ID\nrun-cli-1234\n\n## Batch ID\nbatch_001\n\n## Batch Summary\nThe component renders the Save control.\n\n## File Coverage\n| File | Status | SHA-256 | Purpose |\n| --- | --- | --- | --- |\n| `{unit}` | CHECKED | `{digest}` | Renders the source-backed Save button component. |\n\n## Implementation Inventory\n| File/unit | Contract ID | Contract/responsibility | Entrypoints/source anchors | Implementation/data/side-effect trace | Failure/edge/permission/recovery trace | Verification evidence | Result |\n| --- | --- | --- | --- | --- | --- | --- | --- |\n| `{unit}` | `batch_001:C001` | Basis: interface-promise — `src/components/SaveButton.tsx#SaveButton` Discovery: parsed — `SaveButton@L1:C17` | `SaveButton@L1:C17` | pass — `SaveButton@L1:C17` renders the named control | not applicable — rendering this fixture has no recoverable dependency boundary | pass — evidence-type: source-only; `SaveButton@L1:C17`; invariance: identical properties render the same named control; source inspection verifies the returned element | PASS |\n\n## Interface Inventory\n| File | Surface | Visible text/control/message | Expected behavior path | Actual implementation notes |\n| --- | --- | --- | --- | --- |\n| `src/components/SaveButton.tsx` | control | Save | component return path renders the label | `SaveButton@L1:C17` returns the button element |\n\n## Findings\nNo findings.\n\n## No Finding Notes\n`{unit}` was checked and its render responsibility is present.\n\n## Open Questions\nNone.\n"
    );
    let report_path = output.join("reports/batch_001.md");
    std::fs::write(&report_path, report).unwrap();
    let verified = Command::new(env!("CARGO_BIN_EXE_devcoordinator2-tooling"))
        .args(["audit", "verify-full-repo", "--manifest"])
        .arg(output.join("manifest.json"))
        .arg("--reports")
        .arg(&report_path)
        .args(["--batch-id", "batch_001", "--json"])
        .output()
        .unwrap();
    assert!(
        verified.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&verified.stdout),
        String::from_utf8_lossy(&verified.stderr)
    );
    let verified: serde_json::Value = serde_json::from_slice(&verified.stdout).unwrap();
    assert_eq!(verified["ok"], true);
}
