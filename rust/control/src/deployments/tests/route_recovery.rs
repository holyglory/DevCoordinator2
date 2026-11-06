use super::*;

#[test]
fn explicit_candidate_keeps_deployment_identity_and_previous_generation() {
    let fixture = RouteWorld::new("checkout");
    let first = fixture.apply().unwrap();
    fixture.change();
    let candidate = devcoordinator2_api::params::DeploymentCandidateSource {
        path: fixture.world.worktree.to_string_lossy().into_owned(),
        commit: "b".repeat(40),
    };
    let applied = fixture
        .deployments
        .apply_with_candidate(
            None,
            None,
            Some(&first.deployment_id),
            Some(&candidate),
            &fixture.caller,
        )
        .unwrap();
    assert_eq!(applied.deployment_id, first.deployment_id);
    assert_eq!(applied.current_generation, Some(2));
    assert_eq!(applied.previous_generation, Some(1));
    assert_eq!(applied.domain, first.domain);
    let repeated = fixture
        .deployments
        .apply_with_candidate(
            None,
            None,
            Some(&first.deployment_id),
            Some(&candidate),
            &fixture.caller,
        )
        .unwrap();
    assert_eq!(repeated.current_generation, Some(2));
}

#[test]
fn invalid_candidate_source_never_changes_the_running_deployment() {
    for failure in [
        "dirty",
        "commit",
        "repository",
        "worktree",
        "missing-target",
    ] {
        let fixture = RouteWorld::new(if failure == "worktree" {
            "worktree"
        } else {
            "checkout"
        });
        let first = fixture.apply().unwrap();
        let foreign = RouteWorld::new("checkout");
        fixture.change();
        if failure == "dirty" {
            fixture.git.snapshot.lock().unwrap().dirty = true;
        }
        let candidate = devcoordinator2_api::params::DeploymentCandidateSource {
            path: (if failure == "repository" {
                &foreign.world.worktree
            } else {
                &fixture.world.worktree
            })
            .to_string_lossy()
            .into_owned(),
            commit: if failure == "commit" {
                "c".repeat(40)
            } else {
                "b".repeat(40)
            },
        };
        let before = fixture.git.actions.lock().unwrap().len();
        let target = if failure == "missing-target" {
            None
        } else {
            Some(first.deployment_id.as_str())
        };
        assert!(
            fixture
                .deployments
                .apply_with_candidate(None, None, target, Some(&candidate), &fixture.caller)
                .is_err(),
            "{failure}"
        );
        assert_eq!(
            fixture.git.actions.lock().unwrap().len(),
            before,
            "{failure}"
        );
        let after = fixture.status(&first.deployment_id);
        assert_eq!(
            after.current_generation, first.current_generation,
            "{failure}"
        );
        assert_eq!(after.route_port, first.route_port, "{failure}");
    }
}

struct RouteWorld {
    world: World,
    deployments: Deployments,
    git: Arc<CheckoutGit>,
    systemd: Arc<MutationSystemd>,
    caller: Caller,
}

impl RouteWorld {
    fn new(source: &str) -> Self {
        let mut world = World::new();
        // Let the real registry assign identities; World::new seeds synthetic IDs.
        world.database =
            Database::open(world._temporary.path().join("route-authority.sqlite3")).unwrap();
        // Replace only the deliberately invalid Git marker of this disposable fixture.
        std::fs::remove_file(world.worktree.join(".git")).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "--quiet"])
                .current_dir(&world.worktree)
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(
            world.worktree.join(".devcoordinator.toml"),
            format!(
                r#"
schema=2
[deployment.web]
source="{source}"
domain="app"
components=["api"]
[deployment.web.component.api]
type="process"
command=["serve"]
port=true
route=true
"#
            ),
        )
        .unwrap();
        let config = (*world.deployments.config).clone();
        let git = Arc::new(CheckoutGit::new());
        let systemd = Arc::new(MutationSystemd::new());
        let deployments = Deployments::with_runtime_adapters(
            config.clone(),
            world.database.clone(),
            Registry::new(world.database.clone()),
            Arc::new(MutationDocker::new()),
            systemd.clone(),
            Arc::new(ReadyNetwork),
            git.clone(),
            Arc::new(FixtureHealth),
            DeploymentFiles::new(config.deployments_dir(), config.secrets_dir()),
            Arc::new(FixturePorts),
            Arc::new(HostClock),
        );
        let mut caller = World::caller();
        caller.uid = rustix::process::getuid().as_raw();
        caller.gid = rustix::process::getgid().as_raw();
        assert_ne!(caller.uid, 0);
        Self {
            world,
            deployments,
            git,
            systemd,
            caller,
        }
    }

    fn apply(&self) -> Result<DeploymentStatus, ProtocolError> {
        self.deployments.apply(
            Some(self.world.worktree.to_str().unwrap()),
            Some("web"),
            None,
            &self.caller,
        )
    }

    fn change(&self) {
        self.git.snapshot.lock().unwrap().commit = Some("b".repeat(40));
    }

    fn route(&self) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(self.deployments.config.routes_path()).unwrap())
            .unwrap()
    }

    fn status(&self, id: &str) -> DeploymentStatus {
        self.deployments
            .status(None, None, Some(id), &self.caller)
            .unwrap()
    }
}

