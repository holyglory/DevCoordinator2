//! Real disposable Git/filesystem and PostgreSQL boundary coverage. Relocation
//! extends the existing repository registration acceptance path; no live source
//! or installed Coordinator is used. Run the driver as root for complete native
//! process inspection; Git and PostgreSQL use the explicit non-root fixture UID.
use super::*;
use devcoordinator2_api::ClientKind;
use std::process::Command;

fn caller() -> Caller {
    assert_eq!(
        unsafe { libc::geteuid() },
        0,
        "native relocation fixtures require the root driver"
    );
    let uid = std::env::var("DEVCOORDINATOR2_TEST_POSTGRES_UID")
        .expect("set the explicit non-root fixture UID")
        .parse::<u32>()
        .unwrap();
    let gid = std::env::var("DEVCOORDINATOR2_TEST_POSTGRES_GID")
        .expect("set the explicit non-root fixture GID")
        .parse::<u32>()
        .unwrap();
    assert_ne!(uid, 0, "repository fixture code never executes as root");
    Caller {
        via_edge: false,
        pid: std::process::id(),
        uid,
        gid,
        client_kind: ClientKind::Codex,
        model: None,
        effort: None,
        client_session: None,
        work: None,
        identity: None,
    }
}

fn git(path: &Path, args: &[&str]) -> String {
    let owner = caller();
    let output = Command::new("/usr/bin/setpriv")
        .args([
            format!("--reuid={}", owner.uid),
            format!("--regid={}", owner.gid),
        ])
        .args(["--init-groups", "--reset-env", "--", "/usr/bin/git"])
        .arg("-C")
        .arg(path)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "user.name=Relocation fixture",
            "-c",
            "user.email=fixture@example.invalid",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Git fixture command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn own(path: &Path) {
    let owner = caller();
    std::os::unix::fs::chown(path, Some(owner.uid), Some(owner.gid)).unwrap();
}

fn write_owned(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) {
    let path = path.as_ref();
    std::fs::write(path, contents).unwrap();
    own(path);
}

struct World {
    _temporary: tempfile::TempDir,
    database: Database,
    registry: Registry,
    old: PathBuf,
    new: PathBuf,
    registered: RegisteredRepository,
    request: api::Prepare,
    caller: Caller,
}
impl World {
    fn new() -> Self {
        let caller = caller();
        let temporary = tempfile::tempdir().unwrap();
        own(temporary.path());
        let old = temporary.path().join("old");
        let new = temporary.path().join("new");
        std::fs::create_dir(&old).unwrap();
        own(&old);
        git(&old, &["init", "-q", "-b", "main"]);
        write_owned(old.join("tracked.txt"), "committed\n");
        write_owned(old.join(".gitignore"), "ignored/\n");
        git(&old, &["add", "tracked.txt", ".gitignore"]);
        git(&old, &["commit", "-qm", "initial"]);
        write_owned(old.join("tracked.txt"), "valuable dirty work\n");
        write_owned(old.join("untracked.txt"), "valuable untracked\n");
        std::fs::create_dir(old.join("ignored")).unwrap();
        own(&old.join("ignored"));
        write_owned(old.join("ignored/cache.bin"), b"ignored data\0\x01");
        let database = Database::open(temporary.path().join("authority.fixture")).unwrap();
        let registry = Registry::new(database.clone());
        let registered = registry.register(&old, caller.uid, caller.gid).unwrap();
        let head = git(&old, &["rev-parse", "HEAD"]);
        let checkout = devcoordinator2_api::project::Checkout {
            worktree_id: registered.worktree_id.clone(),
            repository_id: registered.repository_id.clone(),
            relative_path: "old".into(),
            root_kind: "source".into(),
            absolute_path: None,
            role: "primary".into(),
            branch_id: None,
            branch_name: Some("main".into()),
            head: Some(head),
            dirty: Some(true),
            exists: Some(true),
            lifecycle: "active".into(),
            owner: None,
            intent: None,
            outcome: None,
            next_action: None,
            task_id: None,
            revision: 1,
            source_bytes: None,
            build_bytes: None,
            local_bytes: None,
            artifact_ids: vec![],
            protection_reasons: vec!["primary".into()],
            can_remove: false,
            locations: vec![],
            details_omitted: false,
            permissions: devcoordinator2_api::project::Permissions {
                edit: true,
                cleanup: true,
                admin: true,
            },
            registration_state: "registered".into(),
            immutable_generation: false,
            deployment_references: vec![],
        };
        database.transaction(|connection|{
            connection.execute("INSERT INTO projects(project_id,name,slug,created_at,updated_at,updated_by) VALUES('p-relocation','Relocation fixture','relocation-fixture','t','t','fixture')",[])?;
            connection.execute("INSERT INTO project_roots(project_id,root_id,kind,label,absolute_path) VALUES('p-relocation','root','source','Source',$1)",[temporary.path().to_string_lossy().into_owned()])?;
            connection.execute("INSERT INTO project_sources(project_id,repository_id,name,primary_checkout_id) VALUES('p-relocation',$1,'Fixture',$2)",(&registered.repository_id,&registered.worktree_id))?;
            connection.execute("INSERT INTO project_repository_memberships VALUES('p-relocation',$1,$1)",[&registered.repository_id])?;
            connection.execute("INSERT INTO project_checkouts(project_id,worktree_id,repository_id,physical_repository_id,absolute_path,role,protected_role,record_json,updated_at,updated_by) VALUES('p-relocation',$1,$2,$2,$3,'primary','primary',$4,'t','fixture')",(&registered.worktree_id,&registered.repository_id,old.to_string_lossy().into_owned(),serde_json::to_string(&checkout).unwrap()))?;
            Ok(())
        }).unwrap();
        let request = api::Prepare {
            project_id: "p-relocation".into(),
            repository_id: registered.repository_id.clone(),
            worktree_id: registered.worktree_id.clone(),
            old_path: old.to_string_lossy().into_owned(),
            new_path: new.to_string_lossy().into_owned(),
            expected_checkout_revision: 1,
            idempotency_key: "move-source-v1".into(),
        };
        Self {
            _temporary: temporary,
            database,
            registry,
            old,
            new,
            registered,
            request,
            caller,
        }
    }
    fn prepare(&self) -> api::Receipt {
        self.registry
            .relocate_prepare(self.request.clone(), &self.caller)
            .unwrap()
    }
    fn reference(receipt: &api::Receipt) -> api::Reference {
        api::Reference {
            relocation_id: receipt.relocation_id.clone(),
            expected_fingerprint: receipt.fingerprint.clone(),
        }
    }

    fn preview(&self, path: &Path) -> Result<(String, String), ProtocolError> {
        let info = resolve_worktree(path, Some((self.caller.uid, self.caller.gid)))?;
        self.registry.preview_source_identity(&info)
    }

    fn add_linked(&self, path: &Path) -> RegisteredRepository {
        git(
            &self.old,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "linked",
                path.to_str().unwrap(),
            ],
        );
        let error = self
            .registry
            .relocate_prepare(self.request.clone(), &self.caller)
            .unwrap_err();
        assert!(
            error.message.contains("Git worktree membership differs"),
            "an unregistered linked checkout cannot be omitted from native safety checks"
        );
        let linked = self
            .registry
            .register(path, self.caller.uid, self.caller.gid)
            .unwrap();
        self.database.transaction(|connection| {
            let record: String = connection.query_row("SELECT record_json FROM project_checkouts WHERE worktree_id=$1", [&self.registered.worktree_id], |row| row.get(0))?;
            let mut checkout: devcoordinator2_api::project::Checkout = serde_json::from_str(&record).unwrap();
            checkout.worktree_id = linked.worktree_id.clone();
            checkout.relative_path = path.file_name().unwrap().to_str().unwrap().into();
            checkout.role = "worktree".into();
            checkout.protection_reasons.clear();
            connection.execute("INSERT INTO project_checkouts(project_id,worktree_id,repository_id,physical_repository_id,absolute_path,role,record_json,updated_at,updated_by) VALUES('p-relocation',$1,$2,$2,$3,'worktree',$4,'t','fixture')", (&linked.worktree_id, &linked.repository_id, path.to_string_lossy().into_owned(), serde_json::to_string(&checkout).unwrap()))?;
            Ok(())
        }).unwrap();
        linked
    }
}

