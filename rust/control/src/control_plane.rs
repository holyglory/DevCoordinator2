//! Composed protocol-v2 operation dispatcher.
//!
//! Domain modules stay reusable and transport-free. This layer performs the
//! one authorization decision, decodes the registry's typed input, invokes the
//! owning service, filters public collections, and serializes the typed result.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use devcoordinator2_api::params;
use devcoordinator2_api::results;
use devcoordinator2_api::{EmptyParams, ErrorCode, PingData, ProtocolError};
use devcoordinator2_executor_core::log_query::LogQueryOperation;
use rusqlite::OptionalExtension;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use time::{format_description::FormatItem, macros::format_description};

use crate::access::{Access, Caller, RoutePublisher};
use crate::bugs;
use crate::capacity::CapacityBroker;
use crate::config::Config;
use crate::daemon::OperationExecutor;
use crate::database::{Database, DatabaseError};
use crate::deployments::Deployments;
use crate::plan::{PlanService, SqliteDeploymentEvidence};
use crate::platform::{Clock, HostClock};
use crate::repository::Registry;
use crate::routes::RouteFilePublisher;
use crate::telegram::{TelegramEvent, TelegramScope, TelegramService, parse_scope};
use crate::test_artifacts::TestArtifactService;
use crate::test_logs::TestLogService;
use crate::{DATABASE_SCHEMA_VERSION, SOURCE_COMMIT};

