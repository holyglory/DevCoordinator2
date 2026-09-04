use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use devcoordinator2_api::params::{RetryTest, StartTest, ValidationTier as ApiValidationTier};
use devcoordinator2_api::results::{ProofKind as ApiProofKind, StopTest, TestStarted, TestStatus};
use devcoordinator2_control::access::Caller;
use devcoordinator2_control::capacity::CapacityBroker;
use devcoordinator2_control::config::Config;
use devcoordinator2_control::database::Database;
use devcoordinator2_control::docker::{
    DockerControl, DockerError, DockerInvocation, DockerOutput, ExactContainerId, LogFollower,
    RunDetachedRequest,
};
use devcoordinator2_control::platform::{Clock, MonotonicClock, RandomSource};
use devcoordinator2_control::repository::Registry;
use devcoordinator2_control::systemd::{
    PersistentUnitSpec, ProcessState, SystemdControl, SystemdError, TransientUnitSpec, UnitProcess,
};
use devcoordinator2_control::test_command::{TestCommand, TestCommandError};
use devcoordinator2_control::test_lifecycle::{TestLifecycle, TestLifecycleEvent};
use devcoordinator2_control::test_logs::TestLogService;
use devcoordinator2_control::test_state::{REPORT_FILE, TestRunStore, initial_summary};
use devcoordinator2_executor_protocol::{
    ArtifactReceipt, CapacityReport, CheckReport, DiagnosticExit, ExecutionPlan, ExecutionReport,
    LeafStatus, ProofKind, RunStatus, Schema2, ValidationTier,
};
use std::os::unix::process::ExitStatusExt;
use tempfile::tempdir;
use time::{OffsetDateTime, macros::datetime};

struct FixtureClock;

impl Clock for FixtureClock {
    fn now_utc(&self) -> OffsetDateTime {
        datetime!(2026-09-04 00:00 UTC)
    }
}

struct FixtureMonotonic {
    started: Instant,
}

impl FixtureMonotonic {
    fn new() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}

impl MonotonicClock for FixtureMonotonic {
    fn seconds(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 100.0
    }
}

struct SequenceRandom(AtomicU64);

impl RandomSource for SequenceRandom {
    fn fill(&self, destination: &mut [u8]) -> Result<(), getrandom::Error> {
        let value = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        for (index, byte) in destination.iter_mut().enumerate() {
            *byte = value.wrapping_add(index as u64) as u8;
        }
        Ok(())
    }
}

struct FixtureCommand;

impl TestCommand for FixtureCommand {
    fn source_digest(
        &self,
        _executor: &Path,
        _worktree: &Path,
        _uid: u32,
        _gid: u32,
    ) -> Result<String, TestCommandError> {
        Ok("a".repeat(64))
    }

    fn receipts_match(
        &self,
        _executor: &Path,
        _worktree: &Path,
        _receipts: &[ArtifactReceipt],
        _uid: u32,
        _gid: u32,
    ) -> Result<bool, TestCommandError> {
        Ok(true)
    }

    fn executable(&self, _candidate: &Path, _uid: u32, _gid: u32) -> bool {
        true
    }

    fn executor_available(&self, _executor: &Path) -> bool {
        true
    }
}

struct FixtureDocker;

impl DockerControl for FixtureDocker {
    fn invoke(&self, _invocation: DockerInvocation) -> Result<DockerOutput, DockerError> {
        Err(DockerError::InvalidRequest("unexpected Docker call".into()))
    }

    fn spawn_follow_logs(
        &self,
        _container_id: &ExactContainerId,
    ) -> Result<LogFollower, DockerError> {
        Err(DockerError::InvalidRequest("unexpected Docker logs".into()))
    }

    fn list_ids_by_labels(
        &self,
        _labels: &BTreeMap<String, String>,
    ) -> Result<Vec<ExactContainerId>, DockerError> {
        Ok(Vec::new())
    }
}

struct PostgresDocker {
    removed: AtomicU64,
    provisioned: AtomicU64,
}

impl PostgresDocker {
    fn new() -> Self {
        Self {
            removed: AtomicU64::new(0),
            provisioned: AtomicU64::new(0),
        }
    }
}

