use super::*;
use crate::access::{Caller, RouteAccessSection};
use crate::automation_test_support::{Fixture, FixtureClock, START, WEEK};
use crate::control_plane::ControlPlane;
use crate::daemon::OperationExecutor;
use crate::usage_api_fixture::{KEY, Peer, capability, response};
use devcoordinator2_api::work_context::{WorkAttribution, WorkContext, WorkSource};
use serde_json::json;
use std::sync::Arc;

fn input() -> Prepare {
    Prepare {
        repository_id: "project-alpha".into(),
        workstream_id: Some("spec".into()),
        window_start_ms: START,
        window_end_ms: START + WEEK,
        offset: 0,
        limit: 10,
        before_decision_seq: None,
        outcome_cursor: None,
        outcome_limit: None,
    }
}

fn caller() -> Caller {
    Caller {
        via_edge: false,
        pid: 1,
        uid: rustix::process::getuid().as_raw(),
        gid: rustix::process::getgid().as_raw(),
        client_kind: devcoordinator2_api::ClientKind::Codex,
        model: None,
        effort: None,
        client_session: Some("fixture-thread".into()),
        identity: None,
        work: Some(WorkAttribution {
            source: WorkSource::Request,
            diagnostic: None,
            context: Some(WorkContext {
                version: 1,
                alarm: None,
                native_project_id: "fixture-project".into(),
                thread_id: "fixture-thread".into(),
                turn_id: Some("fixture-turn".into()),
                operation_id: Some("governed-op".into()),
                workstream_id: Some("spec".into()),
                outcome_id: Some("spec".into()),
                experiment_ref: Some("experiment@1".into()),
            }),
        }),
    }
}

fn service(fixture: &Fixture, source: crate::config::CodexUsageSource) -> ReviewService {
    let mut config = fixture.config.clone();
    config.codex_usage_sources = vec![source];
    ReviewService::new(
        fixture.database.clone(),
        UsageService::with_clock(
            config,
            fixture.database.clone(),
            crate::repository::Registry::new(fixture.database.clone()),
            Arc::new(FixtureClock),
        ),
    )
}

