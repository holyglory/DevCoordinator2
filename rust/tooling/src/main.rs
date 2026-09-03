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

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Contract {
            command: ContractCommand::Export { output, check },
        } => devcoordinator2_tooling::export_contract(&output, check),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}