impl DockerControl for PostgresDocker {
    fn invoke(&self, _invocation: DockerInvocation) -> Result<DockerOutput, DockerError> {
        Err(DockerError::InvalidRequest("unexpected Docker call".into()))
    }

    fn spawn_follow_logs(
        &self,
        _container_id: &ExactContainerId,
    ) -> Result<LogFollower, DockerError> {
        Err(DockerError::InvalidRequest(
            "high-level readiness fixture expected".into(),
        ))
    }

    fn available(&self) -> bool {
        true
    }

    fn ensure_digest_image(&self, _image: &str) -> Result<(), DockerError> {
        Ok(())
    }

    fn run_detached(&self, request: &RunDetachedRequest) -> Result<ExactContainerId, DockerError> {
        assert_eq!(
            request.env_names,
            ["POSTGRES_USER", "POSTGRES_PASSWORD", "POSTGRES_DB"]
        );
        assert_eq!(request.publish, ["127.0.0.1::5432"]);
        assert_eq!(request.tmpfs.len(), 1);
        assert_eq!(request.label_context.purpose, "test");
        assert_eq!(request.label_context.data_class, "disposable");
        self.provisioned.fetch_add(1, Ordering::SeqCst);
        ExactContainerId::parse("d".repeat(64))
    }

    fn published_host_port(
        &self,
        _container_id: &ExactContainerId,
        _container_port: &str,
    ) -> Result<u16, DockerError> {
        Ok(45_432)
    }

    fn wait_postgres_ready(
        &self,
        _container_id: &ExactContainerId,
        user: &str,
        database: &str,
        _timeout: Duration,
    ) -> Result<(), DockerError> {
        assert_eq!((user, database), ("app", "app_test"));
        Ok(())
    }

    fn list_ids_by_labels(
        &self,
        _labels: &BTreeMap<String, String>,
    ) -> Result<Vec<ExactContainerId>, DockerError> {
        Ok(Vec::new())
    }

    fn remove_container(
        &self,
        _container_id: &ExactContainerId,
        delete_volumes: bool,
    ) -> Result<(), DockerError> {
        assert!(delete_volumes);
        self.removed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct FixtureProcessState {
    done: bool,
    exit: i32,
    uid: u32,
}

type SharedFixtureProcess = Arc<(Mutex<FixtureProcessState>, Condvar)>;

struct FixtureProcess {
    state: SharedFixtureProcess,
    stdout: Option<Box<dyn Read + Send>>,
    stderr: Option<Box<dyn Read + Send>>,
}

struct ErrorReader;

impl Read for ErrorReader {
    fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("fixture capture failure"))
    }
}

impl UnitProcess for FixtureProcess {
    fn id(&self) -> u32 {
        500
    }

    fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>> {
        self.stdout.take()
    }

    fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        self.stderr.take()
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let state = self.state.0.lock().unwrap();
        Ok(state.done.then(|| ExitStatus::from_raw(state.exit << 8)))
    }

    fn wait(&mut self) -> io::Result<ExitStatus> {
        let state = self.state.0.lock().unwrap();
        let state = self.state.1.wait_while(state, |state| !state.done).unwrap();
        Ok(ExitStatus::from_raw(state.exit << 8))
    }
}

struct FixtureSystemd {
    runs: Mutex<HashMap<String, SharedFixtureProcess>>,
    next_exit: AtomicI32,
    starts: AtomicU64,
    stops: AtomicU64,
    fail_spawn: AtomicBool,
    fail_capture: AtomicBool,
    mismatch_uid: AtomicBool,
}

impl FixtureSystemd {
    fn new() -> Self {
        Self {
            runs: Mutex::new(HashMap::new()),
            next_exit: AtomicI32::new(0),
            starts: AtomicU64::new(0),
            stops: AtomicU64::new(0),
            fail_spawn: AtomicBool::new(false),
            fail_capture: AtomicBool::new(false),
            mismatch_uid: AtomicBool::new(false),
        }
    }

    fn finish(&self, unit: &str) {
        let state = self.runs.lock().unwrap().get(unit).cloned().unwrap();
        let mut current = state.0.lock().unwrap();
        current.done = true;
        drop(current);
        state.1.notify_all();
    }

    fn seed_unit(&self, unit: &str, uid: u32) {
        self.runs.lock().unwrap().insert(
            unit.into(),
            Arc::new((
                Mutex::new(FixtureProcessState {
                    done: false,
                    exit: 1,
                    uid,
                }),
                Condvar::new(),
            )),
        );
    }

