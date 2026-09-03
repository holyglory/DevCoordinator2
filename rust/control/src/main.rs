use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand, ValueEnum};
use devcoordinator2_api::{ClientContext, ClientKind, ResponseEnvelope};
use devcoordinator2_control::{
    client, config::Config, control_plane::ControlPlane, daemon, database::Database, mcp,
};
use tokio::sync::watch;

#[derive(Debug, Parser)]
#[command(
    name = "devcoordinator2",
    version,
    about = "Server-wide development coordinator"
)]
struct Cli {
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Json)]
    format: OutputFormat,
    #[arg(long, global = true, value_enum, default_value_t = ClientArg::Other)]
    client: ClientArg,
    #[arg(long, global = true)]
    session: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum OutputFormat {
    #[default]
    Json,
    Human,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum ClientArg {
    Codex,
    Claude,
    Cursor,
    Antigravity,
    Human,
    Edge,
    #[default]
    Other,
}

impl From<ClientArg> for ClientKind {
    fn from(value: ClientArg) -> Self {
        match value {
            ClientArg::Codex => Self::Codex,
            ClientArg::Claude => Self::Claude,
            ClientArg::Cursor => Self::Cursor,
            ClientArg::Antigravity => Self::Antigravity,
            ClientArg::Human => Self::Human,
            ClientArg::Edge => Self::Edge,
            ClientArg::Other => Self::Other,
        }
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    Daemon,
    Mcp,
    Ping,
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();
    let config = match if matches!(&cli.command, Command::Daemon) {
        Config::load_for_daemon()
    } else {
        Config::load()
    } {
        Ok(config) => config,
        Err(error) => {
            eprintln!("configuration failed: {error}");
            return ExitCode::from(2);
        }
    };
    match cli.command {
        Command::Daemon => run_daemon(&config).await,
        Command::Mcp => match mcp::run_stdio(config.socket_path.clone()).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("MCP server failed: {error}");
                ExitCode::from(2)
            }
        },
        Command::Ping => {
            let context = ClientContext {
                kind: cli.client.into(),
                session: cli.session,
                identity: None,
            };
            match client::call(&config.socket_path, "ping", serde_json::json!({}), context).await {
                Ok(response) => render_response(response, cli.format),
                Err(error) => {
                    let response = ResponseEnvelope::failure("", error);
                    let _ = render(&response, cli.format);
                    ExitCode::from(2)
                }
            }
        }
    }
}

async fn run_daemon(config: &Config) -> ExitCode {
    let database = match Database::open(config.database_path()) {
        Ok(database) => database,
        Err(error) => {
            eprintln!("daemon database failed: {error}");
            return ExitCode::from(1);
        }
    };
    let plane = match ControlPlane::new(config.clone(), database) {
        Ok(plane) => plane,
        Err(error) => {
            eprintln!("daemon initialization failed: {error}");
            return ExitCode::from(1);
        }
    };
    let app = Arc::new(daemon::App::with_executor(
        config.edge_uid,
        Arc::new(plane.clone()),
    ));
    let capacity = plane.capacity().clone();
    let daemon_socket = config.socket_path.clone();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let signal_shutdown = shutdown_tx.clone();
    let signal = tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = signal_shutdown.send(true);
    });
    let mut daemon_shutdown = shutdown_rx.clone();
    let mut daemon_task = tokio::spawn(async move {
        daemon::serve_with_app(&daemon_socket, &mut daemon_shutdown, app).await
    });
    let mut capacity_task = tokio::spawn(async move { capacity.serve(shutdown_rx).await });
    let (surface, result) = tokio::select! {
        result = &mut daemon_task => ("daemon", result),
        result = &mut capacity_task => ("capacity broker", result),
    };
    let _ = shutdown_tx.send(true);
    let sibling = if surface == "daemon" {
        capacity_task.await
    } else {
        daemon_task.await
    };
    signal.abort();
    match (result, sibling) {
        (Ok(Ok(())), Ok(Ok(()))) => ExitCode::SUCCESS,
        (Ok(Err(error)), _) => {
            eprintln!("{surface} failed: {error}");
            ExitCode::from(1)
        }
        (Err(error), _) => {
            eprintln!("{surface} task failed: {error}");
            ExitCode::from(1)
        }
        (Ok(Ok(())), Ok(Err(error))) => {
            eprintln!("daemon sibling failed during shutdown: {error}");
            ExitCode::from(1)
        }
        (Ok(Ok(())), Err(error)) => {
            eprintln!("daemon sibling task failed during shutdown: {error}");
            ExitCode::from(1)
        }
    }
}

fn render_response(response: ResponseEnvelope, format: OutputFormat) -> ExitCode {
    let ok = response.is_ok();
    if render(&response, format).is_err() {
        return ExitCode::from(2);
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn render(response: &ResponseEnvelope, format: OutputFormat) -> std::io::Result<()> {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    match format {
        OutputFormat::Json => serde_json::to_writer_pretty(&mut output, response)?,
        OutputFormat::Human => match response {
            ResponseEnvelope::Success { data, .. } => {
                if let Some(version) = data.get("daemon_version").and_then(|value| value.as_str()) {
                    writeln!(output, "DevCoordinator2 {version} is available")?;
                    return Ok(());
                }
                serde_json::to_writer_pretty(&mut output, data)?;
            }
            ResponseEnvelope::Failure { error, .. } => {
                writeln!(output, "{}: {}", error.code, error.message)?;
                return Ok(());
            }
        },
    }
    writeln!(output)
}
