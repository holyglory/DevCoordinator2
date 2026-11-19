//! Boundary regressions extend the existing disposable PostgreSQL fixture and
//! exercise ProjectService -> StorageService -> real Git/filesystem effects.
//! The root acceptance suite covers storage already; these project-specific
//! consent and exact-revision paths cannot be expressed through that old API.
use super::*;
use crate::automation_test_support::{Fixture, FixtureClock};
use crate::{capacity::CapacityBroker, events::EventService, platform::Clock};
use devcoordinator2_api::{ClientKind, storage as storage_api};
use std::{
    cell::RefCell,
    fs,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

const FIXTURE_EDGE_UID: u32 = 31_415;
static REVIEW_KEY: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static FIXTURE_OWNERS:RefCell<Vec<PathBuf>>=const{RefCell::new(Vec::new())};
}

struct FixtureOwnership(PathBuf);
impl FixtureOwnership {
    fn record(temporary: &tempfile::TempDir) -> Self {
        let path = temporary.path().to_path_buf();
        FIXTURE_OWNERS.with(|owners| owners.borrow_mut().push(path.clone()));
        Self(path)
    }
}
impl Drop for FixtureOwnership {
    fn drop(&mut self) {
        FIXTURE_OWNERS.with(|owners| owners.borrow_mut().retain(|path| path != &self.0));
    }
}

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
    use std::os::unix::fs::MetadataExt;
    let caller = caller();
    let metadata = path.symlink_metadata().unwrap();
    if (metadata.uid(), metadata.gid()) != (caller.uid, caller.gid) {
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(
            unsafe { libc::lchown(name.as_ptr(), caller.uid, caller.gid) },
            0
        );
    }
    if metadata.file_type().is_dir() {
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
    _ownership: FixtureOwnership,
}

fn git(path: &Path, args: &[&str]) -> String {
    let owner = if unsafe { libc::geteuid() } == 0 {
        let owners = FIXTURE_OWNERS.with(|owners| {
            owners
                .borrow()
                .iter()
                .filter(|owner| path.starts_with(owner))
                .cloned()
                .collect::<Vec<_>>()
        });
        assert_eq!(
            owners.len(),
            1,
            "Git must have exactly one explicitly recorded TempDir owner"
        );
        Some(owners[0].clone())
    } else {
        None
    };
    if let Some(owner) = &owner {
        own_fixture_tree(owner);
    }
    let mut command = if unsafe { libc::geteuid() } == 0 {
        let caller = caller();
        assert_ne!(
            caller.uid, 0,
            "fixture Git requires a real nonroot execution identity"
        );
        let mut command = Command::new("/usr/bin/setpriv");
        command.args([
            format!("--reuid={}", caller.uid),
            format!("--regid={}", caller.gid),
            "--init-groups".into(),
            "--reset-env".into(),
            "--".into(),
            "/usr/bin/git".into(),
        ]);
        command
    } else {
        Command::new("/usr/bin/git")
    };
    let output = command
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
    if let Some(owner) = owner {
        own_fixture_tree(&owner);
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
        uid: FIXTURE_EDGE_UID,
        via_edge: true,
        client_kind: ClientKind::Edge,
        identity: Some("owner@example.invalid".into()),
        ..caller()
    }
}

impl CheckoutFixture {
    fn new() -> Self {
        let confined = work_root();
        let mut fixture = Fixture::new();
        fixture.config.edge_uid = Some(FIXTURE_EDGE_UID);
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
        let ownership = FixtureOwnership::record(&fixture._temporary);
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
            _ownership: ownership,
        }
    }

    fn review(&self, request: removal::Plan, actor: &Caller) -> removal::ReviewJob {
        let queued = self.service.checkout_remove_plan(request, actor).unwrap();
        assert!(queued.plan.is_none());
        self.storage
            .execute_queued_job_for_fixture(&queued.job.job_id)
            .unwrap();
        self.service
            .checkout_remove_plan_status(
                removal::JobReference {
                    project_id: queued.project_id,
                    job_id: queued.job.job_id,
                },
                actor,
            )
            .unwrap()
    }
    fn plan(&self) -> removal::RemovalPlan {
        own_fixture_tree(self.fixture._temporary.path());
        self.review(
            removal::Plan {
                project_id: "product".into(),
                idempotency_key: format!(
                    "fixture-review-{}",
                    REVIEW_KEY.fetch_add(1, Ordering::Relaxed)
                ),
                expected_project_revision: 1,
                targets: vec![removal::Target {
                    worktree_id: "feature-checkout".into(),
                    expected_revision: 1,
                }],
                include_absolute_paths: false,
            },
            &caller(),
        )
        .plan
        .expect("complete real fixture review")
    }
    fn assert_rejected(&self, start: removal::Start) {
        if let Ok(queued) = self.service.checkout_remove_start(start, &caller()) {
            self.storage
                .execute_queued_job_for_fixture(&queued.job.job_id)
                .unwrap();
            let result = self
                .service
                .checkout_remove_job(
                    removal::JobReference {
                        project_id: "product".into(),
                        job_id: queued.job.job_id,
                    },
                    &caller(),
                )
                .unwrap();
            assert!(
                !result.targets.iter().any(|target| target.fully_removed),
                "{:?}",
                result.job
            );
            assert!(
                matches!(
                    result.job.state,
                    storage_api::JobState::Failed | storage_api::JobState::Cancelled
                ),
                "{:?}",
                result.job
            );
        }
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
        self.storage
            .execute_queued_job_for_fixture(&job.job.job_id)
            .unwrap();
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
    // Exercise the real source catalog family. Its public auth namespace must
    // remain removable without weakening the actual auth.json credential guard.
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../console/locales");
    let destination = fixture.checkout.join("public-translations");
    copy_translation_fixture(&source, &destination);
    git(&fixture.checkout, &["add", "--", "public-translations"]);
    git(
        &fixture.checkout,
        &["commit", "-qm", "public translation source"],
    );
    git(&fixture.checkout, &["push", "-q", "origin", "feature"]);
    git(
        &fixture.fixture.repository,
        &["switch", "-q", "-c", "other-active-work"],
    );
    fs::write(
        fixture.fixture.repository.join("other-work.txt"),
        "first private version\n",
    )
    .unwrap();
    git(&fixture.fixture.repository, &["add", "other-work.txt"]);
    git(
        &fixture.fixture.repository,
        &["commit", "-qm", "private history"],
    );
    let shared_history = git(&fixture.fixture.repository, &["rev-parse", "HEAD"]);
    fs::write(
        fixture.fixture.repository.join("other-work.txt"),
        "second private version\n",
    )
    .unwrap();
    git(&fixture.fixture.repository, &["add", "other-work.txt"]);
    git(
        &fixture.fixture.repository,
        &["commit", "--amend", "--no-edit", "-q"],
    );
    git(&fixture.fixture.repository, &["switch", "-q", "main"]);
    let plan = fixture.plan();
    assert!(plan.targets[0].unpreserved_git_history.is_empty());
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
    git(
        &fixture.fixture.repository,
        &["cat-file", "-e", &format!("{shared_history}^{{commit}}")],
    );
}