#[test]
fn failed_worktree_candidate_is_not_reused_when_its_unit_is_still_running() {
    let fixture = RouteWorld::new("worktree");
    let first = fixture.apply().unwrap();
    let initial_route = fixture.route();
    let occupied = first.components[0]
        .binding
        .identity
        .as_ref()
        .unwrap()
        .replace("-g1.service", "-g2.service");
    assert!(occupied.ends_with("-g2.service"));
    fixture
        .systemd
        .states
        .lock()
        .unwrap()
        .insert(occupied.clone(), "running".into());
    fixture
        .systemd
        .reject_running_starts
        .store(true, Ordering::SeqCst);
    fixture.change();
    let failure = fixture.apply().unwrap_err();
    assert!(
        failure
            .message
            .contains("cannot recreate a non-terminal unit")
    );
    let failed = fixture.status(&first.deployment_id);
    assert_eq!(failed.current_generation, Some(1));
    assert_eq!(failed.components[0].state, "running");
    assert!(!failed.readiness.unwrap().ready);
    assert_eq!(fixture.route()["routes"], initial_route["routes"]);
    let retained = fixture
        .deployments
        .store
        .generations(&first.deployment_id)
        .unwrap();
    assert!(
        retained
            .iter()
            .any(|row| row.number == 2 && row.state == "failed")
    );
    let recovered = fixture.apply().unwrap();
    assert_eq!(recovered.current_generation, Some(3));
    assert_eq!(recovered.route_port, first.route_port);
    assert!(recovered.readiness.unwrap().ready);
    assert_eq!(
        fixture.route()["routes"][0]["lease_id"],
        initial_route["routes"][0]["lease_id"]
    );
    assert_eq!(
        fixture
            .systemd
            .states
            .lock()
            .unwrap()
            .get(&occupied)
            .map(String::as_str),
        Some("running")
    );
}

#[test]
fn publication_during_process_transition_does_not_abort_candidate() {
    for source in ["worktree", "checkout"] {
        let fixture = RouteWorld::new(source);
        let first = fixture.apply().unwrap();
        let lease = fixture.route()["routes"][0]["lease_id"].clone();
        fixture.change();
        let publisher = fixture.deployments.routes.clone();
        let database = fixture.world.database.clone();
        let id = first.deployment_id.clone();
        *fixture.systemd.listener_hook.lock().unwrap() = Some(Box::new(move || {
            // A background publisher observes healthy g2 beside the still-selected g1 route.
            let document = publisher.publish_current().unwrap();
            assert!(
                document
                    .routes
                    .iter()
                    .all(|route| route.deployment_id != id)
            );
            let store = DeploymentStore::new(database);
            assert_eq!(store.get(&id).unwrap().unwrap().state, "applying");
        }));
        let second = fixture.apply().unwrap();
        assert_eq!(second.current_generation, Some(2));
        assert_eq!(second.components[0].generation, Some(2));
        assert_eq!(second.route_port, first.route_port);
        assert!(second.readiness.unwrap().ready);
        let route = fixture.route();
        assert_eq!(route["routes"][0]["lease_id"], lease);
        assert_eq!(route["routes"][0]["generation"], 2);
    }
}

