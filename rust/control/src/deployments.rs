//! Deployment resolution, truthful status, observed control, and route changes.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use devcoordinator2_api::params::{DeploymentList as DeploymentListParams, SetDomain};
use devcoordinator2_api::results::{
    Component, ComponentBinding, ComposeService, DeclaredDeployment, DeploymentList, DeploymentLog,
    DeploymentSource, DeploymentStatus, DomainChanged,
};
use devcoordinator2_api::{ErrorCode, ProtocolError};
use rusqlite::OptionalExtension;

use crate::access::Caller;
use crate::config::Config;
use crate::database::{Database, DatabaseError};
use crate::deployment_state::{ComponentRow, DeploymentRow, DeploymentStore};
use crate::docker::{DockerCli, DockerControl, RuntimeState};
use crate::repository::Registry;
use crate::repository_config::{
    ComponentKind, ComponentSpec, DeploymentSpec, list_deployment_names, load_deployment_spec,
};
use crate::routes::RouteFilePublisher;
use crate::systemd::{SystemdCli, SystemdControl};

pub trait NetworkProbe: Send + Sync + 'static {
    fn tcp(&self, host: &str, port: u16) -> bool;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HostNetworkProbe;

impl NetworkProbe for HostNetworkProbe {
    fn tcp(&self, host: &str, port: u16) -> bool {
        (host, port)
            .to_socket_addrs()
            .ok()
            .into_iter()
            .flatten()
            .any(|address| TcpStream::connect_timeout(&address, Duration::from_secs(3)).is_ok())
    }
}

#[derive(Clone)]
pub struct Deployments {
    config: Arc<Config>,
    database: Database,
    registry: Registry,
    store: DeploymentStore,
    docker: Arc<dyn DockerControl>,
    systemd: Arc<dyn SystemdControl>,
    network: Arc<dyn NetworkProbe>,
    routes: RouteFilePublisher,
}

struct ResolvedDeployment {
    row: DeploymentRow,
    specification: DeploymentSpec,
}

impl Deployments {
    pub fn new(config: Config, database: Database, registry: Registry) -> Self {
        Self::with_adapters(
            config,
            database,
            registry,
            Arc::new(DockerCli::default()),
            Arc::new(SystemdCli::default()),
            Arc::new(HostNetworkProbe),
        )
    }

    pub fn with_adapters(
        config: Config,
        database: Database,
        registry: Registry,
        docker: Arc<dyn DockerControl>,
        systemd: Arc<dyn SystemdControl>,
        network: Arc<dyn NetworkProbe>,
    ) -> Self {
        let routes = RouteFilePublisher::new(
            database.clone(),
            config.routes_path(),
            config.base_domain.clone(),
        );
        Self {
            config: Arc::new(config),
            store: DeploymentStore::new(database.clone()),
            database,
            registry,
            docker,
            systemd,
            network,
            routes,
        }
    }