#[test]
fn review_prepare_uses_authorized_operation_context_over_real_transport_with_legacy_source_separate()
 {
    let mut fixture = Fixture::new();
    let root = fixture._temporary.path();
    let home = root.join("fresh");
    let peer = Peer::start(root, &home, |params| response(params, START + WEEK, 135));
    let legacy = root.join("retired");
    std::fs::create_dir_all(legacy.join("usage")).unwrap();
    let sentinel = legacy.join("usage/usage.sqlite3");
    std::fs::write(&sentinel, "retired-source-must-never-open").unwrap();
    let original = std::fs::metadata(&sentinel).unwrap();
    fixture.config.codex_usage_sources = vec![
        peer.source.clone(),
        crate::config::CodexUsageSource {
            uid: peer.source.uid + 1,
            codex_home: legacy.clone(),
            api_socket: None,
            executable: "/must-not-execute-retired-cli".into(),
        },
    ];
    // An old link must not mask the new negotiated repository identity.
    fixture.database.transaction(|tx| { tx.execute("INSERT INTO codex_usage_repository_links(source_uid,repository_id,codex_repository_id,source_schema,taxonomy_version,resolved_at) VALUES($1,'project-alpha',$2,9,1,'t')",crate::database::params![peer.source.uid,"b".repeat(64)])?;Ok(()) }).unwrap();
    fixture.service = ReviewService::new(
        fixture.database.clone(),
        UsageService::with_clock(
            fixture.config.clone(),
            fixture.database.clone(),
            crate::repository::Registry::new(fixture.database.clone()),
            Arc::new(FixtureClock),
        ),
    );
    let missing = fixture.service.prepare(input(), START + WEEK).unwrap();
    assert!(missing.usage.outcomes.rows.is_empty());
    assert_eq!(missing.usage.outcomes.unattributed.operations, 2);
    let now =
        time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(START + WEEK - 1) * 1_000_000)
            .unwrap();
    let plane = ControlPlane::with_adapters(
        fixture.config.clone(),
        fixture.database.clone(),
        Arc::new(|_: &RouteAccessSection| Ok(())),
        Arc::new(crate::platform::FixedClock(now)),
    )
    .unwrap();
    let authorized = caller();
    plane
        .execute(
            "plan.overview",
            json!({"repository_id":"project-alpha"}),
            &authorized,
        )
        .unwrap();
    let review_plane = ControlPlane::with_adapters(
        fixture.config.clone(),
        fixture.database.clone(),
        Arc::new(|_: &RouteAccessSection| Ok(())),
        Arc::new(FixtureClock),
    )
    .unwrap();
    let prepared: Prepared = serde_json::from_value(
        review_plane
            .execute(
                "review.prepare",
                serde_json::to_value(input()).unwrap(),
                &Caller {
                    work: None,
                    ..authorized.clone()
                },
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(prepared.usage.coverage.available_collectors, 1);
    assert_eq!(prepared.usage.coverage.configured_collectors, 2);
    assert_eq!(
        prepared
            .usage
            .coverage
            .unavailable_reasons
            .get("legacy_source_unsupported"),
        Some(&1)
    );
    assert_eq!(prepared.usage.totals.total_tokens, Some(135));
    assert_eq!(prepared.usage.outcomes.rows.len(), 1);
    assert_eq!(prepared.usage.outcomes.rows[0].outcome_id, "spec");
    assert_eq!(
        prepared.usage.outcomes.rows[0].title.as_deref(),
        Some("Specify the workflow")
    );
    let metadata = &prepared.usage.coverage.sources[0];
    assert_eq!(
        (
            metadata.contract_version,
            metadata.format_version,
            metadata.generation,
            metadata.journal_watermark
        ),
        (1, 3, 7, 10)
    );
    assert_eq!(metadata.freshness, "fresh");
    assert_eq!(metadata.pending_entries, 0);
    let requests = peer.requests.lock().unwrap();
    assert!(
        requests
            .iter()
            .any(|r| r["method"] == "localUsage/repositoryIdentity")
    );
    let binding = requests
        .iter()
        .filter_map(|r| r["params"]["workContextBindings"].as_array())
        .flat_map(|b| b.iter())
        .next()
        .expect("actual persisted context sent");
    assert_eq!(binding["operationId"], "governed-op");
    assert_eq!(binding["workContext"]["id"], "C1");
    for (field, expected) in [
        ("repositoryId", "project-alpha"),
        ("nativeProjectId", "fixture-project"),
        ("threadId", "fixture-thread"),
        ("turnId", "fixture-turn"),
        ("workstreamId", "spec"),
        ("outcomeId", "spec"),
        ("experimentRef", "experiment@1"),
    ] {
        assert_eq!(binding["workContext"][field], expected);
    }
    drop(requests);
    let wire = serde_json::to_string(&prepared).unwrap();
    for private in [
        home.to_str().unwrap(),
        legacy.to_str().unwrap(),
        KEY,
        "fixture-thread",
        "fixture-turn",
        "fixture-project",
        "governed-op",
    ] {
        assert!(
            !wire.contains(private),
            "private context escaped: {private}"
        );
    }
    assert_eq!(
        std::fs::metadata(&sentinel).unwrap().accessed().unwrap(),
        original.accessed().unwrap()
    );
    assert_eq!(
        std::fs::read(&sentinel).unwrap(),
        b"retired-source-must-never-open"
    );
    // An unchanged, evidence-backed decision is conclusive without inventing
    // before/after gains or exact timing that the collector did not capture.
    let mut request = super::tests::record(&fixture);
    request.record.experiment.baseline.evidence_refs = prepared.source_refs.clone();
    request.record.experiment.baseline.missing_measurements = prepared.coverage_gaps.clone();
    let saved = fixture
        .service
        .record(request, "fixture", START + WEEK)
        .unwrap();
    assert!(saved.completed);
    assert_eq!(saved.record.experiment.disposition, Disposition::Unchanged);
    assert_eq!(
        fixture
            .service
            .receipt(Reference {
                reference: saved.reference.clone()
            })
            .unwrap(),
        saved
    );
}

#[test]
fn review_prepare_fails_closed_on_missing_or_unsupported_handshake_and_snapshot_metadata() {
    for mode in 0..5 {
        let fixture = Fixture::new();
        let root = fixture._temporary.path();
        let home = root.join("fresh");
        let mut cap = capability();
        if mode == 1 {
            cap["version"] = json!(2);
        }
        let peer = Peer::with_capability(root, &home, (mode != 0).then_some(cap), move |params| {
            let mut value = response(params, START + WEEK, 135);
            match mode {
                2 => {
                    value.as_object_mut().unwrap().remove("snapshot");
                }
                3 => {
                    value["snapshot"]
                        .as_object_mut()
                        .unwrap()
                        .remove("pendingEntries");
                }
                4 => value["snapshot"]["journalWatermark"] = json!(9),
                _ => {}
            }
            value
        });
        let prepared = service(&fixture, peer.source.clone())
            .prepare(input(), START + WEEK)
            .unwrap();
        assert_eq!(
            prepared.usage.coverage.available_collectors, 0,
            "mode {mode}"
        );
        assert!(prepared.usage.outcomes.rows.is_empty());
        assert_eq!(prepared.usage.totals.total_tokens, None);
        let expected = if mode == 4 {
            "api_snapshot_inconsistent"
        } else {
            "api_contract_unsupported"
        };
        assert!(
            prepared
                .usage
                .coverage
                .unavailable_reasons
                .contains_key(expected),
            "mode {mode}: {:?}",
            prepared.usage.coverage.unavailable_reasons
        );
    }
}

#[test]
fn review_context_window_is_indexed_bounded_uid_scoped_and_conflicts_stay_unattributed() {
    let fixture = Fixture::new();
    let root = fixture._temporary.path();
    let home = root.join("fresh");
    let peer = Peer::start(root, &home, |params| response(params, START + WEEK, 135));
    let call = caller();
    let work = call.work.clone().unwrap();
    crate::usage_context::ingest(
        &fixture.database,
        call.uid,
        "project-alpha",
        &work,
        START + 1,
    )
    .unwrap();
    crate::usage_context::ingest(
        &fixture.database,
        call.uid,
        "project-alpha",
        &work,
        START + 2,
    )
    .unwrap();
    let selected = crate::usage_context::read(
        &fixture.database,
        call.uid,
        "project-alpha",
        START,
        START + WEEK,
    )
    .unwrap();
    assert_eq!(selected.bindings.len(), 1);
    assert_eq!(selected.bindings[0].captured_at_ms, START + 1);
    assert_eq!(
        crate::usage_context::read(
            &fixture.database,
            call.uid + 1,
            "project-alpha",
            START,
            START + WEEK
        )
        .unwrap()
        .bindings
        .len(),
        0
    );
    assert_eq!(
        crate::usage_context::read(
            &fixture.database,
            call.uid,
            "project-alpha",
            START + 2,
            START + WEEK
        )
        .unwrap()
        .bindings
        .len(),
        0
    );
    let mut conflict = work.clone();
    conflict.context.as_mut().unwrap().outcome_id = Some("other".into());
    crate::usage_context::ingest(
        &fixture.database,
        call.uid,
        "project-alpha",
        &conflict,
        START + 3,
    )
    .unwrap();
    let prepared = service(&fixture, peer.source.clone())
        .prepare(input(), START + WEEK)
        .unwrap();
    assert!(prepared.usage.outcomes.rows.is_empty());
    assert_eq!(
        prepared
            .usage
            .outcomes
            .unattributed_reasons
            .get("work_context_binding_conflict"),
        Some(&1)
    );
    // Populate a new finite window beyond the rejection; the reader may fetch
    // one sentinel row beyond its cap, never a collector history scan.
    for index in 0..257 {
        let mut other = work.clone();
        other.context.as_mut().unwrap().operation_id = Some(format!("operation-{index}"));
        crate::usage_context::ingest(
            &fixture.database,
            call.uid,
            "project-alpha",
            &other,
            START + 10,
        )
        .unwrap();
    }
    let window = crate::usage_context::read(
        &fixture.database,
        call.uid,
        "project-alpha",
        START + 10,
        START + WEEK,
    )
    .unwrap();
    assert!(window.truncated);
    assert_eq!(window.bindings.len(), 256);
    let prepared = service(&fixture, peer.source.clone())
        .prepare(input(), START + WEEK)
        .unwrap();
    assert_eq!(
        prepared
            .usage
            .outcomes
            .unattributed_reasons
            .get("work_context_bindings_truncated"),
        Some(&1)
    );
    assert_ne!(prepared.usage.outcomes.coverage, "complete");
}

#[test]
#[ignore = "requires an explicit candidate binary and isolated synthetic usage-v2 fixture"]
fn review_prepare_against_candidate_usage_v2_app_server() {
    use std::os::unix::process::CommandExt;
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};
    struct Candidate(Child);
    impl Drop for Candidate {
        fn drop(&mut self) {
            // Only this exact child and its process group belong to this test.
            unsafe {
                libc::kill(-(self.0.id() as i32), libc::SIGKILL);
            }
            let _ = self.0.wait();
        }
    }
    fn copy_fixture(source: &Path, target: &Path) {
        std::fs::create_dir_all(target).unwrap();
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            assert!(
                !kind.is_symlink(),
                "synthetic fixture contains an unsupported symlink"
            );
            if kind.is_dir() {
                copy_fixture(&entry.path(), &target.join(entry.file_name()));
            } else {
                assert!(kind.is_file());
                std::fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
            }
        }
    }
    let binary =
        std::env::var_os("CODEX_USAGE_V2_APP_SERVER").expect("candidate app-server binary");
    let source = std::path::PathBuf::from(
        std::env::var_os("CODEX_USAGE_V2_FIXTURE_HOME").expect("isolated synthetic fixture home"),
    );
    let mut fixture = Fixture::new();
    let mut request = super::tests::record(&fixture);
    let info: serde_json::Value = serde_json::from_slice(
        &std::fs::read(source.join("fixture-info.json")).expect("synthetic fixture description"),
    )
    .unwrap();
    let start = info["windowStartMs"]
        .as_u64()
        .expect("fixture window start");
    let end = info["windowEndMs"].as_u64().expect("fixture window end");
    let captured = info["capturedAtMs"]
        .as_u64()
        .expect("fixture context timestamp");
    let repository = std::env::var("CODEX_USAGE_V2_FIXTURE_REPOSITORY")
        .ok()
        .or_else(|| info["repositoryPath"].as_str().map(str::to_owned))
        .expect("fixture repository path");
    assert!(Path::new(&repository).is_absolute());
    let home = fixture._temporary.path().join("candidate-home");
    copy_fixture(&source.join("usage-v2"), &home.join("usage-v2"));
    let copied_info: serde_json::Value =
        serde_json::from_slice(&std::fs::read(source.join("fixture-info.json")).unwrap()).unwrap();
    assert_eq!(
        info, copied_info,
        "synthetic fixture changed while being copied"
    );
    fixture.database.transaction(|tx| {
        tx.execute("UPDATE repositories SET root_path=$1 WHERE repository_id='project-alpha'",[&repository])?;
        tx.execute_batch("INSERT INTO tasks(task_id,repository_id,seq,position,title,outcome,kind,status,created_at,created_by,updated_at) VALUES('fixture-outcome','project-alpha',2,2,'Review scoped usage','Prove explicit operation attribution','improvement','in_progress','1970-01-01T00:00:00Z','fixture','1970-01-01T00:00:00Z')")?;
        Ok(())
    }).unwrap();
    let socket = fixture._temporary.path().join("candidate.sock");
    let child = Command::new(binary)
        .arg("--listen")
        .arg(format!("unix://{}", socket.display()))
        .env("CODEX_HOME", &home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("start isolated candidate app-server");
    let mut child = Candidate(child);
    let deadline = Instant::now() + Duration::from_secs(15);
    while !socket.exists() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "candidate app-server exited before binding"
        );
        assert!(
            Instant::now() < deadline,
            "candidate app-server socket readiness deadline"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    fixture.config.codex_usage_sources = vec![crate::config::CodexUsageSource {
        uid: rustix::process::getuid().as_raw(),
        codex_home: home,
        api_socket: Some(socket),
        executable: "/must-not-execute-retired-cli".into(),
    }];
    let clock = Arc::new(crate::platform::FixedClock(
        time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(captured) * 1_000_000).unwrap(),
    ));
    let plane = ControlPlane::with_adapters(
        fixture.config.clone(),
        fixture.database.clone(),
        Arc::new(|_: &RouteAccessSection| Ok(())),
        clock,
    )
    .unwrap();
    let mut operation = caller();
    let context = operation.work.as_mut().unwrap().context.as_mut().unwrap();
    context.operation_id = Some("fixture-model".into());
    context.workstream_id = Some("fixture-workstream".into());
    context.outcome_id = Some("fixture-outcome".into());
    context.experiment_ref = Some("fixture-experiment@1".into());
    plane
        .execute(
            "plan.overview",
            json!({"repository_id":"project-alpha"}),
            &operation,
        )
        .unwrap();
    fixture.service = ReviewService::new(
        fixture.database.clone(),
        UsageService::with_clock(
            fixture.config.clone(),
            fixture.database.clone(),
            crate::repository::Registry::new(fixture.database.clone()),
            Arc::new(crate::platform::FixedClock(
                time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(end) * 1_000_000)
                    .unwrap(),
            )),
        ),
    );
    let scope = Prepare {
        window_start_ms: start,
        window_end_ms: end,
        workstream_id: Some("fixture-workstream".into()),
        ..input()
    };
    let began = Instant::now();
    let cold = fixture.service.prepare(scope.clone(), end).unwrap();
    eprintln!(
        "candidate_cold_prepare_ms={} reasons={:?}",
        began.elapsed().as_millis(),
        cold.usage.coverage.unavailable_reasons
    );
    assert!(
        began.elapsed() < Duration::from_millis(750),
        "cold review exceeded its bounded foreground window"
    );
    if cold.usage.coverage.available_collectors == 0 {
        assert_eq!(cold.usage.totals.total_tokens, None);
        assert_eq!(
            cold.usage
                .coverage
                .unavailable_reasons
                .get("api_refresh_pending"),
            Some(&1)
        );
        let snapshot = cold
            .usage
            .coverage
            .snapshot
            .as_ref()
            .expect("explicit pending snapshot");
        assert!(snapshot.refreshing);
        assert!(!snapshot.refresh_failed);
        assert!(
            fixture
                .service
                .usage
                .wait_for_collector_refresh(Instant::now() + Duration::from_secs(15)),
            "finite source read did not finish"
        );
    }
    let warm_began = Instant::now();
    let prepared = fixture.service.prepare(scope, end).unwrap();
    eprintln!(
        "candidate_warm_prepare_ms={}",
        warm_began.elapsed().as_millis()
    );
    assert!(
        warm_began.elapsed() < Duration::from_millis(500),
        "cached review exceeded foreground budget"
    );
    assert_eq!(
        prepared.usage.coverage.available_collectors, 1,
        "{:?}",
        prepared.usage.coverage.unavailable_reasons
    );
    assert!(
        prepared
            .usage
            .totals
            .total_tokens
            .is_some_and(|value| value >= 120)
    );
    assert_eq!(prepared.usage.coverage.sources[0].source, "usage-v2");
    assert!(
        prepared
            .usage
            .outcomes
            .rows
            .iter()
            .any(|row| row.outcome_id == "fixture-outcome"
                && row.effort.provider_total_tokens.measured == 120)
    );
    assert!(
        prepared
            .usage
            .outcomes
            .totals
            .elapsed_execution_ms
            .exact
            .is_none()
    );
    request.record.window_start_ms = start;
    request.record.window_end_ms = end;
    request.record.workstream_id = Some("fixture-workstream".into());
    request.record.outcome_id = Some("fixture-outcome".into());
    request.record.experiment.baseline.evidence_refs = prepared.source_refs;
    request.record.experiment.baseline.missing_measurements = prepared.coverage_gaps;
    request.record.experiment.baseline.interpretation="The candidate collector reports actual synthetic provider measurements with explicit operation attribution; unavailable timing remains acknowledged.".into();
    request.record.experiment.observations=vec![Observation {
        kind:ObservationKind::IntentionalValidation,
        interpretation:"The same candidate transport and review checks preserve the required quality boundary. These necessary checks are not treated as wasted work.".into(),
        evidence_refs:vec![EvidenceRef { kind:EvidenceKind::Outcome,reference:"fixture-outcome".into() }],
    }];
    request.record.experiment.reason="The available scoped evidence supports retaining the current workflow. There is no claimed performance gain and the unrecorded timing remains unknown.".into();
    let saved = fixture.service.record(request, "fixture", end).unwrap();
    assert!(saved.completed);
    assert_eq!(saved.record.experiment.disposition, Disposition::Unchanged);
    assert_eq!(
        fixture
            .service
            .receipt(Reference {
                reference: saved.reference.clone()
            })
            .unwrap(),
        saved
    );
}