#[test]
#[ignore = "requires root driver with explicit non-root Git/PostgreSQL fixture UID/GID"]
fn real_git_canonical_move_repairs_linked_worktrees_without_changing_their_ids() {
    let world = World::new();
    let path = world.old.with_file_name("linked");
    let linked = world.add_linked(&path);
    write_owned(path.join("tracked.txt"), "linked dirty content\n");
    let mut process = Command::new("/usr/bin/setpriv")
        .args([
            format!("--reuid={}", world.caller.uid),
            format!("--regid={}", world.caller.gid),
        ])
        .args(["--init-groups", "--reset-env", "--", "/usr/bin/sleep", "60"])
        .current_dir(&path)
        .spawn()
        .unwrap();
    let refused = world
        .registry
        .relocate_prepare(world.request.clone(), &world.caller);
    process.kill().unwrap();
    process.wait().unwrap();
    assert_eq!(
        refused.unwrap_err().message,
        "active_process",
        "a process in a linked checkout still uses the shared Git source"
    );
    let prepared = world.prepare();
    std::fs::rename(&world.old, &world.new).unwrap();
    git(&world.new, &["worktree", "repair"]);
    world
        .registry
        .relocate_commit(World::reference(&prepared), &world.caller)
        .unwrap();
    let current = world
        .registry
        .register(&path, world.caller.uid, world.caller.gid)
        .unwrap();
    assert_eq!(
        (current.repository_id, current.worktree_id),
        (linked.repository_id, linked.worktree_id)
    );
    assert_eq!(
        std::fs::read_to_string(path.join("tracked.txt")).unwrap(),
        "linked dirty content\n"
    );
    assert_eq!(
        git(&path, &["rev-parse", "--show-toplevel"]),
        path.to_str().unwrap()
    );
}

