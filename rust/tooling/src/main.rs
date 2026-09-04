use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

const SOURCE_COMMIT: &str = match option_env!("DEVCOORDINATOR2_SOURCE_COMMIT") {
    Some(value) => value,
    None => "development",
};

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
    Legacy {
        #[command(subcommand)]
        command: LegacyCommand,
    },
    Decision {
        #[command(subcommand)]
        command: DecisionCommand,
    },
    Install {
        #[command(subcommand)]
        command: InstallCommand,
    },
}

#[derive(Debug, Subcommand)]
enum InstallCommand {
    Preflight {
        #[arg(long, default_value = ".")]
        source_root: PathBuf,
        #[arg(long)]
        fetch: bool,
    },
    Build {
        #[arg(long, default_value = ".")]
        source_root: PathBuf,
        #[arg(long, default_value = "/etc/devcoordinator2/install-manifest.json")]
        manifest: PathBuf,
    },
    Verify {
        #[arg(long, default_value = "/etc/devcoordinator2/install-manifest.json")]
        manifest: PathBuf,
    },
    Plan {
        #[arg(long, default_value = "/etc/devcoordinator2/install-manifest.json")]
        manifest: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum LegacyCommand {
    Export(LegacyExportArgs),
    Import(LegacyImportArgs),
}

#[derive(Debug, Args)]
struct LegacyExportArgs {
    #[arg(long)]
    authority_db: PathBuf,
    #[arg(long)]
    routes_publication: Option<PathBuf>,
    #[arg(long)]
    access_control: Option<PathBuf>,
    #[arg(long)]
    telegram_state: Option<PathBuf>,
    #[arg(long)]
    bugs_dir: Option<PathBuf>,
    #[arg(long = "out")]
    output: PathBuf,
}

#[derive(Debug, Args)]
struct LegacyImportArgs {
    #[arg(long = "export")]
    export_file: PathBuf,
    #[arg(long)]
    state_dir: PathBuf,
    #[arg(long)]
    bugs_dir: PathBuf,
    #[arg(long)]
    live_containers: Option<PathBuf>,
    #[arg(long)]
    current_route_map: Option<PathBuf>,
    #[arg(long)]
    routes_path: Option<PathBuf>,
    #[arg(long)]
    base_domain: Option<String>,
    #[arg(long)]
    prune_missing_install_fixtures: bool,
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Subcommand)]
enum DecisionCommand {
    Import(DecisionImportArgs),
}

#[derive(Debug, Args)]
struct DecisionImportArgs {
    #[arg(long)]
    path: PathBuf,
    #[arg(long)]
    file: Option<PathBuf>,
    #[arg(long)]
    socket: Option<PathBuf>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long = "aspect")]
    aspect_overrides: Vec<String>,
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
    if source_commit_requested() {
        println!("{SOURCE_COMMIT}");
        return ExitCode::SUCCESS;
    }
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
        Command::Legacy { command } => run_legacy(command),
        Command::Decision { command } => run_decision(command),
        Command::Install { command } => run_install(command),
    }
}

fn run_install(command: InstallCommand) -> ExitCode {
    use devcoordinator2_tooling::install::{self, HostRunner};
    let result = match command {
        InstallCommand::Preflight { source_root, fetch } => {
            install::validate_live_checkout(&source_root, fetch, &HostRunner).map(|commit| {
                serde_json::json!({
                    "source_root": source_root,
                    "source_commit": commit,
                    "clean_main": true,
                })
            })
        }
        InstallCommand::Build {
            source_root,
            manifest: manifest_path,
        } => (|| {
            if rustix::process::geteuid().as_raw() != 0 {
                return Err("install build must run as root".to_owned());
            }
            let source_root = source_root
                .canonicalize()
                .map_err(|error| format!("cannot resolve source root: {error}"))?;
            let source_commit = install::validate_live_checkout(&source_root, false, &HostRunner)?;
            let identity = install::checkout_build_identity(&source_root)?;
            let binaries = install::build_release_binaries(
                &source_root,
                &source_commit,
                &identity,
                &install::BuildTools::default(),
                &HostRunner,
            )?;
            let built_at = timestamp()?;
            let document = install::manifest(&source_root, &source_commit, &built_at, binaries)?;
            install::write_manifest(&manifest_path, &document, (0, 0))?;
            serde_json::to_value(document)
                .map_err(|error| format!("cannot encode installation manifest: {error}"))
        })(),
        InstallCommand::Verify { manifest } => {
            install::read_and_verify_manifest(&manifest, &HostRunner).and_then(|document| {
                serde_json::to_value(document)
                    .map_err(|error| format!("cannot encode installation manifest: {error}"))
            })
        }
        InstallCommand::Plan { manifest } => {
            install::read_and_verify_manifest(&manifest, &HostRunner)
                .and_then(|document| install::installation_plan(&document, &manifest))
                .and_then(|plan| {
                    serde_json::to_value(plan)
                        .map_err(|error| format!("cannot encode installation plan: {error}"))
                })
        }
    };
    match result {
        Ok(value) => emit_report(value, true, 1),
        Err(error) => tooling_error(&error, 2),
    }
}

