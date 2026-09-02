use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use devcoordinator2_executor_protocol::{
    ArtifactReceipt, CapacityReport, CaseManifest, CaseReport, CaseSpec, CheckPlan, CheckReport,
    CompletionMode, DiagnosticExit, DiagnosticOrigin, DiagnosticReportSource, ErrorCategory,
    ExecutionPlan, ExecutionReport, FailureIndexEntry, FailureMode, LeafStatus, LogPhase, LogRef,
    LogStream, LogStreamSummary, MAX_DIAGNOSTIC_EVENTS, MAX_MANIFEST_BYTES, MAX_REASON_BYTES,
    RunStatus, Schema2, TerminationReason,
};
use rustix::process::Signal;
use tokio::task::JoinSet;

use crate::ExecutorError;
use crate::capacity::{CapacityObservation, PermitProvider};
use crate::diagnostics::{
    DiagnosticContext, diagnostic_fingerprint, normalize_diagnostics, parse_declared_report,
};
use crate::evidence::{
    artifact_receipts, receipts_match, source_digest, write_bytes_atomic, write_json_atomic,
};
use crate::process::{
    Cancellation, EventService, ProcessRequest, ProcessResult, ProcessStatus, ServiceExit,
    run_process, signal_group,
};
use crate::{LeafLogMetadata, LeafSelector, RunLogLease, RunLogMetadata, StreamMetadata};

const SERVICE_TERMINATION_GRACE: Duration = Duration::from_secs(2);
const MAX_INLINE_CASES: usize = 128;
const MAX_CASE_EVIDENCE_BYTES: usize = 4 * 1024 * 1024;
const MAX_DECLARED_DIAGNOSTIC_REPORT_BYTES: u64 = 16 * 1024 * 1024;

pub struct Executor {
    plan: ExecutionPlan,
    permits: Arc<dyn PermitProvider>,
    cancellation: Cancellation,
}

struct RunMetadataGuard {
    lease: Arc<RunLogLease>,
    run_id: String,
    test: String,
    started_at_epoch_ms: u64,
    finalized: bool,
}

impl RunMetadataGuard {
    fn start(
        lease: Arc<RunLogLease>,
        run_id: &str,
        test: &str,
        started_at_epoch_ms: u64,
    ) -> Result<Self, ExecutorError> {
        let guard = Self {
            lease,
            run_id: run_id.to_owned(),
            test: test.to_owned(),
            started_at_epoch_ms,
            finalized: false,
        };
        if let Err(error) = guard.lease.publish_run_metadata(&RunLogMetadata {
            schema: 2,
            run_id: run_id.to_owned(),
            test: test.to_owned(),
            started_at_epoch_ms,
            finished_at_epoch_ms: None,
            status: RunStatus::Running,
            complete: false,
        }) {
            let error = ExecutorError::new(error.to_string());
            drop(guard);
            return Err(error);
        }
        Ok(guard)
    }

    fn finish(&mut self, status: RunStatus, complete: bool) -> Result<(), ExecutorError> {
        self.lease
            .publish_run_metadata(&RunLogMetadata {
                schema: 2,
                run_id: self.run_id.clone(),
                test: self.test.clone(),
                started_at_epoch_ms: self.started_at_epoch_ms,
                finished_at_epoch_ms: Some(epoch_ms()),
                status,
                complete,
            })
            .map_err(|error| ExecutorError::new(error.to_string()))?;
        self.finalized = true;
        Ok(())
    }
}