#[test]
#[ignore = "requires root driver with explicit non-root Git/PostgreSQL fixture UID/GID"]
fn real_git_linked_move_preserves_the_canonical_checkout_and_both_identities() {
    let world = World::new();
    let old = world.old.with_file_name("linked");
    let new = world.old.with_file_name("linked-moved");
    let linked = world.add_linked(&old);
    write_owned(old.join("tracked.txt"), "linked dirty content\n");
    let mut request = world.request.clone();
    request.worktree_id = linked.worktree_id.clone();
    request.old_path = old.to_string_lossy().into_owned();
    request.new_path = new.to_string_lossy().into_owned();
    let prepared = world
        .registry
        .relocate_prepare(request, &world.caller)
        .unwrap();
    git(
        &world.old,
        &[
            "worktree",
            "move",
            old.to_str().unwrap(),
            new.to_str().unwrap(),
        ],
    );
    world
        .registry
        .relocate_commit(World::reference(&prepared), &world.caller)
        .unwrap();
    let current = world
        .registry
        .register(&new, world.caller.uid, world.caller.gid)
        .unwrap();
    assert_eq!(
        (current.repository_id, current.worktree_id),
        (linked.repository_id, linked.worktree_id)
    );
    let canonical = world
        .registry
        .register(&world.old, world.caller.uid, world.caller.gid)
        .unwrap();
    assert_eq!(canonical.worktree_id, world.registered.worktree_id);
    assert_eq!(
        std::fs::read_to_string(new.join("tracked.txt")).unwrap(),
        "linked dirty content\n"
    );
    assert_eq!(
        std::fs::read_to_string(world.old.join("tracked.txt")).unwrap(),
        "valuable dirty work\n"
    );
}

#[test]
#[ignore = "requires root driver with explicit non-root Git/PostgreSQL fixture UID/GID"]
fn real_git_cross_filesystem_prepare_preserves_source_and_creates_no_plan() {
    let world = World::new();
    let destination = tempfile::tempdir_in("/dev/shm").unwrap();
    own(destination.path());
    assert_ne!(
        std::fs::metadata(&world.old).unwrap().dev(),
        std::fs::metadata(destination.path()).unwrap().dev(),
        "fixture must use two real filesystems"
    );
    let mut request = world.request.clone();
    request.new_path = destination
        .path()
        .join("copy")
        .to_string_lossy()
        .into_owned();
    let error = world
        .registry
        .relocate_prepare(request, &world.caller)
        .unwrap_err();
    assert!(
        error
            .message
            .contains("cross-filesystem relocation is unavailable")
    );
    assert_eq!(
        std::fs::read_to_string(world.old.join("tracked.txt")).unwrap(),
        "valuable dirty work\n"
    );
    assert!(!destination.path().join("copy").exists());
    world
        .database
        .read(|connection| {
            assert_eq!(
                connection
                    .query_row("SELECT count(*) FROM repository_relocations", [], |row| row
                        .get::<_, i64>(0))?,
                0
            );
            assert_eq!(
                connection.query_row(
                    "SELECT revision FROM project_checkouts WHERE worktree_id=$1",
                    [&world.registered.worktree_id],
                    |row| row.get::<_, i64>(0)
                )?,
                1
            );
            Ok(())
        })
        .unwrap();
}

