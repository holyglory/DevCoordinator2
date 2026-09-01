use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use devcoordinator2_executor_protocol::{
    ArtifactReceipt, CapacityReport, CaseManifest, CaseReport, CaseSpec, CheckPlan, CheckReport,
    CompletionMode, ExecutionPlan, ExecutionReport, FailureIndexEntry, FailureMode, LeafStatus,
    MAX_FAILURE_INDEX, MAX_MANIFEST_BYTES, MAX_REASON_BYTES, OutputStats, RunStatus, Schema2,
};
use rustix::process::Signal;
use tokio::task::JoinSet;

use crate::ExecutorError;
use crate::capacity::{CapacityObservation, PermitProvider};
use crate::evidence::{
    artifact_receipts, receipts_match, source_digest, write_bytes_atomic, write_json_atomic,
};
use crate::process::{
    CHECK_LOG_CAP_BYTES, Cancellation, EventService, ProcessRequest, ProcessResult, ProcessStatus,
    ServiceExit, run_process, signal_group,
};

const SERVICE_TERMINATION_GRACE: Duration = Duration::from_secs(2);
const MAX_INLINE_CASES: usize = 128;
const MAX_CASE_EVIDENCE_BYTES: usize = 4 * 1024 * 1024;

pub struct Executor {
    plan: ExecutionPlan,
    permits: Arc<dyn PermitProvider>,
    cancellation: Cancellation,
}

impl Executor {
    pub fn new(
        plan: ExecutionPlan,
        permits: Arc<dyn PermitProvider>,
        cancellation: Cancellation,
    ) -> Result<Self, ExecutorError> {
        plan.validate()?;
        Ok(Self {
            plan,
            permits,
            cancellation,
        })
    }

