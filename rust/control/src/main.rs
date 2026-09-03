use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use devcoordinator2_api::params::{BugCorrelations, BugReport};
use devcoordinator2_api::results::BugList;
use devcoordinator2_api::{ErrorCode, ProtocolError, ResponseEnvelope};
use devcoordinator2_control::bugs;
use devcoordinator2_control::check_event;
use devcoordinator2_control::cli::{Cli, Invocation, OfflineBugAction, OutputFormat};
use devcoordinator2_control::{
    client, config::Config, control_plane::ControlPlane, daemon, database::Database, mcp,
};
use tokio::sync::watch;
use tokio::task::JoinSet;

type ServiceResult = (&'static str, Result<(), String>);
type JoinedService = Result<ServiceResult, tokio::task::JoinError>;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();
    let format = cli.format;
    let context = cli.client_context();
    let invocation = match cli.into_invocation() {
        Ok(invocation) => invocation,
        Err(error) => {
            return local_error(
                format,
                ErrorCode::ParamsInvalid,
                "command-line arguments are invalid",
                &error.to_string(),
                2,
            );
        }
    };
    match invocation {
        Invocation::Daemon => match Config::load_for_daemon() {
            Ok(config) => run_daemon(&config).await,
            Err(error) => configuration_error(format, &error.to_string()),
        },
        Invocation::Mcp => match Config::load() {
            Ok(config) => match mcp::run_stdio(config.socket_path).await {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => local_error(
                    format,
                    ErrorCode::DaemonUnavailable,
                    "MCP server failed",
                    &error.to_string(),
                    2,
                ),
            },
            Err(error) => configuration_error(format, &error.to_string()),
        },
        Invocation::Remote { operation, params } => match Config::load() {
            Ok(config) => match client::call(&config.socket_path, operation, params, context).await
            {
                Ok(response) => render_response(response, format, Some(operation)),
                Err(error) => {
                    let response = ResponseEnvelope::failure("", error);
                    let _ = devcoordinator2_control::cli::render_response(
                        &response,
                        format,
                        Some(operation),
                    );
                    ExitCode::from(2)
                }
            },
            Err(error) => configuration_error(format, &error.to_string()),
        },
        Invocation::OfflineBug { action } => run_offline_bug(action, format),
        Invocation::TestEvent { status } => match check_event::emit_from_environment(status) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => local_error(
                format,
                ErrorCode::ParamsInvalid,
                "cannot emit governed-check event",
                &error.to_string(),
                2,
            ),
        },
        Invocation::ArtifactMaterialize { .. } => local_error(
            format,
            ErrorCode::InternalError,
            "artifact materialization is not installed in this migration checkpoint",
            "the Rust materializer remains an active migration item",
            2,
        ),
    }
}

fn configuration_error(format: OutputFormat, detail: &str) -> ExitCode {
    local_error(
        format,
        ErrorCode::ParamsInvalid,
        "configuration failed",
        detail,
        2,
    )
}

fn run_offline_bug(action: OfflineBugAction, format: OutputFormat) -> ExitCode {
    let directory = bugs::bugs_dir();
    let reporter = format!("uid:{}", rustix::process::getuid().as_raw());
    let (operation, result) = match action {
        OfflineBugAction::Report {
            component,
            summary,
            expected,
            actual,
            steps,
            run_id,
            deployment_id,
        } => {
            let request = BugReport {
                component,
                summary,
                expected,
                actual,
                steps,
                correlations: BugCorrelations {
                    run_id,
                    deployment_id,
                    component: None,
                    repository_id: None,
                    call_id: None,
                },
            };
            let result = bugs::report(&directory, &request, &reporter).map(|mut record| {
                record.notified = Some(false);
                serde_json::to_value(record)
            });
            ("bug.report", result)
        }
        OfflineBugAction::List => (
            "bug.list",
            bugs::list_open(&directory).map(|records| {
                serde_json::to_value(BugList {
                    bugs: records,
                    store: None,
                })
            }),
        ),
        OfflineBugAction::Close { bug_id } => {
            let result = bugs::close(&directory, &bug_id).map(|mut closed| {
                closed.notified = Some(false);
                serde_json::to_value(closed)
            });
            ("bug.close", result)
        }
    };
    match result {
        Ok(Ok(data)) => match ResponseEnvelope::success("offline", data) {
            Ok(response) => render_response(response, format, Some(operation)),
            Err(error) => local_error(format, error.code, &error.message, &error.detail, 2),
        },
        Ok(Err(error)) => local_error(
            format,
            ErrorCode::InternalError,
            "cannot encode offline bug result",
            &error.to_string(),
            2,
        ),
        Err(error) => local_error(format, ErrorCode::ParamsInvalid, &error.to_string(), "", 1),
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
    let logs = plane.logs().clone();
    let daemon_socket = config.socket_path.clone();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let signal_shutdown = shutdown_tx.clone();
    let signal = tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = signal_shutdown.send(true);
    });
    let mut services = JoinSet::new();
    let mut daemon_shutdown = shutdown_rx.clone();
    services.spawn(async move {
        (
            "daemon",
            daemon::serve_with_app(&daemon_socket, &mut daemon_shutdown, app)
                .await
                .map_err(|error| error.to_string()),
        )
    });
    let capacity_shutdown = shutdown_rx.clone();
    services.spawn(async move {
        (
            "capacity broker",
            capacity
                .serve(capacity_shutdown)
                .await
                .map_err(|error| error.to_string()),
        )
    });
    services.spawn(async move {
        logs.serve_maintenance(shutdown_rx).await;
        ("log maintenance", Ok(()))
    });
    let first = services.join_next().await;
    let _ = shutdown_tx.send(true);
    let mut failure = service_failure(first);
    while let Some(result) = services.join_next().await {
        failure = failure.or_else(|| service_failure(Some(result)));
    }
    signal.abort();
    match failure {
        None => ExitCode::SUCCESS,
        Some((surface, error)) => {
            eprintln!("{surface} failed: {error}");
            ExitCode::from(1)
        }
    }
}

fn service_failure(result: Option<JoinedService>) -> Option<(&'static str, String)> {
    match result {
        Some(Ok((_, Ok(())))) | None => None,
        Some(Ok((surface, Err(error)))) => Some((surface, error)),
        Some(Err(error)) => Some(("daemon service task", error.to_string())),
    }
}

fn render_response(
    response: ResponseEnvelope,
    format: OutputFormat,
    operation: Option<&str>,
) -> ExitCode {
    let ok = response.is_ok();
    if devcoordinator2_control::cli::render_response(&response, format, operation).is_err() {
        return ExitCode::from(2);
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn local_error(
    format: OutputFormat,
    code: ErrorCode,
    message: &str,
    detail: &str,
    exit: u8,
) -> ExitCode {
    let response = ResponseEnvelope::failure(
        "",
        ProtocolError::new(code, message).with_detail(detail.to_owned()),
    );
    let _ = devcoordinator2_control::cli::render_response(&response, format, None);
    ExitCode::from(exit)
}
