//! Boundary regressions extend the existing disposable PostgreSQL fixture and
//! exercise ProjectService -> StorageService -> real Git/filesystem effects.
//! The root acceptance suite covers storage already; these project-specific
//! consent and exact-revision paths cannot be expressed through that old API.
use super::*;
use crate::automation_test_support::{Fixture, FixtureClock};
use crate::{capacity::CapacityBroker, events::EventService, platform::Clock};
use devcoordinator2_api::{ClientKind, storage as storage_api};
use std::{
    fs,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

struct MutableClock(Arc<AtomicU64>);
impl Clock for MutableClock {
    fn now_utc(&self) -> time::OffsetDateTime {
        time::OffsetDateTime::from_unix_timestamp_nanos(
            i128::from(self.0.load(Ordering::SeqCst)) * 1_000_000,
        )
        .unwrap()
    }
}

struct NoHostDocker;
impl crate::docker::DockerControl for NoHostDocker {
    fn invoke(
        &self,
        input: crate::docker::DockerInvocation,
    ) -> Result<crate::docker::DockerOutput, crate::docker::DockerError> {
        assert_eq!(
            input.args().first().and_then(|value| value.to_str()),
            Some("ps"),
            "fixture must never contact or mutate host Docker"
        );
        Ok(crate::docker::DockerOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
        })
    }
    fn spawn_follow_logs(
        &self,
        _: &crate::docker::ExactContainerId,
    ) -> Result<crate::docker::LogFollower, crate::docker::DockerError> {
        panic!("checkout fixture has no host containers")
    }
}

fn work_root() -> PathBuf {
    assert_eq!(
        unsafe { libc::geteuid() },
        0,
        "native process inventory requires the root-daemon boundary"
    );
    let root = PathBuf::from(
        std::env::var_os("DEVCOORDINATOR2_CHECKOUT_TEST_ROOT")
            .expect("explicit new external work root is required"),
    );
    assert!(root.is_absolute());
    assert_eq!(root.canonicalize().unwrap(), root);
    assert_eq!(
        fs::read_to_string(root.join("checkout-lifecycle-fixture.marker")).unwrap(),
        "project-checkout-lifecycle-v1\n"
    );
    root
}

fn own_fixture_tree(path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    let caller = caller();
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(
        unsafe { libc::lchown(name.as_ptr(), caller.uid, caller.gid) },
        0
    );
    if path.symlink_metadata().unwrap().file_type().is_dir() {
        for entry in fs::read_dir(path).unwrap() {
            own_fixture_tree(&entry.unwrap().path());
        }
    }
}

struct CheckoutFixture {
    fixture: Fixture,
    storage: StorageService,
    service: ProjectService,
    checkout: PathBuf,
    now: Arc<AtomicU64>,
}

fn git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("/usr/bin/git")
        .arg("-C")
        .arg(path)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "safe.directory=*",
            "-c",
            "user.name=Checkout fixture",
            "-c",
            "user.email=checkout@example.invalid",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fixture git {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    if unsafe { libc::geteuid() } == 0 {
        let root = work_root();
        let relative = path
            .strip_prefix(&root)
            .expect("Git fixture must remain under its marked work root");
        let first = relative
            .components()
            .next()
            .expect("Git cannot target the work root itself");
        own_fixture_tree(&root.join(first.as_os_str()));
    }
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn caller() -> Caller {
    Caller {
        via_edge: false,
        pid: std::process::id(),
        uid: std::env::var("DEVCOORDINATOR2_TEST_POSTGRES_UID")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|uid| *uid != 0)
            .unwrap_or_else(|| unsafe { libc::geteuid() }),
        gid: std::env::var("DEVCOORDINATOR2_TEST_POSTGRES_GID")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or_else(|| unsafe { libc::getegid() }),
        client_kind: ClientKind::Codex,
        model: None,
        effort: None,
        client_session: None,
        work: None,
        identity: None,
    }
}

fn administrator() -> Caller {
    Caller {
        via_edge: true,
        client_kind: ClientKind::Edge,
        identity: Some("owner@example.invalid".into()),
        ..caller()
    }
}

