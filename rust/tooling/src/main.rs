use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "devcoordinator2-tooling", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Contract {
        #[command(subcommand)]
        command: ContractCommand,
    },
    Check {
        #[command(subcommand)]
        command: CheckCommand,
    },
    Skills {
        #[command(subcommand)]
        command: SkillsCommand,
    },
}

#[derive(Debug, Subcommand)]
enum SkillsCommand {
    Links {
        #[command(subcommand)]
        command: SkillLinksCommand,
    },
    Policy {
        #[command(subcommand)]
        command: PolicyCommand,
    },
}

#[derive(Debug, Args)]
struct SkillSelection {
    #[arg(long, default_value = ".")]
    repo_root: PathBuf,
    #[arg(long, required = true)]
    target_root: Vec<PathBuf>,
    #[arg(long)]
    skill: Vec<String>,
}

#[derive(Debug, Subcommand)]
enum SkillLinksCommand {
    Plan(SkillSelection),
    Verify(SkillSelection),
    Apply {
        #[command(flatten)]
        selection: SkillSelection,
        #[arg(long)]
        transaction_dir: PathBuf,
        #[arg(long)]
        allow_noncanonical: bool,
    },
    Rollback {
        #[arg(long)]
        transaction_dir: PathBuf,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
enum PolicyCommand {
    Plan {
        #[arg(long)]
        repo_root: PathBuf,
        #[arg(long)]
        transaction_dir: PathBuf,
        #[arg(long)]
        codex_target: Vec<PathBuf>,
        #[arg(long)]
        claude_target: Vec<PathBuf>,
        #[arg(long)]
        claude_windows_import_wrapper_target: Vec<PathBuf>,
    },
    Apply {
        #[arg(long)]
        transaction_dir: PathBuf,
        #[arg(long)]
        plan_digest: String,
    },
    Verify {
        #[arg(long)]
        transaction_dir: PathBuf,
        #[arg(long)]
        plan_digest: String,
    },
    Rollback {
        #[arg(long)]
        transaction_dir: PathBuf,
        #[arg(long)]
        plan_digest: String,
    },
}

#[derive(Debug, Subcommand)]
enum ContractCommand {
    Export {
        #[arg(long, default_value = "contracts/devcoordinator2-v2.schema.json")]
        output: PathBuf,
        #[arg(long)]
        check: bool,
    },
}

#[derive(Debug, Subcommand)]
enum CheckCommand {
    /// Reject every executable Python dependency while retaining the seven
    /// approved inert audit fixtures.
    PythonFree {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        report: Option<PathBuf>,
    },
    NoInstanceData {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        patterns: Option<PathBuf>,
    },
    NoTestTimerWaits {
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    AgentNeutrality {
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    AppWidePolicy {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        policy: Option<PathBuf>,
        #[arg(long)]
        project_importer: Option<PathBuf>,
    },
    CiSecurity {
        #[arg(long, default_value = ".github/workflows/validate.yml")]
        workflow: PathBuf,
    },
    RepositoryBoundaries {
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    RepositoryFreshness {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long, default_value = "origin")]
        remote: String,
        #[arg(long)]
        branch: Option<String>,
    },
    UserIssueLedgers {
        #[arg(long, default_value = "UserIssueLedgers")]
        root: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Contract {
            command: ContractCommand::Export { output, check },
        } => match devcoordinator2_tooling::export_contract(&output, check) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(1)
            }
        },
        Command::Check {
            command: CheckCommand::PythonFree { root, report },
        } => {
            let report = report.unwrap_or_else(|| {
                root.join(devcoordinator2_tooling::python_guard::DEFAULT_REPORT_RELATIVE_PATH)
            });
            match devcoordinator2_tooling::python_guard::inspect_repository_to_report(
                &root, &report,
            ) {
                Ok(receipt) => {
                    println!("{}", receipt.to_json());
                    if receipt.is_clean() {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::from(1)
                    }
                }
                Err(error) => {
                    eprintln!(
                        "{}",
                        serde_json::json!({
                            "ok": false,
                            "error": {
                                "code": error.kind.as_str(),
                                "message": error.to_string(),
                            }
                        })
                    );
                    ExitCode::from(2)
                }
            }
        }
        Command::Check {
            command: CheckCommand::NoInstanceData { root, patterns },
        } => {
            let patterns = patterns.unwrap_or_else(|| root.join("instance/forbidden-strings.txt"));
            check_report(
                devcoordinator2_tooling::repository_checks::check_no_instance_data(
                    &root, &patterns,
                ),
            )
        }
        Command::Check {
            command: CheckCommand::NoTestTimerWaits { root },
        } => check_report(
            devcoordinator2_tooling::repository_checks::check_no_test_timer_waits(&root),
        ),
        Command::Check {
            command: CheckCommand::AgentNeutrality { root },
        } => {
            check_report(devcoordinator2_tooling::repository_checks::audit_agent_neutrality(&root))
        }
        Command::Check {
            command:
                CheckCommand::AppWidePolicy {
                    root,
                    policy,
                    project_importer,
                },
        } => {
            let policy = policy.unwrap_or_else(|| root.join("reference/universal/AGENTS.md"));
            let importer = project_importer.or_else(|| {
                (policy == root.join("reference/universal/AGENTS.md"))
                    .then(|| root.join("CLAUDE.md"))
            });
            let report = devcoordinator2_tooling::repository_checks::check_app_wide_policy(
                &policy,
                importer.as_deref(),
            );
            emit_report(report.to_json(), report.is_clean(), 1)
        }
        Command::Check {
            command: CheckCommand::CiSecurity { workflow },
        } => match devcoordinator2_tooling::repository_checks::check_ci_workflow(&workflow) {
            Ok(result) => emit_report(result.to_json(), result.report.is_clean(), 1),
            Err(error) => check_error(error),
        },
        Command::Check {
            command: CheckCommand::RepositoryBoundaries { root },
        } => check_report(
            devcoordinator2_tooling::repository_checks::audit_repository_boundaries(&root),
        ),
        Command::Check {
            command:
                CheckCommand::RepositoryFreshness {
                    root,
                    remote,
                    branch,
                },
        } => match devcoordinator2_tooling::repository_checks::inspect_repository_freshness(
            &root,
            &remote,
            branch.as_deref(),
        ) {
            Ok(result) => emit_report(result.to_json(), result.exit_code == 0, result.exit_code),
            Err(error) => check_error(error),
        },
        Command::Check {
            command: CheckCommand::UserIssueLedgers { root },
        } => {
            let result =
                devcoordinator2_tooling::repository_checks::audit_user_issue_ledgers(&root);
            emit_report(result.to_json(), result.is_clean(), 1)
        }
        Command::Skills {
            command: SkillsCommand::Links { command },
        } => run_skill_links(command),
        Command::Skills {
            command: SkillsCommand::Policy { command },
        } => run_policy(command),
    }
}

fn selected_skills(values: Vec<String>) -> Option<Vec<String>> {
    (!values.is_empty()).then_some(values)
}

fn run_skill_links(command: SkillLinksCommand) -> ExitCode {
    use devcoordinator2_tooling::skill_links;

    let result = match command {
        SkillLinksCommand::Plan(selection) => skill_links::build_skill_link_plan(
            &selection.repo_root,
            &selection.target_root,
            selected_skills(selection.skill).as_deref(),
        )
        .map(|plan| (plan.to_json(), true)),
        SkillLinksCommand::Verify(selection) => skill_links::verify_skill_links(
            &selection.repo_root,
            &selection.target_root,
            selected_skills(selection.skill).as_deref(),
        )
        .map(|verification| {
            let mut value = verification.plan.to_json();
            if let Some(object) = value.as_object_mut() {
                object.insert("failures".into(), serde_json::json!(verification.failures));
            }
            (value, verification.is_verified())
        }),
        SkillLinksCommand::Apply {
            selection,
            transaction_dir,
            allow_noncanonical,
        } => {
            let options = skill_links::SkillLinkApplyOptions {
                selected_skills: selected_skills(selection.skill),
                allow_noncanonical,
                failure_after_links: None,
            };
            skill_links::apply_skill_links(
                &selection.repo_root,
                &selection.target_root,
                &transaction_dir,
                &options,
            )
            .map(|transaction| {
                (
                    serde_json::json!({
                        "status": transaction.status,
                        "transaction_dir": transaction.transaction_dir,
                        "changed_entries": transaction.changed_entries,
                        "journal": transaction.document,
                    }),
                    true,
                )
            })
        }
        SkillLinksCommand::Rollback {
            transaction_dir,
            force,
        } => skill_links::rollback_skill_links(&transaction_dir, force).map(|transaction| {
            (
                serde_json::json!({
                    "status": transaction.status,
                    "transaction_dir": transaction.transaction_dir,
                    "changed_entries": transaction.changed_entries,
                    "journal": transaction.document,
                }),
                true,
            )
        }),
    };
    match result {
        Ok((value, verified)) => emit_report(value, verified, 1),
        Err(error) => link_error(error),
    }
}

fn run_policy(command: PolicyCommand) -> ExitCode {
    use devcoordinator2_tooling::skill_links::{self, PolicyTarget};

    let result = match command {
        PolicyCommand::Plan {
            repo_root,
            transaction_dir,
            codex_target,
            claude_target,
            claude_windows_import_wrapper_target,
        } => {
            let targets = codex_target
                .into_iter()
                .map(PolicyTarget::codex)
                .chain(claude_target.into_iter().map(PolicyTarget::claude))
                .chain(
                    claude_windows_import_wrapper_target
                        .into_iter()
                        .map(PolicyTarget::claude_windows_wrapper),
                )
                .collect::<Vec<_>>();
            skill_links::create_policy_plan(&repo_root, &transaction_dir, &targets).map(|receipt| {
                let mut value = receipt.plan.to_json();
                if let Some(object) = value.as_object_mut() {
                    object.insert("plan_sha256".into(), receipt.digest.into());
                }
                value
            })
        }
        PolicyCommand::Apply {
            transaction_dir,
            plan_digest,
        } => skill_links::apply_policy_transaction(&transaction_dir, &plan_digest)
            .map(|result| result.to_json()),
        PolicyCommand::Verify {
            transaction_dir,
            plan_digest,
        } => skill_links::verify_policy_transaction(&transaction_dir, &plan_digest)
            .map(|result| result.to_json()),
        PolicyCommand::Rollback {
            transaction_dir,
            plan_digest,
        } => skill_links::rollback_policy_transaction(&transaction_dir, &plan_digest)
            .map(|result| result.to_json()),
    };
    match result {
        Ok(value) => emit_report(value, true, 1),
        Err(error) => link_error(error),
    }
}

fn link_error(error: devcoordinator2_tooling::skill_links::LinkError) -> ExitCode {
    eprintln!(
        "{}",
        serde_json::json!({
            "ok": false,
            "error": {"code": error.kind.as_str(), "message": error.to_string()}
        })
    );
    ExitCode::from(2)
}

fn check_report(
    result: Result<
        devcoordinator2_tooling::repository_checks::CheckReport,
        devcoordinator2_tooling::repository_checks::CheckError,
    >,
) -> ExitCode {
    match result {
        Ok(report) => emit_report(report.to_json(), report.is_clean(), 1),
        Err(error) => check_error(error),
    }
}

fn check_error(error: devcoordinator2_tooling::repository_checks::CheckError) -> ExitCode {
    eprintln!(
        "{}",
        serde_json::json!({
            "ok": false,
            "error": {"code": error.kind.as_str(), "message": error.to_string()}
        })
    );
    ExitCode::from(2)
}

fn emit_report(value: serde_json::Value, clean: bool, finding_exit: i32) -> ExitCode {
    match serde_json::to_string_pretty(&value) {
        Ok(rendered) => println!("{rendered}"),
        Err(error) => {
            eprintln!("cannot encode check result: {error}");
            return ExitCode::from(2);
        }
    }
    if clean {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(u8::try_from(finding_exit).unwrap_or(2))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_and_skill_command_families_parse_without_legacy_wrappers() {
        let check = Cli::try_parse_from([
            "devcoordinator2-tooling",
            "check",
            "repository-freshness",
            "--root",
            "/repo",
            "--remote",
            "upstream",
            "--branch",
            "main",
        ])
        .expect("freshness command");
        assert!(matches!(
            check.command,
            Command::Check {
                command: CheckCommand::RepositoryFreshness { .. }
            }
        ));

        let links = Cli::try_parse_from([
            "devcoordinator2-tooling",
            "skills",
            "links",
            "verify",
            "--repo-root",
            "/repo",
            "--target-root",
            "/runtime/skills",
            "--skill",
            "dev-coordinator",
        ])
        .expect("skill verification command");
        assert!(matches!(
            links.command,
            Command::Skills {
                command: SkillsCommand::Links {
                    command: SkillLinksCommand::Verify(_)
                }
            }
        ));

        let policy = Cli::try_parse_from([
            "devcoordinator2-tooling",
            "skills",
            "policy",
            "plan",
            "--repo-root",
            "/repo",
            "--transaction-dir",
            "/state/transaction",
            "--codex-target",
            "/runtime/AGENTS.md",
        ])
        .expect("policy plan command");
        assert!(matches!(
            policy.command,
            Command::Skills {
                command: SkillsCommand::Policy {
                    command: PolicyCommand::Plan { .. }
                }
            }
        ));
    }
}