#[test]
#[ignore = "requires root driver with explicit non-root Git/PostgreSQL fixture UID/GID"]
fn real_git_old_copy_recovery_refuses_mutated_target_and_retains_remaining_source() {
    let world = World::new();
    let prepared = world.prepare();
    let copied = Command::new("/usr/bin/cp")
        .args(["-a", "--"])
        .arg(&world.old)
        .arg(&world.new)
        .status()
        .unwrap();
    assert!(copied.success());
    world.database.transaction(|connection| {
        connection.execute("UPDATE repository_relocations SET native_json=jsonb_set(jsonb_set(native_json,'{copied}','true'::jsonb),'{removal_started_at}','\"interrupted\"'::jsonb) WHERE relocation_id=$1", [&prepared.relocation_id])?;
        Ok(())
    }).unwrap();
    // Deliberately simulate a past interrupted recursive removal inside this
    // disposable fixture, then change the only complete copy.
    std::fs::remove_file(world.old.join("untracked.txt")).unwrap();
    write_owned(world.new.join("tracked.txt"), "changed surviving target\n");
    let reference = World::reference(&prepared);
    for error in [
        world
            .registry
            .relocate_verify(reference.clone(), &world.caller)
            .unwrap_err(),
        world
            .registry
            .relocate_commit(reference.clone(), &world.caller)
            .unwrap_err(),
    ] {
        assert!(
            error
                .message
                .contains("cross-filesystem relocation is unavailable")
        );
    }
    assert_eq!(
        std::fs::read_to_string(world.old.join("tracked.txt")).unwrap(),
        "valuable dirty work\n"
    );
    assert_eq!(
        std::fs::read_to_string(world.new.join("tracked.txt")).unwrap(),
        "changed surviving target\n"
    );
    world
        .database
        .read(|connection| {
            assert_eq!(
                connection.query_row(
                    "SELECT worktree_path FROM worktrees WHERE worktree_id=$1",
                    [&world.registered.worktree_id],
                    |row| row.get::<_, String>(0)
                )?,
                world.old.to_string_lossy()
            );
            assert_eq!(
                load_relocation(connection, &prepared.relocation_id)?
                    .receipt
                    .state,
                "prepared"
            );
            Ok(())
        })
        .unwrap();
}

#[test]
#[ignore = "requires root driver with explicit non-root Git/PostgreSQL fixture UID/GID"]
fn real_git_prepare_refuses_a_process_in_the_source_then_recovers() {
    let world = World::new();
    let owner = &world.caller;
    let mut process = Command::new("/usr/bin/setpriv")
        .args([
            format!("--reuid={}", owner.uid),
            format!("--regid={}", owner.gid),
        ])
        .args(["--init-groups", "--reset-env", "--", "/usr/bin/sleep", "60"])
        .current_dir(&world.old)
        .spawn()
        .unwrap();
    let refused = world
        .registry
        .relocate_prepare(world.request.clone(), &world.caller);
    process.kill().unwrap();
    process.wait().unwrap();
    assert_eq!(refused.unwrap_err().message, "active_process");
    let prepared = world.prepare();
    assert_eq!(prepared.state, "prepared");
    assert!(
        world.preview(&world.old).is_err(),
        "a pending move never becomes a new desired source"
    );
    world
        .registry
        .relocate_cancel(World::reference(&prepared), &world.caller)
        .unwrap();
}

#[test]
#[ignore = "requires root driver with explicit non-root Git/PostgreSQL fixture UID/GID"]
fn real_native_inventory_never_allows_an_incomplete_nonroot_scan() {
    if unsafe { libc::geteuid() } != 0 {
        let path = tempfile::tempdir().unwrap();
        let error =
            crate::storage::ensure_relocation_inactive(&[path.path().to_owned()]).unwrap_err();
        assert_eq!(error.code, ErrorCode::StorageUnavailable);
        assert_eq!(error.message, "process_inventory_incomplete");
        return;
    }
    let owner = caller();
    let result = Command::new("/usr/bin/setpriv")
        .args([format!("--reuid={}", owner.uid), format!("--regid={}", owner.gid)])
        .args(["--init-groups", "--reset-env", "--"])
        .arg(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "repository::relocation::tests::real_native_inventory_never_allows_an_incomplete_nonroot_scan"])
        .output().unwrap();
    assert!(
        result.status.success(),
        "nonroot native safety regression failed: {}",
        String::from_utf8_lossy(&result.stdout)
    );
}