impl CheckoutFixture {
    fn new() -> Self {
        let confined = work_root();
        let fixture = Fixture::new();
        assert!(fixture._temporary.path().starts_with(&confined));
        assert!(
            fixture
                .database
                .data_directory()
                .unwrap()
                .starts_with(&confined)
        );
        assert_ne!(
            caller().uid,
            0,
            "Git execution identity must remain non-root"
        );
        let root = &fixture.repository;
        git(root, &["init", "-q", "-b", "main"]);
        fs::write(root.join("source.txt"), "preserved source\n").unwrap();
        fs::write(
            root.join(".gitignore"),
            "ignored.bin\n.env\n.devcoordinator/\n",
        )
        .unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "baseline"]);
        let remote = fixture._temporary.path().join("remote.git");
        git(root, &["init", "--bare", "-q", remote.to_str().unwrap()]);
        git(&remote, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        git(root, &["remote", "add", "origin", remote.to_str().unwrap()]);
        git(root, &["push", "-q", "-u", "origin", "main"]);
        let checkout = fixture._temporary.path().join("linked-feature");
        git(
            root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature",
                checkout.to_str().unwrap(),
            ],
        );
        git(&checkout, &["push", "-q", "-u", "origin", "feature"]);
        let uid = caller().uid;
        fixture.database.transaction(|connection| {
            connection.execute("UPDATE repositories SET registered_by_uid=$1 WHERE repository_id='project-alpha'",[uid])?;
            connection.execute_batch("INSERT INTO users(user_id,email,administrator,created_at,created_by) VALUES('owner','owner@example.invalid',1,'t','fixture');
                INSERT INTO projects(project_id,name,slug,revision,created_at,updated_at,updated_by) VALUES('product','Product','product',1,'t','t','fixture');
                INSERT INTO project_sources(project_id,repository_id,name,primary_checkout_id) VALUES('product','project-alpha','Source','primary');
                INSERT INTO project_repository_memberships(project_id,repository_id,physical_repository_id) VALUES('product','project-alpha','project-alpha');")?;
            connection.execute("INSERT INTO project_roots(project_id,root_id,kind,label,absolute_path) VALUES('product','root','source','Source',$1)",[fixture._temporary.path().to_string_lossy().into_owned()])?;
            connection.execute("INSERT INTO project_checkouts(project_id,repository_id,physical_repository_id,worktree_id,absolute_path,role,protected_role,branch_name,head,dirty,exists_on_disk,lifecycle,revision,record_json,updated_at,updated_by) VALUES('product','project-alpha','project-alpha','primary',$1,'primary','primary','main',$2,false,true,'active',1,'{}','t','fixture')",params![root.to_string_lossy().into_owned(),git(root,&["rev-parse","HEAD"])])?;
            connection.execute("INSERT INTO project_checkouts(project_id,repository_id,physical_repository_id,worktree_id,absolute_path,role,branch_name,head,dirty,exists_on_disk,lifecycle,task_id,revision,record_json,updated_at,updated_by) VALUES('product','project-alpha','project-alpha','feature-checkout',$1,'worktree','feature',$2,false,true,'paused','spec',1,'{}','t','fixture')",params![checkout.to_string_lossy().into_owned(),git(&checkout,&["rev-parse","HEAD"])])?;
            Ok(())
        }).unwrap();
        let now = Arc::new(AtomicU64::new(
            (FixtureClock.now_utc().unix_timestamp_nanos() / 1_000_000) as u64,
        ));
        let storage = StorageService::with_backend(
            fixture.config.clone(),
            fixture.database.clone(),
            Arc::new(MutableClock(now.clone())),
            CapacityBroker::new(fixture.database.clone(), fixture.config.socket_path.clone())
                .unwrap(),
            EventService::new(fixture.database.clone()).unwrap(),
            Arc::new(crate::storage::native::HostBackend::with_docker(
                fixture.config.clone(),
                fixture.database.clone(),
                Arc::new(NoHostDocker),
            )),
        );
        let publisher: Arc<dyn crate::access::RoutePublisher> =
            Arc::new(|_: &crate::access::RouteAccessSection| Ok(()));
        let access = Access::new(&fixture.config, fixture.database.clone(), publisher).unwrap();
        let service = ProjectService::new(
            fixture.database.clone(),
            Registry::new(fixture.database.clone()),
        )
        .with_storage(storage.clone())
        .with_access(access);
        Self {
            fixture,
            storage,
            service,
            checkout,
            now,
        }
    }

    fn plan(&self) -> removal::RemovalPlan {
        self.service
            .checkout_remove_plan(
                removal::Plan {
                    project_id: "product".into(),
                    expected_project_revision: 1,
                    targets: vec![removal::Target {
                        worktree_id: "feature-checkout".into(),
                        expected_revision: 1,
                    }],
                    include_absolute_paths: false,
                },
                &caller(),
            )
            .unwrap()
    }
    fn start(&self, plan: &removal::RemovalPlan, approval: Option<String>) -> removal::Start {
        removal::Start {
            project_id: "product".into(),
            plan_id: plan.plan_id.clone(),
            fingerprint: plan.fingerprint.clone(),
            idempotency_key: format!("start-{}", plan.plan_id),
            approval_receipt_id: approval,
        }
    }
    fn approve(&self, plan: &removal::RemovalPlan) -> removal::ApprovalReceipt {
        self.service
            .checkout_remove_approve(
                removal::Approve {
                    project_id: "product".into(),
                    plan_id: plan.plan_id.clone(),
                    fingerprint: plan.fingerprint.clone(),
                    idempotency_key: format!("approve-{}", plan.plan_id),
                    consent_decision_id: None,
                },
                &administrator(),
            )
            .unwrap()
    }
    fn execute(&self, start: removal::Start) -> removal::RemovalJob {
        let job = self
            .service
            .checkout_remove_start(start, &caller())
            .unwrap();
        self.storage.execute_job(&job.job.job_id).unwrap();
        self.service
            .checkout_remove_job(
                removal::JobReference {
                    project_id: "product".into(),
                    job_id: job.job.job_id,
                },
                &caller(),
            )
            .unwrap()
    }
}

