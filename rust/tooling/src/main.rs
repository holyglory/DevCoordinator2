use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

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
    }
}