fn source_commit_requested() -> bool {
    let mut arguments = std::env::args_os().skip(1);
    arguments.next().as_deref() == Some(std::ffi::OsStr::new("--source-commit"))
        && arguments.next().is_none()
}

fn run_legacy(command: LegacyCommand) -> ExitCode {
    let now = match timestamp() {
        Ok(now) => now,
        Err(error) => return tooling_error(&error, 2),
    };
    let result = match command {
        LegacyCommand::Export(args) => {
            let options = devcoordinator2_tooling::legacy_export::ExportOptions {
                authority_db: args.authority_db,
                routes_publication: args.routes_publication,
                access_control: args.access_control,
                telegram_state: args.telegram_state,
                bugs_dir: args.bugs_dir,
                output: args.output,
            };
            devcoordinator2_tooling::legacy_export::write_export(&options, &now)
        }
        LegacyCommand::Import(args) => {
            let options = devcoordinator2_tooling::legacy_import::ImportOptions {
                export: args.export_file,
                state_dir: args.state_dir,
                bugs_dir: args.bugs_dir,
                live_containers: args.live_containers,
                current_route_map: args.current_route_map,
                routes_path: args.routes_path,
                base_domain: args.base_domain,
                prune_missing_install_fixtures: args.prune_missing_install_fixtures,
                dry_run: args.dry_run,
            };
            devcoordinator2_tooling::legacy_import::run(&options, &now)
        }
    };
    match result {
        Ok(value) => emit_report(value, true, 1),
        Err(error) => tooling_error(&error, 2),
    }
}

fn run_decision(command: DecisionCommand) -> ExitCode {
    let DecisionCommand::Import(args) = command;
    let source = args
        .file
        .unwrap_or_else(|| args.path.join("DecisionHistory.md"));
    let options = devcoordinator2_tooling::decision_import::ImportOptions {
        repository: args.path,
        source,
        socket: args
            .socket
            .unwrap_or_else(devcoordinator2_tooling::instance::socket_path),
        dry_run: args.dry_run,
        aspect_overrides: args.aspect_overrides,
    };
    let entries = match devcoordinator2_tooling::decision_import::load(&options) {
        Ok(entries) if !entries.is_empty() => entries,
        Ok(_) => return tooling_error("no `## REF — title` decision sections were found", 1),
        Err(error) => return tooling_error(&error, 2),
    };
    if options.dry_run {
        return emit_report(
            serde_json::json!({
                "dry_run": true,
                "entries": entries,
                "recorded": 0,
            }),
            true,
            1,
        );
    }
    match devcoordinator2_tooling::decision_import::import_with(
        &entries,
        &options.repository,
        |params| devcoordinator2_tooling::decision_import::call(&options.socket, params),
    ) {
        Ok(counts) => emit_report(
            serde_json::json!({
                "imported": counts.imported,
                "already_present": counts.skipped,
                "failed": counts.failed,
            }),
            counts.failed == 0,
            1,
        ),
        Err(error) => tooling_error(&error, 2),
    }
}

fn timestamp() -> Result<String, String> {
    use time::{OffsetDateTime, macros::format_description};
    OffsetDateTime::now_utc()
        .format(format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second]Z"
        ))
        .map_err(|error| format!("cannot format current time: {error}"))
}

fn tooling_error(message: &str, code: u8) -> ExitCode {
    eprintln!(
        "{}",
        serde_json::json!({
            "ok": false,
            "error": {"code": "tooling_failed", "message": message}
        })
    );
    ExitCode::from(code)
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
    fn every_ported_command_family_parses_without_python_wrappers() {
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

        let legacy = Cli::try_parse_from([
            "devcoordinator2-tooling",
            "legacy",
            "export",
            "--authority-db",
            "/legacy/authority.sqlite3",
            "--out",
            "/tmp/export.json",
        ])
        .expect("legacy export command");
        assert!(matches!(
            legacy.command,
            Command::Legacy {
                command: LegacyCommand::Export(_)
            }
        ));

        let decision = Cli::try_parse_from([
            "devcoordinator2-tooling",
            "decision",
            "import",
            "--path",
            "/repo",
            "--dry-run",
        ])
        .expect("decision import command");
        assert!(matches!(
            decision.command,
            Command::Decision {
                command: DecisionCommand::Import(_)
            }
        ));

        let install = Cli::try_parse_from([
            "devcoordinator2-tooling",
            "install",
            "preflight",
            "--source-root",
            "/repo",
        ])
        .expect("install preflight command");
        assert!(matches!(
            install.command,
            Command::Install {
                command: InstallCommand::Preflight { .. }
            }
        ));
    }
}
