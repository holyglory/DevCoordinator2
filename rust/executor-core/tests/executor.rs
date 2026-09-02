use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use devcoordinator2_executor_core::{
    Cancellation, Executor, LocalPermitProvider,
    protocol::{
        CaseSpec, CheckPlan, CheckRole, CompletionMode, ErrorCategory, ExecutionPlan,
        FailureIndexEntry, FailureMode, LeafStatus, LogPhase, ProofKind, RunStatus, Schema2,
        TerminationReason, ValidationTier,
    },
    source_digest,
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Repository {
    root: PathBuf,
}

impl Repository {
    fn new(name: &str) -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "dc2-rust-executor-{name}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create repository");
        run_git(&root, &["init", "-q"]);
        fs::write(root.join("README.md"), b"executor fixture\n").expect("fixture source");
        run_git(&root, &["add", "README.md"]);
        Self { root }
    }

    fn current(&self, run_id: &str) -> PathBuf {
        self.root.join(".devcoordinator").join(run_id)
    }
}

impl Drop for Repository {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove repository fixture");
    }
}

fn run_git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git command failed");
}

fn direct(name: &str, command: Vec<String>) -> CheckPlan {
    CheckPlan {
        name: name.into(),
        tier: ValidationTier::Development,
        role: CheckRole::Work,
        after: Vec::new(),
        requires: Vec::new(),
        invalidates: Vec::new(),
        cwd: ".".into(),
        env: BTreeMap::new(),
        timeout_seconds: Some(5),
        completion: CompletionMode::Process,
        on_failure: FailureMode::Continue,
        produces: Vec::new(),
        diagnostic_sources: Vec::new(),
        command: Some(command),
        discover: None,
        case_command: None,
        cases: None,
    }
}

fn python(script: &str) -> Vec<String> {
    vec!["python3".into(), "-c".into(), script.into()]
}

fn dynamic_fanout(name: &str, discovery_script: &str) -> CheckPlan {
    let mut fanout = direct(name, python("raise SystemExit(0)"));
    fanout.discover = Some(python(discovery_script));
    fanout.command = None;
    fanout.case_command = Some(python("raise SystemExit(0)"));
    fanout
}