#[test]
#[ignore = "requires a marked isolated root work area and explicit nonroot Git/PostgreSQL identities"]
fn safe_checkout_removal_keeps_tasks_history_refs_and_canonical_source() {
    let fixture = CheckoutFixture::new();
    let plan = fixture.plan();
    assert!(plan.ready, "{:?}", plan.targets);
    assert!(!plan.requires_approval);
    assert!(!plan.targets[0].path_is_absolute);
    assert!(!Path::new(&plan.targets[0].path).is_absolute());
    assert!(plan.targets[0].task_stays_open);
    let bypass = fixture
        .storage
        .start(
            storage_api::Start {
                plan_id: plan.plan_id.clone(),
                idempotency_key: "generic-start".into(),
            },
            &caller(),
        )
        .unwrap_err();
    assert_eq!(bypass.message, "checkout_plan_requires_project_start");
    let start = fixture.start(&plan, None);
    let result = fixture.execute(start.clone());
    assert!(
        matches!(result.job.state, storage_api::JobState::Completed),
        "{:?}",
        result.job
    );
    assert!(result.targets[0].fully_removed);
    assert!(!fixture.checkout.exists());
    assert!(
        !git(
            &fixture.fixture.repository,
            &["worktree", "list", "--porcelain"]
        )
        .contains(fixture.checkout.to_str().unwrap())
    );
    assert!(
        !git(
            &fixture.fixture.repository,
            &["rev-parse", "refs/heads/feature"]
        )
        .is_empty()
    );
    assert_eq!(
        fs::read_to_string(fixture.fixture.repository.join("source.txt")).unwrap(),
        "preserved source\n"
    );
    let (task,exists,events)=fixture.fixture.database.read(|connection|Ok((connection.query_row("SELECT status FROM tasks WHERE task_id='spec'",[],|row|row.get::<_,String>(0))?,connection.query_row("SELECT exists_on_disk FROM project_checkouts WHERE worktree_id='feature-checkout'",[],|row|row.get::<_,bool>(0))?,connection.query_row("SELECT count(*) FROM project_events WHERE kind='checkout_removed'",[],|row|row.get::<_,i64>(0))?))).unwrap();
    assert_eq!(task, "in_progress");
    assert!(!exists);
    assert_eq!(events, 1);
    let retry = fixture
        .service
        .checkout_remove_start(start, &caller())
        .unwrap();
    assert_eq!(retry.job.job_id, result.job.job_id);
    let verified = fixture
        .service
        .verified_checkout_removal("product", &result.job.job_id, "feature-checkout")
        .unwrap();
    assert_eq!(verified.branch_name(), "feature");
    assert!(!verified.receipt_id().is_empty());
}

#[test]
#[ignore = "requires a marked isolated root work area and explicit nonroot Git/PostgreSQL identities"]
fn exact_console_approval_allows_known_loss_but_stale_content_requires_a_new_plan() {
    let fixture = CheckoutFixture::new();
    fs::write(
        fixture.checkout.join("source.txt"),
        "changed local source\n",
    )
    .unwrap();
    fs::write(fixture.checkout.join("untracked.txt"), "local draft\n").unwrap();
    fs::write(
        fixture.checkout.join("ignored.bin"),
        "local ignored output\n",
    )
    .unwrap();
    let current = fixture.checkout.join(".devcoordinator/test/current");
    fs::create_dir_all(&current).unwrap();
    fs::write(current.join("check-report.json"), "{\"fixture\":true}").unwrap();
    let plan = fixture.plan();
    assert!(plan.requires_approval);
    assert!(!plan.ready);
    assert!(plan.blockers.is_empty(), "{:?}", plan);
    assert_eq!(plan.targets[0].untracked_files, 1);
    assert_eq!(plan.targets[0].ignored_files, 2);
    assert_eq!(
        fixture
            .service
            .checkout_remove_start(fixture.start(&plan, None), &caller())
            .unwrap_err()
            .message,
        "exact_data_loss_approval_required"
    );
    let request = removal::Approve {
        project_id: "product".into(),
        plan_id: plan.plan_id.clone(),
        fingerprint: plan.fingerprint.clone(),
        idempotency_key: "agent-is-not-human-consent".into(),
        consent_decision_id: None,
    };
    assert_eq!(
        fixture
            .service
            .checkout_remove_approve(request, &caller())
            .unwrap_err()
            .message,
        "recorded_human_consent_required"
    );
    let approval = fixture.approve(&plan);
    assert_eq!(fixture.approve(&plan).receipt_id, approval.receipt_id);
    fs::write(fixture.checkout.join("untracked.txt"), "other draft\n").unwrap();
    assert!(
        fixture
            .service
            .checkout_remove_start(fixture.start(&plan, Some(approval.receipt_id)), &caller())
            .is_err()
    );
    assert!(fixture.checkout.exists());
    let fresh = fixture.plan();
    let approval = fixture.approve(&fresh);
    let result = fixture.execute(fixture.start(&fresh, Some(approval.receipt_id)));
    assert!(result.targets[0].fully_removed, "{:?}", result.job);
}

