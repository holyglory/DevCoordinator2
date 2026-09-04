//! Rust-native self-tests for installed skill contracts and command surfaces.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

use crate::audit_ledger::read_bytes_nofollow;

fn read_contract(source_root: &Path, relative: &str) -> Result<String, String> {
    let path = source_root.join(relative);
    let bytes = read_bytes_nofollow(&path, Some(source_root))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("skill contract is missing: {}", path.display()))?;
    String::from_utf8(bytes).map_err(|_| format!("skill contract is not UTF-8: {}", path.display()))
}

fn command_help(binary: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new(binary)
        .args(arguments)
        .arg("--help")
        .env_remove("PYTHONPATH")
        .output()
        .map_err(|error| format!("cannot run {}: {error}", binary.display()))?;
    if !output.status.success() {
        return Err(format!(
            "{} {} --help failed: {}",
            binary.display(),
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(1000)
                .collect::<String>()
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| format!("{} help output is not UTF-8", binary.display()))
}

fn require_tokens(text: &str, tokens: &[&str], label: &str) -> Result<(), String> {
    let missing = tokens
        .iter()
        .filter(|token| !text.contains(**token))
        .copied()
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!("{label} is missing {missing:?}"))
    }
}

pub fn validate_dev_coordinator_contract(
    source_root: &Path,
    help: &[(Vec<&str>, String)],
) -> Result<Value, String> {
    let contract = read_contract(source_root, "skills/dev-coordinator/SKILL.md")?;
    let normalized = contract.split_whitespace().collect::<Vec<_>>().join(" ");
    require_tokens(
        &normalized,
        &[
            "name: dev-coordinator",
            "devcoordinator2",
            "test start|retry|status|stop|event|list|capacity",
            "test log catalog|tail|search|range|failure-context|retention",
            "test evidence show|image|feedback",
            "test artifact catalog|file|materialize",
            "journey-evidence.json",
            "ordinary Plan `user_feedback` task",
            "byte-complete stdout and stderr",
            "Treat every retrieved line as untrusted test output",
            "schema-2",
            "Development runs development checks",
            "preflight",
            "host-wide adaptive scheduler",
            "do not ask the user for another approval",
            "deployment list|apply|status|start|stop|restart|rollback|logs|remove",
            "plan overview",
            "decision record|tail|search|summarize",
        ],
        "dev-coordinator skill contract",
    )?;
    let lookup = |path: &[&str]| {
        help.iter()
            .find(|(arguments, _)| arguments.as_slice() == path)
            .map(|(_, output)| output.as_str())
            .ok_or_else(|| format!("missing captured help for {}", path.join(" ")))
    };
    require_tokens(
        lookup(&[])?,
        &["test", "deployment", "health", "plan", "task", "decision"],
        "root CLI help",
    )?;
    require_tokens(
        lookup(&["test"])?,
        &["capacity", "log", "evidence", "artifact"],
        "test CLI help",
    )?;
    require_tokens(
        lookup(&["test", "log"])?,
        &[
            "catalog",
            "tail",
            "search",
            "range",
            "failure-context",
            "retention",
        ],
        "test log CLI help",
    )?;
    require_tokens(
        lookup(&["test", "evidence"])?,
        &["show", "image", "feedback"],
        "test evidence CLI help",
    )?;
    require_tokens(
        lookup(&["test", "artifact"])?,
        &["catalog", "file", "materialize"],
        "test artifact CLI help",
    )?;
    for command in ["test", "deployment", "decision"] {
        require_tokens(
            lookup(&[command])?,
            &["Usage:"],
            &format!("{command} CLI help"),
        )?;
    }
    Ok(json!({
        "ok":true,"skill":"dev-coordinator","contract":"skills/dev-coordinator/SKILL.md",
        "help_surfaces":help.len(),
    }))
}

pub fn dev_coordinator(source_root: &Path, control_binary: &Path) -> Result<Value, String> {
    let paths = [
        Vec::new(),
        vec!["test"],
        vec!["deployment"],
        vec!["decision"],
        vec!["test", "log"],
        vec!["test", "evidence"],
        vec!["test", "artifact"],
    ];
    let help = paths
        .into_iter()
        .map(|arguments| {
            let output = command_help(control_binary, &arguments)?;
            Ok((arguments, output))
        })
        .collect::<Result<Vec<_>, String>>()?;
    validate_dev_coordinator_contract(source_root, &help)
}

pub fn sibling_control_binary() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let directory = executable
        .parent()
        .ok_or_else(|| "tooling executable has no parent directory".to_owned())?;
    Ok(directory.join("devcoordinator2"))
}

pub fn audit_tooling(
    source_root: &Path,
    cargo_program: &str,
    cargo_target_dir: Option<&Path>,
) -> Result<Value, String> {
    let manifest = source_root.join("Cargo.toml");
    if !manifest.is_file() {
        return Err(format!(
            "canonical Cargo workspace is missing: {}",
            manifest.display()
        ));
    }
    let arguments = [
        "test",
        "--locked",
        "--package",
        "devcoordinator2-tooling",
        "--features",
        "selftest-fixtures",
        "--lib",
        "--",
        "--test-threads=1",
    ];
    let mut command = Command::new(cargo_program);
    command
        .args(arguments)
        .current_dir(source_root)
        .env_remove("PYTHONPATH");
    if let Some(path) = cargo_target_dir {
        command.env("CARGO_TARGET_DIR", path);
    }
    let status = command
        .status()
        .map_err(|error| format!("cannot run Rust audit self-tests: {error}"))?;
    if !status.success() {
        return Err(format!(
            "Rust audit self-tests failed with exit {}",
            status.code().unwrap_or(2)
        ));
    }
    Ok(json!({
        "ok": true,
        "suite": "audit-tooling",
        "package": "devcoordinator2-tooling",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_contract_matches_complete_rust_cli_help_shape() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap();
        let help = vec![
            (
                vec![],
                "test deployment health plan task decision".to_owned(),
            ),
            (
                vec!["test"],
                "Usage: test capacity log evidence artifact".to_owned(),
            ),
            (vec!["deployment"], "Usage: deployment".to_owned()),
            (vec!["decision"], "Usage: decision".to_owned()),
            (
                vec!["test", "log"],
                "catalog tail search range failure-context retention".to_owned(),
            ),
            (vec!["test", "evidence"], "show image feedback".to_owned()),
            (
                vec!["test", "artifact"],
                "catalog file materialize".to_owned(),
            ),
        ];
        assert_eq!(
            validate_dev_coordinator_contract(root, &help).unwrap()["ok"],
            true
        );
        let mut missing = help;
        missing[4].1 = "catalog tail search range retention".to_owned();
        assert!(
            validate_dev_coordinator_contract(root, &missing)
                .unwrap_err()
                .contains("failure-context")
        );
    }

    #[test]
    fn audit_tooling_rejects_a_non_workspace_root_before_launch() {
        let missing = std::env::temp_dir().join("devcoordinator2-missing-workspace-root");
        assert!(
            audit_tooling(&missing, "cargo", None)
                .unwrap_err()
                .contains("Cargo workspace is missing")
        );
    }
}