    pub async fn run(self) -> Result<ExecutionReport, ExecutorError> {
        let plan = Arc::new(self.plan);
        let root = PathBuf::from(&plan.worktree_root)
            .canonicalize()
            .map_err(|error| ExecutorError::new(format!("cannot resolve worktree: {error}")))?;
        let current_requested = PathBuf::from(&plan.current_dir);
        if !current_requested.starts_with(&root) {
            return Err(ExecutorError::new(
                "executor current_dir is outside the worktree",
            ));
        }
        tokio::fs::create_dir_all(&current_requested)
            .await
            .map_err(|error| ExecutorError::new(format!("cannot create run directory: {error}")))?;
        let current = current_requested.canonicalize().map_err(|error| {
            ExecutorError::new(format!("cannot resolve run directory: {error}"))
        })?;
        if !current.starts_with(&root) {
            return Err(ExecutorError::new(
                "executor current_dir resolves outside the worktree",
            ));
        }
        for directory in ["checks", "scratch", "artifacts"] {
            tokio::fs::create_dir_all(current.join(directory))
                .await
                .map_err(|error| {
                    ExecutorError::new(format!("cannot create run artifacts: {error}"))
                })?;
        }

        let digest_root = root.clone();
        let initial_digest = tokio::task::spawn_blocking(move || source_digest(&digest_root))
            .await
            .map_err(|error| ExecutorError::new(format!("source digest task failed: {error}")))??;
        if initial_digest != plan.source_digest {
            return Err(ExecutorError::new(
                "repository source changed before checks started",
            ));
        }

        let started_at = iso_now();
        let started = Instant::now();
        let capacity = Arc::new(Mutex::new(CapacityReport::default()));
        let mut checks = initialize_checks(&plan, &root, &started_at)?;
        let report_path = current.join("check-report.json");
        write_report(
            &report_path,
            &plan,
            &checks,
            &capacity,
            &started_at,
            started,
            RunStatus::Running,
            None,
            false,
            None,
            None,
        )?;

        let invalidators = invalidator_map(&checks);
        let mut running = JoinSet::<(String, CheckOutcome)>::new();
        let mut services = JoinSet::<(String, ServiceExit)>::new();
        let mut service_pgids = BTreeMap::<String, i32>::new();
        let mut abort: Option<Abort> = None;
        let mut cancellation_seen = false;

        loop {
            while let Some(joined) = services.try_join_next() {
                let (name, exit) = joined.map_err(|error| {
                    ExecutorError::new(format!("event service task failed: {error}"))
                })?;
                service_pgids.remove(&name);
                mark_service_exit(&mut checks, &name, exit, false)?;
                if abort.is_none() {
                    abort = Some(Abort::Unsafe(format!(
                        "required long-lived check {name} exited"
                    )));
                    self.cancellation.cancel();
                }
            }
            mark_blocked(&mut checks, &invalidators);
            if self.cancellation.is_cancelled() && abort.is_none() {
                abort = Some(Abort::Cancelled);
            }
            if abort.is_some() {
                cancellation_seen = true;
                mark_pending_cancelled(&mut checks, abort_reason(abort.as_ref()));
            }

            if abort.is_none() {
                let ready = ready_checks(&checks, &invalidators);
                for name in ready {
                    let index = check_index(&checks, &name)?;
                    let runtime = &mut checks[index];
                    runtime.report.status = LeafStatus::Running;
                    runtime.report.started_at = Some(iso_now());
                    let check = runtime.plan.clone();
                    let plan = plan.clone();
                    let root = root.clone();
                    let current = current.clone();
                    let permits = self.permits.clone();
                    let cancellation = self.cancellation.clone();
                    let capacity = capacity.clone();
                    running.spawn(async move {
                        let outcome = execute_check(
                            plan,
                            check,
                            root,
                            current,
                            permits,
                            cancellation,
                            capacity,
                        )
                        .await;
                        (name, outcome)
                    });
                }
            }

            write_report(
                &report_path,
                &plan,
                &checks,
                &capacity,
                &started_at,
                started,
                RunStatus::Running,
                None,
                false,
                None,
                None,
            )?;

            if checks.iter().all(|check| check.report.status.is_terminal()) {
                break;
            }
            if running.is_empty() {
                return Err(ExecutorError::new("executor graph made no progress"));
            }

            tokio::select! {
                joined = running.join_next(), if !running.is_empty() => {
                    let Some(joined) = joined else { continue };
                    let (name, outcome) = joined.map_err(|error| {
                        ExecutorError::new(format!("check task failed: {error}"))
                    })?;
                    let index = check_index(&checks, &name)?;
                    let failure_mode = checks[index].plan.on_failure;
                    let status = outcome.status;
                    apply_outcome(&mut checks[index], outcome, &mut services, &mut service_pgids);
                    if abort.is_none() && (status == LeafStatus::Unsafe
                        || (!status.is_success() && failure_mode == FailureMode::Stop))
                    {
                        abort = Some(if status == LeafStatus::Unsafe {
                            Abort::Unsafe(format!("check {name} became unsafe"))
                        } else {
                            Abort::Stopped(format!("check {name} requested stop on failure"))
                        });
                        self.cancellation.cancel();
                    }
                }
                joined = services.join_next(), if !services.is_empty() => {
                    let Some(joined) = joined else { continue };
                    let (name, exit) = joined.map_err(|error| {
                        ExecutorError::new(format!("event service task failed: {error}"))
                    })?;
                    service_pgids.remove(&name);
                    mark_service_exit(&mut checks, &name, exit, false)?;
                    if abort.is_none() {
                        abort = Some(Abort::Unsafe(format!(
                            "required long-lived check {name} exited"
                        )));
                        self.cancellation.cancel();
                    }
                }
                () = self.cancellation.cancelled(), if !cancellation_seen => {
                    cancellation_seen = true;
                    if abort.is_none() {
                        abort = Some(Abort::Cancelled);
                    }
                }
            }
        }

        stop_services(&mut services, &mut service_pgids, &mut checks).await?;
        let digest_root = root.clone();
        let final_digest = tokio::task::spawn_blocking(move || source_digest(&digest_root)).await;
        let (source_changed, source_error) = match final_digest {
            Ok(Ok(digest)) => (digest != plan.source_digest, None),
            Ok(Err(error)) => (true, Some(format!("cannot verify final source: {error}"))),
            Err(error) => (
                true,
                Some(format!("final source digest task failed: {error}")),
            ),
        };
        let status = if source_changed
            || abort.is_some()
            || checks.iter().any(|check| !check.report.status.is_success())
        {
            RunStatus::Failed
        } else {
            RunStatus::Passed
        };
        let unsafe_reason = source_error.clone().or_else(|| match &abort {
            Some(Abort::Stopped(reason) | Abort::Unsafe(reason)) => Some(reason.clone()),
            Some(Abort::Cancelled) | None => None,
        });
        write_report(
            &report_path,
            &plan,
            &checks,
            &capacity,
            &started_at,
            started,
            status,
            Some(iso_now()),
            source_changed,
            source_error,
            unsafe_reason,
        )
    }
}

struct CheckRuntime {
    plan: CheckPlan,
    report: CheckReport,
    failures: Vec<FailureIndexEntry>,
}

struct CheckOutcome {
    status: LeafStatus,
    exit_code: Option<i32>,
    reason: Option<String>,
    artifacts: Vec<ArtifactReceipt>,
    output: OutputStats,
    case_count: u32,
    cases: Vec<CaseReport>,
    cases_truncated: bool,
    failures: Vec<FailureIndexEntry>,
    service: Option<EventService>,
    duration_seconds: f64,
}

#[derive(Clone)]
struct CaseOutcome {
    report: CaseReport,
    reason: Option<String>,
    output: OutputStats,
}