#[test]
#[ignore = "requires root driver with explicit non-root Git/PostgreSQL fixture UID/GID"]
fn real_git_dirty_primary_move_commits_with_the_same_registry_and_history_identity() {
    let world = World::new();
    let old_status = git(
        &world.old,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    );
    let old_refs = git(&world.old, &["show-ref", "--head"]);
    let prepared = world.prepare();
    assert_eq!(prepared.state, "prepared");
    assert!(
        world
            .registry
            .ensure_execution_available(&world.registered.repository_id)
            .is_err()
    );
    assert!(
        world
            .registry
            .register(&world.old, world.caller.uid, world.caller.gid)
            .is_err()
    );
    assert_eq!(
        world.prepare(),
        prepared,
        "prepare retry retains exact durable plan"
    );
    std::fs::rename(&world.old, &world.new).unwrap();
    assert!(
        world.preview(&world.new).is_err(),
        "a reserved destination cannot obtain a new identity"
    );
    let reference = World::reference(&prepared);
    let committed = world
        .registry
        .relocate_commit(reference.clone(), &world.caller)
        .unwrap();
    assert_eq!(committed.state, "committed");
    world
        .registry
        .ensure_execution_available(&world.registered.repository_id)
        .unwrap();
    assert_eq!(
        world
            .registry
            .relocate_commit(reference, &world.caller)
            .unwrap(),
        committed
    );
    let current = world
        .registry
        .register(&world.new, world.caller.uid, world.caller.gid)
        .unwrap();
    assert_eq!(current.repository_id, world.registered.repository_id);
    assert_eq!(current.worktree_id, world.registered.worktree_id);
    assert!(!current.registered);
    assert_eq!(
        world.preview(&world.new).unwrap(),
        (current.repository_id.clone(), current.worktree_id.clone())
    );
    assert_eq!(
        git(
            &world.new,
            &["status", "--porcelain=v1", "--untracked-files=all"]
        ),
        old_status
    );
    assert_eq!(git(&world.new, &["show-ref", "--head"]), old_refs);
    assert_eq!(
        std::fs::read(world.new.join("ignored/cache.bin")).unwrap(),
        b"ignored data\0\x01"
    );
    let status = world
        .registry
        .repository_status(&world.old, Some((world.caller.uid, world.caller.gid)))
        .unwrap();
    assert_eq!(status.worktree_id, world.registered.worktree_id);
    assert_eq!(status.root_path, world.new.to_string_lossy());
    assert!(
        world
            .registry
            .registered_execution_source(&world.old)
            .is_err(),
        "historical path never grants execution"
    );
    let origin = crate::deployment_state::GenerationSourceOrigin {
        worktree_id: current.worktree_id.clone(),
        path: world.old.clone(),
    };
    assert_eq!(
        crate::deployment_origin::resolve(
            &world.registry,
            &origin,
            &current.repository_id,
            world.caller.uid,
            world.caller.gid
        )
        .unwrap(),
        world.new
    );
}

#[test]
#[ignore = "requires root driver with explicit non-root Git/PostgreSQL fixture UID/GID"]
fn real_git_interrupted_move_verification_rejects_changed_content_and_replacement_sources() {
    let world = World::new();
    let prepared = world.prepare();
    std::fs::rename(&world.old, &world.new).unwrap();
    let reference = World::reference(&prepared);
    write_owned(world.new.join("untracked.txt"), "changed after review");
    assert!(
        world
            .registry
            .relocate_verify(reference.clone(), &world.caller)
            .is_err()
    );
    write_owned(world.new.join("untracked.txt"), "valuable untracked\n");
    assert_eq!(
        world
            .registry
            .relocate_verify(reference.clone(), &world.caller)
            .unwrap()
            .state,
        "verified"
    );
    world
        .registry
        .relocate_commit(reference, &world.caller)
        .unwrap();
    std::fs::create_dir(&world.old).unwrap();
    own(&world.old);
    git(&world.old, &["init", "-q", "-b", "replacement"]);
    assert!(
        world.preview(&world.old).is_err(),
        "a committed historical alias cannot preview a replacement source"
    );
    assert!(
        world
            .registry
            .register(&world.old, world.caller.uid, world.caller.gid)
            .is_err()
    );
    assert!(
        world
            .registry
            .registered_execution_source(&world.old)
            .is_err()
    );
    let preserved = world.new.with_extension("preserved");
    std::fs::rename(&world.new, &preserved).unwrap();
    std::fs::create_dir(&world.new).unwrap();
    own(&world.new);
    git(&world.new, &["init", "-q", "-b", "replacement"]);
    assert!(
        world.preview(&world.new).is_err(),
        "the current location still requires its bound physical source"
    );
    assert!(
        world
            .registry
            .register(&world.new, world.caller.uid, world.caller.gid)
            .is_err()
    );
    assert!(
        world
            .registry
            .registered_execution_source(&world.new)
            .is_err()
    );
}