fn plan(repository: &Repository, run_id: &str, checks: Vec<CheckPlan>) -> ExecutionPlan {
    fs::create_dir_all(repository.current(run_id)).expect("create run directory");
    ExecutionPlan {
        schema: Schema2,
        run_id: run_id.into(),
        test: "complete".into(),
        worktree_root: repository.root.display().to_string(),
        current_dir: repository.current(run_id).display().to_string(),
        log_dir: repository
            .root
            .join(".devcoordinator/test/logs/runs")
            .join(run_id)
            .display()
            .to_string(),
        requested_tier: ValidationTier::Development,
        readiness_eligible: false,
        proof: ProofKind::Complete,
        selection: Vec::new(),
        origin_run_id: None,
        source_digest: source_digest(&repository.root).expect("source digest"),
        config_digest: "c".repeat(64),
        reused: BTreeMap::new(),
        checks,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn symlinked_platform_ancestor_resolves_before_containment_check() {
    let repository = Repository::new("symlink-ancestor");
    let alias = repository.root.with_extension("alias");
    symlink(&repository.root, &alias).expect("create worktree alias");
    let mut execution_plan = plan(
        &repository,
        "run-symlink-ancestor",
        vec![direct("check", python("raise SystemExit(0)"))],
    );
    execution_plan.worktree_root = alias.display().to_string();
    execution_plan.current_dir = alias
        .join(".devcoordinator/run-symlink-ancestor")
        .display()
        .to_string();
    execution_plan.log_dir = alias
        .join(".devcoordinator/test/logs/runs/run-symlink-ancestor")
        .display()
        .to_string();
    let report = execute(execution_plan).await;
    assert_eq!(report.status, RunStatus::Passed);
    fs::remove_file(alias).expect("remove worktree alias");
}

async fn execute(plan: ExecutionPlan) -> devcoordinator2_executor_core::protocol::ExecutionReport {
    Executor::new(
        plan,
        Arc::new(LocalPermitProvider::unbounded()),
        Cancellation::default(),
    )
    .expect("executor")
    .run()
    .await
    .expect("run")
}

fn status(
    report: &devcoordinator2_executor_core::protocol::ExecutionReport,
    name: &str,
) -> LeafStatus {
    report
        .checks
        .iter()
        .find(|check| check.name == name)
        .expect("check report")
        .status
}

fn failure<'a>(
    report: &'a devcoordinator2_executor_core::protocol::ExecutionReport,
    check: Option<&str>,
    case: Option<&str>,
) -> &'a FailureIndexEntry {
    report
        .failure_index
        .iter()
        .find(|entry| entry.check.as_deref() == check && entry.case.as_deref() == case)
        .expect("failure diagnostic")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_preflight_invalidates_only_its_targets_and_siblings_finish() {
    let repository = Repository::new("preflight");
    let mut preflight = direct("preflight", python("raise SystemExit(7)"));
    preflight.role = CheckRole::Preflight;
    preflight.invalidates = vec!["target".into()];
    let mut target = direct("target", python("raise SystemExit(0)"));
    target.requires = vec!["preflight".into()];
    let independent = direct("independent", python("raise SystemExit(0)"));
    let report = execute(plan(
        &repository,
        "run-preflight",
        vec![preflight, target, independent],
    ))
    .await;
    assert_eq!(report.status, RunStatus::Failed);
    assert_eq!(status(&report, "preflight"), LeafStatus::Failed);
    assert_eq!(status(&report, "target"), LeafStatus::Invalidated);
    assert_eq!(status(&report, "independent"), LeafStatus::Passed);
    let process = failure(&report, Some("preflight"), None);
    assert_eq!(process.error_category, ErrorCategory::ProcessExit);
    assert_eq!(process.exit.code, Some(7));
    let invalidated = failure(&report, Some("target"), None);
    assert_eq!(invalidated.error_category, ErrorCategory::Dependency);
    assert!(invalidated.log_refs.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requested_tier_runs_that_tier_and_every_lower_tier_only() {
    let repository = Repository::new("tiers");
    let development = direct("development", python("raise SystemExit(0)"));
    let mut release = direct("release", python("raise SystemExit(9)"));
    release.tier = ValidationTier::Release;
    let report = execute(plan(&repository, "run-tiers", vec![development, release])).await;
    assert_eq!(report.status, RunStatus::Passed);
    assert_eq!(report.checks.len(), 1);
    assert_eq!(report.checks[0].name, "development");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn static_fanout_is_all_settled_and_case_reports_are_sorted() {
    let repository = Repository::new("fanout");
    let mut fanout = direct("cases", python("raise SystemExit(0)"));
    fanout.command = None;
    fanout.case_command = Some(python(
        "import sys; print(sys.argv[1]); raise SystemExit(1 if sys.argv[1] == 'bad' else 0)",
    ));
    fanout.cases = Some(vec![
        CaseSpec {
            id: "z-pass".into(),
            args: vec!["good".into()],
        },
        CaseSpec {
            id: "a-fail".into(),
            args: vec!["bad".into()],
        },
    ]);
    let report = execute(plan(&repository, "run-fanout", vec![fanout])).await;
    let check = &report.checks[0];
    assert_eq!(check.status, LeafStatus::Failed);
    assert_eq!(check.cases[0].id, "a-fail");
    assert_eq!(check.cases[0].status, LeafStatus::Failed);
    assert_eq!(check.cases[1].id, "z-pass");
    assert_eq!(check.cases[1].status, LeafStatus::Passed);
    assert_eq!(check.case_count, 2);
    assert!(!check.cases_truncated);
    assert!(
        repository
            .current("run-fanout")
            .join("checks/cases/case-report.json")
            .is_file()
    );
    assert_eq!(report.failure_index[0].case.as_deref(), Some("a-fail"));
    let failed_case = failure(&report, Some("cases"), Some("a-fail"));
    assert_eq!(failed_case.error_category, ErrorCategory::ProcessExit);
    assert_eq!(failed_case.exit.code, Some(1));
    assert!(
        failed_case
            .log_refs
            .iter()
            .all(|reference| reference.phase == LogPhase::Case
                && reference.case.as_deref() == Some("a-fail"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oversized_dynamic_manifest_fails_without_starting_cases() {
    let repository = Repository::new("manifest");
    let fanout = dynamic_fanout(
        "cases",
        "import os; os.write(int(os.environ['DEVCOORDINATOR_CASE_MANIFEST_FD']), b'x' * (2 * 1024 * 1024 + 1))",
    );
    let report = execute(plan(&repository, "run-manifest", vec![fanout])).await;
    assert_eq!(report.checks[0].status, LeafStatus::Failed);
    let diagnostic = failure(&report, Some("cases"), None);
    assert_eq!(
        diagnostic.error_category,
        ErrorCategory::StructuredEvidenceInvalid
    );
    assert!(
        diagnostic
            .log_refs
            .iter()
            .all(|reference| reference.phase == LogPhase::Discovery)
    );
    assert!(
        report.checks[0]
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("2 MiB"))
    );
    assert!(report.checks[0].cases.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discovery_stdout_is_log_noise_and_manifest_comes_only_from_descriptor() {
    let repository = Repository::new("manifest-descriptor");
    let script = r#"
import os, sys
sys.stdout.write("ordinary discovery noise")
payload = b'{"schema":2,"cases":[{"id":"one","args":[]}]}'
os.write(int(os.environ["DEVCOORDINATOR_CASE_MANIFEST_FD"]), payload)
"#;
    let fanout = dynamic_fanout("cases", script);
    let report = execute(plan(&repository, "run-manifest-descriptor", vec![fanout])).await;
    assert_eq!(report.status, RunStatus::Passed);
    assert_eq!(report.checks[0].case_count, 1);
    assert_eq!(report.checks[0].cases[0].id, "one");
    assert_eq!(
        fs::read_to_string(
            repository
                .current("run-manifest-descriptor")
                .join("checks/cases/discovery-stdout.log")
        )
        .expect("discovery stdout"),
        "ordinary discovery noise"
    );
    let saved = fs::read_to_string(
        repository
            .current("run-manifest-descriptor")
            .join("checks/cases/manifest.json"),
    )
    .expect("saved manifest");
    assert!(saved.starts_with("{\"schema\":2"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_invalid_and_trailing_descriptor_manifests_are_rejected() {
    let cases = [
        ("missing", "raise SystemExit(0)"),
        (
            "invalid",
            "import os; os.write(int(os.environ['DEVCOORDINATOR_CASE_MANIFEST_FD']), b'not-json')",
        ),
        (
            "trailing",
            "import os; os.write(int(os.environ['DEVCOORDINATOR_CASE_MANIFEST_FD']), b'{\"schema\":2,\"cases\":[]} {\"schema\":2,\"cases\":[]}')",
        ),
    ];
    for (name, script) in cases {
        let repository = Repository::new(name);
        let report = execute(plan(
            &repository,
            &format!("run-{name}"),
            vec![dynamic_fanout("cases", script)],
        ))
        .await;
        assert_eq!(report.status, RunStatus::Failed, "{name}");
        assert_eq!(report.checks[0].status, LeafStatus::Failed, "{name}");
        assert!(report.checks[0].cases.is_empty(), "{name}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalid_manifest_cleanup_terminates_discovery_descendants() {
    let repository = Repository::new("manifest-cleanup");
    let script = r#"
import os, pathlib, subprocess
child = subprocess.Popen(["python3", "-c", "import time; time.sleep(30)"])
pathlib.Path(os.environ["DEVCOORDINATOR_CHECK_SCRATCH"]).joinpath("child.pid").write_text(str(child.pid))
os.write(int(os.environ["DEVCOORDINATOR_CASE_MANIFEST_FD"]), b'not-json')
"#;
    let report = execute(plan(
        &repository,
        "run-manifest-cleanup",
        vec![dynamic_fanout("cases", script)],
    ))
    .await;
    assert_eq!(report.status, RunStatus::Failed);
    let pid: u32 = fs::read_to_string(
        repository
            .current("run-manifest-cleanup")
            .join("scratch/cases/child.pid"),
    )
    .expect("child pid")
    .parse()
    .expect("pid number");
    for _ in 0..100 {
        if !PathBuf::from(format!("/proc/{pid}")).exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        !PathBuf::from(format!("/proc/{pid}")).exists(),
        "discovery descendant survived cleanup"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn leaf_deadline_terminates_a_signal_resistant_process_group() {
    let repository = Repository::new("deadline");
    let mut slow = direct(
        "slow",
        python("import signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(30)"),
    );
    slow.timeout_seconds = Some(1);
    let report = execute(plan(&repository, "run-deadline", vec![slow])).await;
    assert_eq!(report.status, RunStatus::Failed);
    assert_eq!(report.checks[0].status, LeafStatus::TimedOut);
    assert!(report.duration_seconds < 5.0);
    let diagnostic = failure(&report, Some("slow"), None);
    assert_eq!(diagnostic.error_category, ErrorCategory::Timeout);
    assert_eq!(
        diagnostic.termination_reason,
        Some(TerminationReason::DeadlineExceeded)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn completion_event_identity_is_exact() {
    let repository = Repository::new("event-identity");
    let script = r#"
import json, os, time
payload = {"schema": 2, "run_id": os.environ["DEVCOORDINATOR_RUN_ID"], "check": "wrong", "status": "passed", "reason": None}
os.write(int(os.environ["DEVCOORDINATOR_EVENT_FD"]), (json.dumps(payload) + "\n").encode())
time.sleep(30)
"#;
    let mut service = direct("service", python(script));
    service.completion = CompletionMode::Event;
    service.timeout_seconds = Some(3);
    let report = execute(plan(&repository, "run-event-wrong", vec![service])).await;
    assert_eq!(report.status, RunStatus::Failed);
    assert_eq!(report.checks[0].status, LeafStatus::Unsafe);
    let diagnostic = failure(&report, Some("service"), None);
    assert_eq!(
        diagnostic.error_category,
        ErrorCategory::StructuredEvidenceInvalid
    );
    assert!(
        report.checks[0]
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("wrong run or check"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn passed_event_keeps_service_alive_for_dependents_then_cleans_it_up() {
    let repository = Repository::new("event-service");
    let script = r#"
import json, os, time
payload = {"schema": 2, "run_id": os.environ["DEVCOORDINATOR_RUN_ID"], "check": os.environ["DEVCOORDINATOR_CHECK_NAME"], "status": "passed", "reason": None}
os.write(int(os.environ["DEVCOORDINATOR_EVENT_FD"]), (json.dumps(payload) + "\n").encode())
time.sleep(30)
"#;
    let mut service = direct("service", python(script));
    service.completion = CompletionMode::Event;
    let mut dependent = direct("dependent", python("raise SystemExit(0)"));
    dependent.requires = vec!["service".into()];
    let report = execute(plan(
        &repository,
        "run-event-pass",
        vec![service, dependent],
    ))
    .await;
    assert_eq!(report.status, RunStatus::Passed);
    assert_eq!(status(&report, "service"), LeafStatus::Passed);
    assert_eq!(status(&report, "dependent"), LeafStatus::Passed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn logs_are_drained_but_retained_to_the_fixed_cap() {
    let repository = Repository::new("logs");
    let noisy = direct(
        "noisy",
        python("import sys; sys.stdout.write('x' * (5 * 1024 * 1024))"),
    );
    let report = execute(plan(&repository, "run-logs", vec![noisy])).await;
    let check = &report.checks[0];
    assert_eq!(check.status, LeafStatus::Passed);
    assert_eq!(check.stdout_bytes_observed, 5 * 1024 * 1024);
    assert_eq!(check.stdout_bytes_retained, 4 * 1024 * 1024);
    assert!(check.stdout_truncated);
    assert_eq!(
        fs::metadata(
            repository
                .current("run-logs")
                .join("checks/noisy/stdout.log")
        )
        .expect("stdout log")
        .len(),
        4 * 1024 * 1024
    );
    assert_eq!(
        fs::metadata(repository.current("run-logs").join("stdout.log"))
            .expect("aggregate stdout log")
            .len(),
        4 * 1024 * 1024
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_leaf_output_is_retained_in_run_aggregate_streams() {
    let repository = Repository::new("aggregate-logs");
    let one = direct(
        "one",
        python("import sys; print('one-out'); print('one-err', file=sys.stderr)"),
    );
    let two = direct(
        "two",
        python("import sys; print('two-out'); print('two-err', file=sys.stderr)"),
    );
    let report = execute(plan(&repository, "run-aggregate-logs", vec![one, two])).await;
    assert_eq!(report.status, RunStatus::Passed);
    let stdout = fs::read_to_string(repository.current("run-aggregate-logs").join("stdout.log"))
        .expect("aggregate stdout");
    let stderr = fs::read_to_string(repository.current("run-aggregate-logs").join("stderr.log"))
        .expect("aggregate stderr");
    assert!(stdout.contains("one-out") && stdout.contains("two-out"));
    assert!(stderr.contains("one-err") && stderr.contains("two-err"));
    assert_eq!(
        u64::try_from(stdout.len()).expect("stdout length"),
        report
            .checks
            .iter()
            .map(|check| check.stdout_bytes_observed)
            .sum::<u64>()
    );
    assert_eq!(
        fs::read_to_string(
            repository
                .current("run-aggregate-logs")
                .join("checks/one/stdout.log")
        )
        .expect("one stdout"),
        "one-out\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn external_cancellation_stops_running_leaf_and_finishes_report() {
    let repository = Repository::new("cancel");
    let slow = direct("slow", python("import time; time.sleep(30)"));
    let execution_plan = plan(&repository, "run-cancel", vec![slow]);
    let cancellation = Cancellation::default();
    let executor = Executor::new(
        execution_plan,
        Arc::new(LocalPermitProvider::unbounded()),
        cancellation.clone(),
    )
    .expect("executor");
    let task = tokio::spawn(executor.run());
    for _ in 0..100 {
        if repository
            .current("run-cancel")
            .join("check-report.json")
            .exists()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    cancellation.cancel();
    let report = task.await.expect("join").expect("report");
    assert_eq!(report.status, RunStatus::Failed);
    assert_eq!(report.checks[0].status, LeafStatus::Cancelled);
    assert!(report.duration_seconds < 5.0);
    let diagnostic = failure(&report, Some("slow"), None);
    assert_eq!(diagnostic.error_category, ErrorCategory::Cancellation);
    assert_eq!(
        diagnostic.termination_reason,
        Some(TerminationReason::RunCancelled)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_artifact_and_source_change_have_distinct_structured_categories() {
    let artifact_repository = Repository::new("artifact-diagnostic");
    let mut artifact = direct("artifact", python("raise SystemExit(0)"));
    artifact.produces = vec!["missing.bin".into()];
    let artifact_report = execute(plan(
        &artifact_repository,
        "run-artifact-diagnostic",
        vec![artifact],
    ))
    .await;
    let artifact_failure = failure(&artifact_report, Some("artifact"), None);
    assert_eq!(artifact_failure.error_category, ErrorCategory::Artifact);
    assert_eq!(artifact_failure.exit.code, Some(0));

    let source_repository = Repository::new("source-diagnostic");
    let mutate = direct(
        "mutate",
        python("from pathlib import Path; Path('README.md').write_text('changed')"),
    );
    let source_report = execute(plan(
        &source_repository,
        "run-source-diagnostic",
        vec![mutate],
    ))
    .await;
    assert!(source_report.source_changed);
    assert_eq!(
        failure(&source_report, None, None).error_category,
        ErrorCategory::SourceChanged
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn signal_exit_is_structured_without_arbitrary_process_prose() {
    let repository = Repository::new("signal-diagnostic");
    let signalled = direct(
        "signalled",
        python("import os, signal; os.kill(os.getpid(), signal.SIGKILL)"),
    );
    let report = execute(plan(&repository, "run-signal-diagnostic", vec![signalled])).await;
    let diagnostic = failure(&report, Some("signalled"), None);
    assert_eq!(diagnostic.error_category, ErrorCategory::ProcessExit);
    assert_eq!(diagnostic.exit.code, None);
    assert_eq!(diagnostic.exit.signal, Some(9));
    let encoded = serde_json::to_string(&report.failure_index).expect("diagnostic JSON");
    assert!(!encoded.contains("process exited"));
}