enum Abort {
    Cancelled,
    Stopped(String),
    Unsafe(String),
}

fn initialize_checks(
    plan: &ExecutionPlan,
    root: &Path,
    started_at: &str,
) -> Result<Vec<CheckRuntime>, ExecutorError> {
    let mut result = Vec::new();
    for check in plan.active_checks() {
        let cwd = root.join(&check.cwd);
        let resolved = cwd.canonicalize().map_err(|error| {
            ExecutorError::new(format!("cannot resolve cwd for {:?}: {error}", check.name))
        })?;
        if !resolved.starts_with(root) {
            return Err(ExecutorError::new(format!(
                "check {:?} cwd escapes the worktree",
                check.name
            )));
        }
        let reused = plan.reused.get(&check.name);
        if let Some(receipts) = reused
            && !receipts_match(root, receipts)?
        {
            return Err(ExecutorError::new(format!(
                "reused evidence for {:?} is stale",
                check.name
            )));
        }
        let status = if reused.is_some() {
            LeafStatus::Reused
        } else {
            LeafStatus::Pending
        };
        result.push(CheckRuntime {
            plan: check.clone(),
            report: CheckReport {
                name: check.name.clone(),
                tier: check.tier,
                role: check.role,
                status,
                started_at: reused.map(|_| started_at.to_owned()),
                finished_at: reused.map(|_| started_at.to_owned()),
                duration_seconds: reused.map(|_| 0.0),
                exit_code: None,
                reason: reused.map(|_| "matching artifact evidence reused".into()),
                artifacts: reused.cloned().unwrap_or_default(),
                output_ref: format!("checks/{}", check.name),
                stdout_bytes_observed: 0,
                stdout_bytes_retained: 0,
                stdout_truncated: false,
                stderr_bytes_observed: 0,
                stderr_bytes_retained: 0,
                stderr_truncated: false,
                case_count: 0,
                cases: Vec::new(),
                cases_truncated: false,
            },
            failures: Vec::new(),
        });
    }
    Ok(result)
}

fn invalidator_map(checks: &[CheckRuntime]) -> BTreeMap<String, Vec<String>> {
    let active: BTreeSet<&str> = checks
        .iter()
        .map(|check| check.plan.name.as_str())
        .collect();
    let mut result = BTreeMap::<String, Vec<String>>::new();
    for check in checks {
        for target in &check.plan.invalidates {
            if active.contains(target.as_str()) {
                result
                    .entry(target.clone())
                    .or_default()
                    .push(check.plan.name.clone());
            }
        }
    }
    for values in result.values_mut() {
        values.sort();
    }
    result
}

fn mark_blocked(checks: &mut [CheckRuntime], invalidators: &BTreeMap<String, Vec<String>>) {
    let statuses: BTreeMap<String, LeafStatus> = checks
        .iter()
        .map(|check| (check.plan.name.clone(), check.report.status))
        .collect();
    for check in checks {
        if check.report.status != LeafStatus::Pending {
            continue;
        }
        if let Some(preflights) = invalidators.get(&check.plan.name) {
            let failed: Vec<&str> = preflights
                .iter()
                .filter(|name| {
                    statuses[name.as_str()].is_terminal() && !statuses[name.as_str()].is_success()
                })
                .map(String::as_str)
                .collect();
            if !failed.is_empty() {
                finish_blocked(
                    check,
                    LeafStatus::Invalidated,
                    format!(
                        "invalidating preflights did not pass: {}",
                        failed.join(", ")
                    ),
                );
                continue;
            }
        }
        let dependencies: Vec<&str> = check
            .plan
            .requires
            .iter()
            .filter(|name| {
                statuses[name.as_str()].is_terminal() && !statuses[name.as_str()].is_success()
            })
            .map(String::as_str)
            .collect();
        let all_terminal = check
            .plan
            .requires
            .iter()
            .all(|name| statuses[name.as_str()].is_terminal());
        if all_terminal && !dependencies.is_empty() {
            finish_blocked(
                check,
                LeafStatus::NotMeaningful,
                format!("required checks did not pass: {}", dependencies.join(", ")),
            );
        }
    }
}

fn finish_blocked(check: &mut CheckRuntime, status: LeafStatus, reason: String) {
    check.report.status = status;
    check.report.finished_at = Some(iso_now());
    check.report.duration_seconds = Some(0.0);
    check.report.reason = Some(reason.clone());
    check.failures.push(FailureIndexEntry {
        check: Some(check.plan.name.clone()),
        case: None,
        status,
        reason: bounded_reason(&reason),
        output_ref: Some(check.report.output_ref.clone()),
    });
}