    fn write_report(specification: &TransientUnitSpec, exit: i32) {
        let plan_path = PathBuf::from(&specification.command[2]);
        let plan = ExecutionPlan::from_json(&std::fs::read(plan_path).unwrap()).unwrap();
        let passed = exit == 0;
        let leaf = if passed {
            LeafStatus::Passed
        } else {
            LeafStatus::Failed
        };
        let selected_status = if passed { "passed" } else { "failed" };
        let mut counts = BTreeMap::new();
        for name in [
            "pending",
            "running",
            "reused",
            "passed",
            "failed",
            "timed_out",
            "invalidated",
            "not_meaningful",
            "cancelled",
            "unsafe",
        ] {
            counts.insert(name.into(), u32::from(name == selected_status));
        }
        let checks = plan
            .checks
            .iter()
            .map(|check| CheckReport {
                name: check.name.clone(),
                tier: check.tier,
                role: check.role,
                status: leaf,
                started_at: Some("2026-09-04T00:00:00Z".into()),
                finished_at: Some("2026-09-04T00:00:01Z".into()),
                duration_seconds: Some(1.0),
                exit: DiagnosticExit {
                    code: Some(exit),
                    signal: None,
                },
                artifacts: Vec::new(),
                retained_artifacts: Vec::new(),
                streams: Vec::new(),
                case_count: 0,
                cases: Vec::new(),
                cases_truncated: false,
            })
            .collect();
        let current_dir = plan.current_dir.clone();
        let report = ExecutionReport {
            schema: Schema2,
            run_id: plan.run_id,
            test: plan.test,
            requested_tier: plan.requested_tier,
            readiness_eligible: plan.readiness_eligible,
            proof: plan.proof,
            selection: plan.selection,
            origin_run_id: plan.origin_run_id,
            status: if passed {
                RunStatus::Passed
            } else {
                RunStatus::Failed
            },
            started_at: "2026-09-04T00:00:00Z".into(),
            finished_at: Some("2026-09-04T00:00:01Z".into()),
            duration_seconds: 1.0,
            source_digest: plan.source_digest,
            config_digest: plan.config_digest,
            source_changed: false,
            capacity: CapacityReport::default(),
            counts,
            checks,
            failure_index: Vec::new(),
            failure_index_truncated: false,
        };
        std::fs::write(
            Path::new(&current_dir).join(REPORT_FILE),
            serde_json::to_vec(&report).unwrap(),
        )
        .unwrap();
    }
}