    pub fn store(&self) -> &DeploymentStore {
        &self.store
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn list(
        &self,
        params: DeploymentListParams,
        caller: &Caller,
    ) -> Result<DeploymentList, ProtocolError> {
        let repositories = self.registry.list_repositories(false)?;
        let active = repositories
            .repositories
            .iter()
            .map(|repository| repository.repository_id.clone())
            .collect::<HashSet<_>>();
        let mut deployments = self.store.managed_list_rows(&active)?;
        deployments.extend(
            self.store
                .observed_list(None)?
                .into_iter()
                .filter(|row| active.contains(&row.repository_id)),
        );
        let mut declared = Vec::new();
        if let Some(path) = params.path {
            if caller.identity.is_some() {
                return Err(ProtocolError::new(
                    ErrorCode::PermissionDenied,
                    "public callers cannot discover deployments from a path",
                ));
            }
            let registered = self
                .registry
                .register(Path::new(&path), caller.uid, caller.gid)?;
            let worktree = PathBuf::from(&registered.worktree_path);
            for name in list_deployment_names(&worktree).map_err(config_error)? {
                let specification = load_deployment_spec(&worktree, &name).map_err(config_error)?;
                for source in specification.sources {
                    declared.push(DeclaredDeployment {
                        deployment_id: DeploymentStore::deployment_id(
                            &registered.worktree_id,
                            &name,
                            &source,
                        ),
                        name: name.clone(),
                        source: deployment_source(&source)?,
                    });
                }
            }
        }
        Ok(DeploymentList {
            deployments,
            declared,
        })
    }

    pub fn status(
        &self,
        path: Option<&str>,
        name: Option<&str>,
        deployment_id: Option<&str>,
        caller: &Caller,
    ) -> Result<DeploymentStatus, ProtocolError> {
        if let Some(deployment_id) = deployment_id
            && self.store.observed_exists(deployment_id)?
        {
            return self
                .store
                .observed_status(deployment_id)?
                .ok_or_else(|| not_found(deployment_id));
        }
        let resolved = self.resolve(path, name, deployment_id, caller)?;
        self.managed_status(&resolved)
    }

    pub fn set_domain(&self, params: SetDomain) -> Result<DomainChanged, ProtocolError> {
        if let Some(domain) = params.domain.as_deref()
            && let Some(owner) = self.store.domain_owner(domain)?
            && owner != params.deployment_id
        {
            return Err(ProtocolError::new(
                ErrorCode::ParamsInvalid,
                format!("domain {domain:?} is already routed to {owner}"),
            ));
        }
        let mut result = if self.store.observed_exists(&params.deployment_id)? {
            self.store.set_observed_domain(
                &params.deployment_id,
                params.domain.as_deref(),
                params.port,
                params.component.as_deref(),
                params.public,
            )?
        } else {
            if params.port.is_some() || params.component.is_some() {
                return Err(ProtocolError::new(
                    ErrorCode::ParamsInvalid,
                    "port and component apply only to observed deployments",
                ));
            }
            let mut changed = self
                .store
                .override_managed_domain(&params.deployment_id, params.domain.as_deref())?;
            if let Some(public) = params.public {
                self.store.set_public(&params.deployment_id, public)?;
                changed.public = Some(public);
            }
            changed
        };
        self.routes.publish_current()?;
        result.observed_only.get_or_insert(false);
        Ok(result)
    }

    pub fn control_observed(
        &self,
        action: &str,
        deployment_id: &str,
        component: Option<&str>,
    ) -> Result<DeploymentStatus, ProtocolError> {
        if !matches!(action, "start" | "stop" | "restart") {
            return Err(ProtocolError::new(
                ErrorCode::ParamsInvalid,
                "observed action is invalid",
            ));
        }
        let containers = self.observed_containers(deployment_id, component)?;
        let mut errors = Vec::new();
        for container in &containers {
            let exact = crate::docker::ExactContainerId::parse(container.0.clone())
                .map_err(|_| observed_action_error("recorded container identity is invalid"))?;
            if self.docker.container_state(&exact).state == RuntimeState::Missing {
                errors.push(format!(
                    "{}: recorded container no longer exists; re-import or adopt it",
                    container.1
                ));
                continue;
            }
            let result = match action {
                "start" => self.docker.start_container(&exact),
                "stop" => self.docker.stop_container(&exact),
                _ => self.docker.restart_container(&exact),
            };
            if let Err(error) = result {
                errors.push(format!("{}: {error}", container.1));
            }
        }
        self.refresh_observed_states(deployment_id)?;
        if !errors.is_empty() {
            return Err(ProtocolError::new(
                ErrorCode::DeploymentActionFailed,
                format!(
                    "{action} failed for {} container(s): {}",
                    errors.len(),
                    truncate(&errors.join("; "), 900)
                ),
            ));
        }
        self.store
            .observed_status(deployment_id)?
            .ok_or_else(|| not_found(deployment_id))
    }

    pub fn observed_logs(
        &self,
        deployment_id: &str,
        component: &str,
        tail_lines: u16,
    ) -> Result<DeploymentLog, ProtocolError> {
        let containers = self.observed_containers(deployment_id, Some(component))?;
        let mut tails = Vec::new();
        for (container_id, _) in containers {
            let exact = crate::docker::ExactContainerId::parse(container_id)
                .map_err(|_| observed_action_error("recorded container identity is invalid"))?;
            tails.push(
                self.docker
                    .container_logs(&exact, tail_lines)
                    .unwrap_or_else(|_| "(logs unavailable)".into()),
            );
        }
        Ok(DeploymentLog {
            deployment_id: Some(deployment_id.into()),
            component: component.into(),
            tail: truncate_tail(&tails.join("\n"), 65_536),
            truncated_before_tail: Some(true),
            log_path: None,
            container_id: None,
            observed_only: Some(true),
        })
    }

    pub fn reject_observed_configuration(&self, deployment_id: &str) -> Result<(), ProtocolError> {
        if self.store.observed_exists(deployment_id)? {
            Err(ProtocolError::new(
                ErrorCode::ObservedOnly,
                "observed deployments can be controlled and inspected, but apply, rollback, remove, and recreation require reviewed repository configuration",
            ))
        } else {
            Ok(())
        }
    }

    fn resolve(
        &self,
        path: Option<&str>,
        name: Option<&str>,
        deployment_id: Option<&str>,
        caller: &Caller,
    ) -> Result<ResolvedDeployment, ProtocolError> {
        if let Some(deployment_id) = deployment_id {
            let row = self
                .store
                .get(deployment_id)?
                .ok_or_else(|| not_found(deployment_id))?;
            let worktree = self.worktree_path(&row.worktree_id)?;
            let specification = load_deployment_spec(&worktree, &row.name).map_err(config_error)?;
            if !specification
                .sources
                .iter()
                .any(|source| source == &row.source)
            {
                return Err(ProtocolError::new(
                    ErrorCode::RepositoryConfigInvalid,
                    "recorded deployment source is no longer declared",
                ));
            }
            return Ok(ResolvedDeployment { row, specification });
        }
        if caller.identity.is_some() {
            return Err(ProtocolError::new(
                ErrorCode::PermissionDenied,
                "public callers address deployments by deployment_id",
            ));
        }
        let path = path.ok_or_else(|| {
            ProtocolError::new(ErrorCode::ParamsInvalid, "path is required with name")
        })?;
        let selected = name.ok_or_else(|| {
            ProtocolError::new(
                ErrorCode::ParamsInvalid,
                "name or deployment_id is required",
            )
        })?;
        let registered = self
            .registry
            .register(Path::new(path), caller.uid, caller.gid)?;
        let worktree = PathBuf::from(&registered.worktree_path);
        let (name, explicit_source) = selected
            .split_once('@')
            .map_or((selected, None), |(name, source)| (name, Some(source)));
        let specification = load_deployment_spec(&worktree, name).map_err(config_error)?;
        let source = match explicit_source {
            Some(source) => source.to_owned(),
            None if specification.sources.len() == 1 => specification.sources[0].clone(),
            None => {
                return Err(ProtocolError::new(
                    ErrorCode::ParamsInvalid,
                    format!(
                        "deployment {name:?} enables {:?}; address it as name@source",
                        specification.sources
                    ),
                ));
            }
        };
        if !specification.sources.contains(&source) {
            return Err(ProtocolError::new(
                ErrorCode::RepositoryConfigInvalid,
                format!("deployment {name:?} does not enable source {source:?}"),
            ));
        }
        let deployment_id = DeploymentStore::deployment_id(&registered.worktree_id, name, &source);
        let row = self.store.get(&deployment_id)?.ok_or_else(|| {
            ProtocolError::new(
                ErrorCode::DeploymentNotFound,
                format!("{name}@{source} was never applied"),
            )
        })?;
        Ok(ResolvedDeployment { row, specification })
    }

    fn managed_status(
        &self,
        resolved: &ResolvedDeployment,
    ) -> Result<DeploymentStatus, ProtocolError> {
        let row = &resolved.row;
        let component_rows = self.store.components(&row.deployment_id)?;
        let mut ports =
            crate::ports::assigned(&self.database, &row.deployment_id, 0).map_err(runtime_error)?;
        if let Some(generation) = row.current_generation {
            ports.extend(
                crate::ports::assigned(&self.database, &row.deployment_id, generation)
                    .map_err(runtime_error)?,
            );
        }
        let mut components = Vec::new();
        for component_row in component_rows {
            let specification = resolved.specification.component(&component_row.name);
            components.push(self.component_status(
                row,
                &component_row,
                specification,
                ports.get(&component_row.name).copied(),
            )?);
        }
        let owned = components
            .iter()
            .filter(|component| component.owned)
            .collect::<Vec<_>>();
        let mut state = if row.state == "applying" {
            "applying"
        } else if !owned.is_empty() && owned.iter().all(|component| component.state == "running") {
            "running"
        } else if owned.iter().all(|component| component.state == "stopped") {
            "stopped"
        } else {
            "degraded"
        }
        .to_owned();
        if state == "running"
            && owned
                .iter()
                .any(|component| component.health == "unhealthy")
        {
            state = "degraded".into();
        }
        let deployment_id = row.deployment_id.clone();
        let (domain, route_port) = self
            .database
            .call(move |connection| {
                connection
                    .query_row(
                        "SELECT domain,port FROM domain_routes WHERE deployment_id=?1",
                        [&deployment_id],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<u16>>(1)?)),
                    )
                    .optional()
                    .map(|value| value.unwrap_or((String::new(), None)))
                    .map_err(DatabaseError::from)
            })
            .map_err(database_error)?;
        let repository_id = row.repository_id.clone();
        let repository_name = self
            .database
            .call(move |connection| {
                connection
                    .query_row(
                        "SELECT display_name FROM repositories WHERE repository_id=?1",
                        [&repository_id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(DatabaseError::from)
            })
            .map_err(database_error)?;
        Ok(DeploymentStatus {
            deployment_id: row.deployment_id.clone(),
            repository_id: row.repository_id.clone(),
            repository_name,
            name: resolved.specification.name.clone(),
            source: deployment_source(&row.source)?,
            state,
            health: None,
            current_generation: row.current_generation,
            previous_generation: row.previous_generation,
            domain: (!domain.is_empty()).then_some(domain),
            route_port,
            route_component: resolved
                .specification
                .route_component()
                .map(|component| component.name.clone()),
            public: row.public,
            ttl_expires_at: row.ttl_expires_at.clone(),
            components,
            // Private host paths are not part of the v2 normal result.
            log_dir: None,
            observed_only: false,
            native_project: None,
            observation_source: None,
            observed_at: None,
            unchanged: None,
            rolled_back_from: None,
            rolled_back_to: None,
        })
    }

    fn component_status(
        &self,
        deployment: &DeploymentRow,
        row: &ComponentRow,
        specification: Option<&ComponentSpec>,
        port: Option<u16>,
    ) -> Result<Component, ProtocolError> {
        let (state, health, restarts, services) = match (
            row.binding_kind.as_deref(),
            row.binding_identity.as_deref(),
            specification,
        ) {
            (_, _, Some(specification)) if specification.kind == ComponentKind::External => {
                let (host, port) = specification
                    .tcp
                    .as_deref()
                    .and_then(|value| value.rsplit_once(':'))
                    .and_then(|(host, port)| port.parse::<u16>().ok().map(|port| (host, port)))
                    .ok_or_else(|| {
                        ProtocolError::new(
                            ErrorCode::RepositoryConfigInvalid,
                            "external TCP target is invalid",
                        )
                    })?;
                let ready = self.network.tcp(host, port);
                (
                    if ready { "running" } else { "failed" }.into(),
                    if ready { "healthy" } else { "unhealthy" }.into(),
                    0,
                    None,
                )
            }
            (_, None, _) => (row.state.clone(), row.health.clone(), row.restarts, None),
            (Some("unit"), Some(identity), _) => {
                let live = self
                    .systemd
                    .process_state(identity)
                    .map_err(systemd_error)?;
                let health = live_health(row, &live.state);
                (live.state, health, live.restarts, None)
            }
            (Some("container"), Some(identity), _) => {
                let exact = crate::docker::ExactContainerId::parse(identity.to_owned())
                    .map_err(|_| observed_action_error("recorded container identity is invalid"))?;
                let live = self.docker.container_state(&exact);
                let state = live.state.as_str().to_owned();
                let health = live_health(row, &state);
                (state, health, live.restarts, None)
            }
            (Some("compose"), Some(project), Some(specification)) => {
                let generation = deployment.current_generation.unwrap_or(0);
                let completions = self.store.compose_completions(
                    &deployment.deployment_id,
                    &row.name,
                    generation,
                )?;
                let mut desires = self
                    .store
                    .compose_service_desires(&deployment.deployment_id, &row.name)?;
                if row.desired_state == "stopped" {
                    for service in specification
                        .services
                        .iter()
                        .filter(|service| !specification.finite_services.contains(*service))
                    {
                        desires.insert(service.clone(), "stopped".into());
                    }
                }
                let desired = desires
                    .into_iter()
                    .map(|(service, state)| {
                        (
                            service,
                            if state == "stopped" {
                                RuntimeState::Stopped
                            } else {
                                RuntimeState::Running
                            },
                        )
                    })
                    .collect::<BTreeMap<_, _>>();
                let live = self
                    .docker
                    .compose_state(
                        project,
                        &specification.services,
                        &specification.finite_services,
                        &completions.keys().cloned().collect::<BTreeSet<_>>(),
                        &desired,
                    )
                    .map_err(runtime_error)?;
                let services = live
                    .services
                    .into_iter()
                    .map(|service| ComposeService {
                        independent: specification.independent_services.contains(&service.name),
                        name: service.name,
                        role: service.role,
                        state: service.state.as_str().into(),
                        desired_state: service.desired_state.as_str().into(),
                        containers: service.containers,
                    })
                    .collect::<Vec<_>>();
                let state = live.state.as_str().to_owned();
                let health = live_health(row, &state);
                (state, health, 0, Some(services))
            }
            _ => (row.state.clone(), row.health.clone(), row.restarts, None),
        };
        let completed_services = if specification
            .is_some_and(|specification| specification.kind == ComponentKind::Compose)
        {
            let generation = deployment.current_generation.unwrap_or(0);
            let rows =
                self.store
                    .compose_completions(&deployment.deployment_id, &row.name, generation)?;
            (!rows.is_empty()).then(|| rows.into_values().collect())
        } else {
            None
        };
        Ok(Component {
            name: row.name.clone(),
            display_name: None,
            r#type: row.kind.clone(),
            state,
            health,
            generation: row.generation,
            binding: ComponentBinding {
                kind: row.binding_kind.clone(),
                identity: row.binding_identity.clone(),
            },
            port,
            restarts: Some(restarts),
            owned: specification.is_some_and(DeploymentStore::is_owned),
            independent_control: specification
                .is_some_and(|specification| specification.independent_control),
            last_error: row.last_error.clone(),
            services,
            completed_services,
        })
    }

    fn observed_containers(
        &self,
        deployment_id: &str,
        component: Option<&str>,
    ) -> Result<Vec<(String, String)>, ProtocolError> {
        let deployment_id = deployment_id.to_owned();
        let component = component.map(str::to_owned);
        let rows = self.database.call(move |connection| {
            let mut statement = connection.prepare(
                "SELECT container_id,compose_service FROM observed_containers WHERE observed_deployment_id=?1 ORDER BY compose_service,name,container_id",
            )?;
            Ok(statement.query_map([deployment_id], |row| {
                Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))
            })?.collect::<Result<Vec<_>,_>>()?)
        }).map_err(database_error)?;
        let rows = component.as_ref().map_or(rows.clone(), |component| {
            rows.into_iter()
                .filter(|(_, service)| service == component)
                .collect()
        });
        if rows.is_empty() {
            return Err(if let Some(component) = component {
                ProtocolError::new(
                    ErrorCode::ParamsInvalid,
                    format!("no observed component {component:?}"),
                )
            } else {
                observed_action_error("no containers are recorded for this observed deployment")
            });
        }
        Ok(rows)
    }

    fn refresh_observed_states(&self, deployment_id: &str) -> Result<(), ProtocolError> {
        let containers = self.observed_containers(deployment_id, None)?;
        let mut states = Vec::new();
        let mut rows = Vec::new();
        for (container_id, _) in containers {
            let exact = crate::docker::ExactContainerId::parse(container_id.clone())
                .map_err(|_| observed_action_error("recorded container identity is invalid"))?;
            let live = self.docker.container_state(&exact);
            let state = live.state.as_str().to_owned();
            let health = if live.state != RuntimeState::Running {
                "none".to_owned()
            } else {
                match live.health.as_deref() {
                    Some("healthy") => "healthy",
                    Some("unhealthy") => "unhealthy",
                    Some("starting") => "starting",
                    _ => "unknown",
                }
                .to_owned()
            };
            states.push((state.clone(), health.clone()));
            rows.push((
                container_id,
                state,
                live.status.unwrap_or_else(|| live.state.as_str().into()),
                health,
            ));
        }
        let deployment_state = if states.iter().all(|(state, _)| state == "running") {
            "running"
        } else if states
            .iter()
            .all(|(state, _)| matches!(state.as_str(), "stopped" | "missing"))
        {
            "stopped"
        } else if states.iter().any(|(state, _)| state == "failed")
            && !states.iter().any(|(state, _)| state == "running")
        {
            "failed"
        } else {
            "degraded"
        }
        .to_owned();
        let deployment_health = if states.iter().any(|(_, health)| health == "unhealthy") {
            "unhealthy"
        } else if states.iter().all(|(_, health)| health == "healthy") {
            "healthy"
        } else {
            "unknown"
        }
        .to_owned();
        let now = self.store.current_timestamp()?;
        let deployment_id = deployment_id.to_owned();
        self.database.transaction(move |transaction| {
            for (container_id,state,status,health) in rows {
                transaction.execute(
                    "UPDATE observed_containers SET state=?1,status=?2,health=?3,observed_at=?4 WHERE container_id=?5",
                    rusqlite::params![state,status,health,now,container_id],
                )?;
            }
            transaction.execute(
                "UPDATE observed_deployments SET state=?1,health=?2,observed_at=?3 WHERE observed_deployment_id=?4",
                rusqlite::params![deployment_state,deployment_health,now,deployment_id],
            )?;
            Ok(())
        }).map_err(database_error)
    }

    fn worktree_path(&self, worktree_id: &str) -> Result<PathBuf, ProtocolError> {
        let worktree_id = worktree_id.to_owned();
        self.database
            .call(move |connection| {
                connection
                    .query_row(
                        "SELECT worktree_path FROM worktrees WHERE worktree_id=?1",
                        [&worktree_id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(DatabaseError::from)
            })
            .map_err(database_error)?
            .map(PathBuf::from)
            .ok_or_else(|| {
                ProtocolError::new(
                    ErrorCode::DeploymentNotFound,
                    "deployment worktree is unregistered",
                )
            })
    }
}

fn live_health(row: &ComponentRow, state: &str) -> String {
    if state == "running" {
        row.health.clone()
    } else if row.state == "running" {
        "unhealthy".into()
    } else {
        "none".into()
    }
}

fn deployment_source(value: &str) -> Result<DeploymentSource, ProtocolError> {
    match value {
        "worktree" => Ok(DeploymentSource::Worktree),
        "checkout" => Ok(DeploymentSource::Checkout),
        "observed" => Ok(DeploymentSource::Observed),
        _ => Err(ProtocolError::new(
            ErrorCode::InternalError,
            "stored deployment source is invalid",
        )),
    }
}

fn config_error(error: crate::repository_config::RepositoryConfigError) -> ProtocolError {
    ProtocolError::new(ErrorCode::RepositoryConfigInvalid, error.to_string())
}

fn not_found(deployment_id: &str) -> ProtocolError {
    ProtocolError::new(
        ErrorCode::DeploymentNotFound,
        format!("no deployment {deployment_id}"),
    )
}

fn observed_action_error(message: impl Into<String>) -> ProtocolError {
    ProtocolError::new(ErrorCode::DeploymentActionFailed, message)
}

fn runtime_error(error: impl std::fmt::Display) -> ProtocolError {
    ProtocolError::new(
        ErrorCode::DeploymentActionFailed,
        "deployment runtime is unavailable",
    )
    .with_detail(truncate(&error.to_string(), 512))
}

fn systemd_error(error: crate::systemd::SystemdError) -> ProtocolError {
    runtime_error(error)
}

fn database_error(error: DatabaseError) -> ProtocolError {
    match error {
        DatabaseError::Domain(error) => error,
        other => ProtocolError::new(
            ErrorCode::InternalError,
            "deployment database operation failed",
        )
        .with_detail(other.to_string()),
    }
}

fn truncate(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_owned();
    }
    let mut end = limit;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn truncate_tail(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_owned();
    }
    let mut start = value.len() - limit;
    while !value.is_char_boundary(start) {
        start += 1;
    }
    value[start..].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment_state::{
        ComponentRuntimePatch, ComposeCompletionInput, ObservedContainerInput,
        ObservedDeploymentInput, RegisteredDeploymentTarget,
    };
    use crate::docker::{
        ComposeServiceState, ComposeState as DockerComposeState, ContainerState, DockerError,
        DockerInvocation, DockerOutput, ExactContainerId, LogFollower,
    };
    use crate::systemd::{
        PersistentUnitSpec, ProcessState, SystemdError, TransientUnitSpec, UnitProcess,
    };
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tempfile::tempdir;

    struct FakeDocker {
        states: Mutex<HashMap<String, ContainerState>>,
        actions: Mutex<Vec<String>>,
        compose: Mutex<Option<DockerComposeState>>,
    }

    impl FakeDocker {
        fn new() -> Self {
            Self {
                states: Mutex::new(HashMap::new()),
                actions: Mutex::new(Vec::new()),
                compose: Mutex::new(None),
            }
        }

        fn running(service: Option<&str>) -> ContainerState {
            ContainerState {
                state: RuntimeState::Running,
                status: Some("running".into()),
                restarts: 2,
                exit_code: Some(0),
                health: Some("healthy".into()),
                started_at: Some("start".into()),
                finished_at: None,
                image_id: Some("sha256:image".into()),
                compose_service: service.map(str::to_owned),
            }
        }
    }

    impl DockerControl for FakeDocker {
        fn invoke(&self, _invocation: DockerInvocation) -> Result<DockerOutput, DockerError> {
            Err(DockerError::InvalidRequest("unexpected invoke".into()))
        }

        fn spawn_follow_logs(
            &self,
            _container_id: &ExactContainerId,
        ) -> Result<LogFollower, DockerError> {
            Err(DockerError::InvalidRequest("unexpected logs".into()))
        }

        fn container_state(&self, container_id: &ExactContainerId) -> ContainerState {
            self.states
                .lock()
                .unwrap()
                .get(container_id.as_str())
                .cloned()
                .unwrap_or(ContainerState {
                    state: RuntimeState::Missing,
                    status: None,
                    restarts: 0,
                    exit_code: None,
                    health: None,
                    started_at: None,
                    finished_at: None,
                    image_id: None,
                    compose_service: None,
                })
        }

        fn start_container(&self, container_id: &ExactContainerId) -> Result<(), DockerError> {
            self.actions
                .lock()
                .unwrap()
                .push(format!("start:{container_id}"));
            Ok(())
        }

        fn stop_container(&self, container_id: &ExactContainerId) -> Result<(), DockerError> {
            self.actions
                .lock()
                .unwrap()
                .push(format!("stop:{container_id}"));
            Ok(())
        }

        fn restart_container(&self, container_id: &ExactContainerId) -> Result<(), DockerError> {
            self.actions
                .lock()
                .unwrap()
                .push(format!("restart:{container_id}"));
            Ok(())
        }

        fn container_logs(
            &self,
            container_id: &ExactContainerId,
            _tail_lines: u16,
        ) -> Result<String, DockerError> {
            Ok(format!("logs:{container_id}"))
        }

        fn compose_state(
            &self,
            _project: &str,
            _services: &[String],
            _finite_services: &[String],
            _completions: &BTreeSet<String>,
            _desired_states: &BTreeMap<String, RuntimeState>,
        ) -> Result<DockerComposeState, DockerError> {
            self.compose
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| DockerError::InvalidOutput("no compose fixture".into()))
        }
    }

    struct FakeSystemd;

    impl SystemdControl for FakeSystemd {
        fn spawn_transient(
            &self,
            _specification: &TransientUnitSpec,
        ) -> Result<Box<dyn UnitProcess>, SystemdError> {
            Err(SystemdError::Operation("unexpected spawn".into()))
        }

        fn start_persistent(
            &self,
            _specification: &PersistentUnitSpec,
        ) -> Result<(), SystemdError> {
            Err(SystemdError::Operation("unexpected start".into()))
        }

        fn process_state(&self, _unit: &str) -> Result<ProcessState, SystemdError> {
            Ok(ProcessState {
                state: "running".into(),
                active_state: "active".into(),
                sub_state: "running".into(),
                result: "success".into(),
                main_pid: 42,
                restarts: 3,
                cgroup: "/fixture".into(),
            })
        }

        fn show_unit(
            &self,
            _unit: &str,
            _properties: &[&str],
        ) -> Result<Vec<(String, String)>, SystemdError> {
            Ok(Vec::new())
        }

        fn list_matching_units(&self, _pattern: &str) -> Result<Vec<String>, SystemdError> {
            Ok(Vec::new())
        }

        fn stop_unit(&self, _unit: &str) -> Result<(), SystemdError> {
            Ok(())
        }

        fn reset_failed(&self, _unit: &str) -> Result<(), SystemdError> {
            Ok(())
        }

        fn control_group_path(&self, _unit: &str) -> Result<Option<PathBuf>, SystemdError> {
            Ok(None)
        }

        fn prove_cgroup_empty(&self, _cgroup: Option<&Path>, _deadline: Duration) -> bool {
            true
        }

        fn process_uids(&self, _pid: u32) -> Option<[u32; 4]> {
            Some([1000; 4])
        }
    }

    struct ReadyNetwork;
    impl NetworkProbe for ReadyNetwork {
        fn tcp(&self, _host: &str, _port: u16) -> bool {
            true
        }
    }

    struct World {
        _temporary: tempfile::TempDir,
        database: Database,
        deployments: Deployments,
        docker: Arc<FakeDocker>,
        worktree: PathBuf,
    }

    impl World {
        fn new() -> Self {
            let temporary = tempdir().unwrap();
            let worktree = temporary.path().join("repository");
            std::fs::create_dir(&worktree).unwrap();
            std::fs::write(worktree.join(".git"), "gitdir: /missing\n").unwrap();
            std::fs::write(
                worktree.join(".devcoordinator.toml"),
                r#"
schema=2
[deployment.web]
domain="app"
components=["api","cache","stack","smtp"]
[deployment.web.component.api]
type="process"
command=["serve"]
port=true
route=true
[deployment.web.component.cache]
type="docker"
image="cache:1"
[deployment.web.component.stack]
type="compose"
services=["bootstrap","worker"]
finite_services=["bootstrap"]
independent_services=["worker"]
[deployment.web.component.smtp]
type="external"
tcp="127.0.0.1:25"
"#,
            )
            .unwrap();
            let database = Database::open(temporary.path().join("authority.sqlite3")).unwrap();
            let root = worktree.display().to_string();
            database.transaction(move |transaction| {
                transaction.execute("INSERT INTO repositories(repository_id,root_path,display_name,registered_at,registered_by_uid,last_seen_at) VALUES('r1111111111111111',?1,'repo','t',1000,'t')",[&root])?;
                transaction.execute("INSERT INTO worktrees(worktree_id,repository_id,worktree_path,registered_at,last_seen_at) VALUES('w1111111111111111','r1111111111111111',?1,'t','t')",[&root])?;
                Ok(())
            }).unwrap();
            let config = Config {
                socket_path: temporary.path().join("daemon.sock"),
                state_dir: temporary.path().join("state"),
                unit_prefix: "devcoordinator2-test".into(),
                slice_name: "devcoordinator2-tests.slice".into(),
                client_group: "clients".into(),
                port_range: (40000, 40100),
                base_domain: "example.test".into(),
                edge_uid: None,
                admin_emails: Vec::new(),
                telegram_token_file: None,
                telegram_api: "https://api.telegram.org".into(),
                bugs_dir: temporary.path().join("bugs"),
                compose_env_allowlist_file: None,
                compose_env_authorizations: HashSet::new(),
                codex_usage_sources_file: None,
                codex_usage_sources: Vec::new(),
            };
            let docker = Arc::new(FakeDocker::new());
            let deployments = Deployments::with_adapters(
                config,
                database.clone(),
                Registry::new(database.clone()),
                docker.clone(),
                Arc::new(FakeSystemd),
                Arc::new(ReadyNetwork),
            );
            Self {
                _temporary: temporary,
                database,
                deployments,
                docker,
                worktree,
            }
        }

        fn caller() -> Caller {
            Caller {
                pid: 1,
                uid: 1000,
                gid: 1000,
                client_kind: devcoordinator2_api::ClientKind::Other,
                client_session: None,
                identity: None,
            }
        }

        fn seed_managed(&self) -> String {
            let specification = load_deployment_spec(&self.worktree, "web").unwrap();
            let deployment_id =
                DeploymentStore::deployment_id("w1111111111111111", "web", "worktree");
            self.deployments
                .store
                .upsert(
                    &deployment_id,
                    &RegisteredDeploymentTarget {
                        repository_id: "r1111111111111111".into(),
                        worktree_id: "w1111111111111111".into(),
                    },
                    &specification,
                    "worktree",
                    Some("app"),
                    "running",
                    1000,
                    "other",
                    None,
                )
                .unwrap();
            for (name, kind, identity) in [
                ("api", "unit", "api.service".to_owned()),
                ("cache", "container", "a".repeat(64)),
                ("stack", "compose", "dc2-stack".to_owned()),
            ] {
                self.deployments
                    .store
                    .set_component_runtime(
                        &deployment_id,
                        name,
                        ComponentRuntimePatch {
                            state: Some("running".into()),
                            health: Some("healthy".into()),
                            generation: Some(Some(1)),
                            binding_kind: Some(Some(kind.into())),
                            binding_identity: Some(Some(identity)),
                            ..Default::default()
                        },
                    )
                    .unwrap();
            }
            self.deployments
                .store
                .set_component_runtime(
                    &deployment_id,
                    "smtp",
                    ComponentRuntimePatch {
                        state: Some("running".into()),
                        health: Some("healthy".into()),
                        ..Default::default()
                    },
                )
                .unwrap();
            self.database.transaction({let deployment_id=deployment_id.clone(); move |transaction| {
                transaction.execute("INSERT INTO port_assignments(port,deployment_id,component,generation,assigned_at) VALUES(20001,?1,'api',1,'t')",[deployment_id])?;
                Ok(())
            }}).unwrap();
            self.deployments
                .store
                .set_route(
                    Some("app"),
                    &deployment_id,
                    Some("api"),
                    Some(20001),
                    Some(1),
                )
                .unwrap();
            self.deployments
                .store
                .set_deployment_runtime(&deployment_id, "running", Some(1), None)
                .unwrap();
            self.deployments
                .store
                .record_compose_completions(
                    &deployment_id,
                    "stack",
                    1,
                    &[ComposeCompletionInput {
                        service: "bootstrap".into(),
                        container_id: "b".repeat(64),
                        image_id: None,
                        exit_code: 0,
                        started_at: None,
                        finished_at: None,
                    }],
                )
                .unwrap();
            self.docker
                .states
                .lock()
                .unwrap()
                .insert("a".repeat(64), FakeDocker::running(None));
            *self.docker.compose.lock().unwrap() = Some(DockerComposeState {
                state: RuntimeState::Running,
                containers: 2,
                running: 1,
                services: vec![
                    ComposeServiceState {
                        name: "bootstrap".into(),
                        role: "finite".into(),
                        state: RuntimeState::Completed,
                        desired_state: RuntimeState::Running,
                        containers: 1,
                    },
                    ComposeServiceState {
                        name: "worker".into(),
                        role: "running".into(),
                        state: RuntimeState::Running,
                        desired_state: RuntimeState::Running,
                        containers: 1,
                    },
                ],
                completion_candidates: Vec::new(),
            });
            deployment_id
        }
    }

    #[test]
    fn managed_status_and_list_refresh_real_runtime_shapes() {
        let world = World::new();
        let deployment_id = world.seed_managed();
        let status = world
            .deployments
            .status(None, None, Some(&deployment_id), &World::caller())
            .unwrap();
        assert_eq!(status.state, "running");
        assert_eq!(status.domain.as_deref(), Some("app"));
        assert_eq!(status.route_port, Some(20001));
        assert!(status.log_dir.is_none());
        let api = status
            .components
            .iter()
            .find(|component| component.name == "api")
            .unwrap();
        assert_eq!(api.restarts, Some(3));
        assert_eq!(api.binding.kind.as_deref(), Some("unit"));
        let stack = status
            .components
            .iter()
            .find(|component| component.name == "stack")
            .unwrap();
        assert_eq!(stack.services.as_ref().unwrap()[0].containers, 1);
        assert_eq!(
            stack.completed_services.as_ref().unwrap()[0].service,
            "bootstrap"
        );
        let list = world
            .deployments
            .list(DeploymentListParams { path: None }, &World::caller())
            .unwrap();
        assert_eq!(list.deployments.len(), 1);
        assert_eq!(list.deployments[0].deployment_id, deployment_id);
    }

    #[test]
    fn observed_control_logs_and_routes_use_only_recorded_exact_ids() {
        let world = World::new();
        let deployment_id = "d2222222222222222";
        let container_id = "c".repeat(64);
        world
            .deployments
            .store
            .replace_observed_current(
                &[ObservedDeploymentInput {
                    deployment_id: deployment_id.into(),
                    repository_id: "r1111111111111111".into(),
                    name: "native".into(),
                    native_project: "native".into(),
                    state: "running".into(),
                    health: "healthy".into(),
                    evidence: serde_json::json!({}),
                }],
                &[ObservedContainerInput {
                    container_id: container_id.clone(),
                    deployment_id: deployment_id.into(),
                    repository_id: "r1111111111111111".into(),
                    name: "native-api".into(),
                    image: "image".into(),
                    compose_service: "api".into(),
                    status: "running".into(),
                    health: "healthy".into(),
                }],
                &[],
                "2026-09-04T00:00:00Z",
            )
            .unwrap();
        world
            .docker
            .states
            .lock()
            .unwrap()
            .insert(container_id.clone(), FakeDocker::running(Some("api")));
        let status = world
            .deployments
            .control_observed("restart", deployment_id, Some("api"))
            .unwrap();
        assert_eq!(status.state, "running");
        assert_eq!(
            world.docker.actions.lock().unwrap().as_slice(),
            [format!("restart:{container_id}")]
        );
        let logs = world
            .deployments
            .observed_logs(deployment_id, "api", 20)
            .unwrap();
        assert!(logs.tail.contains(&container_id));
        let changed = world
            .deployments
            .set_domain(SetDomain {
                deployment_id: deployment_id.into(),
                domain: Some("native".into()),
                port: Some(20002),
                component: None,
                public: Some(true),
            })
            .unwrap();
        assert_eq!(
            (changed.route_port, changed.public),
            (Some(20002), Some(true))
        );
        assert!(
            world
                .deployments
                .reject_observed_configuration(deployment_id)
                .is_err()
        );
    }

    #[test]
    fn path_resolution_and_managed_domain_rules_remain_strict() {
        let world = World::new();
        let deployment_id = world.seed_managed();
        let by_name = world.deployments.status(
            Some(world.worktree.to_str().unwrap()),
            Some("web"),
            None,
            &World::caller(),
        );
        // The fixture is deliberately not a Git repository; name resolution
        // must not bypass canonical registration.
        assert_eq!(by_name.unwrap_err().code, ErrorCode::RepositoryNotFound);
        let error = world
            .deployments
            .set_domain(SetDomain {
                deployment_id: deployment_id.clone(),
                domain: Some("other".into()),
                port: Some(1),
                component: None,
                public: None,
            })
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::ParamsInvalid);
        let changed = world
            .deployments
            .set_domain(SetDomain {
                deployment_id,
                domain: Some("other".into()),
                port: None,
                component: None,
                public: Some(true),
            })
            .unwrap();
        assert_eq!(
            (changed.domain.as_deref(), changed.public),
            (Some("other"), Some(true))
        );
    }
}