#[test]
#[ignore = "requires root driver with explicit non-root Git/PostgreSQL fixture UID/GID"]
fn real_git_prepare_refuses_active_deployment_and_public_caller_then_can_cancel() {
    let world = World::new();
    let mut public = world.caller.clone();
    public.identity = Some("admin@example.invalid".into());
    public.via_edge = true;
    assert_eq!(
        world
            .registry
            .relocate_prepare(world.request.clone(), &public)
            .unwrap_err()
            .code,
        ErrorCode::PermissionDenied
    );
    world.database.transaction(|connection|{
        connection.execute("INSERT INTO deployments(deployment_id,repository_id,worktree_id,name,source,spec_fingerprint,spec_json,state,current_generation,created_at,created_by_uid,client,updated_at) VALUES('d-relocation',$1,$2,'fixture','checkout','f','{}','stopped',1,'t',1,'fixture','t')",(&world.registered.repository_id,&world.registered.worktree_id))?;
        Ok(())
    }).unwrap();
    for (state, current, previous) in [
        ("running", Some(1_u32), None),
        ("stopped", Some(1_u32), None),
        ("stopped", None, Some(1_u32)),
        ("stopped", None, None),
        ("failed", None, None),
    ] {
        world.database.transaction(|connection| {
            connection.execute("UPDATE deployments SET state=$1,current_generation=$2,previous_generation=$3 WHERE deployment_id='d-relocation'", (state, current, previous))?;
            Ok(())
        }).unwrap();
        let error = world
            .registry
            .relocate_prepare(world.request.clone(), &world.caller)
            .unwrap_err();
        assert!(
            error
                .message
                .contains("deployment-bound relocation is unavailable")
        );
        assert!(world.old.is_dir() && !world.new.exists());
    }
    world
        .database
        .transaction(|connection| {
            connection.execute(
                "DELETE FROM deployments WHERE deployment_id='d-relocation'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let prepared = world.prepare();
    let cancelled = world
        .registry
        .relocate_cancel(World::reference(&prepared), &world.caller)
        .unwrap();
    assert_eq!(cancelled.state, "cancelled");
    world
        .registry
        .ensure_execution_available(&world.registered.repository_id)
        .unwrap();
    assert_eq!(
        world
            .registry
            .register(&world.old, world.caller.uid, world.caller.gid)
            .unwrap()
            .worktree_id,
        world.registered.worktree_id
    );
}

#[test]
#[ignore = "requires root driver with explicit non-root Git/PostgreSQL fixture UID/GID"]
fn real_git_registration_rejects_replacement_before_any_relocation() {
    let world = World::new();
    let preserved = world.old.with_extension("preserved");
    std::fs::rename(&world.old, &preserved).unwrap();
    assert!(
        world.preview(&preserved).is_err(),
        "an uncommitted move cannot allocate a replacement identity"
    );
    std::fs::create_dir(&world.old).unwrap();
    own(&world.old);
    git(&world.old, &["init", "-q", "-b", "replacement"]);
    assert!(
        world.preview(&world.old).is_err(),
        "a replaced physical source cannot inherit its old identity"
    );
    assert!(
        world
            .registry
            .register(&world.old, world.caller.uid, world.caller.gid)
            .is_err()
    );
    assert!(
        world
            .registry
            .repository_status(&world.old, Some((world.caller.uid, world.caller.gid)))
            .is_err()
    );
    let origin = crate::deployment_state::GenerationSourceOrigin {
        worktree_id: world.registered.worktree_id.clone(),
        path: world.old.clone(),
    };
    assert!(
        crate::deployment_origin::resolve(
            &world.registry,
            &origin,
            &world.registered.repository_id,
            world.caller.uid,
            world.caller.gid
        )
        .is_err()
    );
}