impl SystemdControl for FixtureSystemd {
    fn spawn_transient(
        &self,
        specification: &TransientUnitSpec,
    ) -> Result<Box<dyn UnitProcess>, SystemdError> {
        if self.fail_spawn.load(Ordering::SeqCst) {
            return Err(SystemdError::Operation("fixture spawn failure".into()));
        }
        let exit = self.next_exit.swap(0, Ordering::SeqCst);
        Self::write_report(specification, exit);
        let state = Arc::new((
            Mutex::new(FixtureProcessState {
                done: false,
                exit,
                uid: specification.uid,
            }),
            Condvar::new(),
        ));
        self.runs
            .lock()
            .unwrap()
            .insert(specification.unit.clone(), Arc::clone(&state));
        self.starts.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FixtureProcess {
            state,
            stdout: Some(if self.fail_capture.load(Ordering::SeqCst) {
                Box::new(ErrorReader) as Box<dyn Read + Send>
            } else {
                Box::new(Cursor::new(b"wrapper out\n".to_vec())) as Box<dyn Read + Send>
            }),
            stderr: Some(Box::new(Cursor::new(b"wrapper err\n".to_vec()))),
        }))
    }

    fn start_persistent(&self, _specification: &PersistentUnitSpec) -> Result<(), SystemdError> {
        Err(SystemdError::Operation("unexpected persistent unit".into()))
    }

    fn process_state(&self, unit: &str) -> Result<ProcessState, SystemdError> {
        let state = self.runs.lock().unwrap().get(unit).cloned();
        let running = state
            .as_ref()
            .is_some_and(|state| !state.0.lock().unwrap().done);
        Ok(ProcessState {
            state: if running { "running" } else { "stopped" }.into(),
            active_state: if running { "active" } else { "inactive" }.into(),
            sub_state: "fixture".into(),
            result: if running { "" } else { "success" }.into(),
            main_pid: u32::from(running) * 500,
            restarts: 0,
            cgroup: "/fixture".into(),
        })
    }

    fn show_unit(
        &self,
        unit: &str,
        properties: &[&str],
    ) -> Result<Vec<(String, String)>, SystemdError> {
        let state = self.runs.lock().unwrap().get(unit).cloned();
        let (running, exit) = state.as_ref().map_or((false, 0), |state| {
            let state = state.0.lock().unwrap();
            (!state.done, state.exit)
        });
        Ok(properties
            .iter()
            .map(|name| {
                let value = match *name {
                    "ActiveState" if running => "active".into(),
                    "ActiveState" => "inactive".into(),
                    "MainPID" if running => "500".into(),
                    "MainPID" => "0".into(),
                    "ExecMainStatus" => exit.to_string(),
                    "Result" if running => String::new(),
                    "Result" => "success".into(),
                    _ => String::new(),
                };
                ((*name).into(), value)
            })
            .collect())
    }

    fn list_matching_units(&self, _pattern: &str) -> Result<Vec<String>, SystemdError> {
        Ok(self
            .runs
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, state)| !state.0.lock().unwrap().done)
            .map(|(unit, _)| unit.clone())
            .collect())
    }

    fn stop_unit(&self, unit: &str) -> Result<(), SystemdError> {
        self.stops.fetch_add(1, Ordering::SeqCst);
        if let Some(state) = self.runs.lock().unwrap().get(unit).cloned() {
            let mut current = state.0.lock().unwrap();
            current.done = true;
            current.exit = 1;
            drop(current);
            state.1.notify_all();
        }
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

    fn process_uids(&self, pid: u32) -> Option<[u32; 4]> {
        self.runs.lock().unwrap().values().find_map(|state| {
            let state = state.0.lock().unwrap();
            (!state.done && pid == 500).then_some(
                [if self.mismatch_uid.load(Ordering::SeqCst) {
                    state.uid.saturating_add(1)
                } else {
                    state.uid
                }; 4],
            )
        })
    }
}

struct LifecycleWorld {
    _temporary: tempfile::TempDir,
    worktree: PathBuf,
    lifecycle: TestLifecycle,
    systemd: Arc<FixtureSystemd>,
    events: Arc<Mutex<Vec<TestLifecycleEvent>>>,
    caller: Caller,
}

