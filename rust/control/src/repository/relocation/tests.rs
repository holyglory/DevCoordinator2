//! Real disposable Git/filesystem and PostgreSQL boundary coverage. Relocation
//! extends the existing repository registration acceptance path; no live source
//! or installed Coordinator is used. Run this suite as a non-root Git owner.
use super::*;
use devcoordinator2_api::ClientKind;
use std::process::Command;

fn caller() -> Caller {
    let uid = unsafe { libc::geteuid() };
    let gid = unsafe { libc::getegid() };
    assert_ne!(
        uid, 0,
        "relocation fixture Git must execute as a non-root owner"
    );
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
    assert_ne!(
        unsafe { libc::geteuid() },
        0,
        "repository fixture code never executes as root"
    );
    let output = Command::new("/usr/bin/git")
        .arg("-C")
        .arg(path)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=Relocation fixture",
            "-c",
            "user.email=fixture@example.invalid",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "Git fixture command failed");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
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
        let old = temporary.path().join("old");
        let new = temporary.path().join("new");
        std::fs::create_dir(&old).unwrap();
        git(&old, &["init", "-q", "-b", "main"]);
        std::fs::write(old.join("tracked.txt"), "committed\n").unwrap();
        std::fs::write(old.join(".gitignore"), "ignored/\n").unwrap();
        git(&old, &["add", "tracked.txt", ".gitignore"]);
        git(&old, &["commit", "-qm", "initial"]);
        std::fs::write(old.join("tracked.txt"), "valuable dirty work\n").unwrap();
        std::fs::write(old.join("untracked.txt"), "valuable untracked\n").unwrap();
        std::fs::create_dir(old.join("ignored")).unwrap();
        std::fs::write(old.join("ignored/cache.bin"), b"ignored data\0\x01").unwrap();
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
            permissions: devcoordinator2_api::project::Permissions { edit: true, cleanup: true, admin: true },
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
}

#[test]
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
            .register(&world.old, world.caller.uid, world.caller.gid)
            .is_err()
    );
    assert_eq!(
        world.prepare(),
        prepared,
        "prepare retry retains exact durable plan"
    );
    std::fs::rename(&world.old, &world.new).unwrap();
    let reference = World::reference(&prepared);
    let committed = world
        .registry
        .relocate_commit(reference.clone(), &world.caller)
        .unwrap();
    assert_eq!(committed.state, "committed");
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
fn real_git_interrupted_move_verification_rejects_changed_content_and_replacement_sources() {
    let world = World::new();
    let prepared = world.prepare();
    std::fs::rename(&world.old, &world.new).unwrap();
    let reference = World::reference(&prepared);
    std::fs::write(world.new.join("untracked.txt"), "changed after review").unwrap();
    assert!(
        world
            .registry
            .relocate_verify(reference.clone(), &world.caller)
            .is_err()
    );
    std::fs::write(world.new.join("untracked.txt"), "valuable untracked\n").unwrap();
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
    git(&world.old, &["init", "-q", "-b", "replacement"]);
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
    git(&world.new, &["init", "-q", "-b", "replacement"]);
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
    assert!(
        world
            .registry
            .relocate_prepare(world.request.clone(), &world.caller)
            .is_err(),
        "stopped current generation still requires explicit consumer rebind"
    );
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
fn real_git_registration_rejects_replacement_before_any_relocation() {
    let world = World::new();
    let preserved = world.old.with_extension("preserved");
    std::fs::rename(&world.old, &preserved).unwrap();
    std::fs::create_dir(&world.old).unwrap();
    git(&world.old, &["init", "-q", "-b", "replacement"]);
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