#[test]
fn delayed_collector_finishes_once_after_bounded_cold_review_and_supplies_attributed_receipt() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    let mut fixture = Fixture::new();
    let root = fixture._temporary.path();
    let home = root.join("slow-source");
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let peer = Peer::start(root, &home, move |params| {
        observed.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(1_200));
        response(params, START + WEEK, 135)
    });
    let caller = caller();
    crate::usage_context::ingest(
        &fixture.database,
        caller.uid,
        "project-alpha",
        caller.work.as_ref().unwrap(),
        START + 1,
    )
    .unwrap();
    fixture.service = service(&fixture, peer.source.clone());
    let began = Instant::now();
    let cold = fixture.service.prepare(input(), START + WEEK).unwrap();
    assert!(began.elapsed() < Duration::from_millis(750));
    assert_eq!(cold.usage.totals.total_tokens, None);
    assert_eq!(
        cold.usage
            .coverage
            .unavailable_reasons
            .get("api_refresh_pending"),
        Some(&1)
    );
    let snapshot = cold.usage.coverage.snapshot.unwrap();
    assert!(snapshot.refreshing);
    assert!(!snapshot.refresh_failed);
    // A second request joins the same source read instead of poisoning the
    // source with a timeout or starting a duplicate request.
    let _ = fixture.service.prepare(input(), START + WEEK).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        fixture
            .service
            .usage
            .wait_for_collector_refresh(Instant::now() + Duration::from_secs(5))
    );
    let began = Instant::now();
    let warm = fixture.service.prepare(input(), START + WEEK).unwrap();
    assert!(began.elapsed() < Duration::from_millis(500));
    assert_eq!(warm.usage.coverage.available_collectors, 1);
    assert_eq!(warm.usage.totals.total_tokens, Some(135));
    assert_eq!(warm.usage.outcomes.rows.len(), 1);
    assert_eq!(warm.usage.outcomes.rows[0].outcome_id, "spec");
    assert!(!warm.usage.coverage.snapshot.unwrap().refreshing);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let request = super::tests::record(&fixture);
    let saved = fixture
        .service
        .record(request, "fixture", START + WEEK)
        .unwrap();
    assert!(saved.completed);
    assert_eq!(saved.record.experiment.disposition, Disposition::Unchanged);
}