impl Drop for RunMetadataGuard {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }
        let _ = self.lease.publish_run_metadata(&RunLogMetadata {
            schema: 2,
            run_id: self.run_id.clone(),
            test: self.test.clone(),
            started_at_epoch_ms: self.started_at_epoch_ms,
            finished_at_epoch_ms: Some(epoch_ms()),
            status: RunStatus::Failed,
            complete: false,
        });
    }
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
        let root_requested = PathBuf::from(&plan.worktree_root);
        let current_requested = PathBuf::from(&plan.current_dir);
        let log_requested = PathBuf::from(&plan.log_dir);
        if !current_requested.starts_with(&root_requested) {
            return Err(ExecutorError::new(
                "executor current_dir is outside the requested worktree",
            ));
        }
        let root = root_requested
            .canonicalize()
            .map_err(|error| ExecutorError::new(format!("cannot resolve worktree: {error}")))?;
        if !log_requested.starts_with(&root_requested) {
            return Err(ExecutorError::new(
                "executor log_dir is outside the requested worktree",
            ));
        }
        let log_dir = Arc::new(log_requested.canonicalize().map_err(|error| {
            ExecutorError::new(format!("cannot resolve pre-created log directory: {error}"))
        })?);
        if !log_dir.starts_with(&root) {
            return Err(ExecutorError::new(
                "executor log_dir resolves outside the worktree",
            ));
        }
        let log_lease = Arc::new(
            RunLogLease::acquire(&log_dir, &plan.run_id)
                .map_err(|error| ExecutorError::new(error.to_string()))?,
        );
        let started_at = iso_now();
        let started_epoch_ms = epoch_ms();
        let started = Instant::now();
        let mut run_metadata = RunMetadataGuard::start(
            log_lease.clone(),
            &plan.run_id,
            &plan.test,
            started_epoch_ms,
        )?;
        let current = current_requested.canonicalize().map_err(|error| {
            ExecutorError::new(format!("cannot resolve pre-created run directory: {error}"))
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

        let capacity = Arc::new(Mutex::new(CapacityReport::default()));
        let mut checks = initialize_checks(&plan, &root, &started_at)?;
        publish_reused_leaf_metadata(&mut checks, &plan.run_id, &log_lease, started_epoch_ms);
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
                mark_service_exit(&mut checks, &plan, &log_lease, &log_dir, &name, exit, false)?;
                if abort.is_none() {
                    abort = Some(Abort::Unsafe(format!(
                        "required long-lived check {name} exited"
                    )));
                    self.cancellation.cancel();
                }
            }
            mark_blocked(&mut checks, &invalidators, &plan.run_id, &log_lease);
            if self.cancellation.is_cancelled() && abort.is_none() {
                abort = Some(Abort::Cancelled);
            }
            if abort.is_some() {
                cancellation_seen = true;
                mark_pending_cancelled(
                    &mut checks,
                    &plan.run_id,
                    &log_lease,
                    abort_reason(abort.as_ref()),
                );
            }

            if abort.is_none() {
                let ready = ready_checks(&checks, &invalidators);
                for name in ready {
                    let index = check_index(&checks, &name)?;
                    let runtime = &mut checks[index];
                    runtime.report.status = LeafStatus::Running;
                    runtime.report.started_at = Some(iso_now());
                    runtime.started_epoch_ms = Some(epoch_ms());
                    let check = runtime.plan.clone();
                    let plan = plan.clone();
                    let root = root.clone();
                    let current = current.clone();
                    let permits = self.permits.clone();
                    let cancellation = self.cancellation.clone();
                    let capacity = capacity.clone();
                    let log_lease = log_lease.clone();
                    let log_dir = log_dir.clone();
                    running.spawn(async move {
                        let outcome = execute_check(
                            plan,
                            check,
                            root,
                            current,
                            permits,
                            cancellation,
                            capacity,
                            log_lease,
                            log_dir,
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
                    mark_service_exit(
                        &mut checks,
                        &plan,
                        &log_lease,
                        &log_dir,
                        &name,
                        exit,
                        false,
                    )?;
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

        stop_services(
            &mut services,
            &mut service_pgids,
            &mut checks,
            &plan,
            &log_lease,
            &log_dir,
        )
        .await?;
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
        let finished_at = iso_now();
        let mut report = write_report(
            &report_path,
            &plan,
            &checks,
            &capacity,
            &started_at,
            started,
            status,
            Some(finished_at),
            source_changed,
            source_error,
        )?;
        if run_metadata.finish(status, true).is_err() {
            report.status = RunStatus::Failed;
            report.failure_index.push(failure_entry(
                &plan.run_id,
                None,
                None,
                LeafStatus::Unsafe,
                None,
                Some(TerminationReason::UnsafeStop),
                ErrorCategory::LogStorage,
                None,
            ));
            let normalized = normalize_diagnostics(report.failure_index)
                .map_err(|_| ExecutorError::new("cannot normalize run log storage failure"))?;
            report.failure_index = normalized.entries;
            report.failure_index_truncated |= normalized.truncated;
            write_json_atomic(&report_path, &report)?;
        }
        Ok(report)
    }
}

struct CheckRuntime {
    plan: CheckPlan,
    report: CheckReport,
    failures: Vec<FailureIndexEntry>,
    started_epoch_ms: Option<u64>,
}

struct CheckOutcome {
    status: LeafStatus,
    exit_code: Option<i32>,
    reason: Option<String>,
    artifacts: Vec<ArtifactReceipt>,
    streams: Vec<LogStreamSummary>,
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
    failures: Vec<FailureIndexEntry>,
}

enum Abort {
    Cancelled,
    Stopped(String),
    Unsafe(String),
}

#[allow(clippy::too_many_arguments)]
fn failure_entry(
    run_id: &str,
    check: Option<&str>,
    case: Option<&str>,
    status: LeafStatus,
    exit_code: Option<i32>,
    termination_reason: Option<TerminationReason>,
    error_category: ErrorCategory,
    phase: Option<LogPhase>,
) -> FailureIndexEntry {
    let exit = match exit_code {
        Some(value) if value < 0 => DiagnosticExit {
            code: None,
            signal: u8::try_from(value.unsigned_abs()).ok(),
        },
        code => DiagnosticExit { code, signal: None },
    };
    let log_refs = match (check, phase) {
        (Some(check), Some(phase)) => [LogStream::Stdout, LogStream::Stderr]
            .into_iter()
            .map(|stream| LogRef {
                run_id: run_id.to_owned(),
                check: Some(check.to_owned()),
                phase,
                case: case.map(str::to_owned),
                stream,
            })
            .collect(),
        _ => Vec::new(),
    };
    let mut entry = FailureIndexEntry {
        check: check.map(str::to_owned),
        case: case.map(str::to_owned),
        status,
        exit,
        termination_reason,
        source: None,
        error_category,
        expected: None,
        actual: None,
        fingerprint: String::new(),
        occurrences: 1,
        log_refs,
        origin: DiagnosticOrigin::Executor,
    };
    entry.fingerprint = diagnostic_fingerprint(&entry);
    entry
}

fn process_failure_semantics(
    process: &ProcessResult,
    completion: CompletionMode,
) -> (Option<TerminationReason>, ErrorCategory) {
    if process.log_storage_failed {
        return (
            Some(TerminationReason::UnsafeStop),
            ErrorCategory::LogStorage,
        );
    }
    if process.structured_evidence_invalid {
        return (
            Some(TerminationReason::UnsafeStop),
            ErrorCategory::StructuredEvidenceInvalid,
        );
    }
    if completion == CompletionMode::Event
        && process.status == ProcessStatus::Failed
        && !process.reason.as_deref().is_some_and(|reason| {
            reason.contains("before its completion event")
                || reason.contains("event process exited")
        })
    {
        return (None, ErrorCategory::Exception);
    }
    leaf_failure_semantics(
        process.status.into(),
        process.exit_code,
        process.reason.as_deref(),
    )
}

fn leaf_failure_semantics(
    status: LeafStatus,
    exit_code: Option<i32>,
    reason: Option<&str>,
) -> (Option<TerminationReason>, ErrorCategory) {
    match status {
        LeafStatus::Failed if exit_code.is_some() => (None, ErrorCategory::ProcessExit),
        LeafStatus::Failed => (None, ErrorCategory::Internal),
        LeafStatus::TimedOut => (
            Some(TerminationReason::DeadlineExceeded),
            ErrorCategory::Timeout,
        ),
        LeafStatus::Cancelled => (
            Some(TerminationReason::RunCancelled),
            ErrorCategory::Cancellation,
        ),
        LeafStatus::Unsafe
            if reason.is_some_and(|reason| {
                reason.contains("leaf output")
                    || reason.contains("retain leaf output")
                    || reason.contains("sync leaf output")
                    || reason.contains("log storage")
                    || reason.contains("leaf log metadata")
                    || reason.contains("stdout task")
                    || reason.contains("stderr task")
            }) =>
        {
            (
                Some(TerminationReason::UnsafeStop),
                ErrorCategory::LogStorage,
            )
        }
        LeafStatus::Unsafe if reason.is_some_and(|reason| reason.contains("event")) => (
            Some(TerminationReason::UnsafeStop),
            ErrorCategory::StructuredEvidenceInvalid,
        ),
        LeafStatus::Unsafe => (Some(TerminationReason::UnsafeStop), ErrorCategory::Internal),
        LeafStatus::Invalidated | LeafStatus::NotMeaningful => (None, ErrorCategory::Dependency),
        LeafStatus::Pending | LeafStatus::Running | LeafStatus::Reused | LeafStatus::Passed => {
            (None, ErrorCategory::Internal)
        }
    }
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
                exit: DiagnosticExit::default(),
                artifacts: reused.cloned().unwrap_or_default(),
                streams: Vec::new(),
                case_count: 0,
                cases: Vec::new(),
                cases_truncated: false,
            },
            failures: Vec::new(),
            started_epoch_ms: reused.map(|_| epoch_ms()),
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

fn publish_reused_leaf_metadata(
    checks: &mut [CheckRuntime],
    run_id: &str,
    log_lease: &RunLogLease,
    started_epoch_ms: u64,
) {
    for check in checks {
        if check.report.status != LeafStatus::Reused {
            continue;
        }
        let selector = LeafSelector::check(check.plan.name.clone()).expect("validated selector");
        if log_lease
            .publish_leaf_metadata(&LeafLogMetadata {
                schema: 2,
                selector,
                status: LeafStatus::Reused,
                exit: DiagnosticExit::default(),
                started_at_epoch_ms: started_epoch_ms,
                finished_at_epoch_ms: Some(started_epoch_ms),
                process_started: false,
                complete: true,
                structured_evidence_formats: Vec::new(),
                structured_evidence_count: 0,
            })
            .is_err()
        {
            check.report.status = LeafStatus::Unsafe;
            check.failures.push(failure_entry(
                run_id,
                Some(&check.plan.name),
                None,
                LeafStatus::Unsafe,
                None,
                Some(TerminationReason::UnsafeStop),
                ErrorCategory::LogStorage,
                Some(LogPhase::Check),
            ));
        }
    }
}

fn mark_blocked(
    checks: &mut [CheckRuntime],
    invalidators: &BTreeMap<String, Vec<String>>,
    run_id: &str,
    log_lease: &RunLogLease,
) {
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
                    run_id,
                    log_lease,
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
                run_id,
                log_lease,
                LeafStatus::NotMeaningful,
                format!("required checks did not pass: {}", dependencies.join(", ")),
            );
        }
    }
}

fn finish_blocked(
    check: &mut CheckRuntime,
    run_id: &str,
    log_lease: &RunLogLease,
    status: LeafStatus,
    _reason: String,
) {
    check.report.status = status;
    check.report.finished_at = Some(iso_now());
    check.report.duration_seconds = Some(0.0);
    let (termination_reason, error_category) = match status {
        LeafStatus::Invalidated | LeafStatus::NotMeaningful => (None, ErrorCategory::Dependency),
        LeafStatus::Cancelled => (
            Some(TerminationReason::RunCancelled),
            ErrorCategory::Cancellation,
        ),
        LeafStatus::TimedOut => (
            Some(TerminationReason::DeadlineExceeded),
            ErrorCategory::Timeout,
        ),
        LeafStatus::Unsafe => (Some(TerminationReason::UnsafeStop), ErrorCategory::Internal),
        _ => (None, ErrorCategory::Internal),
    };
    let selector = LeafSelector::check(check.plan.name.clone()).expect("validated selector");
    let mut error_category = error_category;
    let mut termination_reason = termination_reason;
    if log_lease
        .publish_leaf_metadata(&LeafLogMetadata {
            schema: 2,
            selector,
            status,
            exit: DiagnosticExit::default(),
            started_at_epoch_ms: check.started_epoch_ms.unwrap_or_else(epoch_ms),
            finished_at_epoch_ms: Some(epoch_ms()),
            process_started: false,
            complete: true,
            structured_evidence_formats: Vec::new(),
            structured_evidence_count: 0,
        })
        .is_err()
    {
        check.report.status = LeafStatus::Unsafe;
        error_category = ErrorCategory::LogStorage;
        termination_reason = Some(TerminationReason::UnsafeStop);
    }
    check.failures.push(failure_entry(
        run_id,
        Some(&check.plan.name),
        None,
        check.report.status,
        None,
        termination_reason,
        error_category,
        None,
    ));
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

#[allow(clippy::too_many_arguments)]
async fn execute_check(
    plan: Arc<ExecutionPlan>,
    check: CheckPlan,
    root: PathBuf,
    current: PathBuf,
    permits: Arc<dyn PermitProvider>,
    cancellation: Cancellation,
    capacity: Arc<Mutex<CapacityReport>>,
    log_lease: Arc<RunLogLease>,
    log_dir: Arc<PathBuf>,
) -> CheckOutcome {
    let started = Instant::now();
    let started_epoch_ms = epoch_ms();
    if let Some(command) = &check.command {
        let selector = LeafSelector::check(check.name.clone()).expect("validated check selector");
        let request = process_request(
            &plan,
            &check,
            &root,
            &current,
            log_lease.clone(),
            &log_dir,
            selector.clone(),
            &check.name,
            command.clone(),
            check.completion,
            false,
        );
        let mut process = run_process(request, permits, cancellation).await;
        observe_capacity(&capacity, process.capacity);
        finalize_leaf_evidence(
            &plan,
            &check,
            &selector,
            &log_lease,
            &log_dir,
            &mut process,
            started_epoch_ms,
        );
        let artifacts = if process.status == ProcessStatus::Passed {
            match artifact_receipts(&root, &check.produces) {
                Ok(receipts) => receipts,
                Err(error) => {
                    process.status = ProcessStatus::Failed;
                    process.reason = Some(error.to_string());
                    record_leaf_postprocess_failure(
                        &plan,
                        &check,
                        &selector,
                        &log_lease,
                        &mut process,
                        started_epoch_ms,
                        LeafStatus::Failed,
                        ErrorCategory::Artifact,
                        None,
                    );
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        direct_outcome(process, artifacts, started.elapsed())
    } else {
        execute_fanout(
            &plan,
            &check,
            &root,
            &current,
            permits,
            cancellation,
            capacity,
            log_lease,
            log_dir,
            started,
            started_epoch_ms,
        )
        .await
    }
}

fn direct_outcome(
    mut process: ProcessResult,
    artifacts: Vec<ArtifactReceipt>,
    duration: Duration,
) -> CheckOutcome {
    let status: LeafStatus = process.status.into();
    let failures = std::mem::take(&mut process.diagnostics);
    CheckOutcome {
        status,
        exit_code: process.exit_code,
        reason: process.reason.take().map(|value| bounded_reason(&value)),
        artifacts,
        streams: process.streams,
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
    log_lease: Arc<RunLogLease>,
    log_dir: Arc<PathBuf>,
    started: Instant,
    _started_epoch_ms: u64,
) -> CheckOutcome {
    let mut streams = Vec::new();
    let mut cases = if let Some(static_cases) = &check.cases {
        static_cases.clone()
    } else {
        let discovery_started_epoch_ms = epoch_ms();
        let selector =
            LeafSelector::discovery(check.name.clone()).expect("validated discovery selector");
        let request = process_request(
            plan,
            check,
            root,
            current,
            log_lease.clone(),
            &log_dir,
            selector.clone(),
            &format!("{}/discovery", check.name),
            check.discover.clone().unwrap_or_default(),
            CompletionMode::Process,
            true,
        );
        let mut discovery = run_process(request, permits.clone(), cancellation.clone()).await;
        observe_capacity(&capacity, discovery.capacity);
        finalize_leaf_evidence(
            plan,
            check,
            &selector,
            &log_lease,
            &log_dir,
            &mut discovery,
            discovery_started_epoch_ms,
        );
        streams.extend(discovery.streams.iter().cloned());
        if discovery.status != ProcessStatus::Passed {
            return fanout_setup_failure(
                &plan.run_id,
                check,
                discovery,
                streams,
                started.elapsed(),
            );
        }
        let Some(manifest_capture) = discovery.manifest.take() else {
            return discovery_postprocess_failure(
                plan,
                check,
                &selector,
                &log_lease,
                discovery,
                discovery_started_epoch_ms,
                streams,
                started.elapsed(),
                LeafStatus::Failed,
                "case discovery closed without a descriptor manifest".into(),
                ErrorCategory::StructuredEvidenceInvalid,
                None,
            );
        };
        if manifest_capture.truncated || manifest_capture.observed > MAX_MANIFEST_BYTES as u64 {
            return discovery_postprocess_failure(
                plan,
                check,
                &selector,
                &log_lease,
                discovery,
                discovery_started_epoch_ms,
                streams,
                started.elapsed(),
                LeafStatus::Failed,
                "case manifest descriptor exceeds 2 MiB".into(),
                ErrorCategory::StructuredEvidenceInvalid,
                None,
            );
        }
        if manifest_capture.payload.is_empty() {
            return discovery_postprocess_failure(
                plan,
                check,
                &selector,
                &log_lease,
                discovery,
                discovery_started_epoch_ms,
                streams,
                started.elapsed(),
                LeafStatus::Failed,
                "case discovery descriptor closed without a manifest".into(),
                ErrorCategory::StructuredEvidenceInvalid,
                None,
            );
        }
        let path = current
            .join("checks")
            .join(&check.name)
            .join("manifest.json");
        if let Err(error) = write_bytes_atomic(&path, &manifest_capture.payload, MAX_MANIFEST_BYTES)
        {
            return discovery_postprocess_failure(
                plan,
                check,
                &selector,
                &log_lease,
                discovery,
                discovery_started_epoch_ms,
                streams,
                started.elapsed(),
                LeafStatus::Unsafe,
                error.to_string(),
                ErrorCategory::LogStorage,
                Some(TerminationReason::UnsafeStop),
            );
        }
        match CaseManifest::from_json(&manifest_capture.payload) {
            Ok(manifest) => manifest.cases,
            Err(error) => {
                return discovery_postprocess_failure(
                    plan,
                    check,
                    &selector,
                    &log_lease,
                    discovery,
                    discovery_started_epoch_ms,
                    streams,
                    started.elapsed(),
                    LeafStatus::Failed,
                    error.to_string(),
                    ErrorCategory::StructuredEvidenceInvalid,
                    None,
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
        let log_lease = log_lease.clone();
        let log_dir = log_dir.clone();
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
                log_lease,
                log_dir,
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
        failures.extend(outcome.failures.iter().cloned());
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
        failures.push(failure_entry(
            &plan.run_id,
            Some(&check.name),
            None,
            status,
            None,
            Some(TerminationReason::UnsafeStop),
            ErrorCategory::Internal,
            None,
        ));
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
        failures.push(failure_entry(
            &plan.run_id,
            Some(&check.name),
            None,
            LeafStatus::Unsafe,
            None,
            Some(TerminationReason::UnsafeStop),
            ErrorCategory::LogStorage,
            None,
        ));
    }
    let artifacts = if final_status == LeafStatus::Passed {
        match artifact_receipts(root, &check.produces) {
            Ok(receipts) => receipts,
            Err(error) => {
                final_status = LeafStatus::Failed;
                final_reason = Some(error.to_string());
                failures.push(failure_entry(
                    &plan.run_id,
                    Some(&check.name),
                    None,
                    LeafStatus::Failed,
                    None,
                    None,
                    ErrorCategory::Artifact,
                    None,
                ));
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
        streams,
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
    log_lease: Arc<RunLogLease>,
    log_dir: Arc<PathBuf>,
) -> CaseOutcome {
    let started = Instant::now();
    let started_epoch_ms = epoch_ms();
    let mut command = base_command.to_vec();
    command.extend(case.args.clone());
    let leaf = format!("{}/case/{}", check.name, case.id);
    let selector =
        LeafSelector::case(check.name.clone(), case.id.clone()).expect("validated case selector");
    let request = process_request(
        plan,
        check,
        root,
        current,
        log_lease.clone(),
        &log_dir,
        selector.clone(),
        &leaf,
        command,
        CompletionMode::Process,
        false,
    );
    let mut process = run_process(request, permits, cancellation).await;
    observe_capacity(&capacity, process.capacity);
    finalize_leaf_evidence(
        plan,
        check,
        &selector,
        &log_lease,
        &log_dir,
        &mut process,
        started_epoch_ms,
    );
    CaseOutcome {
        report: CaseReport {
            id: case.id.clone(),
            status: process.status.into(),
            exit: diagnostic_exit(process.exit_code),
            duration_ms: millis(started.elapsed()),
            streams: process.streams,
        },
        failures: process.diagnostics,
    }
}

#[allow(clippy::too_many_arguments)]
fn process_request(
    plan: &ExecutionPlan,
    check: &CheckPlan,
    root: &Path,
    current: &Path,
    log_lease: Arc<RunLogLease>,
    log_dir: &Path,
    log_selector: LeafSelector,
    leaf: &str,
    command: Vec<String>,
    completion: CompletionMode,
    capture_manifest: bool,
) -> ProcessRequest {
    let scratch = if let Some(case_id) = leaf.strip_prefix(&format!("{}/case/", check.name)) {
        current
            .join("scratch")
            .join(&check.name)
            .join("cases")
            .join(case_id)
    } else {
        current.join("scratch").join(&check.name)
    };
    let diagnostics_dir = leaf_log_directory(log_dir, &log_selector).join("diagnostics");
    let evidence_dir = leaf_log_directory(log_dir, &log_selector).join("evidence");
    ProcessRequest {
        run_id: plan.run_id.clone(),
        check_name: check.name.clone(),
        leaf_id: leaf.into(),
        command,
        cwd: root.join(&check.cwd),
        env: check.env.clone(),
        scratch,
        shared_artifacts: current.join("artifacts"),
        diagnostics_dir,
        evidence_dir,
        log_lease,
        log_selector,
        timeout_seconds: check.timeout_seconds,
        completion,
        capture_manifest,
    }
}

fn finalize_leaf_evidence(
    plan: &ExecutionPlan,
    check: &CheckPlan,
    selector: &LeafSelector,
    log_lease: &RunLogLease,
    log_dir: &Path,
    process: &mut ProcessResult,
    started_at_epoch_ms: u64,
) {
    if process.service.is_some() {
        return;
    }
    supplement_stream_summaries(plan, log_dir, selector, &mut process.streams);
    if process.process_started
        && (process.streams.len() != 2 || process.streams.iter().any(|stream| !stream.complete))
    {
        process.status = ProcessStatus::Unsafe;
        process.log_storage_failed = true;
        process.reason = Some("log storage became incomplete".into());
    }
    let context = DiagnosticContext {
        run_id: plan.run_id.clone(),
        check: check.name.clone(),
        case: selector.case_id.clone(),
        phase: selector.phase,
    };
    let mut formats = Vec::new();
    let mut invalid = process.structured_evidence_invalid;
    for source in &check.diagnostic_sources {
        match read_declared_diagnostics(log_lease, selector, source, &context) {
            Ok(entries) => {
                formats.push(source.format);
                process.diagnostics.extend(entries);
            }
            Err(()) => invalid = true,
        }
    }
    if !process.diagnostics.is_empty() && process.status == ProcessStatus::Passed {
        process.status = ProcessStatus::Failed;
    }
    if invalid {
        process.status = ProcessStatus::Unsafe;
        process.structured_evidence_invalid = true;
        process.reason = Some("structured diagnostic evidence is invalid".into());
        process.diagnostics.push(failure_entry(
            &plan.run_id,
            Some(&check.name),
            selector.case_id.as_deref(),
            LeafStatus::Unsafe,
            process.exit_code,
            Some(TerminationReason::UnsafeStop),
            ErrorCategory::StructuredEvidenceInvalid,
            Some(selector.phase),
        ));
    }
    let status: LeafStatus = process.status.into();
    if !status.is_success()
        && !process
            .diagnostics
            .iter()
            .any(|entry| entry.origin == DiagnosticOrigin::Executor)
    {
        let (termination_reason, error_category) =
            process_failure_semantics(process, check.completion);
        process.diagnostics.push(failure_entry(
            &plan.run_id,
            Some(&check.name),
            selector.case_id.as_deref(),
            status,
            process.exit_code,
            termination_reason,
            error_category,
            Some(selector.phase),
        ));
    }
    match normalize_diagnostics(std::mem::take(&mut process.diagnostics)) {
        Ok(normalized) => process.diagnostics = normalized.entries,
        Err(_) => {
            process.status = ProcessStatus::Unsafe;
            process.structured_evidence_invalid = true;
            process.diagnostics = vec![failure_entry(
                &plan.run_id,
                Some(&check.name),
                selector.case_id.as_deref(),
                LeafStatus::Unsafe,
                process.exit_code,
                Some(TerminationReason::UnsafeStop),
                ErrorCategory::StructuredEvidenceInvalid,
                Some(selector.phase),
            )];
        }
    }
    if log_lease
        .publish_leaf_diagnostics(selector, &process.diagnostics)
        .is_err()
    {
        process.status = ProcessStatus::Unsafe;
        process.log_storage_failed = true;
        process.reason = Some("cannot publish structured diagnostic evidence".into());
        process.diagnostics.push(failure_entry(
            &plan.run_id,
            Some(&check.name),
            selector.case_id.as_deref(),
            LeafStatus::Unsafe,
            process.exit_code,
            Some(TerminationReason::UnsafeStop),
            ErrorCategory::LogStorage,
            Some(selector.phase),
        ));
    }
    formats.sort();
    formats.dedup();
    let complete = !process.log_storage_failed
        && (!process.process_started
            || (process.streams.len() == 2
                && process.streams.iter().all(|stream| stream.complete)));
    let metadata = LeafLogMetadata {
        schema: 2,
        selector: selector.clone(),
        status: process.status.into(),
        exit: diagnostic_exit(process.exit_code),
        started_at_epoch_ms,
        finished_at_epoch_ms: Some(epoch_ms()),
        process_started: process.process_started,
        complete,
        structured_evidence_formats: formats,
        structured_evidence_count: process.diagnostics.len() as u64,
    };
    if log_lease.publish_leaf_metadata(&metadata).is_err() {
        process.status = ProcessStatus::Unsafe;
        process.log_storage_failed = true;
        process.reason = Some("cannot publish leaf log metadata".into());
        process.diagnostics.push(failure_entry(
            &plan.run_id,
            Some(&check.name),
            selector.case_id.as_deref(),
            LeafStatus::Unsafe,
            process.exit_code,
            Some(TerminationReason::UnsafeStop),
            ErrorCategory::LogStorage,
            Some(selector.phase),
        ));
    }
}

#[allow(clippy::too_many_arguments)]
fn record_leaf_postprocess_failure(
    plan: &ExecutionPlan,
    check: &CheckPlan,
    selector: &LeafSelector,
    log_lease: &RunLogLease,
    process: &mut ProcessResult,
    started_at_epoch_ms: u64,
    status: LeafStatus,
    error_category: ErrorCategory,
    termination_reason: Option<TerminationReason>,
) {
    process.status = if status == LeafStatus::Unsafe {
        ProcessStatus::Unsafe
    } else {
        ProcessStatus::Failed
    };
    if error_category == ErrorCategory::LogStorage {
        process.log_storage_failed = true;
    }
    if error_category == ErrorCategory::StructuredEvidenceInvalid {
        process.structured_evidence_invalid = true;
    }
    process.diagnostics.push(failure_entry(
        &plan.run_id,
        Some(&check.name),
        selector.case_id.as_deref(),
        status,
        process.exit_code,
        termination_reason,
        error_category,
        Some(selector.phase),
    ));
    if let Ok(normalized) = normalize_diagnostics(std::mem::take(&mut process.diagnostics)) {
        process.diagnostics = normalized.entries;
    } else {
        process.structured_evidence_invalid = true;
    }
    let mut metadata_failed = log_lease
        .publish_leaf_diagnostics(selector, &process.diagnostics)
        .is_err();
    let mut formats: Vec<_> = check
        .diagnostic_sources
        .iter()
        .map(|source| source.format)
        .collect();
    formats.sort();
    formats.dedup();
    let complete = !process.log_storage_failed
        && (!process.process_started
            || (process.streams.len() == 2
                && process.streams.iter().all(|stream| stream.complete)));
    metadata_failed |= log_lease
        .publish_leaf_metadata(&LeafLogMetadata {
            schema: 2,
            selector: selector.clone(),
            status,
            exit: diagnostic_exit(process.exit_code),
            started_at_epoch_ms,
            finished_at_epoch_ms: Some(epoch_ms()),
            process_started: process.process_started,
            complete,
            structured_evidence_formats: formats,
            structured_evidence_count: process.diagnostics.len() as u64,
        })
        .is_err();
    if metadata_failed {
        process.status = ProcessStatus::Unsafe;
        process.log_storage_failed = true;
        process.reason = Some("cannot publish terminal leaf evidence".into());
        process.diagnostics.push(failure_entry(
            &plan.run_id,
            Some(&check.name),
            selector.case_id.as_deref(),
            LeafStatus::Unsafe,
            process.exit_code,
            Some(TerminationReason::UnsafeStop),
            ErrorCategory::LogStorage,
            Some(selector.phase),
        ));
    }
}

fn supplement_stream_summaries(
    plan: &ExecutionPlan,
    log_dir: &Path,
    selector: &LeafSelector,
    streams: &mut Vec<LogStreamSummary>,
) {
    let leaf_dir = leaf_log_directory(log_dir, selector);
    for stream in [LogStream::Stdout, LogStream::Stderr] {
        if streams
            .iter()
            .any(|summary| summary.log_ref.stream == stream)
        {
            continue;
        }
        let name = match stream {
            LogStream::Stdout => "stdout.meta.json",
            LogStream::Stderr => "stderr.meta.json",
        };
        let Ok(payload) = fs::read(leaf_dir.join(name)) else {
            continue;
        };
        let Ok(metadata) = serde_json::from_slice::<StreamMetadata>(&payload) else {
            continue;
        };
        if metadata.selector != *selector || metadata.stream != stream {
            continue;
        }
        streams.push(LogStreamSummary {
            log_ref: LogRef {
                run_id: plan.run_id.clone(),
                check: selector.check.clone(),
                phase: selector.phase,
                case: selector.case_id.clone(),
                stream,
            },
            bytes: metadata.bytes,
            lines: metadata.lines,
            sha256: metadata.sha256,
            first_write_epoch_ms: metadata.first_write_epoch_ms,
            last_write_epoch_ms: metadata.last_write_epoch_ms,
            complete: metadata.complete,
        });
    }
    streams.sort_by_key(|summary| summary.log_ref.stream);
}

fn read_declared_diagnostics(
    log_lease: &RunLogLease,
    selector: &LeafSelector,
    source: &DiagnosticReportSource,
    context: &DiagnosticContext,
) -> Result<Vec<FailureIndexEntry>, ()> {
    let relative = format!("diagnostics/{}", source.path);
    let input = log_lease
        .read_declared_report(selector, &relative, MAX_DECLARED_DIAGNOSTIC_REPORT_BYTES)
        .map_err(|_| ())?;
    parse_declared_report(source, &input, context)
        .map(|parsed| parsed.entries)
        .map_err(|_| ())
}

fn leaf_log_directory(run_dir: &Path, selector: &LeafSelector) -> PathBuf {
    match selector.phase {
        LogPhase::Executor => run_dir.join("executor"),
        LogPhase::Check | LogPhase::Discovery => run_dir
            .join("checks")
            .join(selector.check.as_deref().expect("validated check selector"))
            .join(match selector.phase {
                LogPhase::Check => "check",
                LogPhase::Discovery => "discovery",
                _ => unreachable!(),
            }),
        LogPhase::Case => run_dir
            .join("checks")
            .join(selector.check.as_deref().expect("validated case selector"))
            .join("cases")
            .join(
                selector
                    .case_id
                    .as_deref()
                    .expect("validated case selector"),
            ),
    }
}

fn fanout_setup_failure(
    _run_id: &str,
    _check: &CheckPlan,
    mut process: ProcessResult,
    streams: Vec<LogStreamSummary>,
    duration: Duration,
) -> CheckOutcome {
    let status: LeafStatus = process.status.into();
    let exit_code = process.exit_code;
    let reason = process
        .reason
        .unwrap_or_else(|| "case discovery failed".into());
    CheckOutcome {
        status,
        exit_code,
        reason: Some(bounded_reason(&reason)),
        artifacts: Vec::new(),
        streams,
        case_count: 0,
        cases: Vec::new(),
        cases_truncated: false,
        failures: std::mem::take(&mut process.diagnostics),
        service: None,
        duration_seconds: seconds(duration),
    }
}

#[allow(clippy::too_many_arguments)]
fn discovery_postprocess_failure(
    plan: &ExecutionPlan,
    check: &CheckPlan,
    selector: &LeafSelector,
    log_lease: &RunLogLease,
    mut process: ProcessResult,
    started_at_epoch_ms: u64,
    streams: Vec<LogStreamSummary>,
    duration: Duration,
    status: LeafStatus,
    reason: String,
    error_category: ErrorCategory,
    termination_reason: Option<TerminationReason>,
) -> CheckOutcome {
    record_leaf_postprocess_failure(
        plan,
        check,
        selector,
        log_lease,
        &mut process,
        started_at_epoch_ms,
        status,
        error_category,
        termination_reason,
    );
    CheckOutcome {
        status: process.status.into(),
        exit_code: process.exit_code,
        reason: Some(bounded_reason(&reason)),
        artifacts: Vec::new(),
        streams,
        case_count: 0,
        cases: Vec::new(),
        cases_truncated: false,
        failures: process.diagnostics,
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
    runtime.report.exit = diagnostic_exit(outcome.exit_code);
    let _ = outcome.reason;
    runtime.report.artifacts = outcome.artifacts;
    runtime.report.streams = outcome.streams;
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
    plan: &ExecutionPlan,
    log_lease: &RunLogLease,
    log_dir: &Path,
    name: &str,
    exit: ServiceExit,
    cleanup: bool,
) -> Result<(), ExecutorError> {
    let index = check_index(checks, name)?;
    let runtime = &mut checks[index];
    let storage_failed = exit.log_storage_failed;
    let evidence_invalid = exit.structured_evidence_invalid;
    let mut process = ProcessResult {
        status: if !cleanup || storage_failed || evidence_invalid {
            ProcessStatus::Unsafe
        } else {
            ProcessStatus::Passed
        },
        exit_code: exit.exit_code,
        reason: None,
        streams: exit.streams,
        diagnostics: exit.diagnostics,
        log_storage_failed: storage_failed,
        structured_evidence_invalid: evidence_invalid,
        process_started: true,
        service: None,
        manifest: None,
        capacity: Default::default(),
    };
    let selector = LeafSelector::check(name.to_owned()).expect("validated service selector");
    finalize_leaf_evidence(
        plan,
        &runtime.plan,
        &selector,
        log_lease,
        log_dir,
        &mut process,
        runtime.started_epoch_ms.unwrap_or_else(epoch_ms),
    );
    runtime.report.exit = diagnostic_exit(process.exit_code);
    runtime.report.streams = process.streams;
    runtime.failures.extend(process.diagnostics);
    if process.status != ProcessStatus::Passed {
        runtime.report.status = process.status.into();
    }
    if !cleanup {
        runtime.report.status = LeafStatus::Unsafe;
    }
    if process.log_storage_failed || process.structured_evidence_invalid {
        runtime.report.status = LeafStatus::Unsafe;
    }
    Ok(())
}

async fn stop_services(
    services: &mut JoinSet<(String, ServiceExit)>,
    service_pgids: &mut BTreeMap<String, i32>,
    checks: &mut [CheckRuntime],
    plan: &ExecutionPlan,
    log_lease: &RunLogLease,
    log_dir: &Path,
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
                mark_service_exit(checks, plan, log_lease, log_dir, &name, exit, true)?;
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
                    mark_service_exit(checks, plan, log_lease, log_dir, &name, exit, true)?;
                }
                break;
            }
        }
    }
    Ok(())
}

fn mark_pending_cancelled(
    checks: &mut [CheckRuntime],
    run_id: &str,
    log_lease: &RunLogLease,
    reason: &str,
) {
    for check in checks {
        if check.report.status == LeafStatus::Pending {
            finish_blocked(
                check,
                run_id,
                log_lease,
                LeafStatus::Cancelled,
                reason.into(),
            );
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
) -> Result<ExecutionReport, ExecutorError> {
    let mut failures = Vec::new();
    for check in checks {
        failures.extend(check.failures.iter().cloned());
    }
    if source_changed {
        let unsafe_failure = source_error.is_some();
        failures.push(failure_entry(
            &plan.run_id,
            None,
            None,
            if unsafe_failure {
                LeafStatus::Unsafe
            } else {
                LeafStatus::Failed
            },
            None,
            unsafe_failure.then_some(TerminationReason::UnsafeStop),
            ErrorCategory::SourceChanged,
            None,
        ));
    }
    let candidate_truncated = failures.len() > MAX_DIAGNOSTIC_EVENTS;
    failures.truncate(MAX_DIAGNOSTIC_EVENTS);
    let normalized = normalize_diagnostics(failures)
        .map_err(|error| ExecutorError::new(format!("cannot normalize diagnostics: {error}")))?;
    let failure_index_truncated = candidate_truncated || normalized.truncated;
    let failures = normalized.entries;
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

fn diagnostic_exit(exit_code: Option<i32>) -> DiagnosticExit {
    match exit_code {
        Some(value) if value < 0 => DiagnosticExit {
            code: None,
            signal: u8::try_from(value.unsigned_abs()).ok(),
        },
        code => DiagnosticExit { code, signal: None },
    }
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

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
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