#[test]
#[ignore = "requires a marked isolated root work area and explicit nonroot Git/PostgreSQL identities"]
fn replacement_active_leases_and_expiry_cannot_be_approved_away() {
    let replacement = CheckoutFixture::new();
    let plan = replacement.plan();
    fs::rename(
        &replacement.checkout,
        replacement.checkout.with_extension("preserved"),
    )
    .unwrap();
    fs::create_dir(&replacement.checkout).unwrap();
    fs::write(
        replacement.checkout.join("precious.txt"),
        "replacement data",
    )
    .unwrap();
    assert!(
        replacement
            .service
            .checkout_remove_start(replacement.start(&plan, None), &caller())
            .is_err()
    );
    assert_eq!(
        fs::read_to_string(replacement.checkout.join("precious.txt")).unwrap(),
        "replacement data"
    );

    let leased = CheckoutFixture::new();
    let plan = leased.plan();
    let artifact = leased.storage.stored_plan(&plan.plan_id).unwrap().records[0]
        .artifact
        .artifact_id
        .clone();
    leased
        .storage
        .lease(
            storage_api::LeaseSet {
                lease_id: None,
                artifact_ids: vec![artifact],
                duration_seconds: 600,
            },
            &caller(),
        )
        .unwrap();
    assert!(
        leased
            .service
            .checkout_remove_start(leased.start(&plan, None), &caller())
            .is_err()
    );
    assert!(leased.checkout.exists());

    let expired = CheckoutFixture::new();
    fs::write(expired.checkout.join("draft"), "unfinished").unwrap();
    let plan = expired.plan();
    let approval = expired.approve(&plan);
    expired.now.store(plan.expires_at_ms + 1, Ordering::SeqCst);
    assert!(
        expired
            .service
            .checkout_remove_start(expired.start(&plan, Some(approval.receipt_id)), &caller())
            .is_err()
    );
    assert!(expired.checkout.exists());
}

#[test]
#[ignore = "requires a marked isolated root work area and explicit nonroot Git/PostgreSQL identities"]
fn protected_credentials_nested_sources_and_primary_copies_remain_blocked() {
    for scope in [
        "credentials",
        "nested",
        "primary",
        "locked",
        "retained-evidence",
    ] {
        let fixture = CheckoutFixture::new();
        match scope {
            "credentials" => fs::write(
                fixture.checkout.join(".env"),
                "fixture-secret-never-returned",
            )
            .unwrap(),
            "nested" => {
                let nested = fixture.checkout.join("nested");
                fs::create_dir(&nested).unwrap();
                git(&nested, &["init", "-q"]);
            }
            "locked" => {
                git(
                    &fixture.fixture.repository,
                    &["worktree", "lock", fixture.checkout.to_str().unwrap()],
                );
            }
            "retained-evidence" => {
                let logs = fixture
                    .checkout
                    .join(".devcoordinator/test/logs/runs/retained");
                fs::create_dir_all(&logs).unwrap();
                fs::write(logs.join("stdout.log"), "retained fixture evidence").unwrap();
            }
            _ => {
                fixture.fixture.database.call(|connection|{connection.execute("UPDATE project_checkouts SET protected_role='primary' WHERE worktree_id='feature-checkout'",[])?;Ok(())}).unwrap();
            }
        }
        let plan = fixture.plan();
        assert!(!plan.blockers.is_empty(), "{scope}");
        let approval = fixture.service.checkout_remove_approve(
            removal::Approve {
                project_id: "product".into(),
                plan_id: plan.plan_id.clone(),
                fingerprint: plan.fingerprint.clone(),
                idempotency_key: "cannot-waive-protection".into(),
                consent_decision_id: None,
            },
            &administrator(),
        );
        assert!(approval.is_err(), "{scope}");
        assert!(fixture.checkout.exists());
        let encoded = serde_json::to_string(&plan).unwrap();
        assert!(!encoded.contains("fixture-secret-never-returned"));
    }
}