fn copy_translation_fixture(source: &Path, destination: &Path) {
    fs::create_dir(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        let target = destination.join(entry.file_name());
        if kind.is_dir() {
            copy_translation_fixture(&entry.path(), &target);
        } else {
            assert!(
                kind.is_file(),
                "catalog fixtures must contain regular source files"
            );
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
#[ignore = "requires a marked isolated root work area and explicit nonroot Git/PostgreSQL identities"]
fn exact_console_approval_allows_known_loss_but_stale_content_requires_a_new_plan() {
    for (count, minimum) in [(64, 64 * 1024), (1024, 1024 * 1024)] {
        let large = CheckoutFixture::new();
        let (retained, bytes) = ignored_listing_fixture(&large, count);
        assert!(bytes > minimum);
        assert!(bytes < crate::repository::GIT_AUDIT_OUTPUT_LIMIT);
        let plan = large.plan();
        assert!(plan.blockers.is_empty(), "{plan:?}");
        assert!(!plan.targets[0].dirty);
        assert_eq!(plan.targets[0].untracked_files, 0);
        assert_eq!(plan.targets[0].ignored_files, count as u64);
        assert!(plan.requires_approval && !plan.ready);
        assert_eq!(
            large
                .service
                .checkout_remove_start(large.start(&plan, None), &caller())
                .unwrap_err()
                .message,
            "exact_data_loss_approval_required"
        );
        assert!(retained.is_file());
        let approval = large.approve(&plan);
        fs::write(&retained, "ignored bytes changed after approval\n").unwrap();
        large.assert_rejected(large.start(&plan, Some(approval.receipt_id)));
        assert!(retained.is_file());
        let fresh = large.plan();
        let approval = large.approve(&fresh);
        assert!(
            large
                .execute(large.start(&fresh, Some(approval.receipt_id)))
                .targets[0]
                .fully_removed
        );
        assert!(!large.checkout.exists());
        assert!(large.fixture.repository.join("source.txt").is_file());
    }
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
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(
        fixture.checkout.join(".devcoordinator"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let inaccessible = fixture.review(
        removal::Plan {
            project_id: "product".into(),
            idempotency_key: format!(
                "fixture-review-{}",
                REVIEW_KEY.fetch_add(1, Ordering::Relaxed)
            ),
            expected_project_revision: 1,
            targets: vec![removal::Target {
                worktree_id: "feature-checkout".into(),
                expected_revision: 1,
            }],
            include_absolute_paths: false,
        },
        &caller(),
    );
    assert!(matches!(
        inaccessible.job.state,
        storage_api::JobState::Failed
    ));
    assert!(inaccessible.plan.is_none());
    assert!(
        matches!(
            inaccessible.job.error_code.as_deref().unwrap_or(""),
            "ignored_files_unverified" | "checkout_status_unverified"
        ),
        "{inaccessible:?}"
    );
    assert!(current.join("check-report.json").exists());
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
    let spoofed = Caller {
        uid: caller().uid,
        ..administrator()
    };
    let error = fixture
        .service
        .checkout_remove_approve(
            removal::Approve {
                project_id: "product".into(),
                plan_id: plan.plan_id.clone(),
                fingerprint: plan.fingerprint.clone(),
                idempotency_key: "untrusted-edge-claim".into(),
                consent_decision_id: None,
            },
            &spoofed,
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::PermissionDenied);
    let approval = fixture.approve(&plan);
    assert_eq!(fixture.approve(&plan).receipt_id, approval.receipt_id);
    fs::write(fixture.checkout.join("untracked.txt"), "other draft\n").unwrap();
    fixture.assert_rejected(fixture.start(&plan, Some(approval.receipt_id)));
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
    replacement.assert_rejected(replacement.start(&plan, None));
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
    leased.assert_rejected(leased.start(&plan, None));
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
        "auth-credentials",
        "tracked-auth-credentials",
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
            "auth-credentials" | "tracked-auth-credentials" => {
                fs::write(
                    fixture.checkout.join("auth.json"),
                    "fixture-secret-never-returned",
                )
                .unwrap();
                if scope == "tracked-auth-credentials" {
                    git(&fixture.checkout, &["add", "auth.json"]);
                    git(
                        &fixture.checkout,
                        &["commit", "-qm", "tracked credential fixture"],
                    );
                    git(&fixture.checkout, &["push", "-q", "origin", "feature"]);
                }
            }
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
        assert_eq!(
            approval.unwrap_err().message,
            "plan_has_blockers",
            "{scope}"
        );
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
    fixture
        .storage
        .execute_queued_job_for_fixture(&queued.job.job_id)
        .unwrap();
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
    assert_eq!(
        target.local_refs,
        git(&clone, &["for-each-ref", "--format=%(refname)", "refs/"])
            .lines()
            .count() as u64
    );
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
    let clean = CheckoutFixture::new();
    let clean_clone = independent_history_clone(&clean, "clean-preserved");
    enlarge_reflog_message(&clean_clone.join(".git/logs/HEAD"));
    assert!(
        git(
            &clean_clone,
            &["reflog", "show", "--all", "--format=%H%x00%gD%x00%gs"]
        )
        .len()
            > 64 * 1024
    );
    let clean_plan = clean.plan();
    assert!(clean_plan.ready, "{clean_plan:?}");
    assert!(clean_plan.targets[0].reflog_entries > 0);
    assert!(clean_plan.targets[0].unpreserved_git_history.is_empty());
    assert!(clean.execute(clean.start(&clean_plan, None)).targets[0].fully_removed);
    assert!(!clean_clone.exists());

    let history = CheckoutFixture::new();
    let clone = independent_history_clone(&history, "amended-history");
    fs::write(clone.join("source.txt"), "unpublished old content\n").unwrap();
    git(&clone, &["add", "source.txt"]);
    git(&clone, &["commit", "-qm", "unpublished old draft"]);
    let old = git(&clone, &["rev-parse", "HEAD"]);
    fs::write(clone.join("source.txt"), "published amended content\n").unwrap();
    git(&clone, &["add", "source.txt"]);
    git(&clone, &["commit", "--amend", "--no-edit", "-q"]);
    git(&clone, &["push", "-q", "origin", "feature"]);
    assert!(git(&clone, &["status", "--porcelain=v1"]).is_empty());
    let reflog = history.plan();
    assert!(
        reflog.targets[0]
            .unpreserved_git_history
            .contains(&format!("commit:{old}"))
    );
    assert!(
        reflog.targets[0]
            .possible_data_loss
            .iter()
            .any(|reason| reason == "unpreserved_local_history")
    );
    assert!(reflog.requires_approval && !reflog.ready && reflog.blockers.is_empty());
    history.assert_rejected(history.start(&reflog, None));
    git(&clone, &["reflog", "expire", "--expire=now", "--all"]);
    let dangling = history.plan();
    assert_eq!(dangling.targets[0].reflog_entries, 0);
    assert!(
        dangling.targets[0]
            .unpreserved_git_history
            .contains(&format!("commit:{old}"))
    );
    let approval = history.approve(&dangling);
    let blob_path = history
        .fixture
        ._temporary
        .path()
        .join("new-unreferenced-blob");
    fs::write(&blob_path, "new object after exact approval\n").unwrap();
    git(&clone, &["hash-object", "-w", blob_path.to_str().unwrap()]);
    history.assert_rejected(history.start(&dangling, Some(approval.receipt_id)));
    assert!(clone.exists());
    let fresh = history.plan();
    let approval = history.approve(&fresh);
    assert!(
        history
            .execute(history.start(&fresh, Some(approval.receipt_id)))
            .targets[0]
            .fully_removed
    );
    assert!(!clone.exists() && history.fixture.repository.exists() && history.checkout.exists());

    let corrupt = CheckoutFixture::new();
    let clone = independent_history_clone(&corrupt, "corrupt-objects");
    let invalid = clone.join(".git/objects/aa");
    fs::create_dir_all(&invalid).unwrap();
    fs::write(
        invalid.join("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        "invalid compressed Git object\n",
    )
    .unwrap();
    own_fixture_tree(corrupt.fixture._temporary.path());
    let review = corrupt.review(
        removal::Plan {
            project_id: "product".into(),
            idempotency_key: "corrupt-native-object-review".into(),
            expected_project_revision: 1,
            targets: vec![removal::Target {
                worktree_id: "feature-checkout".into(),
                expected_revision: 1,
            }],
            include_absolute_paths: false,
        },
        &caller(),
    );
    assert!(matches!(review.job.state, storage_api::JobState::Failed));
    assert!(review.plan.is_none());
    assert!(clone.exists());
}

fn independent_history_clone(fixture: &CheckoutFixture, name: &str) -> PathBuf {
    let clone = fixture.fixture._temporary.path().join(name);
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
    fixture.fixture.database.transaction(|connection| {
        connection.execute("INSERT INTO repositories(repository_id,root_path,display_name,registered_at,registered_by_uid,last_seen_at) VALUES('clone',$1,'History clone','t',$2,'t')",params![clone.to_string_lossy().into_owned(),caller().uid])?;
        connection.execute("INSERT INTO project_repository_memberships(project_id,repository_id,physical_repository_id) VALUES('product','project-alpha','clone')",[])?;
        connection.execute("UPDATE project_checkouts SET physical_repository_id='clone',absolute_path=$1,role='clone' WHERE worktree_id='feature-checkout'",[clone.to_string_lossy().into_owned()])?;
        Ok(())
    }).unwrap();
    clone
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
        .review(
            removal::Plan {
                project_id: "product".into(),
                idempotency_key: format!(
                    "fixture-review-{}",
                    REVIEW_KEY.fetch_add(1, Ordering::Relaxed)
                ),
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
        .plan
        .expect("completed fixture review");
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
    fixture
        .storage
        .execute_queued_job_for_fixture(&queued.job.job_id)
        .unwrap();
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
    leased
        .storage
        .execute_queued_job_for_fixture(&queued.job.job_id)
        .unwrap();
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
    fixture
        .storage
        .execute_queued_job_for_fixture(&queued.job.job_id)
        .unwrap();
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
    let tree = git(&fixture.fixture.repository, &["rev-parse", "HEAD^{tree}"]);
    let old = git(
        &fixture.fixture.repository,
        &["commit-tree", &tree, "-m", "unpreserved reflog history"],
    );
    git(
        &fixture.fixture.repository,
        &["update-ref", "refs/heads/feature", &old],
    );
    let long_message = "h".repeat(70 * 1024);
    git(
        &fixture.fixture.repository,
        &[
            "update-ref",
            "-m",
            &long_message,
            "refs/heads/feature",
            proof.head_sha(),
        ],
    );
    assert!(
        fs::metadata(
            fixture
                .fixture
                .repository
                .join(".git/logs/refs/heads/feature")
        )
        .unwrap()
        .len()
            > 64 * 1024
    );
    let blocked = fixture
        .service
        .retire_local_checkout_branch("product", "project-alpha", "feature", &proof, &caller())
        .unwrap_err();
    assert!(
        blocked.message.contains("history not preserved"),
        "{blocked:?}"
    );
    assert_eq!(
        git(
            &fixture.fixture.repository,
            &["rev-parse", "refs/heads/feature"]
        ),
        proof.head_sha()
    );
    git(
        &fixture.fixture.repository,
        &["reflog", "delete", "refs/heads/feature@{1}"],
    );
    assert!(
        !git(
            &fixture.fixture.repository,
            &["reflog", "show", "--format=%H", "refs/heads/feature"]
        )
        .lines()
        .any(|oid| oid == old)
    );
    assert!(
        fs::read_to_string(
            fixture
                .fixture
                .repository
                .join(".git/logs/refs/heads/feature")
        )
        .unwrap()
        .contains(&old)
    );
    let blocked = fixture
        .service
        .retire_local_checkout_branch("product", "project-alpha", "feature", &proof, &caller())
        .unwrap_err();
    assert!(
        blocked.message.contains("history not preserved"),
        "{blocked:?}"
    );
    // Existing active work in the canonical main branch now preserves this
    // exact old commit; the product does not create a recovery/archive ref.
    git(
        &fixture.fixture.repository,
        &["update-ref", "refs/heads/main", &old],
    );

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
        &["update-ref", "refs/heads/feature", proof.head_sha()],
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

fn queued_review(fixture: &CheckoutFixture, key: &str) -> removal::ReviewJob {
    own_fixture_tree(fixture.fixture._temporary.path());
    fixture
        .service
        .checkout_remove_plan(
            removal::Plan {
                project_id: "product".into(),
                idempotency_key: key.into(),
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

#[test]
#[ignore = "requires a marked isolated root work area and explicit nonroot Git/PostgreSQL identities"]
fn pending_review_exact_project_and_source_authorization_precedes_status_and_cancel() {
    let fixture = CheckoutFixture::new();
    let review = queued_review(&fixture, "pending-auth");
    assert!(matches!(review.job.state, storage_api::JobState::Queued));
    assert_eq!(review.job.actor, caller().actor());
    assert_eq!(review.progress.targets_total, 1);
    assert_eq!(review.progress.bytes_hashed, 0);
    assert!(review.plan.is_none());
    assert_eq!(
        queued_review(&fixture, "pending-auth").job.job_id,
        review.job.job_id
    );
    fixture.fixture.database.call(|connection| {
        connection.execute("INSERT INTO projects(project_id,name,slug,revision,created_at,updated_at,updated_by) VALUES('different','Different','different',1,'t','t','fixture')",[])?;
        connection.execute("INSERT INTO users(user_id,email,administrator,created_at,created_by) VALUES('outsider','outsider@example.invalid',0,'t','fixture')",[])?;
        Ok(())
    }).unwrap();
    let wrong_project = removal::JobReference {
        project_id: "different".into(),
        job_id: review.job.job_id.clone(),
    };
    assert!(
        fixture
            .service
            .checkout_remove_plan_status(wrong_project.clone(), &administrator())
            .is_err()
    );
    assert!(
        fixture
            .service
            .checkout_remove_plan_cancel(wrong_project, &administrator())
            .is_err()
    );
    let outsider = Caller {
        identity: Some("outsider@example.invalid".into()),
        ..administrator()
    };
    let exact = removal::JobReference {
        project_id: "product".into(),
        job_id: review.job.job_id.clone(),
    };
    assert!(
        fixture
            .service
            .checkout_remove_plan_status(exact.clone(), &outsider)
            .is_err()
    );
    assert!(
        fixture
            .service
            .checkout_remove_plan_cancel(exact.clone(), &outsider)
            .is_err()
    );
    assert!(matches!(
        fixture.storage.job(&review.job.job_id).unwrap().state,
        storage_api::JobState::Queued
    ));
    let cancelled = fixture
        .service
        .checkout_remove_plan_cancel(exact, &caller())
        .unwrap();
    assert!(matches!(
        cancelled.job.state,
        storage_api::JobState::Cancelled
    ));
    assert!(cancelled.plan.is_none());
    assert!(fixture.checkout.exists());
    fixture.fixture.database.call(|connection| {
        connection.execute("INSERT INTO users(user_id,email,administrator,created_at,created_by) VALUES('operator','operator@example.invalid',0,'t','fixture')",[])?;
        connection.execute("INSERT INTO deployments(deployment_id,repository_id,worktree_id,name,source,spec_fingerprint,spec_json,state,created_at,created_by_uid,client,updated_at) VALUES('operator-access','project-alpha','w1111111111111111','Operator source grant','checkout','fixture','{}','stopped','t',$1,'fixture','t')",[caller().uid])?;
        connection.execute("INSERT INTO grants(user_id,deployment_id,role,granted_at,granted_by) VALUES('operator','operator-access','operator','t','fixture')",[])?;
        Ok(())
    }).unwrap();
    let operator = Caller {
        identity: Some("operator@example.invalid".into()),
        ..administrator()
    };
    let private_review = fixture
        .service
        .checkout_remove_plan(
            removal::Plan {
                project_id: "product".into(),
                idempotency_key: "administrator-absolute-review".into(),
                expected_project_revision: 1,
                targets: vec![removal::Target {
                    worktree_id: "feature-checkout".into(),
                    expected_revision: 1,
                }],
                include_absolute_paths: true,
            },
            &administrator(),
        )
        .unwrap();
    let reference = removal::JobReference {
        project_id: "product".into(),
        job_id: private_review.job.job_id.clone(),
    };
    for completed in [false, true] {
        if completed {
            fixture
                .storage
                .execute_queued_job_for_fixture(&reference.job_id)
                .unwrap();
        }
        assert_eq!(
            fixture
                .service
                .checkout_remove_plan_status(reference.clone(), &operator)
                .unwrap_err()
                .code,
            ErrorCode::PermissionDenied
        );
        assert_eq!(
            fixture
                .service
                .checkout_remove_plan_cancel(reference.clone(), &operator)
                .unwrap_err()
                .code,
            ErrorCode::PermissionDenied
        );
        let observed = fixture
            .service
            .checkout_remove_plan_status(reference.clone(), &administrator())
            .unwrap();
        assert!(if completed {
            matches!(observed.job.state, storage_api::JobState::Completed)
        } else {
            matches!(observed.job.state, storage_api::JobState::Queued)
        });
        if completed {
            assert!(observed.plan.unwrap().targets[0].path_is_absolute);
        }
    }
}

#[test]
#[ignore = "requires a marked isolated root work area and explicit nonroot Git/PostgreSQL identities"]
fn review_hashing_yields_reports_real_progress_cancels_and_restarts_without_partial_plan() {
    let tracked_listing_bytes = |fixture: &CheckoutFixture, count: usize| {
        let directory = fixture.checkout.join("tracked-index-fixture");
        fs::create_dir(&directory).unwrap();
        let suffix = "x".repeat(96);
        for index in 0..count {
            fs::write(
                directory.join(format!("file-{index:04}-{suffix}.txt")),
                format!("tracked fixture entry {index}\n"),
            )
            .unwrap();
        }
        git(&fixture.checkout, &["add", "--", "tracked-index-fixture"]);
        git(
            &fixture.checkout,
            &["commit", "-qm", "tracked listing fixture"],
        );
        git(&fixture.checkout, &["push", "-q", "origin", "feature"]);
        git(&fixture.checkout, &["ls-files", "--stage", "-z"]).len()
    };
    let fixture = CheckoutFixture::new();
    let listing_bytes = tracked_listing_bytes(&fixture, 512);
    assert!(listing_bytes > 64 * 1024);
    assert!(listing_bytes < crate::repository::WORKTREE_LIST_OUTPUT_LIMIT);
    fs::write(
        fixture.checkout.join("ignored.bin"),
        vec![b'x'; 3 * 1024 * 1024],
    )
    .unwrap();
    let review = queued_review(&fixture, "bounded-review");
    fixture
        .storage
        .begin_checkout_review_for_fixture(&review.job.job_id)
        .unwrap();
    let mut cursor = None;
    let budget = || crate::storage::native::DiscoveryBudget {
        entries: 8,
        elapsed: std::time::Duration::from_millis(25),
    };
    let mut previous = 0;
    for _ in 0..512 {
        assert!(
            !fixture
                .storage
                .execute_checkout_review_step(&review.job.job_id, &mut cursor, budget())
                .unwrap()
        );
        let progress = fixture
            .service
            .checkout_remove_plan_status(
                removal::JobReference {
                    project_id: "product".into(),
                    job_id: review.job.job_id.clone(),
                },
                &caller(),
            )
            .unwrap();
        assert!(progress.plan.is_none());
        assert_eq!(progress.progress.targets_done, 0);
        assert!(progress.progress.bytes_hashed >= previous);
        assert!(progress.progress.bytes_hashed - previous <= 256 * 1024);
        previous = progress.progress.bytes_hashed;
        if previous > 512 * 1024 {
            break;
        }
    }
    assert!(
        previous > 512 * 1024,
        "real file content must be incrementally read"
    );
    let cancelled = fixture
        .service
        .checkout_remove_plan_cancel(
            removal::JobReference {
                project_id: "product".into(),
                job_id: review.job.job_id.clone(),
            },
            &caller(),
        )
        .unwrap();
    assert!(matches!(
        cancelled.job.state,
        storage_api::JobState::Cancelling
    ));
    assert!(
        fixture
            .storage
            .execute_checkout_review_step(&review.job.job_id, &mut cursor, budget())
            .unwrap()
    );
    assert!(fixture.checkout.join("ignored.bin").is_file());
    let plans = fixture
        .fixture
        .database
        .read(|connection| {
            connection
                .query_row("SELECT count(*) FROM storage_plans", [], |row| {
                    row.get::<_, u64>(0)
                })
                .map_err(DatabaseError::from)
        })
        .unwrap();
    assert_eq!(plans, 0, "a cancelled partial proof cannot enable approval");

    let recovered = queued_review(&fixture, "recovered-review");
    fixture
        .storage
        .begin_checkout_review_for_fixture(&recovered.job.job_id)
        .unwrap();
    let mut old_cursor = None;
    for _ in 0..512 {
        assert!(
            !fixture
                .storage
                .execute_checkout_review_step(&recovered.job.job_id, &mut old_cursor, budget())
                .unwrap()
        );
        if fixture
            .storage
            .checkout_review(&recovered.job.job_id)
            .unwrap()
            .1
            .progress
            .bytes_hashed
            > 512 * 1024
        {
            break;
        }
    }
    drop(old_cursor); // Native handles/digests are lost on daemon interruption.
    fs::write(
        fixture.checkout.join("source.txt"),
        "work changed while review was interrupted\n",
    )
    .unwrap();
    own_fixture_tree(fixture.fixture._temporary.path());
    let mut restored = None;
    assert!(
        !fixture
            .storage
            .execute_checkout_review_step(&recovered.job.job_id, &mut restored, budget())
            .unwrap()
    );
    let reset = fixture
        .storage
        .checkout_review(&recovered.job.job_id)
        .unwrap();
    assert_eq!(reset.1.progress.bytes_hashed, 0);
    assert_eq!(reset.1.progress.entries_hashed, 0);
    assert!(reset.0.plan_id.is_none());
    for _ in 0..1024 {
        if fixture
            .storage
            .execute_checkout_review_step(&recovered.job.job_id, &mut restored, budget())
            .unwrap()
        {
            break;
        }
    }
    let completed = fixture
        .service
        .checkout_remove_plan_status(
            removal::JobReference {
                project_id: "product".into(),
                job_id: recovered.job.job_id.clone(),
            },
            &caller(),
        )
        .unwrap();
    assert!(matches!(
        completed.job.state,
        storage_api::JobState::Completed
    ));
    assert_eq!(completed.progress.targets_done, 1);
    assert!(completed.progress.bytes_hashed >= 3 * 1024 * 1024);
    assert!(completed.progress.entries_hashed > 0);
    let plan = completed.plan.unwrap();
    assert!(plan.targets[0].dirty);
    assert!(plan.requires_approval);
    let actor = fixture
        .fixture
        .database
        .read(|connection| {
            connection
                .query_row(
                    "SELECT actor FROM storage_plans WHERE plan_id=$1",
                    [&plan.plan_id],
                    |row| row.get::<_, String>(0),
                )
                .map_err(DatabaseError::from)
        })
        .unwrap();
    assert_eq!(
        actor,
        caller().actor(),
        "review attribution must not become uid:0"
    );
    let approval = fixture.approve(&plan);
    let removed = fixture.execute(fixture.start(&plan, Some(approval.receipt_id)));
    assert!(removed.targets[0].fully_removed);
    assert!(!fixture.checkout.exists());
    assert!(fixture.fixture.repository.join("source.txt").is_file());
}

#[test]
#[ignore = "requires a marked isolated root work area and explicit nonroot Git/PostgreSQL identities"]
fn staged_listing_overflow_fails_without_a_removal_plan() {
    let fixture = CheckoutFixture::new();
    // Keep the oversized listing fixture deterministic: Git's automatic
    // maintenance is unrelated to the review bound and may race its linked
    // worktree while thousands of synthetic paths are added.
    git(&fixture.checkout, &["config", "gc.auto", "0"]);
    let directory = fixture.checkout.join("oversized-index-fixture");
    fs::create_dir(&directory).unwrap();
    let suffix = "x".repeat(96);
    for index in 0..6144 {
        fs::write(
            directory.join(format!("file-{index:04}-{suffix}.txt")),
            format!("tracked fixture entry {index}\n"),
        )
        .unwrap();
    }
    git(&fixture.checkout, &["add", "--", "oversized-index-fixture"]);
    git(
        &fixture.checkout,
        &["commit", "-qm", "oversized tracked listing fixture"],
    );
    git(&fixture.checkout, &["push", "-q", "origin", "feature"]);
    assert!(
        git(&fixture.checkout, &["ls-files", "--stage", "-z"]).len()
            > crate::repository::WORKTREE_LIST_OUTPUT_LIMIT
    );
    let review = queued_review(&fixture, "oversized-staged-listing");
    fixture
        .storage
        .execute_checkout_review_for_fixture(&review.job.job_id)
        .unwrap();
    let rejected = fixture
        .service
        .checkout_remove_plan_status(
            removal::JobReference {
                project_id: "product".into(),
                job_id: review.job.job_id,
            },
            &caller(),
        )
        .unwrap();
    assert!(matches!(rejected.job.state, storage_api::JobState::Failed));
    assert_eq!(
        rejected.job.error_code.as_deref(),
        Some("git_baseline_unverified")
    );
    assert!(rejected.plan.is_none());
    assert!(rejected.job.plan_id.is_none());
    assert!(fixture.checkout.join("source.txt").is_file());
    let plans = fixture
        .fixture
        .database
        .read(|connection| {
            connection
                .query_row("SELECT count(*) FROM storage_plans", [], |row| {
                    row.get::<_, u64>(0)
                })
                .map_err(DatabaseError::from)
        })
        .unwrap();
    assert_eq!(
        plans, 0,
        "an incomplete staged listing cannot enable approval"
    );

    let ignored = CheckoutFixture::new();
    let (retained, bytes) = ignored_listing_fixture(&ignored, 6144);
    assert!(bytes > crate::repository::GIT_AUDIT_OUTPUT_LIMIT);
    let review = queued_review(&ignored, "oversized-ignored-listing");
    ignored
        .storage
        .execute_checkout_review_for_fixture(&review.job.job_id)
        .unwrap();
    let rejected = ignored
        .service
        .checkout_remove_plan_status(
            removal::JobReference {
                project_id: "product".into(),
                job_id: review.job.job_id,
            },
            &caller(),
        )
        .unwrap();
    assert!(matches!(rejected.job.state, storage_api::JobState::Failed));
    assert_eq!(
        rejected.job.error_code.as_deref(),
        Some("git_baseline_unverified")
    );
    assert!(rejected.plan.is_none() && rejected.job.plan_id.is_none());
    assert!(retained.is_file());
    let plans = ignored
        .fixture
        .database
        .read(|connection| {
            connection
                .query_row("SELECT count(*) FROM storage_plans", [], |row| {
                    row.get::<_, u64>(0)
                })
                .map_err(DatabaseError::from)
        })
        .unwrap();
    assert_eq!(
        plans, 0,
        "an incomplete ignored listing cannot enable approval"
    );
}

fn ignored_listing_fixture(fixture: &CheckoutFixture, count: usize) -> (PathBuf, usize) {
    let mut directory = fixture.checkout.join("ignored.bin");
    for index in 0..5 {
        directory.push(format!("segment-{index}-{}", "x".repeat(220)));
    }
    fs::create_dir_all(&directory).unwrap();
    let mut retained = directory.clone();
    for index in 0..count {
        retained = directory.join(format!("entry-{index:04}-{}", "y".repeat(220)));
        fs::write(&retained, "ignored fixture output\n").unwrap();
    }
    let bytes = git(
        &fixture.checkout,
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "-z",
        ],
    )
    .len();
    (retained, bytes)
}

fn enlarge_reflog_message(log: &Path) {
    let original = fs::read_to_string(log).unwrap();
    assert!(original.contains('\t'));
    fs::write(
        log,
        original.replacen('\t', &format!("\t{}", "r".repeat(70 * 1024)), 1),
    )
    .unwrap();
}

#[test]
#[ignore = "requires a marked isolated root work area and explicit nonroot Git/PostgreSQL identities"]
fn staged_only_change_with_unchanged_mm_status_invalidates_exact_loss_approval() {
    let fixture = CheckoutFixture::new();
    fs::write(
        fixture.checkout.join("source.txt"),
        "first staged content\n",
    )
    .unwrap();
    git(&fixture.checkout, &["add", "source.txt"]);
    fs::write(
        fixture.checkout.join("source.txt"),
        "unchanged working bytes\n",
    )
    .unwrap();
    let original = git(&fixture.checkout, &["status", "--porcelain=v1"]);
    assert!(original.starts_with("MM "), "{original}");
    let plan = fixture.plan();
    let approval = fixture.approve(&plan);
    let snapshot_before = crate::storage::fs::checkout_snapshot(&fixture.checkout)
        .unwrap()
        .fingerprint;
    let blob_path = fixture.fixture._temporary.path().join("new-stage-blob");
    fs::write(&blob_path, "different staged content\n").unwrap();
    let blob = git(
        &fixture.checkout,
        &["hash-object", "-w", blob_path.to_str().unwrap()],
    );
    git(
        &fixture.checkout,
        &[
            "update-index",
            "--cacheinfo",
            &format!("100644,{blob},source.txt"),
        ],
    );
    assert_eq!(
        git(&fixture.checkout, &["status", "--porcelain=v1"]),
        original
    );
    assert_eq!(
        crate::storage::fs::checkout_snapshot(&fixture.checkout)
            .unwrap()
            .fingerprint,
        snapshot_before,
        "only external linked-worktree index bytes changed"
    );
    fixture.assert_rejected(fixture.start(&plan, Some(approval.receipt_id)));
    assert_eq!(
        fs::read_to_string(fixture.checkout.join("source.txt")).unwrap(),
        "unchanged working bytes\n"
    );
    let fresh = fixture.plan();
    let approval = fixture.approve(&fresh);
    let log = PathBuf::from(git(
        &fixture.checkout,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "logs/HEAD",
        ],
    ));
    let heads_before = git(
        &fixture.checkout,
        &["reflog", "show", "--format=%H", "HEAD"],
    );
    let content_before = crate::storage::fs::checkout_snapshot(&fixture.checkout)
        .unwrap()
        .fingerprint;
    let original_log = fs::read_to_string(&log).unwrap();
    assert!(original_log.contains("Checkout fixture <checkout@example.invalid>"));
    fs::write(
        &log,
        original_log.replace(
            "Checkout fixture <checkout@example.invalid>",
            "Altered author <checkout@example.invalid>",
        ),
    )
    .unwrap();
    assert_eq!(
        git(
            &fixture.checkout,
            &["reflog", "show", "--format=%H", "HEAD"]
        ),
        heads_before
    );
    assert_eq!(
        crate::storage::fs::checkout_snapshot(&fixture.checkout)
            .unwrap()
            .fingerprint,
        content_before
    );
    fixture.assert_rejected(fixture.start(&fresh, Some(approval.receipt_id)));
    assert!(fixture.checkout.join("source.txt").is_file());
    let private = CheckoutFixture::new();
    let tree = git(&private.checkout, &["rev-parse", "HEAD^{tree}"]);
    let old = git(
        &private.checkout,
        &["commit-tree", &tree, "-m", "private HEAD history"],
    );
    git(&private.checkout, &["switch", "-q", "--detach", &old]);
    git(&private.checkout, &["switch", "-q", "feature"]);
    git(&private.checkout, &["reflog", "delete", "HEAD@{1}"]);
    assert!(
        !git(
            &private.checkout,
            &["reflog", "show", "--format=%H", "HEAD"]
        )
        .lines()
        .any(|oid| oid == old)
    );
    let head_log = PathBuf::from(git(
        &private.checkout,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "logs/HEAD",
        ],
    ));
    assert!(fs::read_to_string(&head_log).unwrap().contains(&old));
    enlarge_reflog_message(&head_log);
    assert!(
        git(
            &private.checkout,
            &["reflog", "show", "--format=%H%x00%gD%x00%gs", "HEAD"]
        )
        .len()
            > 64 * 1024
    );
    let plan = private.plan();
    assert!(
        plan.targets[0]
            .unpreserved_git_history
            .contains(&format!("commit:{old}"))
    );
    assert!(plan.requires_approval && !plan.ready && plan.blockers.is_empty());
    private.assert_rejected(private.start(&plan, None));
    let approval = private.approve(&plan);
    assert!(
        private
            .execute(private.start(&plan, Some(approval.receipt_id)))
            .targets[0]
            .fully_removed
    );
    assert!(private.fixture.repository.exists());
}

fn checkout_build_record(
    fixture: &CheckoutFixture,
    template: &crate::storage::model::Record,
    path: &Path,
    id: &str,
) -> crate::storage::model::Record {
    fs::create_dir_all(path).unwrap();
    fs::write(
        path.join("check-report.json"),
        "{\"disposable_build_fixture\":true}\n",
    )
    .unwrap();
    own_fixture_tree(fixture.fixture._temporary.path());
    let measured = crate::storage::fs::measure(path).unwrap();
    let mut record = template.clone();
    record.artifact.artifact_id = id.into();
    record.artifact.revision = 1;
    record.artifact.accounting_id = format!("allocation-{id}");
    record.artifact.name = id.into();
    record.artifact.kind = storage_api::Kind::BuildOutput;
    record.artifact.effect = storage_api::Effect::Rebuildable;
    record.artifact.allocated_bytes = Some(measured.bytes);
    record.artifact.removed_at_ms = None;
    record.locator = crate::storage::model::Locator::Directory {
        path: path.into(),
        device: measured.device,
        inode: measured.inode,
        root: fixture.fixture._temporary.path().into(),
        git_root: None,
    };
    record.resource_key = format!("fs:{}:{}", measured.device, measured.inode);
    record.ancestor_keys = path
        .ancestors()
        .skip(1)
        .filter_map(|parent| {
            crate::storage::fs::identity(parent)
                .ok()
                .map(|(dev, ino)| format!("fs:{dev}:{ino}"))
        })
        .collect();
    record.private_aliases = vec![path.into()];
    record.blockers.clear();
    fixture.fixture.database.call(|connection| {
        connection.execute("INSERT INTO storage_artifacts(artifact_id,repository_id,kind,revision,record_json,updated_at_ms) VALUES($1,'project-alpha','build_output',1,$2,1)",params![id,json(&record)?])?;
        Ok(())
    }).unwrap();
    record
}
fn bind_checkout_build(
    fixture: &CheckoutFixture,
    record: &crate::storage::model::Record,
    checkout: &str,
    ownership: &str,
) {
    let (device, inode) = match &record.locator {
        crate::storage::model::Locator::Directory { device, inode, .. } => (*device, *inode),
        _ => unreachable!(),
    };
    fixture.fixture.database.call(|connection| {
        connection.execute("INSERT INTO project_checkout_artifacts(association_id,project_id,worktree_id,artifact_id,revision,ownership,bound_device,bound_inode,active,provenance,created_at,updated_at,updated_by) VALUES($1,'product',$2,$3,1,$4,$5,$6,true,'explicit','t','t','fixture')",params![format!("association-{}",record.artifact.artifact_id),checkout,record.artifact.artifact_id,ownership,device.to_string(),inode.to_string()])?;
        connection.execute("INSERT INTO project_storage(project_id,repository_id,physical_repository_id,accounting_id,artifact_id,worktree_id,kind,allocated_bytes,measured_at) VALUES('product','project-alpha','project-alpha',$1,$2,$3,'build',$4,'t')",params![record.artifact.accounting_id,record.artifact.artifact_id,checkout,record.artifact.allocated_bytes])?;
        Ok(())
    }).unwrap();
}

#[test]
#[ignore = "requires a marked isolated root work area and explicit nonroot Git/PostgreSQL identities"]
fn exact_cleanup_retains_external_outputs_freezes_ownership_and_clears_only_removed_bytes() {
    let fixture = CheckoutFixture::new();
    let first = fixture.plan();
    let template = fixture.storage.stored_plan(&first.plan_id).unwrap().records[0].clone();
    let external_path = fixture.fixture._temporary.path().join("external-output");
    let inside_path = fixture.checkout.join(".devcoordinator/test/current");
    let external = checkout_build_record(&fixture, &template, &external_path, "external-build");
    let inside = checkout_build_record(&fixture, &template, &inside_path, "inside-build");
    bind_checkout_build(&fixture, &external, "feature-checkout", "exclusive");
    bind_checkout_build(&fixture, &inside, "feature-checkout", "exclusive");
    fixture.fixture.database.call(|connection| {
        connection.execute("INSERT INTO project_storage(project_id,repository_id,physical_repository_id,accounting_id,artifact_id,worktree_id,kind,allocated_bytes,measured_at) VALUES('product','project-alpha','project-alpha','fixture-source','','feature-checkout','source',999,'t')",[])?;
        connection.execute("UPDATE project_checkouts SET record_json=record_json||jsonb_build_object('source_bytes',999,'build_bytes',999,'local_bytes',1998,'locations',jsonb_build_array(jsonb_build_object('kind','artifact','artifact_id','external-build'),jsonb_build_object('kind','artifact','artifact_id','inside-build'))) WHERE worktree_id='feature-checkout'",[])?;
        Ok(())
    }).unwrap();
    let plan = fixture.plan();
    assert_eq!(plan.targets[0].retained_external_artifacts, 1);
    assert_eq!(
        plan.targets[0].retained_external_bytes,
        external.artifact.allocated_bytes
    );
    assert!(plan.blockers.is_empty(), "{:?}", plan);
    let approval = fixture.approve(&plan);
    let start = fixture.start(&plan, Some(approval.receipt_id));
    let queued = fixture
        .service
        .checkout_remove_start(start.clone(), &caller())
        .unwrap();
    let detach = fixture
        .service
        .detach_artifact(
            devcoordinator2_api::project::artifacts::Detach {
                project_id: "product".into(),
                worktree_id: "feature-checkout".into(),
                association_id: "association-external-build".into(),
                expected_revision: 1,
                idempotency_key: "detach-during-cleanup".into(),
            },
            &caller(),
        )
        .unwrap_err();
    assert_eq!(detach.code, ErrorCode::Busy, "{detach:?}");
    // Another checkout cannot claim a nested allocation after this exact tree
    // was accepted for cleanup, including through another ownership link.
    let adopt = fixture
        .service
        .adopt_artifact(
            devcoordinator2_api::project::artifacts::Adopt {
                project_id: "product".into(),
                worktree_id: "primary".into(),
                artifact_id: inside.artifact.artifact_id.clone(),
                expected_checkout_revision: 1,
                expected_artifact_revision: 1,
                expected_revision: 0,
                ownership: "shared".into(),
                idempotency_key: "adopt-during-cleanup".into(),
            },
            &caller(),
        )
        .unwrap_err();
    assert_eq!(adopt.code, ErrorCode::Busy, "{adopt:?}");
    assert!(matches!(
        fixture.storage.job(&queued.job.job_id).unwrap().state,
        storage_api::JobState::Queued
    ));
    let result = fixture.execute(start);
    assert!(result.targets[0].fully_removed, "{:?}", result.job);
    assert!(external_path.join("check-report.json").is_file());
    assert!(!inside_path.exists());
    let (active_external,active_inside,bytes,projection)=fixture.fixture.database.read(|connection|Ok((
        connection.query_row("SELECT active FROM project_checkout_artifacts WHERE artifact_id='external-build'",[],|row|row.get::<_,bool>(0))?,
        connection.query_row("SELECT active FROM project_checkout_artifacts WHERE artifact_id='inside-build'",[],|row|row.get::<_,bool>(0))?,
        connection.query_row("SELECT sum(allocated_bytes)::bigint FROM project_storage WHERE project_id='product' AND worktree_id='feature-checkout'",[],|row|row.get::<_,u64>(0))?,
        connection.query_row("SELECT record_json FROM project_checkouts WHERE worktree_id='feature-checkout'",[],|row|row.get::<_,String>(0))?
    ))).unwrap();
    assert!(active_external);
    assert!(!active_inside);
    assert_eq!(Some(bytes), external.artifact.allocated_bytes);
    let projection: serde_json::Value = serde_json::from_str(&projection).unwrap();
    assert_eq!(projection["source_bytes"], 0);
    assert_eq!(projection["build_bytes"], bytes);
    assert_eq!(projection["local_bytes"], bytes);
    assert_eq!(projection["locations"].as_array().unwrap().len(), 1);
    assert_eq!(projection["locations"][0]["artifact_id"], "external-build");
    assert!(
        fixture
            .storage
            .record("inside-build")
            .unwrap()
            .artifact
            .removed_at_ms
            .is_some()
    );
    assert!(
        fixture
            .storage
            .record("external-build")
            .unwrap()
            .artifact
            .removed_at_ms
            .is_none()
    );

    let shared = CheckoutFixture::new();
    let base = shared.plan();
    let template = shared.storage.stored_plan(&base.plan_id).unwrap().records[0].clone();
    let nested = checkout_build_record(
        &shared,
        &template,
        &shared.checkout.join(".devcoordinator/test/current"),
        "shared-nested",
    );
    bind_checkout_build(&shared, &nested, "primary", "shared");
    let blocked = shared.plan();
    assert!(
        blocked
            .blockers
            .iter()
            .any(|reason| reason == "shared_artifact_inside_checkout")
    );
    assert!(
        shared
            .service
            .checkout_remove_approve(
                removal::Approve {
                    project_id: "product".into(),
                    plan_id: blocked.plan_id,
                    fingerprint: blocked.fingerprint,
                    idempotency_key: "cannot-delete-shared-child".into(),
                    consent_decision_id: None
                },
                &administrator()
            )
            .is_err()
    );
    assert!(
        shared
            .checkout
            .join(".devcoordinator/test/current/check-report.json")
            .is_file()
    );
}

#[test]
#[ignore = "requires a marked isolated root work area and explicit nonroot Git/PostgreSQL identities"]
fn pure_refresh_keeps_loaded_cleanup_revisions_but_real_project_edits_invalidate_them() {
    let fixture = CheckoutFixture::new();
    own_fixture_tree(fixture.fixture._temporary.path());
    let registered = fixture
        .service
        .registry
        .register(&fixture.fixture.repository, caller().uid, caller().gid)
        .unwrap();
    fixture
        .service
        .registry
        .register(&fixture.checkout, caller().uid, caller().gid)
        .unwrap();
    let configuration =
        |project_id: Option<String>, expected_revision, name: &str| api::Configure {
            project_id,
            expected_revision,
            name: name.into(),
            slug: "cleanup-refresh-native".into(),
            roots: vec![api::RootInput {
                kind: "source".into(),
                label: "Source".into(),
                path: fixture
                    .fixture
                    ._temporary
                    .path()
                    .to_string_lossy()
                    .into_owned(),
            }],
            repositories: vec![api::RepositoryInput {
                repository_id: registered.repository_id.clone(),
                name: "Source".into(),
                github_url: None,
                default_branch: Some("main".into()),
                physical_repository_ids: vec![registered.repository_id.clone()],
            }],
        };
    // The older native cleanup fixture intentionally uses minimal registry
    // rows. Refresh requires complete persisted Checkout DTOs, so this journey
    // creates its product through the actual configuration boundary.
    let configured = fixture
        .service
        .configure(configuration(None, 0, "Native cleanup refresh"), &caller())
        .unwrap();
    let project = configured.project.project_id;
    let refresh = |key: &str| {
        let job = fixture
            .service
            .refresh(
                api::Refresh {
                    project_id: project.clone(),
                    idempotency_key: key.into(),
                    include_remote: false,
                },
                &caller(),
            )
            .unwrap();
        for _ in 0..256 {
            let mut budget = crate::maintenance::Budget::new(
                std::time::Duration::from_secs(5),
                256,
                1024 * 1024,
            );
            fixture.service.refresh_step(&mut budget);
            let state = fixture
                .service
                .job(
                    api::JobReference {
                        project_id: project.clone(),
                        job_id: job.job_id.clone(),
                    },
                    &caller(),
                )
                .unwrap();
            if matches!(state.state.as_str(), "succeeded" | "failed" | "partial") {
                assert_eq!(state.state, "succeeded", "{state:?}");
                return;
            }
        }
        panic!("real root project refresh did not reach a terminal result");
    };
    refresh("cleanup-initial-observation");
    let loaded = fixture
        .service
        .status(
            api::Reference {
                project_id: project.clone(),
            },
            &caller(),
        )
        .unwrap();
    let target = loaded
        .checkouts
        .iter()
        .find(|row| row.absolute_path.as_deref() == fixture.checkout.to_str())
        .expect("registered real feature checkout");
    let request = removal::Plan {
        project_id: project.clone(),
        idempotency_key: "review-after-pure-refresh".into(),
        expected_project_revision: u64::from(loaded.project.revision),
        targets: vec![removal::Target {
            worktree_id: target.worktree_id.clone(),
            expected_revision: u64::from(target.revision),
        }],
        include_absolute_paths: false,
    };
    refresh("cleanup-unchanged-observation");
    let current = fixture
        .service
        .status(
            api::Reference {
                project_id: project.clone(),
            },
            &caller(),
        )
        .unwrap();
    assert_eq!(current.project.revision, loaded.project.revision);
    assert_eq!(
        current
            .checkouts
            .iter()
            .find(|row| row.worktree_id == target.worktree_id)
            .unwrap()
            .revision,
        target.revision
    );
    let reviewed = fixture.review(request.clone(), &caller());
    let plan = reviewed
        .plan
        .expect("unchanged refresh must permit a complete real native review");
    assert!(plan.ready, "{plan:?}");
    fixture
        .service
        .configure(
            configuration(
                Some(project.clone()),
                loaded.project.revision,
                "Operator edited metadata",
            ),
            &caller(),
        )
        .unwrap();
    let mut stale = request;
    stale.idempotency_key = "review-after-real-metadata-edit".into();
    let rejected = fixture
        .service
        .checkout_remove_plan(stale, &caller())
        .unwrap_err();
    assert_eq!(rejected.code, ErrorCode::ConfigurationConflict);
    let rejected_start = fixture
        .service
        .checkout_remove_start(
            removal::Start {
                project_id: project,
                plan_id: plan.plan_id,
                fingerprint: plan.fingerprint,
                idempotency_key: "start-after-real-metadata-edit".into(),
                approval_receipt_id: None,
            },
            &caller(),
        )
        .unwrap_err();
    assert_eq!(rejected_start.code, ErrorCode::StorageConflict);
    assert_eq!(rejected_start.message, "project_changed");
    assert!(fixture.checkout.join("source.txt").is_file());
}