#[test]
fn publication_failure_restores_previous_runtime_and_checkout_can_retry() {
    let fixture = RouteWorld::new("checkout");
    let first = fixture.apply().unwrap();
    let original = fixture.route();
    let route_path = fixture.deployments.config.routes_path();
    fixture.change();
    let blocked_path = route_path.clone();
    *fixture.systemd.listener_hook.lock().unwrap() = Some(Box::new(move || {
        std::fs::rename(&blocked_path, blocked_path.with_extension("retained")).unwrap();
        std::fs::create_dir(&blocked_path).unwrap();
    }));
    let error = fixture.apply().unwrap_err();
    assert_eq!(error.code, ErrorCode::DeploymentApplyFailed);
    let failed = fixture.status(&first.deployment_id);
    assert_eq!(failed.current_generation, Some(1));
    assert_eq!(failed.components[0].generation, Some(1));
    assert_eq!(failed.components[0].state, "running");
    assert!(!failed.readiness.unwrap().ready);
    // Recovery of the injected private output fault; no live route file is involved.
    std::fs::remove_dir(&route_path).unwrap();
    std::fs::rename(route_path.with_extension("retained"), &route_path).unwrap();
    fixture.deployments.reconcile_routes().unwrap();
    assert_eq!(fixture.route()["routes"], original["routes"]);
    let recovered = fixture.apply().unwrap();
    assert_eq!(recovered.route_port, first.route_port);
    assert!(recovered.current_generation.unwrap() > 1);
    assert!(recovered.readiness.unwrap().ready);
    assert_eq!(
        fixture.route()["routes"][0]["lease_id"],
        original["routes"][0]["lease_id"]
    );
}

#[test]
fn retained_uncommitted_checkout_generation_is_not_reused() {
    let fixture = RouteWorld::new("checkout");
    let first = fixture.apply().unwrap();
    let row = fixture
        .deployments
        .store
        .get(&first.deployment_id)
        .unwrap()
        .unwrap();
    let orphan = fixture
        .deployments
        .files
        .generation_path(&first.deployment_id, 2)
        .unwrap();
    std::fs::create_dir(&orphan).unwrap();
    std::fs::write(orphan.join("retained-evidence"), "failed publication").unwrap();
    fixture
        .deployments
        .store
        .add_generation(
            &first.deployment_id,
            2,
            Some(&"b".repeat(40)),
            false,
            &orphan,
            &row.spec_fingerprint,
        )
        .unwrap();
    fixture.change();
    let recovered = fixture.apply().unwrap();
    assert_eq!(recovered.current_generation, Some(3));
    assert!(recovered.readiness.unwrap().ready);
}

#[test]
fn healthy_process_without_its_route_is_not_ready() {
    let fixture = RouteWorld::new("checkout");
    let first = fixture.apply().unwrap();
    let id = first.deployment_id.clone();
    fixture
        .world
        .database
        .transaction(move |connection| crate::ports::withdraw_conflict(connection, &id, "api"))
        .unwrap();
    let status = fixture.status(&first.deployment_id);
    assert_eq!(status.components[0].state, "running");
    assert!(status.route_port.is_none());
    let readiness = status.readiness.unwrap();
    assert!(!readiness.ready);
    let repair = readiness
        .next_actions
        .iter()
        .find(|action| action.id == "repair-route")
        .expect("current-source route withdrawal must be actionable");
    assert_eq!(repair.operation.as_deref(), Some("deployment.apply"));
    assert_eq!(repair.target.as_deref(), Some(first.deployment_id.as_str()));
    let recovered = fixture.apply().unwrap();
    assert!(recovered.readiness.unwrap().ready);
    assert_eq!(recovered.route_port, first.route_port);
}