#[test]
#[ignore = "requires a marked isolated root work area and explicit nonroot Git/PostgreSQL identities"]
fn cancelled_cleanup_and_recreated_directory_never_issue_verified_removal() {
    let fixture = CheckoutFixture::new();
    let plan = fixture.plan();
    let queued = fixture
        .service
        .checkout_remove_start(fixture.start(&plan, None), &caller())
        .unwrap();
    fixture
        .storage
        .cancel(
            storage_api::JobReference {
                job_id: queued.job.job_id.clone(),
            },
            &caller(),
        )
        .unwrap();
    fixture.storage.execute_job(&queued.job.job_id).unwrap();
    let result = fixture
        .service
        .checkout_remove_job(
            removal::JobReference {
                project_id: "product".into(),
                job_id: queued.job.job_id.clone(),
            },
            &caller(),
        )
        .unwrap();
    assert!(!result.targets[0].fully_removed);
    assert!(fixture.checkout.exists());
    assert!(
        fixture
            .service
            .verified_checkout_removal("product", &queued.job.job_id, "feature-checkout")
            .is_err()
    );

    let completed = CheckoutFixture::new();
    let plan = completed.plan();
    let result = completed.execute(completed.start(&plan, None));
    fs::create_dir(&completed.checkout).unwrap();
    fs::write(completed.checkout.join("replacement"), "new work").unwrap();
    assert!(
        completed
            .service
            .verified_checkout_removal("product", &result.job.job_id, "feature-checkout")
            .is_err()
    );
}

