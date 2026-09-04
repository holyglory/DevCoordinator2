use std::process::Command;

#[test]
fn rust_wrapper_launches_the_retained_node_verifier_without_python() {
    let output = Command::new(env!("CARGO_BIN_EXE_devcoordinator2-tooling"))
        .args(["formal-ui", "verify", "--", "--help"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("formal_web_ui_verify.mjs"));
    assert!(stdout.contains("--journey-evidence-out"));
}