#[test]
fn reconcile_recovers_missing_saved_process_on_original_route_lease() {
    let fixture = RouteWorld::new("checkout");
    let first = fixture.apply().unwrap();
    let starts_before_reconcile = fixture
        .systemd
        .actions
        .lock()
        .unwrap()
        .iter()
        .filter(|action| action.starts_with("start:"))
        .count();
    fixture.deployments.reconcile_routes().unwrap();
    assert_eq!(
        fixture
            .systemd
            .actions
            .lock()
            .unwrap()
            .iter()
            .filter(|action| action.starts_with("start:"))
            .count(),
        starts_before_reconcile,
        "healthy same-generation unit was restarted",
    );
    let initial = fixture.route();
    let unit = first.components[0].binding.identity.clone().unwrap();
    fixture.systemd.states.lock().unwrap().remove(&unit);
    fixture
        .world
        .database
        .transaction({
            let id = first.deployment_id.clone();
            move |connection| crate::ports::withdraw_conflict(connection, &id, "api")
        })
        .unwrap();
    fixture.deployments.reconcile_routes().unwrap();
    let recovered = fixture.status(&first.deployment_id);
    assert_eq!(recovered.state, "running");
    assert_eq!(recovered.current_generation, first.current_generation);
    assert_eq!(recovered.route_port, first.route_port);
    assert_eq!(
        recovered.components[0].binding.identity.as_deref(),
        Some(unit.as_str())
    );
    assert_eq!(
        fixture.route()["routes"][0]["lease_id"],
        initial["routes"][0]["lease_id"]
    );
    assert!(
        fixture
            .systemd
            .actions
            .lock()
            .unwrap()
            .iter()
            .any(|action| action == &format!("start:{unit}"))
    );

    // Hold the real reconciliation path inside live listener validation. A
    // healthy status can already be visible here; the next mutation must hand
    // off asynchronously rather than report a foreground conflict.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        *fixture.systemd.listener_hook.lock().unwrap() = Some(Box::new(move || {
            entered_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }));
        let service = fixture.deployments.clone();
        let reconciliation = tokio::task::spawn_blocking(move || service.reconcile_routes());
        tokio::time::timeout(Duration::from_secs(5), entered_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(
            fixture
                .status(&first.deployment_id)
                .readiness
                .unwrap()
                .ready
        );
        let admission = fixture
            .deployments
            .prepare_mutation_admission(None, None, Some(&first.deployment_id), &fixture.caller)
            .unwrap()
            .unwrap();
        let crate::daemon::PreparedAdmission::Waiting(wait) = admission else {
            panic!("maintenance was not held by the reconciliation hook");
        };
        let admitted = tokio::spawn(wait);
        release_tx.send(()).unwrap();
        reconciliation.await.unwrap().unwrap();
        let hold = admitted.await.unwrap().unwrap();
        assert!(
            fixture
                .deployments
                .acquire_mutation(&first.deployment_id, MutationOwner::Maintenance,)
                .is_err(),
            "maintenance reacquired ahead of the admitted foreground mutation"
        );
        let restarted = fixture
            .deployments
            .control(
                "restart",
                None,
                None,
                Some(&first.deployment_id),
                Some("api"),
                &fixture.caller,
            )
            .unwrap();
        assert_eq!(restarted.current_generation, first.current_generation);
        assert_eq!(restarted.route_port, first.route_port);
        drop(hold);
        assert!(fixture.deployments.busy.entries.lock().unwrap().is_empty());

        // A true foreground overlap remains Busy, and even a rejected execution
        // releases the intent that kept maintenance from barging in.
        let foreground = fixture
            .deployments
            .acquire_busy(&first.deployment_id)
            .unwrap();
        let crate::daemon::PreparedAdmission::Ready(hold) = fixture
            .deployments
            .foreground_admission(&first.deployment_id, Duration::from_secs(15))
        else {
            panic!("foreground conflict was turned into a maintenance wait");
        };
        assert_eq!(
            fixture
                .deployments
                .control(
                    "restart",
                    None,
                    None,
                    Some(&first.deployment_id),
                    Some("api"),
                    &fixture.caller,
                )
                .unwrap_err()
                .code,
            ErrorCode::Busy
        );
        drop(hold);
        drop(foreground);
        assert!(fixture.deployments.busy.entries.lock().unwrap().is_empty());

        let maintenance = crate::database::maintenance_context(|| {
            fixture.deployments.acquire_busy(&first.deployment_id)
        })
        .unwrap();
        let crate::daemon::PreparedAdmission::Waiting(wait) = fixture
            .deployments
            .foreground_admission(&first.deployment_id, Duration::ZERO)
        else {
            panic!("missing maintenance wait");
        };
        let error = match wait.await {
            Ok(_) => panic!("held maintenance was admitted"),
            Err(error) => error,
        };
        assert_eq!(error.code, ErrorCode::Busy);
        assert!(error.message.contains("mutation was not started"));
        let crate::daemon::PreparedAdmission::Waiting(cancelled) = fixture
            .deployments
            .foreground_admission(&first.deployment_id, Duration::from_secs(15))
        else {
            panic!("missing cancellable maintenance wait");
        };
        let cancelled = tokio::spawn(cancelled);
        cancelled.abort();
        let Err(cancelled) = cancelled.await else {
            panic!("cancelled admission completed");
        };
        assert!(cancelled.is_cancelled());
        assert_eq!(
            fixture.deployments.busy.entries.lock().unwrap()[&first.deployment_id].pending,
            0
        );
        drop(maintenance);
        assert!(fixture.deployments.busy.entries.lock().unwrap().is_empty());
    });
}