#[test]
#[ignore = "requires a marked isolated root work area and explicit nonroot Git/PostgreSQL identities"]
fn independent_clone_review_includes_every_local_ref_and_every_stash_entry() {
    let fixture = CheckoutFixture::new();
    let clone = fixture.fixture._temporary.path().join("independent-clone");
    let remote = fixture.fixture._temporary.path().join("remote.git");
    git(
        &fixture.fixture.repository,
        &[
            "clone",
            "-q",
            "--no-hardlinks",
            remote.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    git(&clone, &["switch", "-q", "feature"]);
    git(&clone, &["switch", "-q", "-c", "hidden-local"]);
    fs::write(clone.join("unpublished.txt"), "unique committed work\n").unwrap();
    git(&clone, &["add", "unpublished.txt"]);
    git(&clone, &["commit", "-qm", "unpublished extra ref"]);
    git(&clone, &["switch", "-q", "feature"]);
    for value in ["first stash\n", "second stash\n"] {
        fs::write(clone.join("source.txt"), value).unwrap();
        git(&clone, &["stash", "push", "-q"]);
    }
    fixture.fixture.database.transaction(|connection| {
        connection.execute("INSERT INTO repositories(repository_id,root_path,display_name,registered_at,registered_by_uid,last_seen_at) VALUES('clone',$1,'Clone','t',$2,'t')",params![clone.to_string_lossy().into_owned(),caller().uid])?;
        connection.execute_batch("INSERT INTO project_repository_memberships(project_id,repository_id,physical_repository_id) VALUES('product','project-alpha','clone');")?;
        connection.execute("UPDATE project_checkouts SET physical_repository_id='clone',absolute_path=$1,role='clone' WHERE worktree_id='feature-checkout'",[clone.to_string_lossy().into_owned()])?;
        Ok(())
    }).unwrap();
    let plan = fixture.plan();
    let target = &plan.targets[0];
    assert!(target.independent_clone);
    assert_eq!(target.stash_entries, 2);
    assert!(!target.remote_recoverable);
    assert!(
        target
            .unpublished_refs
            .iter()
            .any(|name| name == "refs/heads/hidden-local")
    );
    assert!(
        target
            .unpublished_refs
            .iter()
            .any(|name| name == "stash@{0}")
    );
    assert!(
        target
            .unpublished_refs
            .iter()
            .any(|name| name == "stash@{1}")
    );
    assert!(
        fixture
            .service
            .checkout_remove_start(fixture.start(&plan, None), &caller())
            .is_err()
    );
    let approval = fixture.approve(&plan);
    let result = fixture.execute(fixture.start(&plan, Some(approval.receipt_id)));
    assert!(result.targets[0].fully_removed, "{:?}", result.job);
    assert!(!clone.exists());
    assert!(fixture.checkout.exists());
    assert!(fixture.fixture.repository.exists());
}

#[test]
#[ignore = "requires a marked isolated root work area and explicit nonroot Git/PostgreSQL identities"]
fn partial_cleanup_and_late_leases_do_not_complete_unremoved_targets() {
    let fixture = CheckoutFixture::new();
    let second = fixture.fixture._temporary.path().join("second-feature");
    git(
        &fixture.fixture.repository,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature-two",
            second.to_str().unwrap(),
        ],
    );
    git(&second, &["push", "-q", "-u", "origin", "feature-two"]);
    fixture.fixture.database.call(|connection| {
        connection.execute("INSERT INTO project_checkouts(project_id,repository_id,physical_repository_id,worktree_id,absolute_path,role,branch_name,head,dirty,exists_on_disk,lifecycle,revision,record_json,updated_at,updated_by) VALUES('product','project-alpha','project-alpha','second-checkout',$1,'worktree','feature-two',$2,false,true,'active',1,'{}','t','fixture')",params![second.to_string_lossy().into_owned(),git(&second,&["rev-parse","HEAD"])])?;Ok(())
    }).unwrap();
    let plan = fixture
        .service
        .checkout_remove_plan(
            removal::Plan {
                project_id: "product".into(),
                expected_project_revision: 1,
                targets: vec![
                    removal::Target {
                        worktree_id: "feature-checkout".into(),
                        expected_revision: 1,
                    },
                    removal::Target {
                        worktree_id: "second-checkout".into(),
                        expected_revision: 1,
                    },
                ],
                include_absolute_paths: false,
            },
            &caller(),
        )
        .unwrap();
    assert!(plan.ready, "{:?}", plan);
    let queued = fixture
        .service
        .checkout_remove_start(fixture.start(&plan, None), &caller())
        .unwrap();
    fs::write(
        second.join("late-draft.txt"),
        "work created after cleanup was queued",
    )
    .unwrap();
    fixture.storage.execute_job(&queued.job.job_id).unwrap();
    let result = fixture
        .service
        .checkout_remove_job(
            removal::JobReference {
                project_id: "product".into(),
                job_id: queued.job.job_id.clone(),
            },
            &caller(),
        )
        .unwrap();
    assert!(
        matches!(result.job.state, storage_api::JobState::Partial),
        "{:?}",
        result.job
    );
    assert!(
        result
            .targets
            .iter()
            .find(|target| target.worktree_id == "feature-checkout")
            .unwrap()
            .fully_removed
    );
    assert!(
        !result
            .targets
            .iter()
            .find(|target| target.worktree_id == "second-checkout")
            .unwrap()
            .fully_removed
    );
    assert_eq!(
        fs::read_to_string(second.join("late-draft.txt")).unwrap(),
        "work created after cleanup was queued"
    );
    assert!(
        fixture
            .service
            .verified_checkout_removal("product", &queued.job.job_id, "second-checkout")
            .is_err()
    );

    let leased = CheckoutFixture::new();
    let plan = leased.plan();
    let queued = leased
        .service
        .checkout_remove_start(leased.start(&plan, None), &caller())
        .unwrap();
    let artifact = leased.storage.stored_plan(&plan.plan_id).unwrap().records[0]
        .artifact
        .artifact_id
        .clone();
    leased
        .storage
        .lease(
            storage_api::LeaseSet {
                lease_id: None,
                artifact_ids: vec![artifact],
                duration_seconds: 600,
            },
            &caller(),
        )
        .unwrap();
    leased.storage.execute_job(&queued.job.job_id).unwrap();
    let result = leased
        .service
        .checkout_remove_job(
            removal::JobReference {
                project_id: "product".into(),
                job_id: queued.job.job_id,
            },
            &caller(),
        )
        .unwrap();
    assert!(!result.targets[0].fully_removed);
    assert!(leased.checkout.exists());
}

#[test]
#[ignore = "requires a marked isolated root work area and explicit nonroot Git/PostgreSQL identities"]
fn squash_proof_preserves_original_feature_tip_and_rejects_postmerge_work() {
    let fixture = CheckoutFixture::new();
    fs::write(fixture.checkout.join("feature.txt"), "squash feature\n").unwrap();
    git(&fixture.checkout, &["add", "feature.txt"]);
    git(&fixture.checkout, &["commit", "-qm", "feature tip"]);
    git(&fixture.checkout, &["push", "-q", "origin", "feature"]);
    let original = git(&fixture.checkout, &["rev-parse", "HEAD"]);
    git(
        &fixture.fixture.repository,
        &["merge", "--squash", "feature"],
    );
    git(
        &fixture.fixture.repository,
        &["commit", "-qm", "squash integration"],
    );
    git(
        &fixture.fixture.repository,
        &["push", "-q", "origin", "main"],
    );
    git(
        &fixture.fixture.repository,
        &["push", "-q", "origin", "--delete", "feature"],
    );
    assert!(fixture.plan().requires_approval);
    let merge = git(&fixture.fixture.repository, &["rev-parse", "HEAD"]);
    let proof = IntegrationProof {
        branch_name: "feature".into(),
        original_head: original.clone(),
        merge_commit: merge.clone(),
        integration_branch: "main".into(),
        integration_head: merge.clone(),
        remote_identity: crate::storage::hash(
            format!(
                "{}\n",
                git(
                    &fixture.fixture.repository,
                    &["remote", "get-url", "origin"]
                )
            )
            .as_bytes(),
        ),
        verified_at_ms: fixture.now.load(Ordering::SeqCst),
    };
    let value =
        serde_json::json!({"repository_id":"project-alpha","branch_name":"feature","proof":proof})
            .to_string();
    fixture.fixture.database.call(|connection|{connection.execute("INSERT INTO storage_changes(kind,actor,created_at_ms,change_json) VALUES('checkout_integration_verified','fixture',$1,$2)",params![fixture.now.load(Ordering::SeqCst) as i64,value])?;Ok(())}).unwrap();
    let integrated = fixture.plan();
    assert!(integrated.ready, "{:?}", integrated.targets);
    assert_eq!(
        integrated.targets[0].head.as_deref(),
        Some(original.as_str())
    );
    assert_eq!(
        integrated.targets[0].integration_merge_commit.as_deref(),
        Some(merge.as_str())
    );
    fixture
        .now
        .fetch_add(crate::storage::SAFETY_FRESH_MS + 1, Ordering::SeqCst);
    let later = fixture.plan();
    assert!(
        later.ready,
        "immutable integration mapping still needs fresh remote recoverability: {:?}",
        later
    );
    fs::write(fixture.checkout.join("postmerge.txt"), "later work\n").unwrap();
    git(&fixture.checkout, &["add", "postmerge.txt"]);
    git(&fixture.checkout, &["commit", "-qm", "postmerge commit"]);
    assert!(
        fixture
            .service
            .checkout_remove_start(fixture.start(&integrated, None), &caller())
            .is_err()
    );
    let after = fixture.plan();
    assert!(after.requires_approval);
    assert!(after.targets[0].integration_merge_commit.is_none());
    assert!(fixture.checkout.exists());
}

#[test]
#[ignore = "requires a marked isolated root work area and explicit nonroot Git/PostgreSQL identities"]
fn expired_approval_recovers_only_the_already_removed_checkout_metadata() {
    let fixture = CheckoutFixture::new();
    fs::write(
        fixture.checkout.join("approved-draft"),
        "approved fixture data loss",
    )
    .unwrap();
    let plan = fixture.plan();
    let approval = fixture.approve(&plan);
    let queued = fixture
        .service
        .checkout_remove_start(fixture.start(&plan, Some(approval.receipt_id)), &caller())
        .unwrap();
    let original = fixture.storage.stored_plan(&plan.plan_id).unwrap().records[0].clone();
    fixture.fixture.database.call(|connection| {
        connection.execute("INSERT INTO storage_item_intents(job_id,artifact_id,fingerprint,revision,started_at_ms) VALUES($1,$2,$3,$4,$5)",params![queued.job.job_id,original.artifact.artifact_id,original.fingerprint,original.artifact.revision as i64,fixture.now.load(Ordering::SeqCst) as i64])?;
        Ok(())
    }).unwrap();
    let identity = crate::storage::fs::identity(&fixture.checkout).unwrap();
    crate::storage::fs::remove_tree(&fixture.checkout, identity, true, true).unwrap();
    assert!(
        git(
            &fixture.fixture.repository,
            &["worktree", "list", "--porcelain"]
        )
        .contains(fixture.checkout.to_str().unwrap())
    );
    fixture.now.store(plan.expires_at_ms + 1, Ordering::SeqCst);
    fixture.storage.execute_job(&queued.job.job_id).unwrap();
    let result = fixture
        .service
        .checkout_remove_job(
            removal::JobReference {
                project_id: "product".into(),
                job_id: queued.job.job_id,
            },
            &caller(),
        )
        .unwrap();
    assert!(result.targets[0].fully_removed, "{:?}", result.job);
    assert!(
        !git(
            &fixture.fixture.repository,
            &["worktree", "list", "--porcelain"]
        )
        .contains(fixture.checkout.to_str().unwrap())
    );
}

#[test]
#[ignore = "requires a marked isolated root work area and explicit nonroot Git/PostgreSQL identities"]
fn local_agent_approval_retains_the_exact_recorded_human_message_citation() {
    let fixture = CheckoutFixture::new();
    fs::write(fixture.checkout.join("draft"), "approved fixture draft").unwrap();
    let plan = fixture.plan();
    let note=serde_json::json!({"kind":"project_checkout_data_loss_consent","project_id":"product","plan_id":plan.plan_id,"fingerprint":plan.fingerprint,"effect":plan.effect,"user_consent_source":"fixture-chat:human-message-17"}).to_string();
    fixture.fixture.database.call(|connection| {
        connection.execute("INSERT INTO decisions(decision_id,repository_id,seq,aspect,title,body,technical_note,created_at,created_by) VALUES('exact-human-consent','project-alpha',2,'data','Remove the local draft','The fixture human approved the exact displayed loss.',$1,'t',$2)",params![note,caller().actor()])?;Ok(())
    }).unwrap();
    let receipt = fixture
        .service
        .checkout_remove_approve(
            removal::Approve {
                project_id: "product".into(),
                plan_id: plan.plan_id.clone(),
                fingerprint: plan.fingerprint.clone(),
                idempotency_key: "cited-exact-loss".into(),
                consent_decision_id: Some("exact-human-consent".into()),
            },
            &caller(),
        )
        .unwrap();
    assert_eq!(
        receipt.consent_decision_id.as_deref(),
        Some("exact-human-consent")
    );
    assert_eq!(
        receipt.user_consent_source.as_deref(),
        Some("fixture-chat:human-message-17")
    );
    let result = fixture.execute(fixture.start(&plan, Some(receipt.receipt_id)));
    assert!(result.targets[0].fully_removed);
}

#[test]
#[ignore = "requires a marked isolated root work area and explicit nonroot Git/PostgreSQL identities"]
fn selected_rollback_origins_and_real_processes_must_be_released_before_removal() {
    let fixture = CheckoutFixture::new();
    fixture.fixture.database.transaction(|connection| {
        connection.execute("INSERT INTO deployments(deployment_id,repository_id,worktree_id,name,source,spec_fingerprint,spec_json,state,current_generation,previous_generation,created_at,created_by_uid,client,updated_at) VALUES('deployment','project-alpha','w1111111111111111','web','checkout','fixture','{}','stopped',2,1,'t',$1,'fixture','t')",[caller().uid])?;
        for number in [1,2] {
            connection.execute("INSERT INTO generations(deployment_id,number,commit_hash,dirty,path,fingerprint,created_at,state) VALUES('deployment',$1,'fixture',0,$2,'fixture','t','current')",params![number,fixture.fixture._temporary.path().join(format!("immutable-{number}")).to_string_lossy().into_owned()])?;
        }
        connection.execute("INSERT INTO generation_source_origins(deployment_id,number,worktree_id,path) VALUES('deployment',1,'feature-checkout',$1)",[fixture.checkout.to_string_lossy().into_owned()])?;
        Ok(())
    }).unwrap();
    let plan = fixture.plan();
    assert!(
        plan.targets[0]
            .active_uses
            .iter()
            .any(|reason| reason == "current_or_rollback_deployment")
    );
    assert!(
        fixture
            .service
            .checkout_remove_start(fixture.start(&plan, None), &caller())
            .is_err()
    );
    fixture
        .fixture
        .database
        .call(|connection| {
            connection.execute(
                "UPDATE generation_source_origins SET path=$1 WHERE deployment_id='deployment'",
                [fixture.fixture.repository.to_string_lossy().into_owned()],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(fixture.plan().ready);

    struct OwnedProcess(std::process::Child);
    impl Drop for OwnedProcess {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let process = OwnedProcess(
        Command::new("/usr/bin/sleep")
            .arg("60")
            .current_dir(&fixture.checkout)
            .spawn()
            .unwrap(),
    );
    let busy = fixture.plan();
    assert!(
        busy.targets[0]
            .active_uses
            .iter()
            .any(|reason| reason == "active_process")
    );
    assert!(
        fixture
            .service
            .checkout_remove_start(fixture.start(&busy, None), &caller())
            .is_err()
    );
    drop(process);
    assert!(fixture.plan().ready);
}

#[test]
#[ignore = "requires a marked isolated root work area and explicit nonroot Git/PostgreSQL identities"]
fn local_ref_retirement_requires_cleanup_and_compare_and_swap_head() {
    let fixture = CheckoutFixture::new();
    let plan = fixture.plan();
    let result = fixture.execute(fixture.start(&plan, None));
    let proof = fixture
        .service
        .verified_checkout_removal("product", &result.job.job_id, "feature-checkout")
        .unwrap();
    for unavailable_owner in [0_u32, 4_294_967_200] {
        fixture.fixture.database.call(|connection|{connection.execute("UPDATE repositories SET registered_by_uid=$1 WHERE repository_id='project-alpha'",[unavailable_owner])?;Ok(())}).unwrap();
        assert!(
            fixture
                .service
                .retire_local_checkout_branch(
                    "product",
                    "project-alpha",
                    "feature",
                    &proof,
                    &caller()
                )
                .is_err()
        );
        assert!(
            !git(
                &fixture.fixture.repository,
                &["rev-parse", "refs/heads/feature"]
            )
            .is_empty()
        );
    }
    fixture
        .fixture
        .database
        .call(|connection| {
            connection.execute(
                "UPDATE repositories SET registered_by_uid=$1 WHERE repository_id='project-alpha'",
                [caller().uid],
            )?;
            Ok(())
        })
        .unwrap();
    git(
        &fixture.fixture.repository,
        &[
            "update-ref",
            "refs/heads/feature",
            &git(&fixture.fixture.repository, &["rev-parse", "HEAD"]),
        ],
    );
    let retired = fixture
        .service
        .retire_local_checkout_branch("product", "project-alpha", "feature", &proof, &caller())
        .unwrap();
    assert!(!retired.receipt_id.is_empty());
    let again = fixture
        .service
        .retire_local_checkout_branch("product", "project-alpha", "feature", &proof, &caller())
        .unwrap();
    assert_eq!(retired.receipt_id, again.receipt_id);
    fs::write(
        fixture.fixture.repository.join("main-change"),
        "another head",
    )
    .unwrap();
    git(&fixture.fixture.repository, &["add", "main-change"]);
    git(&fixture.fixture.repository, &["commit", "-qm", "new head"]);
    let other = git(&fixture.fixture.repository, &["rev-parse", "HEAD"]);
    git(
        &fixture.fixture.repository,
        &["update-ref", "refs/heads/feature", &other],
    );
    assert!(
        fixture
            .service
            .retire_local_checkout_branch("product", "project-alpha", "feature", &proof, &caller())
            .is_err()
    );
    assert_eq!(
        git(
            &fixture.fixture.repository,
            &["rev-parse", "refs/heads/feature"]
        ),
        other
    );
}