fn ready_checks(
    checks: &[CheckRuntime],
    invalidators: &BTreeMap<String, Vec<String>>,
) -> Vec<String> {
    let statuses: BTreeMap<&str, LeafStatus> = checks
        .iter()
        .map(|check| (check.plan.name.as_str(), check.report.status))
        .collect();
    checks
        .iter()
        .filter(|check| check.report.status == LeafStatus::Pending)
        .filter(|check| {
            check
                .plan
                .after
                .iter()
                .chain(&check.plan.requires)
                .all(|name| statuses[name.as_str()].is_terminal())
        })
        .filter(|check| {
            check
                .plan
                .requires
                .iter()
                .all(|name| statuses[name.as_str()].is_success())
        })
        .filter(|check| {
            invalidators
                .get(&check.plan.name)
                .into_iter()
                .flatten()
                .all(|name| statuses[name.as_str()].is_success())
        })
        .map(|check| check.plan.name.clone())
        .collect()
}

async fn execute_check(
    plan: Arc<ExecutionPlan>,
    check: CheckPlan,
    root: PathBuf,
    current: PathBuf,
    permits: Arc<dyn PermitProvider>,
    cancellation: Cancellation,
    capacity: Arc<Mutex<CapacityReport>>,
) -> CheckOutcome {
    let started = Instant::now();
    if let Some(command) = &check.command {
        let request = process_request(
            &plan,
            &check,
            &root,
            &current,
            &check.name,
            command.clone(),
            check.completion,
            false,
            CHECK_LOG_CAP_BYTES,
            "stdout.log",
            "stderr.log",
        );
        let mut process = run_process(request, permits, cancellation).await;
        observe_capacity(&capacity, process.capacity);
        let artifacts = if process.status == ProcessStatus::Passed {
            match artifact_receipts(&root, &check.produces) {
                Ok(receipts) => receipts,
                Err(error) => {
                    process.status = ProcessStatus::Failed;
                    process.reason = Some(error.to_string());
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        direct_outcome(&check, process, artifacts, started.elapsed())
    } else {
        execute_fanout(
            &plan,
            &check,
            &root,
            &current,
            permits,
            cancellation,
            capacity,
            started,
        )
        .await
    }
}

fn direct_outcome(
    check: &CheckPlan,
    mut process: ProcessResult,
    artifacts: Vec<ArtifactReceipt>,
    duration: Duration,
) -> CheckOutcome {
    let status: LeafStatus = process.status.into();
    let mut failures = Vec::new();
    if !status.is_success() {
        failures.push(FailureIndexEntry {
            check: Some(check.name.clone()),
            case: None,
            status,
            reason: bounded_reason(process.reason.as_deref().unwrap_or("leaf failed")),
            output_ref: Some(format!("checks/{}", check.name)),
        });
    }
    CheckOutcome {
        status,
        exit_code: process.exit_code,
        reason: process.reason.take().map(|value| bounded_reason(&value)),
        artifacts,
        output: process.output,
        case_count: 0,
        cases: Vec::new(),
        cases_truncated: false,
        failures,
        service: process.service.take(),
        duration_seconds: seconds(duration),
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_fanout(
    plan: &ExecutionPlan,
    check: &CheckPlan,
    root: &Path,
    current: &Path,
    permits: Arc<dyn PermitProvider>,
    cancellation: Cancellation,
    capacity: Arc<Mutex<CapacityReport>>,
    started: Instant,
) -> CheckOutcome {
    let mut aggregate = OutputStats::default();
    let mut cases = if let Some(static_cases) = &check.cases {
        static_cases.clone()
    } else {
        let request = process_request(
            plan,
            check,
            root,
            current,
            &format!("{}/discovery", check.name),
            check.discover.clone().unwrap_or_default(),
            CompletionMode::Process,
            true,
            CHECK_LOG_CAP_BYTES,
            "discovery-stdout.log",
            "discovery-stderr.log",
        );
        let mut discovery = run_process(request, permits.clone(), cancellation.clone()).await;
        observe_capacity(&capacity, discovery.capacity);
        add_output(&mut aggregate, &discovery.output);
        if discovery.status != ProcessStatus::Passed {
            return fanout_setup_failure(check, discovery, aggregate, started.elapsed());
        }
        let Some(manifest_capture) = discovery.manifest.take() else {
            return simple_outcome(
                check,
                LeafStatus::Failed,
                "case discovery closed without a descriptor manifest",
                aggregate,
                started.elapsed(),
            );
        };
        if manifest_capture.truncated || manifest_capture.observed > MAX_MANIFEST_BYTES as u64 {
            return simple_outcome(
                check,
                LeafStatus::Failed,
                "case manifest descriptor exceeds 2 MiB",
                aggregate,
                started.elapsed(),
            );
        }
        if manifest_capture.payload.is_empty() {
            return simple_outcome(
                check,
                LeafStatus::Failed,
                "case discovery descriptor closed without a manifest",
                aggregate,
                started.elapsed(),
            );
        }
        let path = current
            .join("checks")
            .join(&check.name)
            .join("manifest.json");
        if let Err(error) = write_bytes_atomic(&path, &manifest_capture.payload, MAX_MANIFEST_BYTES)
        {
            return simple_outcome(
                check,
                LeafStatus::Unsafe,
                &error.to_string(),
                aggregate,
                started.elapsed(),
            );
        }
        match CaseManifest::from_json(&manifest_capture.payload) {
            Ok(manifest) => manifest.cases,
            Err(error) => {
                return simple_outcome(
                    check,
                    LeafStatus::Failed,
                    &error.to_string(),
                    aggregate,
                    started.elapsed(),
                );
            }
        }
    };
    cases.sort_by(|left, right| left.id.cmp(&right.id));
    let case_command = check.case_command.clone().unwrap_or_default();
    let mut tasks = JoinSet::<CaseOutcome>::new();
    for case in cases {
        let plan = plan.clone();
        let check = check.clone();
        let root = root.to_path_buf();
        let current = current.to_path_buf();
        let permits = permits.clone();
        let cancellation = cancellation.clone();
        let capacity = capacity.clone();
        let case_command = case_command.clone();
        tasks.spawn(async move {
            run_case(
                &plan,
                &check,
                &case,
                &case_command,
                &root,
                &current,
                permits,
                cancellation,
                capacity,
            )
            .await
        });
    }
    let mut outcomes = Vec::new();
    let mut join_failure = None;
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok(outcome) => outcomes.push(outcome),
            Err(error) => join_failure = Some(format!("case task failed internally: {error}")),
        }
    }
    outcomes.sort_by(|left, right| left.report.id.cmp(&right.report.id));
    let mut failures = Vec::new();
    for outcome in &outcomes {
        add_output(&mut aggregate, &outcome.output);
        if !outcome.report.status.is_success() {
            failures.push(FailureIndexEntry {
                check: Some(check.name.clone()),
                case: Some(outcome.report.id.clone()),
                status: outcome.report.status,
                reason: bounded_reason(outcome.reason.as_deref().unwrap_or("case failed")),
                output_ref: Some(outcome.report.output_ref.clone()),
            });
        }
    }
    let case_reports: Vec<CaseReport> = outcomes.into_iter().map(|row| row.report).collect();
    let (status, reason) = if let Some(reason) = join_failure {
        (LeafStatus::Unsafe, Some(reason))
    } else if case_reports
        .iter()
        .any(|case| case.status == LeafStatus::Unsafe)
    {
        (
            LeafStatus::Unsafe,
            Some("one or more cases became unsafe".into()),
        )
    } else if case_reports
        .iter()
        .any(|case| case.status == LeafStatus::TimedOut)
    {
        (
            LeafStatus::TimedOut,
            Some("one or more cases timed out".into()),
        )
    } else if case_reports
        .iter()
        .any(|case| case.status == LeafStatus::Cancelled)
    {
        (
            LeafStatus::Cancelled,
            Some("one or more cases were cancelled".into()),
        )
    } else if case_reports
        .iter()
        .any(|case| case.status == LeafStatus::Failed)
    {
        (LeafStatus::Failed, Some("one or more cases failed".into()))
    } else {
        (LeafStatus::Passed, None)
    };
    if status == LeafStatus::Unsafe && failures.is_empty() {
        failures.push(FailureIndexEntry {
            check: Some(check.name.clone()),
            case: None,
            status,
            reason: bounded_reason(reason.as_deref().unwrap_or("case task became unsafe")),
            output_ref: Some(format!("checks/{}", check.name)),
        });
    }
    let mut final_status = status;
    let mut final_reason = reason;
    let case_count = u32::try_from(case_reports.len()).unwrap_or(u32::MAX);
    let case_evidence = serde_json::json!({
        "schema": 2,
        "check": check.name,
        "cases": &case_reports,
    });
    let evidence_result = serde_json::to_vec(&case_evidence)
        .map_err(|error| ExecutorError::new(format!("cannot encode case evidence: {error}")))
        .and_then(|payload| {
            write_bytes_atomic(
                &current
                    .join("checks")
                    .join(&check.name)
                    .join("case-report.json"),
                &payload,
                MAX_CASE_EVIDENCE_BYTES,
            )
        });
    if let Err(error) = evidence_result {
        final_status = LeafStatus::Unsafe;
        final_reason = Some(error.to_string());
        failures.push(FailureIndexEntry {
            check: Some(check.name.clone()),
            case: None,
            status: LeafStatus::Unsafe,
            reason: bounded_reason(&error.to_string()),
            output_ref: Some(format!("checks/{}", check.name)),
        });
    }
    let artifacts = if final_status == LeafStatus::Passed {
        match artifact_receipts(root, &check.produces) {
            Ok(receipts) => receipts,
            Err(error) => {
                final_status = LeafStatus::Failed;
                final_reason = Some(error.to_string());
                failures.push(FailureIndexEntry {
                    check: Some(check.name.clone()),
                    case: None,
                    status: LeafStatus::Failed,
                    reason: bounded_reason(&error.to_string()),
                    output_ref: Some(format!("checks/{}", check.name)),
                });
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    CheckOutcome {
        status: final_status,
        exit_code: None,
        reason: final_reason.map(|value| bounded_reason(&value)),
        artifacts,
        output: aggregate,
        case_count,
        cases: case_reports
            .iter()
            .take(MAX_INLINE_CASES)
            .cloned()
            .collect(),
        cases_truncated: case_reports.len() > MAX_INLINE_CASES,
        failures,
        service: None,
        duration_seconds: seconds(started.elapsed()),
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_case(
    plan: &ExecutionPlan,
    check: &CheckPlan,
    case: &CaseSpec,
    base_command: &[String],
    root: &Path,
    current: &Path,
    permits: Arc<dyn PermitProvider>,
    cancellation: Cancellation,
    capacity: Arc<Mutex<CapacityReport>>,
) -> CaseOutcome {
    let started = Instant::now();
    let mut command = base_command.to_vec();
    command.extend(case.args.clone());
    let leaf = format!("{}/case/{}", check.name, case.id);
    let request = process_request(
        plan,
        check,
        root,
        current,
        &leaf,
        command,
        CompletionMode::Process,
        false,
        CHECK_LOG_CAP_BYTES,
        "stdout.log",
        "stderr.log",
    );
    let process = run_process(request, permits, cancellation).await;
    observe_capacity(&capacity, process.capacity);
    CaseOutcome {
        report: CaseReport {
            id: case.id.clone(),
            status: process.status.into(),
            exit_code: process.exit_code,
            duration_ms: millis(started.elapsed()),
            output_ref: format!("checks/{}/cases/{}", check.name, case.id),
        },
        reason: process.reason,
        output: process.output,
    }
}

#[allow(clippy::too_many_arguments)]
fn process_request(
    plan: &ExecutionPlan,
    check: &CheckPlan,
    root: &Path,
    current: &Path,
    leaf: &str,
    command: Vec<String>,
    completion: CompletionMode,
    capture_manifest: bool,
    stdout_cap: usize,
    stdout_filename: &str,
    stderr_filename: &str,
) -> ProcessRequest {
    let (output_dir, scratch) =
        if let Some(case_id) = leaf.strip_prefix(&format!("{}/case/", check.name)) {
            (
                current
                    .join("checks")
                    .join(&check.name)
                    .join("cases")
                    .join(case_id),
                current
                    .join("scratch")
                    .join(&check.name)
                    .join("cases")
                    .join(case_id),
            )
        } else {
            (
                current.join("checks").join(&check.name),
                current.join("scratch").join(&check.name),
            )
        };
    ProcessRequest {
        run_id: plan.run_id.clone(),
        check_name: check.name.clone(),
        leaf_id: leaf.into(),
        command,
        cwd: root.join(&check.cwd),
        env: check.env.clone(),
        scratch,
        shared_artifacts: current.join("artifacts"),
        stdout_path: output_dir.join(stdout_filename),
        stderr_path: output_dir.join(stderr_filename),
        stdout_cap,
        stderr_cap: CHECK_LOG_CAP_BYTES,
        timeout_seconds: check.timeout_seconds,
        completion,
        capture_manifest,
    }
}

fn fanout_setup_failure(
    check: &CheckPlan,
    process: ProcessResult,
    output: OutputStats,
    duration: Duration,
) -> CheckOutcome {
    let status: LeafStatus = process.status.into();
    let reason = process
        .reason
        .unwrap_or_else(|| "case discovery failed".into());
    simple_outcome(check, status, &reason, output, duration)
}

fn simple_outcome(
    check: &CheckPlan,
    status: LeafStatus,
    reason: &str,
    output: OutputStats,
    duration: Duration,
) -> CheckOutcome {
    CheckOutcome {
        status,
        exit_code: None,
        reason: Some(bounded_reason(reason)),
        artifacts: Vec::new(),
        output,
        case_count: 0,
        cases: Vec::new(),
        cases_truncated: false,
        failures: vec![FailureIndexEntry {
            check: Some(check.name.clone()),
            case: None,
            status,
            reason: bounded_reason(reason),
            output_ref: Some(format!("checks/{}", check.name)),
        }],
        service: None,
        duration_seconds: seconds(duration),
    }
}

fn apply_outcome(
    runtime: &mut CheckRuntime,
    mut outcome: CheckOutcome,
    services: &mut JoinSet<(String, ServiceExit)>,
    service_pgids: &mut BTreeMap<String, i32>,
) {
    runtime.report.status = outcome.status;
    runtime.report.finished_at = Some(iso_now());
    runtime.report.duration_seconds = Some(outcome.duration_seconds);
    runtime.report.exit_code = outcome.exit_code;
    runtime.report.reason = outcome.reason;
    runtime.report.artifacts = outcome.artifacts;
    apply_output(&mut runtime.report, &outcome.output);
    runtime.report.case_count = outcome.case_count;
    runtime.report.cases = outcome.cases;
    runtime.report.cases_truncated = outcome.cases_truncated;
    runtime.failures = outcome.failures;
    if let Some(service) = outcome.service.take() {
        let name = runtime.plan.name.clone();
        service_pgids.insert(name.clone(), service.pgid);
        services.spawn(async move { (name, service.future.await) });
    }
}

fn mark_service_exit(
    checks: &mut [CheckRuntime],
    name: &str,
    exit: ServiceExit,
    cleanup: bool,
) -> Result<(), ExecutorError> {
    let index = check_index(checks, name)?;
    let runtime = &mut checks[index];
    runtime.report.exit_code = exit.exit_code;
    if let Ok(output) = exit.output {
        apply_output(&mut runtime.report, &output);
    }
    if !cleanup {
        runtime.report.status = LeafStatus::Unsafe;
        runtime.report.reason = Some("long-lived check exited after its completion event".into());
        runtime.failures.push(FailureIndexEntry {
            check: Some(name.into()),
            case: None,
            status: LeafStatus::Unsafe,
            reason: "long-lived check exited after its completion event".into(),
            output_ref: Some(runtime.report.output_ref.clone()),
        });
    }
    Ok(())
}

async fn stop_services(
    services: &mut JoinSet<(String, ServiceExit)>,
    service_pgids: &mut BTreeMap<String, i32>,
    checks: &mut [CheckRuntime],
) -> Result<(), ExecutorError> {
    for pgid in service_pgids.values().copied() {
        signal_group(pgid, Signal::TERM)?;
    }
    let deadline = tokio::time::Instant::now() + SERVICE_TERMINATION_GRACE;
    while !services.is_empty() {
        match tokio::time::timeout_at(deadline, services.join_next()).await {
            Ok(Some(joined)) => {
                let (name, exit) = joined.map_err(|error| {
                    ExecutorError::new(format!("event service task failed: {error}"))
                })?;
                service_pgids.remove(&name);
                mark_service_exit(checks, &name, exit, true)?;
            }
            Ok(None) => break,
            Err(_) => {
                for pgid in service_pgids.values().copied() {
                    signal_group(pgid, Signal::KILL)?;
                }
                while let Some(joined) = services.join_next().await {
                    let (name, exit) = joined.map_err(|error| {
                        ExecutorError::new(format!("event service task failed: {error}"))
                    })?;
                    service_pgids.remove(&name);
                    mark_service_exit(checks, &name, exit, true)?;
                }
                break;
            }
        }
    }
    Ok(())
}

fn mark_pending_cancelled(checks: &mut [CheckRuntime], reason: &str) {
    for check in checks {
        if check.report.status == LeafStatus::Pending {
            finish_blocked(check, LeafStatus::Cancelled, reason.into());
        }
    }
}

fn abort_reason(abort: Option<&Abort>) -> &str {
    match abort {
        Some(Abort::Stopped(reason) | Abort::Unsafe(reason)) => reason,
        Some(Abort::Cancelled) | None => "run cancelled",
    }
}

#[allow(clippy::too_many_arguments)]
fn write_report(
    path: &Path,
    plan: &ExecutionPlan,
    checks: &[CheckRuntime],
    capacity: &Mutex<CapacityReport>,
    started_at: &str,
    started: Instant,
    status: RunStatus,
    finished_at: Option<String>,
    source_changed: bool,
    source_error: Option<String>,
    unsafe_reason: Option<String>,
) -> Result<ExecutionReport, ExecutorError> {
    let mut failures = Vec::new();
    for check in checks {
        failures.extend(check.failures.iter().cloned());
    }
    if source_changed {
        failures.push(FailureIndexEntry {
            check: None,
            case: None,
            status: if source_error.is_some() {
                LeafStatus::Unsafe
            } else {
                LeafStatus::Failed
            },
            reason: bounded_reason(
                source_error
                    .as_deref()
                    .unwrap_or("repository source changed during the run"),
            ),
            output_ref: None,
        });
    }
    let failure_index_truncated = failures.len() > MAX_FAILURE_INDEX;
    failures.truncate(MAX_FAILURE_INDEX);
    let mut counts: BTreeMap<String, u32> = [
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
    ]
    .into_iter()
    .map(|name| (name.to_owned(), 0))
    .collect();
    for check in checks {
        let key = status_key(check.report.status).to_owned();
        *counts.entry(key).or_insert(0) += 1;
    }
    let report = ExecutionReport {
        schema: Schema2,
        run_id: plan.run_id.clone(),
        test: plan.test.clone(),
        requested_tier: plan.requested_tier,
        readiness_eligible: plan.readiness_eligible,
        proof: plan.proof,
        selection: plan.selection.clone(),
        origin_run_id: plan.origin_run_id.clone(),
        status,
        started_at: started_at.to_owned(),
        finished_at,
        duration_seconds: seconds(started.elapsed()),
        source_digest: plan.source_digest.clone(),
        config_digest: plan.config_digest.clone(),
        source_changed,
        unsafe_reason: unsafe_reason.map(|value| bounded_reason(&value)),
        capacity: capacity
            .lock()
            .map_err(|_| ExecutorError::new("capacity report lock poisoned"))?
            .clone(),
        counts,
        checks: checks.iter().map(|check| check.report.clone()).collect(),
        failure_index: failures,
        failure_index_truncated,
    };
    write_json_atomic(path, &report)?;
    Ok(report)
}

fn observe_capacity(capacity: &Mutex<CapacityReport>, observation: CapacityObservation) {
    if let Ok(mut current) = capacity.lock() {
        if observation.learned_capacity.is_some() {
            current.learned_capacity = observation.learned_capacity;
        }
        if observation.effective_capacity.is_some() {
            current.effective_capacity = observation.effective_capacity;
        }
        if observation.waited {
            current.capacity_wait_count = current.capacity_wait_count.saturating_add(1);
        }
    }
}

fn apply_output(report: &mut CheckReport, output: &OutputStats) {
    report.stdout_bytes_observed = output.stdout_bytes_observed;
    report.stdout_bytes_retained = output.stdout_bytes_retained;
    report.stdout_truncated = output.stdout_truncated;
    report.stderr_bytes_observed = output.stderr_bytes_observed;
    report.stderr_bytes_retained = output.stderr_bytes_retained;
    report.stderr_truncated = output.stderr_truncated;
}

fn add_output(total: &mut OutputStats, value: &OutputStats) {
    total.stdout_bytes_observed = total
        .stdout_bytes_observed
        .saturating_add(value.stdout_bytes_observed);
    total.stdout_bytes_retained = total
        .stdout_bytes_retained
        .saturating_add(value.stdout_bytes_retained);
    total.stdout_truncated |= value.stdout_truncated;
    total.stderr_bytes_observed = total
        .stderr_bytes_observed
        .saturating_add(value.stderr_bytes_observed);
    total.stderr_bytes_retained = total
        .stderr_bytes_retained
        .saturating_add(value.stderr_bytes_retained);
    total.stderr_truncated |= value.stderr_truncated;
}

fn check_index(checks: &[CheckRuntime], name: &str) -> Result<usize, ExecutorError> {
    checks
        .iter()
        .position(|check| check.plan.name == name)
        .ok_or_else(|| ExecutorError::new(format!("unknown active check {name:?}")))
}

fn bounded_reason(value: &str) -> String {
    let mut result = String::new();
    for character in value.chars() {
        if result.len() + character.len_utf8() > MAX_REASON_BYTES {
            break;
        }
        result.push(character);
    }
    result
}

fn status_key(status: LeafStatus) -> &'static str {
    match status {
        LeafStatus::Pending => "pending",
        LeafStatus::Running => "running",
        LeafStatus::Reused => "reused",
        LeafStatus::Passed => "passed",
        LeafStatus::Failed => "failed",
        LeafStatus::TimedOut => "timed_out",
        LeafStatus::Invalidated => "invalidated",
        LeafStatus::NotMeaningful => "not_meaningful",
        LeafStatus::Cancelled => "cancelled",
        LeafStatus::Unsafe => "unsafe",
    }
}

fn iso_now() -> String {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = since_epoch.as_secs();
    let days = i64::try_from(seconds / 86_400).unwrap_or(i64::MAX);
    let day_seconds = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = day_seconds / 3600;
    let minute = (day_seconds % 3600) / 60;
    let second = day_seconds % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn millis(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

fn seconds(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 1000.0).round() / 1000.0
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let shifted = days_since_epoch.saturating_add(719_468);
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted.saturating_sub(146_096)
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::civil_from_days;

    #[test]
    fn civil_date_conversion_matches_epoch_and_leap_day() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }
}