const TIMESTAMP_FORMAT: &[FormatItem<'static>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");

/// Operations whose domain implementations are installed in this migration
/// checkpoint. Other registered operations fail visibly until their owning
/// service is ported; they never report synthetic success.
pub const FOUNDATION_OPERATIONS: &[&str] = &[
    "ping",
    "user.whoami",
    "user.accept_invitation",
    "user.list",
    "user.invite",
    "user.remove",
    "grant.set",
    "grant.remove",
    "repository.register",
    "repository.list",
    "repository.status",
    "repository.archive",
    "repository.unarchive",
    "deployment.list",
    "deployment.status",
    "deployment.set_domain",
    "test.log.catalog",
    "test.log.tail",
    "test.log.search",
    "test.log.range",
    "test.log.failure_context",
    "test.log.retention.get",
    "test.log.retention.set",
    "test.artifact.catalog",
    "test.artifact.file",
    "test.capacity.get",
    "test.capacity.set",
    "telegram.link",
    "telegram.subscribe",
    "telegram.unsubscribe",
    "telegram.list",
    "plan.overview",
    "task.history",
    "task.create",
    "task.update",
    "release.create",
    "release.update",
    "release.request",
    "release.deliver",
    "decision.tail",
    "decision.search",
    "decision.record",
    "decision.summarize",
    "bug.report",
    "bug.list",
    "bug.close",
];

#[derive(Clone)]
pub struct ControlPlane {
    config: Arc<Config>,
    database: Database,
    access: Access,
    registry: Registry,
    plan: PlanService,
    logs: TestLogService,
    artifacts: TestArtifactService,
    deployments: Deployments,
    capacity: CapacityBroker,
    telegram: TelegramService,
    clock: Arc<dyn Clock>,
}

impl ControlPlane {
    pub fn new(config: Config, database: Database) -> Result<Self, ProtocolError> {
        let publisher = Arc::new(RouteFilePublisher::new(
            database.clone(),
            config.routes_path(),
            config.base_domain.clone(),
        ));
        Self::with_adapters(config, database, publisher, Arc::new(HostClock))
    }

    pub fn with_adapters(
        config: Config,
        database: Database,
        publisher: Arc<dyn RoutePublisher>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, ProtocolError> {
        let access = Access::new(&config, database.clone(), publisher)?;
        let registry = Registry::new(database.clone());
        let deployments = Deployments::new(config.clone(), database.clone(), registry.clone());
        let evidence = Arc::new(SqliteDeploymentEvidence::new(
            database.clone(),
            config.base_domain.clone(),
        ));
        let plan = PlanService::new(database.clone(), evidence);
        let logs = TestLogService::new(database.clone(), registry.clone());
        let artifacts = TestArtifactService::new(database.clone(), registry.clone());
        let capacity = CapacityBroker::new(database.clone(), config.capacity_socket_path())?;
        let telegram = TelegramService::from_config(&config, database.clone());
        Ok(Self {
            config: Arc::new(config),
            database,
            access,
            registry,
            plan,
            logs,
            artifacts,
            deployments,
            capacity,
            telegram,
            clock,
        })
    }

    pub fn access(&self) -> &Access {
        &self.access
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn capacity(&self) -> &CapacityBroker {
        &self.capacity
    }

    pub fn logs(&self) -> &TestLogService {
        &self.logs
    }

    pub fn telegram(&self) -> &TelegramService {
        &self.telegram
    }

    fn dispatch_authorized(
        &self,
        operation: &str,
        params: Value,
        caller: &Caller,
    ) -> Result<Value, ProtocolError> {
        let now = self.timestamp()?;
        let actor = caller.actor();
        match operation {
            "ping" => {
                let _: EmptyParams = decode(params)?;
                encode(PingData {
                    daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
                    protocol_version: devcoordinator2_api::PROTOCOL_VERSION,
                    schema_version: DATABASE_SCHEMA_VERSION,
                    executor_schema: devcoordinator2_executor_protocol::EXECUTOR_SCHEMA,
                    source_commit: SOURCE_COMMIT.to_owned(),
                    socket: self.config.socket_path.display().to_string(),
                })
            }
            "user.whoami" => {
                let _: params::Empty = decode(params)?;
                encode(self.access.who_am_i(caller)?)
            }
            "user.accept_invitation" => encode(self.access.accept_invitation(decode(params)?)?),
            "user.list" => {
                let _: params::Empty = decode(params)?;
                encode(self.access.list_users()?)
            }
            "user.invite" => {
                let result = self.access.invite(decode(params)?, caller)?;
                self.notify(TelegramEvent::new("user.invited").with("email", result.email.clone()));
                encode(result)
            }
            "user.remove" => {
                let result = self.access.remove_user(decode(params)?, caller)?;
                self.notify(TelegramEvent::new("user.removed").with("email", result.email.clone()));
                encode(result)
            }
            "grant.set" => {
                let result = self.access.set_grant(decode(params)?, caller)?;
                self.notify(
                    TelegramEvent::new("grant.set")
                        .with("email", result.email.clone())
                        .with("deployment_id", result.deployment_id.clone()),
                );
                encode(result)
            }
            "grant.remove" => {
                let result = self.access.remove_grant(decode(params)?, caller)?;
                self.notify(
                    TelegramEvent::new("grant.removed")
                        .with("email", result.email.clone())
                        .with("deployment_id", result.deployment_id.clone()),
                );
                encode(result)
            }
            "repository.register" => {
                let params: params::PathOnly = decode(params)?;
                encode(
                    self.registry
                        .register(Path::new(&params.path), caller.uid, caller.gid)?,
                )
            }
            "repository.list" => {
                let params: params::RepositoryList = decode(params)?;
                encode(self.registry.list_repositories(params.include_archived)?)
            }
            "repository.status" => {
                let params: params::PathOnly = decode(params)?;
                encode(
                    self.registry.repository_status(
                        Path::new(&params.path),
                        Some((caller.uid, caller.gid)),
                    )?,
                )
            }
            "repository.archive" => {
                let params: params::ArchiveRepository = decode(params)?;
                encode(self.registry.archive(
                    &params.repository_id,
                    &params.merged_into_repository_id,
                    &params.note,
                    caller.uid,
                )?)
            }
            "repository.unarchive" => {
                let params: params::UnarchiveRepository = decode(params)?;
                encode(
                    self.registry
                        .unarchive(&params.repository_id, &params.note, caller.uid)?,
                )
            }
            "deployment.list" => encode(self.deployments.list(decode(params)?, caller)?),
            "deployment.status" => {
                let params: params::DeploymentReference = decode(params)?;
                encode(self.deployments.status(
                    params.path.as_deref(),
                    params.name.as_deref(),
                    params.deployment_id.as_deref(),
                    caller,
                )?)
            }
            "deployment.logs" => {
                let params: params::DeploymentLogs = decode(params)?;
                let deployment_id = params.deployment_id.as_deref().ok_or_else(|| {
                    ProtocolError::new(
                        ErrorCode::InternalError,
                        "managed deployment logs are not installed in this migration checkpoint",
                    )
                })?;
                if !self.deployments.store().observed_exists(deployment_id)? {
                    return Err(ProtocolError::new(
                        ErrorCode::InternalError,
                        "managed deployment logs are not installed in this migration checkpoint",
                    ));
                }
                encode(self.deployments.observed_logs(
                    deployment_id,
                    &params.component,
                    params.tail_lines,
                )?)
            }
            "deployment.start" | "deployment.stop" | "deployment.restart" => {
                let action = operation.split('.').nth(1).unwrap_or_default();
                let params: params::DeploymentControl = decode(params)?;
                let deployment_id = params.deployment_id.as_deref().ok_or_else(|| {
                    ProtocolError::new(
                        ErrorCode::InternalError,
                        "managed deployment control is not installed in this migration checkpoint",
                    )
                })?;
                if !self.deployments.store().observed_exists(deployment_id)? {
                    return Err(ProtocolError::new(
                        ErrorCode::InternalError,
                        "managed deployment control is not installed in this migration checkpoint",
                    ));
                }
                let result = self.deployments.control_observed(
                    action,
                    deployment_id,
                    params.component.as_deref(),
                )?;
                self.notify(
                    TelegramEvent::new(operation)
                        .with("deployment_id", deployment_id)
                        .with("name", result.name.clone())
                        .with("source", "observed")
                        .with("component", params.component)
                        .with("repository_id", result.repository_id.clone())
                        .with("state", result.state.clone()),
                );
                encode(result)
            }
            "deployment.set_domain" => {
                let params: params::SetDomain = decode(params)?;
                let deployment_id = params.deployment_id.clone();
                let result = self.deployments.set_domain(params)?;
                self.notify(
                    TelegramEvent::new("deployment.domain_changed")
                        .with("deployment_id", deployment_id)
                        .with("domain", result.domain.clone()),
                );
                encode(result)
            }
            "test.log.catalog" => {
                let params: params::LogCatalog = decode(params)?;
                self.logs.query(
                    LogQueryOperation::Catalog,
                    Path::new(&params.path),
                    encode(&params)?,
                    caller,
                )
            }
            "test.log.tail" => {
                let params: params::LogTail = decode(params)?;
                self.logs.query(
                    LogQueryOperation::Tail,
                    Path::new(&params.path),
                    encode(&params)?,
                    caller,
                )
            }
            "test.log.search" => {
                let params: params::LogSearch = decode(params)?;
                self.logs.query(
                    LogQueryOperation::Search,
                    Path::new(&params.path),
                    encode(&params)?,
                    caller,
                )
            }
            "test.log.range" => {
                let params: params::LogRange = decode(params)?;
                self.logs.query(
                    LogQueryOperation::Range,
                    Path::new(&params.path),
                    encode(&params)?,
                    caller,
                )
            }
            "test.log.failure_context" => {
                let params: params::LogFailureContext = decode(params)?;
                self.logs.query(
                    LogQueryOperation::FailureContext,
                    Path::new(&params.path),
                    encode(&params)?,
                    caller,
                )
            }
            "test.log.retention.get" => {
                let _: params::Empty = decode(params)?;
                encode(self.logs.retention()?)
            }
            "test.log.retention.set" => {
                let params: params::SetRetention = decode(params)?;
                encode(self.logs.set_retention(
                    params.max_age_seconds,
                    params.case_depth,
                    &actor,
                    &now,
                )?)
            }
            "test.artifact.catalog" => encode(self.artifacts.catalog(decode(params)?, caller)?),
            "test.artifact.file" => encode(self.artifacts.file(decode(params)?, caller)?),
            "test.capacity.get" => {
                let _: params::Empty = decode(params)?;
                encode(self.capacity.snapshot()?)
            }
            "test.capacity.set" => {
                let params: params::SetCapacity = decode(params)?;
                let cap = match params.cap {
                    params::CapacityCap::Value(cap) => Some(u32::from(cap)),
                    params::CapacityCap::Null => None,
                };
                encode(self.capacity.set_cap(cap, &actor)?)
            }
            "telegram.link" => {
                let params: params::TelegramLink = decode(params)?;
                let principal = self.access.principal(caller)?;
                let email = params.email.or(principal.identity.clone()).ok_or_else(|| {
                    ProtocolError::new(ErrorCode::ParamsInvalid, "email is required")
                })?;
                let email = email.to_lowercase();
                if !principal.local
                    && !principal.administrator
                    && principal.identity.as_deref() != Some(&email)
                {
                    return Err(ProtocolError::new(
                        ErrorCode::PermissionDenied,
                        "may only link chats to yourself",
                    ));
                }
                encode(self.telegram.link(&params.code, &email)?)
            }
            "telegram.subscribe" => {
                let params: params::TelegramSubscription = decode(params)?;
                self.authorize_telegram_chat(caller, params.chat_id)?;
                self.authorize_telegram_scope(caller, &params.scope)?;
                encode(self.telegram.subscribe(params)?)
            }
            "telegram.unsubscribe" => {
                let params: params::TelegramSubscription = decode(params)?;
                self.authorize_telegram_chat(caller, params.chat_id)?;
                encode(self.telegram.unsubscribe(params)?)
            }
            "telegram.list" => {
                let _: params::Empty = decode(params)?;
                let principal = self.access.principal(caller)?;
                let email = (!principal.local && !principal.administrator)
                    .then_some(principal.identity)
                    .flatten();
                encode(self.telegram.list(email.as_deref())?)
            }
            "plan.overview" => {
                let params: params::PlanReference = decode(params)?;
                if params.path.is_none() && params.repository_id.is_none() {
                    return encode(self.plan.overview(None)?);
                }
                let repository = self.resolve_repository(
                    params.path.as_deref(),
                    params.repository_id.as_deref(),
                    caller,
                    true,
                )?;
                encode(self.plan.overview(Some(&repository.repository_id))?)
            }
            "task.history" => {
                let params: params::TaskHistory = decode(params)?;
                encode(self.plan.task_history(&params.task_id)?)
            }
            "task.create" => {
                let params: params::TaskCreate = decode(params)?;
                let repository = self.resolve_repository(
                    params.path.as_deref(),
                    params.repository_id.as_deref(),
                    caller,
                    false,
                )?;
                encode(
                    self.plan
                        .create_task(&repository.repository_id, params, &actor, &now)?,
                )
            }
            "task.update" => encode(self.plan.update_task(decode(params)?, &actor, &now)?),
            "release.create" => {
                let params: params::ReleaseCreate = decode(params)?;
                let repository = self.resolve_repository(
                    params.path.as_deref(),
                    params.repository_id.as_deref(),
                    caller,
                    false,
                )?;
                encode(
                    self.plan
                        .create_release(&repository.repository_id, params, &actor, &now)?,
                )
            }
            "release.update" => encode(self.plan.update_release(decode(params)?, &actor, &now)?),
            "release.request" => {
                let params: params::ReleaseRequest = decode(params)?;
                let repository = self.resolve_repository(
                    params.path.as_deref(),
                    params.repository_id.as_deref(),
                    caller,
                    false,
                )?;
                let result =
                    self.plan
                        .request_release(&repository.repository_id, params, &actor, &now)?;
                self.notify(
                    TelegramEvent::new("release.requested")
                        .with("repository_id", result.repository_id.clone())
                        .with("repository_name", repository.display_name)
                        .with("name", result.name.clone()),
                );
                encode(result)
            }
            "release.deliver" => {
                let params: params::ReleaseDeliver = decode(params)?;
                let (repository_id, name) = self.release_event_identity(&params.release_id)?;
                let result = self.plan.deliver_release(params, &actor, &now)?;
                let mut event = TelegramEvent::new("release.delivered")
                    .with("repository_id", repository_id)
                    .with("name", name)
                    .with("port", result.port)
                    .with("dirty", result.dirty);
                if let Some(url) = &result.url {
                    event = event.with("url", url.clone());
                }
                self.notify(event);
                encode(result)
            }
            "decision.tail" => {
                let params: params::DecisionTail = decode(params)?;
                let repository = self.resolve_repository(
                    params.path.as_deref(),
                    params.repository_id.as_deref(),
                    caller,
                    true,
                )?;
                encode(self.plan.decision_tail(&repository.repository_id, params)?)
            }
            "decision.search" => {
                let params: params::DecisionSearch = decode(params)?;
                let repository = self.resolve_repository(
                    params.path.as_deref(),
                    params.repository_id.as_deref(),
                    caller,
                    true,
                )?;
                encode(
                    self.plan
                        .search_decisions(&repository.repository_id, params)?,
                )
            }
            "decision.record" => {
                let params: params::DecisionRecord = decode(params)?;
                let repository = self.resolve_repository(
                    params.path.as_deref(),
                    params.repository_id.as_deref(),
                    caller,
                    false,
                )?;
                encode(self.plan.record_decision(
                    &repository.repository_id,
                    params,
                    &actor,
                    &now,
                )?)
            }
            "decision.summarize" => {
                let params: params::DecisionSummarize = decode(params)?;
                let repository = self.resolve_repository(
                    params.path.as_deref(),
                    params.repository_id.as_deref(),
                    caller,
                    false,
                )?;
                encode(self.plan.summarize_decisions(
                    &repository.repository_id,
                    params,
                    &actor,
                    &now,
                )?)
            }
            "bug.report" => {
                let params: params::BugReport = decode(params)?;
                let result =
                    bugs::report(&self.config.bugs_dir, &params, &actor).map_err(bug_error)?;
                if !result.duplicate {
                    let mut event = TelegramEvent::new("bug.opened")
                        .with("component", result.component.clone())
                        .with("summary", result.summary.clone());
                    if let Some(repository_id) = &result.correlations.repository_id {
                        event = event.with("repository_id", repository_id.clone());
                    }
                    if let Some(deployment_id) = &result.correlations.deployment_id {
                        event = event.with("deployment_id", deployment_id.clone());
                    }
                    self.notify(event);
                }
                encode(result)
            }
            "bug.list" => {
                let _: params::Empty = decode(params)?;
                encode(results::BugList {
                    bugs: bugs::list_open(&self.config.bugs_dir).map_err(bug_error)?,
                    // The backing host path is deliberately private.
                    store: None,
                })
            }
            "bug.close" => {
                let params: params::BugClose = decode(params)?;
                let result =
                    bugs::close(&self.config.bugs_dir, &params.bug_id).map_err(bug_error)?;
                self.notify(
                    TelegramEvent::new("bug.closed")
                        .with("component", result.component.clone())
                        .with("summary", result.summary.clone()),
                );
                encode(result)
            }
            _ => Err(ProtocolError::new(
                ErrorCode::InternalError,
                "operation handler is not installed in this migration checkpoint",
            )),
        }
    }

    fn resolve_repository(
        &self,
        path: Option<&str>,
        repository_id: Option<&str>,
        caller: &Caller,
        allow_archived: bool,
    ) -> Result<RepositoryIdentity, ProtocolError> {
        if let Some(repository_id) = repository_id {
            return self.repository_by_id(repository_id, allow_archived);
        }
        let path = path.ok_or_else(|| {
            ProtocolError::new(
                ErrorCode::ParamsInvalid,
                "path or repository_id is required",
            )
        })?;
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err(ProtocolError::new(
                ErrorCode::ParamsInvalid,
                "path must be absolute",
            ));
        }
        if allow_archived {
            match self
                .registry
                .repository_status(&path, Some((caller.uid, caller.gid)))
            {
                Ok(status) => {
                    return Ok(RepositoryIdentity {
                        repository_id: status.repository_id,
                        display_name: status.display_name,
                    });
                }
                Err(error) if error.code == ErrorCode::RepositoryNotFound => {}
                Err(error) => return Err(error),
            }
        }
        let registered = self.registry.register(&path, caller.uid, caller.gid)?;
        Ok(RepositoryIdentity {
            repository_id: registered.repository_id,
            display_name: registered.display_name,
        })
    }

    fn repository_by_id(
        &self,
        repository_id: &str,
        allow_archived: bool,
    ) -> Result<RepositoryIdentity, ProtocolError> {
        let repository_id = repository_id.to_owned();
        let lookup = repository_id.clone();
        let state = self
            .database
            .call(move |connection| {
                connection
                    .query_row(
                        "SELECT archived_at,merged_into_repository_id,display_name FROM repositories WHERE repository_id=?1",
                        [&lookup],
                        |row| {
                            Ok((
                                row.get::<_, Option<String>>(0)?,
                                row.get::<_, Option<String>>(1)?,
                                row.get::<_, String>(2)?,
                            ))
                        },
                    )
                    .optional()
                    .map_err(DatabaseError::from)
            })
            .map_err(database_error)?
            .ok_or_else(|| {
                ProtocolError::new(
                    ErrorCode::RepositoryNotFound,
                    format!("no repository {repository_id}"),
                )
            })?;
        if !allow_archived && state.0.is_some() {
            let suffix = state
                .1
                .map(|replacement| format!("; use {replacement}"))
                .unwrap_or_default();
            return Err(ProtocolError::new(
                ErrorCode::RepositoryArchived,
                format!("repository {repository_id} is archived{suffix}"),
            ));
        }
        Ok(RepositoryIdentity {
            repository_id,
            display_name: state.2,
        })
    }

    fn timestamp(&self) -> Result<String, ProtocolError> {
        self.clock
            .now_utc()
            .format(TIMESTAMP_FORMAT)
            .map_err(|error| {
                ProtocolError::new(
                    ErrorCode::InternalError,
                    "cannot format operation timestamp",
                )
                .with_detail(error.to_string())
            })
    }

    fn authorize_telegram_chat(&self, caller: &Caller, chat_id: i64) -> Result<(), ProtocolError> {
        let principal = self.access.principal(caller)?;
        if principal.local || principal.administrator {
            return Ok(());
        }
        if self.telegram.chat_email(chat_id)? == principal.identity {
            Ok(())
        } else {
            Err(ProtocolError::new(
                ErrorCode::PermissionDenied,
                "chat is linked to another identity",
            ))
        }
    }

    fn release_event_identity(&self, release_id: &str) -> Result<(String, String), ProtocolError> {
        let release_id = release_id.to_owned();
        let lookup = release_id.clone();
        self.database
            .call(move |connection| {
                connection
                    .query_row(
                        "SELECT repository_id,name FROM releases WHERE release_id=?1",
                        [&lookup],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                    )
                    .optional()
                    .map_err(DatabaseError::from)
            })
            .map_err(database_error)?
            .ok_or_else(|| {
                ProtocolError::new(
                    ErrorCode::ReleaseNotFound,
                    format!("no release {release_id}"),
                )
            })
    }

    fn authorize_telegram_scope(&self, caller: &Caller, scope: &str) -> Result<(), ProtocolError> {
        let principal = self.access.principal(caller)?;
        if principal.local || principal.administrator {
            return Ok(());
        }
        match parse_scope(scope)? {
            TelegramScope::Server => Err(ProtocolError::new(
                ErrorCode::PermissionDenied,
                "server scope requires administrator",
            )),
            TelegramScope::Deployment(deployment_id) => {
                if principal.at_least(&deployment_id, &params::AccessRole::Viewer) {
                    Ok(())
                } else {
                    Err(ProtocolError::new(
                        ErrorCode::PermissionDenied,
                        "viewer on the deployment required",
                    ))
                }
            }
            TelegramScope::Repository(repository_id) => {
                let deployments = principal.grants.keys().cloned().collect::<Vec<_>>();
                let visible = self.database.call(move |connection| {
                    for deployment in deployments {
                        let found = connection.query_row(
                            "SELECT EXISTS(SELECT 1 FROM deployments WHERE deployment_id=?1 AND repository_id=?2) OR EXISTS(SELECT 1 FROM observed_deployments WHERE observed_deployment_id=?1 AND repository_id=?2)",
                            rusqlite::params![deployment, repository_id],
                            |row| row.get::<_, i64>(0),
                        )? != 0;
                        if found {
                            return Ok(true);
                        }
                    }
                    Ok(false)
                }).map_err(database_error)?;
                if visible {
                    Ok(())
                } else {
                    Err(ProtocolError::new(
                        ErrorCode::PermissionDenied,
                        "no viewable deployment in that repository",
                    ))
                }
            }
        }
    }

    fn notify(&self, event: TelegramEvent) {
        let _ = self.telegram.enqueue_event(&event);
    }
}

impl OperationExecutor for ControlPlane {
    fn execute(
        &self,
        operation: &str,
        params: Value,
        caller: &Caller,
    ) -> Result<Value, ProtocolError> {
        let authorization = self.access.authorize(operation, &params, caller)?;
        let result = self.dispatch_authorized(operation, authorization.params.clone(), caller)?;
        authorization.apply_result(result)
    }
}

struct RepositoryIdentity {
    repository_id: String,
    display_name: String,
}

fn decode<T: DeserializeOwned>(params: Value) -> Result<T, ProtocolError> {
    serde_json::from_value(params).map_err(|error| {
        ProtocolError::new(ErrorCode::ParamsInvalid, "operation parameters are invalid")
            .with_detail(error.to_string())
    })
}

fn encode<T: Serialize>(result: T) -> Result<Value, ProtocolError> {
    serde_json::to_value(result).map_err(|error| {
        ProtocolError::new(ErrorCode::InternalError, "cannot encode operation result")
            .with_detail(error.to_string())
    })
}

fn bug_error(error: bugs::BugError) -> ProtocolError {
    ProtocolError::new(ErrorCode::ParamsInvalid, error.to_string())
}

fn database_error(error: DatabaseError) -> ProtocolError {
    match error {
        DatabaseError::Domain(error) => error,
        other => ProtocolError::new(ErrorCode::InternalError, "database operation failed")
            .with_detail(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devcoordinator2_api::params::{
        AccessRole, InvitationGrant, InviteUser, TaskKind, TaskStatus,
    };
    use std::collections::HashSet;
    use tempfile::tempdir;
    use time::macros::datetime;

    fn config(root: &Path) -> Config {
        Config {
            socket_path: root.join("daemon.sock"),
            state_dir: root.join("state"),
            unit_prefix: "devcoordinator2-test".into(),
            slice_name: "devcoordinator2-tests.slice".into(),
            client_group: "devcoordinator2-clients".into(),
            port_range: (40_000, 40_100),
            base_domain: "example.test".into(),
            edge_uid: Some(999),
            admin_emails: Vec::new(),
            telegram_token_file: None,
            telegram_api: "https://api.telegram.org".into(),
            bugs_dir: root.join("bugs"),
            compose_env_allowlist_file: None,
            compose_env_authorizations: HashSet::new(),
            codex_usage_sources_file: None,
            codex_usage_sources: Vec::new(),
        }
    }

    fn local() -> Caller {
        Caller {
            pid: 1,
            uid: 1000,
            gid: 1000,
            client_kind: devcoordinator2_api::ClientKind::Codex,
            client_session: Some("fixture".into()),
            identity: None,
        }
    }

    fn public(identity: &str) -> Caller {
        Caller {
            pid: 2,
            uid: 999,
            gid: 999,
            client_kind: devcoordinator2_api::ClientKind::Edge,
            client_session: None,
            identity: Some(identity.into()),
        }
    }

    #[test]
    fn composed_dispatch_authorizes_and_persists_typed_planning_results() {
        let temporary = tempdir().expect("tempdir");
        let database = Database::open(temporary.path().join("authority.sqlite3")).expect("db");
        database
            .transaction(|transaction| {
                transaction.execute("INSERT INTO repositories(repository_id,root_path,display_name,registered_at,registered_by_uid,last_seen_at) VALUES('r1111111111111111','/repo','repo','t',1000,'t')", [])?;
                transaction.execute("INSERT INTO worktrees VALUES('w1111111111111111','r1111111111111111','/repo','t','t')", [])?;
                Ok(())
            })
            .expect("fixture");
        let plane = ControlPlane::with_adapters(
            config(temporary.path()),
            database.clone(),
            Arc::new(|_: &crate::access::RouteAccessSection| Ok(())),
            Arc::new(crate::platform::FixedClock(datetime!(2026-09-03 12:00 UTC))),
        )
        .expect("control plane");

        let created = plane
            .execute(
                "task.create",
                serde_json::json!({
                    "repository_id":"r1111111111111111",
                    "title":"Port one complete operation",
                    "kind":"goal",
                    "estimated_loc":120
                }),
                &local(),
            )
            .expect("task create");
        assert_eq!(created["status"], serde_json::json!(TaskStatus::Planned));
        assert_eq!(created["repository_id"], "r1111111111111111");
        let overview = plane
            .execute(
                "plan.overview",
                serde_json::json!({"repository_id":"r1111111111111111"}),
                &local(),
            )
            .expect("overview");
        assert_eq!(
            overview["tasks"][0]["kind"],
            serde_json::json!(TaskKind::Goal)
        );
        assert_eq!(overview["tasks"][0]["estimated_loc"], 120);

        let unavailable = plane
            .execute("test.start", serde_json::json!({"path":"/repo"}), &local())
            .expect_err("not yet installed");
        assert_eq!(unavailable.code, ErrorCode::InternalError);
    }

    #[test]
    fn every_foundation_operation_exists_in_the_exhaustive_registry() {
        for operation in FOUNDATION_OPERATIONS {
            assert!(
                devcoordinator2_api::operation(operation).is_some(),
                "{operation} is not registered"
            );
        }
    }

    #[test]
    fn composed_telegram_operations_enforce_chat_and_scope_visibility() {
        let temporary = tempdir().expect("tempdir");
        let database = Database::open(temporary.path().join("authority.sqlite3")).expect("db");
        database
            .transaction(|transaction| {
                transaction.execute("INSERT INTO repositories(repository_id,root_path,display_name,registered_at,registered_by_uid,last_seen_at) VALUES('r1111111111111111','/repo','repo','t',1000,'t'),('r2222222222222222','/other','other','t',1000,'t')", [])?;
                transaction.execute("INSERT INTO worktrees VALUES('w1111111111111111','r1111111111111111','/repo','t','t'),('w2222222222222222','r2222222222222222','/other','t','t')", [])?;
                transaction.execute("INSERT INTO deployments(deployment_id,repository_id,worktree_id,name,source,spec_fingerprint,spec_json,state,created_at,created_by_uid,client,updated_at) VALUES('d1111111111111111','r1111111111111111','w1111111111111111','one','worktree','f','{}','running','t',1,'other','t'),('d2222222222222222','r2222222222222222','w2222222222222222','two','worktree','f','{}','running','t',1,'other','t')", [])?;
                Ok(())
            })
            .expect("fixture");
        let plane = ControlPlane::with_adapters(
            config(temporary.path()),
            database,
            Arc::new(|_: &crate::access::RouteAccessSection| Ok(())),
            Arc::new(crate::platform::FixedClock(datetime!(2026-09-03 12:00 UTC))),
        )
        .expect("control plane");
        plane
            .access
            .invite(
                InviteUser {
                    email: "viewer@example.test".into(),
                    administrator: false,
                    grants: vec![InvitationGrant {
                        deployment_id: "d1111111111111111".into(),
                        role: AccessRole::Viewer,
                    }],
                },
                &local(),
            )
            .expect("invite");
        plane
            .access
            .accept_invitation(params::AcceptInvitation {
                email: "viewer@example.test".into(),
                subject: Some("subject".into()),
                display_name: None,
            })
            .expect("accept");
        let code = plane.telegram.issue_link_code(7, "viewer").expect("code");
        plane
            .execute(
                "telegram.link",
                serde_json::json!({"code":code}),
                &public("viewer@example.test"),
            )
            .expect("link own chat");
        plane
            .execute(
                "telegram.subscribe",
                serde_json::json!({"chat_id":7,"scope":"deployment:d1111111111111111"}),
                &public("viewer@example.test"),
            )
            .expect("deployment subscription");
        plane
            .execute(
                "telegram.subscribe",
                serde_json::json!({"chat_id":7,"scope":"repository:r1111111111111111"}),
                &public("viewer@example.test"),
            )
            .expect("repository subscription");
        for scope in [
            "server",
            "deployment:d2222222222222222",
            "repository:r2222222222222222",
        ] {
            let error = plane
                .execute(
                    "telegram.subscribe",
                    serde_json::json!({"chat_id":7,"scope":scope}),
                    &public("viewer@example.test"),
                )
                .expect_err("scope denied");
            assert_eq!(error.code, ErrorCode::PermissionDenied);
        }
        let listing = plane
            .execute(
                "telegram.list",
                serde_json::json!({}),
                &public("viewer@example.test"),
            )
            .expect("listing");
        assert_eq!(listing["chats"].as_array().map(Vec::len), Some(1));
        assert_eq!(listing["chats"][0]["chat_id"], 7);
        let denied = plane
            .execute(
                "telegram.unsubscribe",
                serde_json::json!({"chat_id":8,"scope":"server"}),
                &public("viewer@example.test"),
            )
            .expect_err("other chat denied");
        assert_eq!(denied.code, ErrorCode::PermissionDenied);
    }
}