#[test]
fn reconcile_does_not_restart_stopped_or_repair_a_mismatched_route_lease() {
    let fixture = RouteWorld::new("checkout");
    let first = fixture.apply().unwrap();
    fixture
        .deployments
        .control(
            "stop",
            None,
            None,
            Some(&first.deployment_id),
            None,
            &fixture.caller,
        )
        .unwrap();
    let starts = fixture
        .systemd
        .actions
        .lock()
        .unwrap()
        .iter()
        .filter(|action| action.starts_with("start:"))
        .count();
    fixture.deployments.reconcile_routes().unwrap();
    assert_eq!(
        fixture
            .systemd
            .actions
            .lock()
            .unwrap()
            .iter()
            .filter(|action| action.starts_with("start:"))
            .count(),
        starts
    );

    let fixture = RouteWorld::new("checkout");
    let first = fixture.apply().unwrap();
    let mismatch_starts = fixture
        .systemd
        .actions
        .lock()
        .unwrap()
        .iter()
        .filter(|action| action.starts_with("start:"))
        .count();
    fixture
        .world
        .database
        .transaction({
            let id = first.deployment_id.clone();
            move |connection| {
                connection.execute(
                    "UPDATE domain_routes SET lease_id='foreign-lease' WHERE deployment_id=$1",
                    [&id],
                )?;
                Ok(())
            }
        })
        .unwrap();
    fixture
        .systemd
        .states
        .lock()
        .unwrap()
        .remove(&first.components[0].binding.identity.clone().unwrap());
    fixture.deployments.reconcile_routes().unwrap();
    assert_eq!(
        fixture
            .systemd
            .actions
            .lock()
            .unwrap()
            .iter()
            .filter(|action| action.starts_with("start:"))
            .count(),
        mismatch_starts
    );
}

#[test]
fn first_publication_failure_can_retry_without_a_previous_generation() {
    let fixture = RouteWorld::new("checkout");
    let route_path = fixture.deployments.config.routes_path();
    std::fs::create_dir_all(&route_path).unwrap();
    let error = fixture.apply().unwrap_err();
    assert_eq!(error.code, ErrorCode::DeploymentApplyFailed);
    let rows = fixture.deployments.store.list(None).unwrap();
    let row = rows.first().unwrap();
    let failed = fixture.status(&row.deployment_id);
    assert!(failed.current_generation.is_none());
    assert!(failed.route_port.is_none());
    assert!(!failed.readiness.unwrap().ready);
    std::fs::remove_dir(&route_path).unwrap();
    assert!(fixture.apply().unwrap().readiness.unwrap().ready);
}

#[test]
fn unrecorded_checkout_path_is_preserved_and_generation_mismatch_is_not_ready() {
    let fixture = RouteWorld::new("checkout");
    let first = fixture.apply().unwrap();
    let orphan = fixture
        .deployments
        .files
        .generation_path(&first.deployment_id, 2)
        .unwrap();
    std::fs::create_dir(&orphan).unwrap();
    std::fs::write(orphan.join("retained-evidence"), "interrupted preparation").unwrap();
    fixture.change();
    let recovered = fixture.apply().unwrap();
    assert_eq!(recovered.current_generation, Some(3));
    assert!(orphan.join("retained-evidence").is_file());
    fixture
        .deployments
        .store
        .set_component_runtime(
            &first.deployment_id,
            "api",
            ComponentRuntimePatch {
                generation: Some(Some(4)),
                ..Default::default()
            },
        )
        .unwrap();
    let status = fixture.status(&first.deployment_id);
    assert!(!status.readiness.unwrap().ready);
    let published = fixture.deployments.routes.publish_current().unwrap();
    assert!(published.routes.is_empty());
}