impl LifecycleWorld {
    fn new() -> Self {
        let temporary = tempdir().unwrap();
        let worktree = temporary.path().join("repository");
        std::fs::create_dir(&worktree).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "--quiet"])
                .current_dir(&worktree)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", "/nonexistent")
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(
            worktree.join(".devcoordinator.toml"),
            r#"
schema=2
[test.all]
[[test.all.check]]
name="unit"
tier="release"
command=["true"]
"#,
        )
        .unwrap();
        let state = temporary.path().join("state");
        let runtime = temporary.path().join("runtime");
        std::fs::create_dir(&state).unwrap();
        std::fs::create_dir(&runtime).unwrap();
        let database = Database::open(state.join("authority.sqlite3")).unwrap();
        let config = Config {
            socket_path: runtime.join("daemon.sock"),
            state_dir: state,
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
        let registry = Registry::new(database.clone());
        let capacity =
            CapacityBroker::new(database.clone(), config.capacity_socket_path()).unwrap();
        let logs = TestLogService::new(database.clone(), registry.clone());
        let systemd = Arc::new(FixtureSystemd::new());
        let lifecycle = TestLifecycle::with_adapters(
            config,
            database,
            registry,
            capacity,
            logs,
            systemd.clone(),
            Arc::new(FixtureDocker),
            Arc::new(FixtureCommand),
            TestRunStore,
            Arc::new(FixtureClock),
            Arc::new(FixtureMonotonic::new()),
            Arc::new(SequenceRandom(AtomicU64::new(0))),
            PathBuf::from("/fixture/devcoordinator2-executor"),
        )
        .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&events);
        lifecycle.set_event_sink(Arc::new(move |event| captured.lock().unwrap().push(event)));
        let caller = Caller {
            pid: 1,
            uid: rustix::process::getuid().as_raw(),
            gid: rustix::process::getgid().as_raw(),
            client_kind: devcoordinator2_api::ClientKind::Codex,
            client_session: Some("fixture".into()),
            identity: None,
        };
        assert_ne!(caller.uid, 0, "lifecycle fixture requires non-root caller");
        Self {
            _temporary: temporary,
            worktree,
            lifecycle,
            systemd,
            events,
            caller,
        }
    }

    fn start(&self) -> TestStarted {
        self.lifecycle
            .start(
                StartTest {
                    path: self.worktree.to_string_lossy().into_owned(),
                    test: Some("all".into()),
                    checks: Vec::new(),
                    tier: ApiValidationTier::Release,
                },
                &self.caller,
            )
            .unwrap()
    }

    fn wait_status(&self, expected: TestStatus) -> devcoordinator2_api::results::TestSummary {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let status = self
                .lifecycle
                .status(self.worktree.to_str().unwrap(), &self.caller)
                .unwrap();
            if status.status == expected {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "status did not become {expected:?}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn lifecycle_completes_cancels_supersedes_lists_and_retries() {
    let world = LifecycleWorld::new();
    let first = world.start();
    let running = world
        .lifecycle
        .status(world.worktree.to_str().unwrap(), &world.caller)
        .unwrap();
    assert_eq!(running.status, TestStatus::Running);
    assert_eq!(running.check_summary.as_ref().unwrap()["passed"], 1);
    assert!(running.capacity.is_some());
    world.systemd.finish(&first.unit);
    let passed = world.wait_status(TestStatus::Passed);
    assert_eq!(passed.stdout_bytes_observed, 0);
    assert_eq!(
        passed.checks.as_ref().unwrap()[0].status,
        devcoordinator2_api::results::LeafStatus::Passed
    );
    assert_eq!(world.lifecycle.list_current().unwrap().runs.len(), 1);

    let cancelled = world.start();
    let stopped = world
        .lifecycle
        .stop(
            world.worktree.to_str().unwrap(),
            Some("owner requested stop".into()),
            &world.caller,
        )
        .unwrap();
    assert!(matches!(
        stopped,
        StopTest::Cancelled {
            status: TestStatus::Cancelled,
            ..
        }
    ));
    assert_eq!(
        world.wait_status(TestStatus::Cancelled).run_id,
        cancelled.run_id
    );

    let superseded = world.start();
    let successor = world.start();
    assert_ne!(superseded.run_id, successor.run_id);
    assert!(world.systemd.stops.load(Ordering::SeqCst) >= 2);
    world.systemd.finish(&successor.unit);
    world.wait_status(TestStatus::Passed);

    world.systemd.next_exit.store(1, Ordering::SeqCst);
    let failed = world.start();
    world.systemd.finish(&failed.unit);
    world.wait_status(TestStatus::Failed);
    let retry = world
        .lifecycle
        .retry(
            RetryTest {
                path: world.worktree.to_string_lossy().into_owned(),
                test: Some("all".into()),
                run_id: failed.run_id.clone(),
                check: "unit".into(),
            },
            &world.caller,
        )
        .unwrap();
    assert_eq!(retry.proof, ApiProofKind::Retry);
    assert_eq!(retry.origin_run_id.as_deref(), Some(failed.run_id.as_str()));
    world.systemd.finish(&retry.unit);
    world.wait_status(TestStatus::Passed);
    assert!(TestRunStore.read_history(&world.worktree).unwrap().len() >= 5);
    let events = world.events.lock().unwrap();
    assert!(events.iter().any(|event| event.kind == "test.started"));
    assert!(events.iter().any(|event| {
        event.kind == "test.finished" && event.status == Some(TestStatus::Cancelled)
    }));
    drop(events);

    world.systemd.fail_spawn.store(true, Ordering::SeqCst);
    let failed_launch = world.lifecycle.start(
        StartTest {
            path: world.worktree.to_string_lossy().into_owned(),
            test: Some("all".into()),
            checks: Vec::new(),
            tier: ApiValidationTier::Release,
        },
        &world.caller,
    );
    assert_eq!(
        failed_launch.unwrap_err().code,
        devcoordinator2_api::ErrorCode::TestStartFailed
    );
    assert_eq!(
        world
            .lifecycle
            .status(world.worktree.to_str().unwrap(), &world.caller)
            .unwrap_err()
            .code,
        devcoordinator2_api::ErrorCode::TestNotFound
    );

    world.systemd.fail_spawn.store(false, Ordering::SeqCst);
    world.systemd.fail_capture.store(true, Ordering::SeqCst);
    let stops_before = world.systemd.stops.load(Ordering::SeqCst);
    let capture_start = world.lifecycle.start(
        StartTest {
            path: world.worktree.to_string_lossy().into_owned(),
            test: Some("all".into()),
            checks: Vec::new(),
            tier: ApiValidationTier::Release,
        },
        &world.caller,
    );
    match capture_start {
        Ok(started) => {
            let failed = world.wait_status(TestStatus::Failed);
            assert_eq!(failed.run_id, started.run_id);
        }
        Err(error) => {
            assert_eq!(error.code, devcoordinator2_api::ErrorCode::TestStartFailed);
            world.wait_status(TestStatus::Failed);
        }
    }
    assert!(world.systemd.stops.load(Ordering::SeqCst) > stops_before);

    world.systemd.fail_capture.store(false, Ordering::SeqCst);
    world.systemd.mismatch_uid.store(true, Ordering::SeqCst);
    let mismatch = world.lifecycle.start(
        StartTest {
            path: world.worktree.to_string_lossy().into_owned(),
            test: Some("all".into()),
            checks: Vec::new(),
            tier: ApiValidationTier::Release,
        },
        &world.caller,
    );
    assert_eq!(
        mismatch.unwrap_err().code,
        devcoordinator2_api::ErrorCode::TestStartFailed
    );
    world.wait_status(TestStatus::Failed);
}

#[test]
fn lifecycle_provisions_private_ephemeral_postgres_and_removes_it_on_finish() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempdir().unwrap();
    let worktree = temporary.path().join("repository");
    std::fs::create_dir(&worktree).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&worktree)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", "/nonexistent")
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(
        worktree.join(".devcoordinator.toml"),
        format!(
            r#"
schema=2
[test.database]
[[test.database.check]]
name="unit"
tier="release"
command=["true"]
[test.database.postgres]
image="postgres@sha256:{}"
user="app"
database="app_test"
"#,
            "a".repeat(64)
        ),
    )
    .unwrap();
    let state = temporary.path().join("state");
    let runtime = temporary.path().join("runtime");
    std::fs::create_dir(&state).unwrap();
    std::fs::create_dir(&runtime).unwrap();
    let database = Database::open(state.join("authority.sqlite3")).unwrap();
    let config = Config {
        socket_path: runtime.join("daemon.sock"),
        state_dir: state,
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
    let registry = Registry::new(database.clone());
    let capacity = CapacityBroker::new(database.clone(), config.capacity_socket_path()).unwrap();
    let logs = TestLogService::new(database.clone(), registry.clone());
    let systemd = Arc::new(FixtureSystemd::new());
    let docker = Arc::new(PostgresDocker::new());
    let lifecycle = TestLifecycle::with_adapters(
        config,
        database,
        registry,
        capacity,
        logs,
        systemd.clone(),
        docker.clone(),
        Arc::new(FixtureCommand),
        TestRunStore,
        Arc::new(FixtureClock),
        Arc::new(FixtureMonotonic::new()),
        Arc::new(SequenceRandom(AtomicU64::new(0))),
        PathBuf::from("/fixture/devcoordinator2-executor"),
    )
    .unwrap();
    let caller = Caller {
        pid: 1,
        uid: rustix::process::getuid().as_raw(),
        gid: rustix::process::getgid().as_raw(),
        client_kind: devcoordinator2_api::ClientKind::Codex,
        client_session: None,
        identity: None,
    };
    assert_ne!(caller.uid, 0, "PostgreSQL lifecycle fixture needs non-root");
    let started = lifecycle
        .start(
            StartTest {
                path: worktree.to_string_lossy().into_owned(),
                test: Some("database".into()),
                checks: Vec::new(),
                tier: ApiValidationTier::Release,
            },
            &caller,
        )
        .unwrap();
    assert_eq!(docker.provisioned.load(Ordering::SeqCst), 1);
    let environment = worktree.join(".devcoordinator/test/current/env");
    assert_eq!(
        std::fs::metadata(&environment)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let private_environment = std::fs::read_to_string(&environment).unwrap();
    assert!(private_environment.contains("PGPASSWORD="));
    let summary = lifecycle
        .status(worktree.to_str().unwrap(), &caller)
        .unwrap();
    let public = serde_json::to_string(&summary).unwrap();
    assert!(!public.contains("PGPASSWORD"));
    assert!(!public.contains("postgresql://"));

    systemd.finish(&started.unit);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let status = lifecycle
            .status(worktree.to_str().unwrap(), &caller)
            .unwrap();
        if status.status == TestStatus::Passed {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(docker.removed.load(Ordering::SeqCst), 1);
}

#[test]
fn recovery_skips_unavailable_worktree_and_records_interrupted_summary() {
    let temporary = tempdir().unwrap();
    let worktree = temporary.path().join("repository");
    std::fs::create_dir(&worktree).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&worktree)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", "/nonexistent")
            .status()
            .unwrap()
            .success()
    );
    let state = temporary.path().join("state");
    let runtime = temporary.path().join("runtime");
    std::fs::create_dir(&state).unwrap();
    std::fs::create_dir(&runtime).unwrap();
    let database = Database::open(state.join("authority.sqlite3")).unwrap();
    let config = Config {
        socket_path: runtime.join("daemon.sock"),
        state_dir: state,
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
    let caller = Caller {
        pid: 1,
        uid: rustix::process::getuid().as_raw(),
        gid: rustix::process::getgid().as_raw(),
        client_kind: devcoordinator2_api::ClientKind::Other,
        client_session: None,
        identity: None,
    };
    let registry = Registry::new(database.clone());
    let registered = registry
        .register(&worktree, caller.uid, caller.gid)
        .unwrap();
    let unavailable = temporary.path().join("unavailable-repository");
    std::fs::create_dir(&unavailable).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&unavailable)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", "/nonexistent")
            .status()
            .unwrap()
            .success()
    );
    registry
        .register(&unavailable, caller.uid, caller.gid)
        .unwrap();
    std::fs::remove_dir_all(&unavailable).unwrap();
    let run_id = "t20260904T000000Z-aabbcc";
    let store = TestRunStore;
    let prepared = store
        .prepare(
            &worktree.canonicalize().unwrap(),
            run_id,
            caller.uid,
            caller.gid,
        )
        .unwrap();
    let summary = initial_summary(
        run_id,
        "all",
        "2026-09-04T00:00:00Z",
        caller.uid,
        "other",
        ProofKind::Complete,
        Vec::new(),
        None,
        ValidationTier::Release,
    );
    store
        .write_summary(&prepared.current, &summary, caller.uid, caller.gid)
        .unwrap();
    let systemd = Arc::new(FixtureSystemd::new());
    let unit = devcoordinator2_control::ids::unit_name(
        &config.unit_prefix,
        &registered.worktree_id,
        "stale",
    );
    systemd.seed_unit(&unit, caller.uid);
    let capacity = CapacityBroker::new(database.clone(), config.capacity_socket_path()).unwrap();
    let logs = TestLogService::new(database.clone(), registry.clone());
    let lifecycle = TestLifecycle::with_adapters(
        config,
        database,
        registry,
        capacity,
        logs,
        systemd.clone(),
        Arc::new(FixtureDocker),
        Arc::new(FixtureCommand),
        store,
        Arc::new(FixtureClock),
        Arc::new(FixtureMonotonic::new()),
        Arc::new(SequenceRandom(AtomicU64::new(0))),
        PathBuf::from("/fixture/devcoordinator2-executor"),
    )
    .unwrap();
    lifecycle.recover().unwrap();
    let interrupted = lifecycle
        .status(worktree.to_str().unwrap(), &caller)
        .unwrap();
    assert_eq!(interrupted.status, TestStatus::Interrupted);
    assert_eq!(
        interrupted.termination_reason,
        Some(devcoordinator2_api::results::RunTerminationReason::Interrupted)
    );
    assert_eq!(store.read_history(&worktree).unwrap().len(), 1);
    assert!(systemd.stops.load(Ordering::SeqCst) >= 1);
}
