//! Fail-closed activation state machine for the one live Rust cutover.
//!
//! Concrete host access is an adapter so the sequence can be exhaustively
//! rehearsed. The database backup is restored only when integrity fails; an
//! ordinary Rust startup or acceptance failure keeps the unchanged schema-15
//! database and restores only units, links, socket, and the Python service.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CutoverReceipt {
    pub status: String,
    pub backup: String,
    pub database_restored: bool,
    pub checks: Vec<String>,
}

pub trait CutoverAdapter {
    type Drain;
    type InstallationSnapshot;

    fn close_admission(&mut self) -> Result<Self::Drain, String>;
    fn wait_for_quiescence(&mut self, drain: &Self::Drain) -> Result<(), String>;
    fn deployment_is_applying(&mut self) -> Result<bool, String>;
    fn backup_database(&mut self) -> Result<String, String>;
    fn capture_installation(&mut self) -> Result<Self::InstallationSnapshot, String>;
    fn fence_legacy_socket(&mut self) -> Result<(), String>;
    fn stop_legacy(&mut self) -> Result<(), String>;
    fn install_rust(&mut self) -> Result<(), String>;
    fn start_rust(&mut self) -> Result<(), String>;
    fn verify_live(&mut self) -> Result<Vec<String>, String>;
    fn commit_activation(&mut self, snapshot: &Self::InstallationSnapshot) -> Result<(), String>;
    fn stop_rust(&mut self) -> Result<(), String>;
    fn restore_installation(&mut self, snapshot: &Self::InstallationSnapshot)
    -> Result<(), String>;
    fn database_integrity_ok(&mut self) -> Result<bool, String>;
    fn restore_database(&mut self, backup: &str) -> Result<(), String>;
    fn restore_legacy_socket(&mut self) -> Result<(), String>;
    fn start_legacy(&mut self) -> Result<(), String>;
    fn reopen_admission(&mut self, drain: Self::Drain) -> Result<(), String>;
}

pub fn activate<A: CutoverAdapter>(adapter: &mut A) -> Result<CutoverReceipt, String> {
    let drain = adapter.close_admission()?;
    let outcome = activate_drained(adapter, &drain);
    let reopen = adapter.reopen_admission(drain);
    match (outcome, reopen) {
        (Ok(receipt), Ok(())) => Ok(receipt),
        (Ok(_), Err(error)) => Err(format!(
            "Rust activated but test admission could not reopen: {error}"
        )),
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(reopen)) => Err(format!(
            "{error}; test admission also could not reopen: {reopen}"
        )),
    }
}

fn activate_drained<A: CutoverAdapter>(
    adapter: &mut A,
    drain: &A::Drain,
) -> Result<CutoverReceipt, String> {
    adapter.fence_legacy_socket()?;
    if let Err(error) = adapter.wait_for_quiescence(drain) {
        return Err(combine(
            error,
            "restore legacy socket",
            adapter.restore_legacy_socket(),
        ));
    }
    match adapter.deployment_is_applying() {
        Ok(false) => {}
        Ok(true) => {
            return Err(combine(
                "cutover blocked while a deployment is applying".to_owned(),
                "restore legacy socket",
                adapter.restore_legacy_socket(),
            ));
        }
        Err(error) => {
            return Err(combine(
                error,
                "restore legacy socket",
                adapter.restore_legacy_socket(),
            ));
        }
    }
    let backup = match adapter.backup_database() {
        Ok(backup) => backup,
        Err(error) => {
            return Err(combine(
                error,
                "restore legacy socket",
                adapter.restore_legacy_socket(),
            ));
        }
    };
    let snapshot = match adapter.capture_installation() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return Err(combine(
                error,
                "restore legacy socket",
                adapter.restore_legacy_socket(),
            ));
        }
    };
    if let Err(error) = adapter.stop_legacy() {
        let restore_socket = adapter.restore_legacy_socket();
        return Err(combine(error, "restore legacy socket", restore_socket));
    }
    let activation = (|| {
        adapter.install_rust()?;
        adapter.start_rust()?;
        let checks = adapter.verify_live()?;
        adapter.commit_activation(&snapshot)?;
        Ok(checks)
    })();
    match activation {
        Ok(checks) => Ok(CutoverReceipt {
            status: "activated".to_owned(),
            backup,
            database_restored: false,
            checks,
        }),
        Err(error) => rollback(adapter, &snapshot, &backup, error),
    }
}

fn rollback<A: CutoverAdapter>(
    adapter: &mut A,
    snapshot: &A::InstallationSnapshot,
    backup: &str,
    activation_error: String,
) -> Result<CutoverReceipt, String> {
    let mut failures = Vec::new();
    if let Err(error) = adapter.stop_rust() {
        failures.push(format!("stop Rust: {error}"));
    }
    if let Err(error) = adapter.restore_installation(snapshot) {
        failures.push(format!("restore installation: {error}"));
    }
    let mut database_restored = false;
    match adapter.database_integrity_ok() {
        Ok(true) => {}
        Ok(false) => match adapter.restore_database(backup) {
            Ok(()) => database_restored = true,
            Err(error) => failures.push(format!("restore database: {error}")),
        },
        Err(error) => failures.push(format!("check database integrity: {error}")),
    }
    if let Err(error) = adapter.restore_legacy_socket() {
        failures.push(format!("restore legacy socket: {error}"));
    }
    if let Err(error) = adapter.start_legacy() {
        failures.push(format!("restart Python: {error}"));
    }
    let suffix = if failures.is_empty() {
        format!(
            "rollback restored the prior service{}",
            if database_restored {
                " and database backup"
            } else {
                " without replacing the intact database"
            }
        )
    } else {
        format!("rollback incomplete: {}", failures.join("; "))
    };
    Err(format!(
        "Rust activation failed: {activation_error}; {suffix}"
    ))
}

