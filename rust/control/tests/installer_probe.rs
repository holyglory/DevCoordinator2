use std::process::Command;

#[test]
fn installer_probes_report_embedded_commit_and_strict_repository_configuration() {
    let binary = env!("CARGO_BIN_EXE_devcoordinator2");
    let commit = Command::new(binary)
        .arg("--source-commit")
        .output()
        .expect("source commit probe");
    assert!(commit.status.success());
    assert!(!String::from_utf8(commit.stdout).unwrap().trim().is_empty());

    let temporary = tempfile::tempdir().unwrap();
    std::fs::write(
        temporary.path().join(".devcoordinator.toml"),
        r#"schema = 2
[test]
default = "unit"
[test.unit]
[[test.unit.check]]
name = "main"
tier = "release"
command = ["true"]
"#,
    )
    .unwrap();
    let validated = Command::new(binary)
        .arg("--validate-repository-config")
        .arg(temporary.path())
        .output()
        .expect("configuration probe");
    assert!(
        validated.status.success(),
        "{}",
        String::from_utf8_lossy(&validated.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&validated.stdout).unwrap();
    assert_eq!(value["schema"], 2);
    assert_eq!(value["tests"], serde_json::json!(["unit"]));
    assert_eq!(value["deployments"], serde_json::json!([]));

    std::fs::write(
        temporary.path().join(".devcoordinator.toml"),
        "schema = 1\n[test.unit]\ncommand = ['true']\n",
    )
    .unwrap();
    let rejected = Command::new(binary)
        .arg("--validate-repository-config")
        .arg(temporary.path())
        .output()
        .expect("invalid configuration probe");
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("not ready for schema 2"));
}