fn combine(primary: String, operation: &str, secondary: Result<(), String>) -> String {
    match secondary {
        Ok(()) => primary,
        Err(error) => format!("{primary}; {operation} failed: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct Fake {
        calls: Vec<&'static str>,
        applying: bool,
        activation: Result<Vec<String>, String>,
        integrity: Result<bool, String>,
        auxiliary_failures: VecDeque<&'static str>,
    }

    impl Fake {
        fn success() -> Self {
            Self {
                calls: Vec::new(),
                applying: false,
                activation: Ok(vec!["v2-cli".to_owned(), "restart-recovery".to_owned()]),
                integrity: Ok(true),
                auxiliary_failures: VecDeque::new(),
            }
        }

        fn called(&mut self, name: &'static str) -> Result<(), String> {
            self.calls.push(name);
            if self.auxiliary_failures.front() == Some(&name) {
                self.auxiliary_failures.pop_front();
                Err(format!("{name} failed"))
            } else {
                Ok(())
            }
        }
    }

    impl CutoverAdapter for Fake {
        type Drain = String;
        type InstallationSnapshot = String;

        fn close_admission(&mut self) -> Result<Self::Drain, String> {
            self.called("close_admission")?;
            Ok("drain".to_owned())
        }
        fn wait_for_quiescence(&mut self, _: &Self::Drain) -> Result<(), String> {
            self.called("wait_for_quiescence")
        }
        fn deployment_is_applying(&mut self) -> Result<bool, String> {
            self.called("deployment_is_applying")?;
            Ok(self.applying)
        }
        fn backup_database(&mut self) -> Result<String, String> {
            self.called("backup_database")?;
            Ok("backup.sqlite3".to_owned())
        }
        fn capture_installation(&mut self) -> Result<Self::InstallationSnapshot, String> {
            self.called("capture_installation")?;
            Ok("snapshot".to_owned())
        }
        fn fence_legacy_socket(&mut self) -> Result<(), String> {
            self.called("fence_legacy_socket")
        }
        fn stop_legacy(&mut self) -> Result<(), String> {
            self.called("stop_legacy")
        }
        fn install_rust(&mut self) -> Result<(), String> {
            self.called("install_rust")
        }
        fn start_rust(&mut self) -> Result<(), String> {
            self.called("start_rust")
        }
        fn verify_live(&mut self) -> Result<Vec<String>, String> {
            self.calls.push("verify_live");
            self.activation.clone()
        }
        fn commit_activation(&mut self, _: &Self::InstallationSnapshot) -> Result<(), String> {
            self.called("commit_activation")
        }
        fn stop_rust(&mut self) -> Result<(), String> {
            self.called("stop_rust")
        }
        fn restore_installation(&mut self, _: &Self::InstallationSnapshot) -> Result<(), String> {
            self.called("restore_installation")
        }
        fn database_integrity_ok(&mut self) -> Result<bool, String> {
            self.called("database_integrity_ok")?;
            self.integrity.clone()
        }
        fn restore_database(&mut self, _: &str) -> Result<(), String> {
            self.called("restore_database")
        }
        fn restore_legacy_socket(&mut self) -> Result<(), String> {
            self.called("restore_legacy_socket")
        }
        fn start_legacy(&mut self) -> Result<(), String> {
            self.called("start_legacy")
        }
        fn reopen_admission(&mut self, _: Self::Drain) -> Result<(), String> {
            self.called("reopen_admission")
        }
    }

    #[test]
    fn successful_cutover_orders_every_safety_gate_before_activation() {
        let mut fake = Fake::success();
        let receipt = activate(&mut fake).unwrap();
        assert_eq!(receipt.status, "activated");
        assert_eq!(receipt.checks, ["v2-cli", "restart-recovery"]);
        assert_eq!(
            fake.calls,
            [
                "close_admission",
                "fence_legacy_socket",
                "wait_for_quiescence",
                "deployment_is_applying",
                "backup_database",
                "capture_installation",
                "stop_legacy",
                "install_rust",
                "start_rust",
                "verify_live",
                "commit_activation",
                "reopen_admission",
            ]
        );
    }

    #[test]
    fn applying_deployment_blocks_before_backup_or_runtime_mutation() {
        let mut fake = Fake::success();
        fake.applying = true;
        assert!(
            activate(&mut fake)
                .unwrap_err()
                .contains("deployment is applying")
        );
        assert_eq!(
            fake.calls,
            [
                "close_admission",
                "fence_legacy_socket",
                "wait_for_quiescence",
                "deployment_is_applying",
                "restore_legacy_socket",
                "reopen_admission"
            ]
        );
    }

    #[test]
    fn ordinary_activation_failure_preserves_intact_schema_fifteen_data() {
        let mut fake = Fake::success();
        fake.activation = Err("acceptance failed".to_owned());
        let error = activate(&mut fake).unwrap_err();
        assert!(error.contains("without replacing the intact database"));
        assert!(!fake.calls.contains(&"restore_database"));
        assert!(fake.calls.ends_with(&[
            "database_integrity_ok",
            "restore_legacy_socket",
            "start_legacy",
            "reopen_admission"
        ]));
    }

    #[test]
    fn failed_integrity_restores_the_private_backup_before_python() {
        let mut fake = Fake::success();
        fake.activation = Err("daemon failed".to_owned());
        fake.integrity = Ok(false);
        let error = activate(&mut fake).unwrap_err();
        assert!(error.contains("and database backup"));
        let restore = fake
            .calls
            .iter()
            .position(|call| *call == "restore_database")
            .unwrap();
        let restart = fake
            .calls
            .iter()
            .position(|call| *call == "start_legacy")
            .unwrap();
        assert!(restore < restart);
    }
}
