//! Acceptance regressions extend the existing real PostgreSQL fixture with
//! actual Git clones/worktrees. Existing control-plane/registry suites do not
//! express explicit multi-source products or clone-scoped cached projections.
use super::*;
use crate::{automation_test_support::Fixture, maintenance::Budget};
use devcoordinator2_api::ClientKind;
use std::{fs, process::Command, time::Duration};

fn caller() -> Caller {
    Caller {
        via_edge: false,
        pid: std::process::id(),
        uid: unsafe { libc::geteuid() },
        gid: unsafe { libc::getegid() },
        client_kind: ClientKind::Codex,
        model: None,
        effort: None,
        client_session: None,
        work: None,
        identity: None,
    }
}
fn git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("/usr/bin/git")
        .arg("-C")
        .arg(path)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=Project fixture",
            "-c",
            "user.email=project@example.invalid",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Git fixture failed: {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}
fn initialize(path: &Path) {
    fs::create_dir_all(path).unwrap();
    git(path, &["init", "-q", "-b", "main"]);
    fs::write(path.join("source.txt"), "source\n").unwrap();
    git(path, &["add", "source.txt"]);
    git(path, &["commit", "-qm", "initial"]);
}
fn seed(f: &Fixture, id: &str, path: &Path, wid: &str) {
    f.database.transaction(|c|{c.execute("INSERT INTO repositories(repository_id,root_path,display_name,registered_at,registered_by_uid,last_seen_at) VALUES($1,$2,$1,'t',$3,'t')",params![id,path.to_string_lossy().into_owned(),caller().uid])?;c.execute("INSERT INTO worktrees(worktree_id,repository_id,worktree_path,registered_at,last_seen_at) VALUES($1,$2,$3,'t','t')",params![wid,id,path.to_string_lossy().into_owned()])?;Ok(())}).unwrap();
}
fn source(id: &str, physical: &[&str]) -> api::RepositoryInput {
    api::RepositoryInput {
        repository_id: id.into(),
        name: id.into(),
        github_url: None,
        default_branch: Some("main".into()),
        physical_repository_ids: physical.iter().map(|s| s.to_string()).collect(),
    }
}
fn configure(
    service: &ProjectService,
    f: &Fixture,
    name: &str,
    sources: Vec<api::RepositoryInput>,
) -> api::Status {
    service
        .configure(
            api::Configure {
                project_id: None,
                expected_revision: 0,
                name: name.into(),
                slug: name.to_ascii_lowercase(),
                roots: vec![api::RootInput {
                    kind: "source".into(),
                    label: "Sources".into(),
                    path: f._temporary.path().to_string_lossy().into_owned(),
                }],
                repositories: sources,
            },
            &caller(),
        )
        .unwrap()
}
fn drain(service: &ProjectService, project: &str, key: &str) {
    let job = service
        .refresh(
            api::Refresh {
                project_id: project.into(),
                idempotency_key: key.into(),
                include_remote: false,
            },
            &caller(),
        )
        .unwrap();
    for _ in 0..80 {
        let mut budget = Budget::new(Duration::from_secs(10), 256, 1024 * 1024);
        let step = service.refresh_step(&mut budget);
        assert!(
            !matches!(step.status, crate::maintenance::JobStatus::Retry { .. }),
            "refresh unexpectedly retried"
        );
        let result = service
            .job(
                api::JobReference {
                    project_id: project.into(),
                    job_id: job.job_id.clone(),
                },
                &caller(),
            )
            .unwrap();
        if matches!(result.state.as_str(), "succeeded" | "failed" | "partial") {
            assert_eq!(result.state, "succeeded", "{:?}", result);
            return;
        }
    }
    panic!("refresh did not finish its bounded fixture inventory");
}
fn reference(id: &str) -> api::Reference {
    api::Reference {
        project_id: id.into(),
    }
}
fn page(id: &str, limit: u16) -> api::Page {
    api::Page {
        project_id: id.into(),
        cursor: None,
        limit,
        query: None,
        lifecycle: None,
    }
}

#[test]
fn persistent_products_group_explicit_sources_and_clones_and_preserve_missing_intent() {
    let fixture = Fixture::new();
    initialize(&fixture.repository);
    fixture
        .database
        .transaction(|c| {
            c.execute(
                "UPDATE repositories SET registered_by_uid=$1 WHERE repository_id='project-alpha'",
                [caller().uid],
            )?;
            Ok(())
        })
        .unwrap();
    let clone = fixture._temporary.path().join("clone");
    git(
        fixture._temporary.path(),
        &[
            "clone",
            "-q",
            fixture.repository.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    seed(&fixture, "physical-clone", &clone, "clone-checkout");
    let second = fixture._temporary.path().join("second");
    initialize(&second);
    seed(&fixture, "source-second", &second, "second-checkout");
    let unrelated = fixture._temporary.path().join("unrelated");
    initialize(&unrelated);
    seed(
        &fixture,
        "source-unrelated",
        &unrelated,
        "unrelated-checkout",
    );
    let linked = fixture._temporary.path().join("linked with spaces");
    git(
        &fixture.repository,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            linked.to_str().unwrap(),
        ],
    );
    fs::write(linked.join("untracked.txt"), "untracked\n").unwrap();
    git(
        &fixture.repository,
        &[
            "worktree",
            "lock",
            linked.to_str().unwrap(),
            "--reason",
            "fixture lock",
        ],
    );
    let service = ProjectService::new(
        fixture.database.clone(),
        Registry::new(fixture.database.clone()),
    );
    let product = configure(
        &service,
        &fixture,
        "Product",
        vec![
            source("project-alpha", &["project-alpha", "physical-clone"]),
            source("source-second", &["source-second"]),
        ],
    );
    let other = configure(
        &service,
        &fixture,
        "Other",
        vec![source("source-unrelated", &["source-unrelated"])],
    );
    assert_eq!(product.project.repository_count, 2);
    assert_eq!(product.project.checkout_count, 3);
    assert_eq!(product.project.local_bytes, None);
    drain(&service, &product.project.project_id, "first");
    let status = service
        .status(reference(&product.project.project_id), &caller())
        .unwrap();
    assert_eq!(status.checkouts.len(), 4);
    assert_eq!(status.repositories.len(), 2);
    let feature = status
        .branches
        .iter()
        .find(|b| b.branch_name == "feature")
        .unwrap();
    let linked_row = status
        .checkouts
        .iter()
        .find(|c| c.branch_name.as_deref() == Some("feature"))
        .unwrap();
    assert_eq!(linked_row.dirty, Some(true));
    assert!(!linked_row.can_remove);
    assert!(
        linked_row
            .protection_reasons
            .iter()
            .any(|r| r.contains("locked"))
    );
    let patch:api::BranchUpdate=serde_json::from_value(serde_json::json!({"project_id":status.project.project_id,"branch_id":feature.branch_id,"expected_revision":feature.revision,"intent":"Finish user workflow","owner":"Agent","task_id":"spec","lifecycle":"paused"})).unwrap();
    let edited = service.update_branch(patch.clone(), &caller()).unwrap();
    assert_eq!(edited.intent.as_deref(), Some("Finish user workflow"));
    assert!(service.update_branch(patch, &caller()).is_err());
    let clear:api::BranchUpdate=serde_json::from_value(serde_json::json!({"project_id":status.project.project_id,"branch_id":edited.branch_id,"expected_revision":edited.revision,"owner":null})).unwrap();
    let edited = service.update_branch(clear, &caller()).unwrap();
    assert_eq!(edited.owner, None);
    assert!(edited.intent.is_some());
    let invalid:api::BranchUpdate=serde_json::from_value(serde_json::json!({"project_id":status.project.project_id,"branch_id":edited.branch_id,"expected_revision":edited.revision,"lifecycle":"retired"})).unwrap();
    assert!(service.update_branch(invalid, &caller()).is_err());
    let wid = linked_row.worktree_id.clone();
    fs::remove_dir_all(&linked).unwrap();
    drain(&service, &status.project.project_id, "missing");
    let recreated = ProjectService::new(
        fixture.database.clone(),
        Registry::new(fixture.database.clone()),
    );
    let status = recreated
        .status(reference(&status.project.project_id), &caller())
        .unwrap();
    assert_eq!(
        status
            .branches
            .iter()
            .find(|b| b.branch_id == edited.branch_id)
            .unwrap()
            .intent
            .as_deref(),
        Some("Finish user workflow")
    );
    assert_eq!(
        status
            .checkouts
            .iter()
            .find(|c| c.worktree_id == wid)
            .unwrap()
            .exists,
        Some(false)
    );
    assert!(
        service
            .status(reference(&other.project.project_id), &caller())
            .unwrap()
            .branches
            .is_empty()
    );
    let first = service
        .branches(page(&status.project.project_id, 1), &caller())
        .unwrap();
    assert!(first.next_cursor.is_some());
    let mut next = page(&status.project.project_id, 1);
    next.cursor = first.next_cursor.clone();
    assert_ne!(
        first.branches[0].branch_id,
        service.branches(next.clone(), &caller()).unwrap().branches[0].branch_id
    );
    next.project_id = other.project.project_id.clone();
    assert!(service.branches(next, &caller()).is_err());
    let context = service
        .context(
            api::Context {
                repository_ids: vec!["physical-clone".into()],
            },
            &caller(),
        )
        .unwrap();
    assert_eq!(context.memberships[0].source_repository_id, "project-alpha");
    // Removing Git metadata proves the normal read paths use only persisted data.
    fs::rename(
        fixture.repository.join(".git"),
        fixture.repository.join(".git.offline"),
    )
    .unwrap();
    assert_eq!(
        service
            .status(reference(&status.project.project_id), &caller())
            .unwrap()
            .project
            .project_id,
        status.project.project_id
    );
    assert_eq!(
        service
            .list(api::List::default(), &caller())
            .unwrap()
            .projects
            .len(),
        2
    );
}

#[test]
fn commit_pages_keep_root_parents_and_continue_beyond_two_hundred() {
    let fixture = Fixture::new();
    initialize(&fixture.repository);
    fixture
        .database
        .transaction(|c| {
            c.execute(
                "UPDATE repositories SET registered_by_uid=$1",
                [caller().uid],
            )?;
            Ok(())
        })
        .unwrap();
    for index in 0..204 {
        git(
            &fixture.repository,
            &["commit", "--allow-empty", "-qm", &format!("commit {index}")],
        );
    }
    let service = ProjectService::new(
        fixture.database.clone(),
        Registry::new(fixture.database.clone()),
    );
    let status = configure(
        &service,
        &fixture,
        "History",
        vec![source("project-alpha", &["project-alpha"])],
    );
    drain(&service, &status.project.project_id, "inventory");
    let status = service
        .status(reference(&status.project.project_id), &caller())
        .unwrap();
    let branch = &status.branches[0];
    let request = api::BranchReference {
        project_id: status.project.project_id.clone(),
        branch_id: branch.branch_id.clone(),
        cursor: None,
        limit: 100,
    };
    assert!(
        service
            .commits(request.clone(), &caller())
            .unwrap()
            .refresh_required
    );
    let job = service
        .refresh_commits_page(
            api::CommitRefresh {
                project_id: status.project.project_id.clone(),
                branch_id: branch.branch_id.clone(),
                idempotency_key: "history-first".into(),
                cursor: None,
            },
            &caller(),
        )
        .unwrap();
    service.refresh_step(&mut Budget::new(Duration::from_secs(10), 256, 1024 * 1024));
    assert_eq!(
        service
            .job(
                api::JobReference {
                    project_id: status.project.project_id.clone(),
                    job_id: job.job_id
                },
                &caller()
            )
            .unwrap()
            .state,
        "succeeded"
    );
    let first = service.commits(request.clone(), &caller()).unwrap();
    assert_eq!(first.commits.len(), 100);
    assert!(!first.refresh_required);
    assert_eq!(first.commits[0].parents.len(), 1);
    let mut second = request.clone();
    second.cursor = first.next_cursor;
    let second = service.commits(second, &caller()).unwrap();
    assert_eq!(second.commits.len(), 100);
    let mut third = request;
    third.cursor = second.next_cursor;
    assert!(
        service
            .commits(third.clone(), &caller())
            .unwrap()
            .refresh_required
    );
    service
        .refresh_commits_page(
            api::CommitRefresh {
                project_id: third.project_id.clone(),
                branch_id: third.branch_id.clone(),
                idempotency_key: "history-next".into(),
                cursor: third.cursor.clone(),
            },
            &caller(),
        )
        .unwrap();
    service.refresh_step(&mut Budget::new(Duration::from_secs(10), 256, 1024 * 1024));
    let end = service.commits(third, &caller()).unwrap();
    assert_eq!(end.commits.len(), 5);
    assert!(end.next_cursor.is_none());
    assert!(end.commits.last().unwrap().parents.is_empty());
}

#[test]
fn viewer_operator_admin_and_clone_isolation_apply_to_every_cached_projection() {
    let mut fixture = Fixture::new();
    initialize(&fixture.repository);
    fixture.config.edge_uid = Some(caller().uid);
    fixture
        .database
        .transaction(|c| {
            c.execute(
                "UPDATE repositories SET registered_by_uid=$1",
                [caller().uid],
            )?;
            Ok(())
        })
        .unwrap();
    let clone = fixture._temporary.path().join("private-clone");
    git(
        fixture._temporary.path(),
        &[
            "clone",
            "-q",
            fixture.repository.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    git(&clone, &["branch", "private-only"]);
    seed(&fixture, "private-clone", &clone, "private-checkout");
    fixture.database.transaction(|c|{c.execute_batch("INSERT INTO users(user_id,email,administrator,created_at,created_by) VALUES('viewer','viewer@example.invalid',0,'t','fixture'),('operator','operator@example.invalid',0,'t','fixture'),('admin','admin@example.invalid',1,'t','fixture');
      INSERT INTO observed_deployments(observed_deployment_id,repository_id,name,native_project,state,health,source,evidence_json,observed_at,imported_at) VALUES('public-deployment','project-alpha','Public','public','running','healthy','compose','{}','t','t'),('private-deployment','private-clone','Private','private','running','healthy','compose','{}','t','t');
      INSERT INTO grants(user_id,deployment_id,role,granted_at,granted_by) VALUES('viewer','public-deployment','viewer','t','fixture'),('operator','public-deployment','operator','t','fixture');")?;Ok(())}).unwrap();
    let publisher: Arc<dyn crate::access::RoutePublisher> =
        Arc::new(|_: &crate::access::RouteAccessSection| Ok(()));
    let access = Access::new(&fixture.config, fixture.database.clone(), publisher).unwrap();
    let service = ProjectService::new(
        fixture.database.clone(),
        Registry::new(fixture.database.clone()),
    )
    .with_access(access.clone());
    let product = configure(
        &service,
        &fixture,
        "Scoped",
        vec![source("project-alpha", &["project-alpha", "private-clone"])],
    );
    drain(&service, &product.project.project_id, "scope-snapshot");
    let public = |who: &str| Caller {
        via_edge: true,
        client_kind: ClientKind::Edge,
        identity: Some(format!("{who}@example.invalid")),
        ..caller()
    };
    let viewer = public("viewer");
    let operator = public("operator");
    let admin = public("admin");
    let status = service
        .status(reference(&product.project.project_id), &viewer)
        .unwrap();
    assert_eq!(status.checkouts.len(), 1);
    assert_eq!(status.project.checkout_count, 1);
    assert_eq!(status.project.branch_count, 1);
    assert!(
        status
            .branches
            .iter()
            .all(|b| b.branch_name != "private-only")
    );
    assert!(status.checkouts.iter().all(|c| c.absolute_path.is_none()));
    assert!(!status.permissions.edit);
    assert!(!status.checkouts[0].permissions.edit);
    let operator_status = service
        .status(reference(&product.project.project_id), &operator)
        .unwrap();
    assert!(!operator_status.repositories[0].permissions.edit);
    assert!(operator_status.checkouts[0].permissions.edit);
    assert!(operator_status.checkouts[0].permissions.cleanup);
    let serialized = serde_json::to_string(&status).unwrap();
    assert!(!serialized.contains(fixture._temporary.path().to_str().unwrap()));
    assert!(!serialized.contains("private-checkout"));
    assert!(
        service
            .context(
                api::Context {
                    repository_ids: vec!["private-clone".into()]
                },
                &viewer
            )
            .unwrap()
            .memberships
            .is_empty()
    );
    let checkout = &status.checkouts[0];
    let patch = |revision| {
        serde_json::from_value::<api::CheckoutUpdate>(serde_json::json!({"project_id":product.project.project_id,"worktree_id":checkout.worktree_id,"expected_revision":revision,"intent":"Resume source"})).unwrap()
    };
    assert_eq!(
        service
            .update_checkout(patch(checkout.revision), &viewer)
            .unwrap_err()
            .code,
        ErrorCode::PermissionDenied
    );
    let edited = service
        .update_checkout(patch(checkout.revision), &operator)
        .unwrap();
    assert_eq!(edited.intent.as_deref(), Some("Resume source"));
    assert!(edited.permissions.edit);
    assert!(edited.absolute_path.is_none());
    // A source-wide branch edit requires operator authority for all physical
    // members; one visible clone cannot modify another clone's shared intent.
    let branch = &status.branches[0];
    let patch:api::BranchUpdate=serde_json::from_value(serde_json::json!({"project_id":product.project.project_id,"branch_id":branch.branch_id,"expected_revision":branch.revision,"intent":"Shared intent"})).unwrap();
    assert_eq!(
        service.update_branch(patch, &operator).unwrap_err().code,
        ErrorCode::PermissionDenied
    );
    let full = service
        .status(reference(&product.project.project_id), &admin)
        .unwrap();
    assert_eq!(full.checkouts.len(), 2);
    assert!(
        full.branches
            .iter()
            .any(|b| b.branch_name == "private-only")
    );
    assert!(full.permissions.admin);
    assert!(full.checkouts.iter().all(|c| c.absolute_path.is_some()));
    let public_checkout = full
        .checkouts
        .iter()
        .find(|row| row.absolute_path.as_deref() == fixture.repository.to_str())
        .unwrap();
    let public_output = fixture._temporary.path().join("public-output");
    let private_output = fixture._temporary.path().join("private-output");
    save_project_artifact(&fixture, &public_output, "public-artifact", "project-alpha");
    save_project_artifact(
        &fixture,
        &private_output,
        "private-artifact",
        "private-clone",
    );
    let adopt = api::artifacts::Adopt {
        project_id: product.project.project_id.clone(),
        worktree_id: public_checkout.worktree_id.clone(),
        artifact_id: "public-artifact".into(),
        expected_checkout_revision: public_checkout.revision,
        expected_artifact_revision: 1,
        expected_revision: 0,
        ownership: "shared".into(),
        idempotency_key: "operator-artifact".into(),
    };
    assert_eq!(
        service
            .adopt_artifact(adopt.clone(), &viewer)
            .unwrap_err()
            .code,
        ErrorCode::PermissionDenied
    );
    assert_eq!(
        service
            .adopt_artifact(
                api::artifacts::Adopt {
                    artifact_id: "private-artifact".into(),
                    idempotency_key: "private-artifact-denied".into(),
                    ..adopt.clone()
                },
                &operator
            )
            .unwrap_err()
            .code,
        ErrorCode::PermissionDenied
    );
    service.adopt_artifact(adopt, &operator).unwrap();
    let input = serde_json::json!({"project_id":product.project.project_id});
    assert!(access.authorize("project.status", &input, &viewer).is_ok());
    assert_eq!(
        access
            .authorize(
                "repository.status",
                &serde_json::json!({"path":fixture.repository}),
                &viewer
            )
            .unwrap_err()
            .code,
        ErrorCode::PermissionDenied
    );
    assert!(
        service
            .refresh(
                api::Refresh {
                    project_id: product.project.project_id.clone(),
                    idempotency_key: "viewer-refresh".into(),
                    include_remote: false
                },
                &viewer
            )
            .is_ok()
    );
    fixture
        .database
        .transaction(|c| {
            c.execute("DELETE FROM grants WHERE user_id='viewer'", [])?;
            Ok(())
        })
        .unwrap();
    assert!(
        service
            .status(reference(&product.project.project_id), &viewer)
            .is_err()
    );
    assert!(
        service
            .list(api::List::default(), &viewer)
            .unwrap()
            .projects
            .is_empty()
    );
}

#[test]
fn refreshing_facts_does_not_reject_or_overwrite_concurrent_intent_edits() {
    let fixture = Fixture::new();
    initialize(&fixture.repository);
    fixture
        .database
        .transaction(|c| {
            c.execute(
                "UPDATE repositories SET registered_by_uid=$1",
                [caller().uid],
            )?;
            Ok(())
        })
        .unwrap();
    let linked = fixture._temporary.path().join("feature-copy");
    git(
        &fixture.repository,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            linked.to_str().unwrap(),
        ],
    );
    let service = ProjectService::new(
        fixture.database.clone(),
        Registry::new(fixture.database.clone()),
    );
    let status = configure(
        &service,
        &fixture,
        "Concurrent",
        vec![source("project-alpha", &["project-alpha"])],
    );
    let configured_revision = status.project.revision;
    drain(&service, &status.project.project_id, "initial");
    let status = service
        .status(reference(&status.project.project_id), &caller())
        .unwrap();
    assert_eq!(
        status.project.revision, configured_revision,
        "initial discovery changes observations, not project metadata"
    );
    assert!(status.refreshed_at.is_some());
    let snapshot = status
        .branches
        .iter()
        .find(|b| b.branch_name == "feature")
        .unwrap()
        .clone();
    let checkout = status
        .checkouts
        .iter()
        .find(|c| c.branch_name.as_deref() == Some("feature"))
        .unwrap()
        .clone();
    assert_eq!(snapshot.lifecycle, "unreviewed");
    let job = service
        .refresh(
            api::Refresh {
                project_id: status.project.project_id.clone(),
                idempotency_key: "while-editing".into(),
                include_remote: false,
            },
            &caller(),
        )
        .unwrap();
    // The inventory step observes the same refs while an editor holds the old
    // revision. Its metadata save must remain valid during the remaining steps.
    service.refresh_step(&mut Budget::new(Duration::from_secs(10), 256, 1024 * 1024));
    assert_eq!(
        service
            .status(reference(&status.project.project_id), &caller())
            .unwrap()
            .project
            .revision,
        status.project.revision,
        "an inventory chunk must preserve the page's project metadata revision"
    );
    let edited=service.update_branch(serde_json::from_value(serde_json::json!({"project_id":status.project.project_id,"branch_id":snapshot.branch_id,"expected_revision":snapshot.revision,"intent":"Keep the newer user intent","owner":"Owner","outcome":"Accepted result","lifecycle":"paused"})).unwrap(),&caller()).unwrap();
    let edited_checkout=service.update_checkout(serde_json::from_value(serde_json::json!({"project_id":status.project.project_id,"worktree_id":checkout.worktree_id,"expected_revision":checkout.revision,"intent":"Preserve checkout intent","role":"scratch"})).unwrap(),&caller()).unwrap();
    let edited_revision = service
        .status(reference(&status.project.project_id), &caller())
        .unwrap()
        .project
        .revision;
    assert_eq!(
        edited_revision,
        status.project.revision + 2,
        "each real metadata edit must invalidate the earlier project revision"
    );
    // A slow remote observation may have loaded its row before either save.
    // Apply that actual persistence path against the old captured rows.
    fixture
        .database
        .transaction(|c| {
            super::refresh::persist_branch(
                c,
                &status.project.project_id,
                &snapshot,
                &timestamp(),
                "delayed-remote-observation",
            )?;
            super::refresh::persist_checkout(
                c,
                &status.project.project_id,
                "project-alpha",
                &checkout,
                &timestamp(),
                "delayed-filesystem-observation",
            )?;
            Ok(())
        })
        .unwrap();
    let mut finished = false;
    for _ in 0..30 {
        service.refresh_step(&mut Budget::new(Duration::from_secs(10), 256, 1024 * 1024));
        assert_eq!(
            service
                .status(reference(&status.project.project_id), &caller())
                .unwrap()
                .project
                .revision,
            edited_revision,
            "background facts must preserve the newer metadata revision in every chunk"
        );
        if service
            .job(
                api::JobReference {
                    project_id: status.project.project_id.clone(),
                    job_id: job.job_id.clone(),
                },
                &caller(),
            )
            .unwrap()
            .state
            == "succeeded"
        {
            finished = true;
            break;
        }
    }
    assert!(finished, "the complete observation refresh must finish");
    let refreshed = service
        .status(reference(&status.project.project_id), &caller())
        .unwrap();
    assert!(refreshed.refreshed_at.is_some());
    assert!(
        refreshed
            .activity
            .iter()
            .any(|event| event.kind == "project.refreshed")
    );
    let branch = service
        .branch(
            api::BranchItem {
                project_id: status.project.project_id.clone(),
                branch_id: snapshot.branch_id,
            },
            &caller(),
        )
        .unwrap();
    assert_eq!(branch.intent, edited.intent);
    assert_eq!(branch.owner, edited.owner);
    assert_eq!(branch.outcome, edited.outcome);
    assert_eq!(branch.lifecycle, "paused");
    assert_eq!(branch.revision, edited.revision);
    let checkout = service
        .checkout(
            api::CheckoutItem {
                project_id: status.project.project_id.clone(),
                worktree_id: checkout.worktree_id,
            },
            &caller(),
        )
        .unwrap();
    assert_eq!(checkout.intent, edited_checkout.intent);
    assert_eq!(checkout.role, "scratch");
    assert_eq!(checkout.revision, edited_checkout.revision);
    let configured = api::Configure {
        project_id: Some(status.project.project_id.clone()),
        expected_revision: edited_revision,
        name: "Updated Concurrent".into(),
        slug: "concurrent".into(),
        roots: vec![api::RootInput {
            kind: "source".into(),
            label: "Sources".into(),
            path: fixture._temporary.path().to_string_lossy().into_owned(),
        }],
        repositories: vec![source("project-alpha", &["project-alpha"])],
    };
    let updated = service.configure(configured.clone(), &caller()).unwrap();
    assert_eq!(updated.project.revision, edited_revision + 1);
    assert_eq!(
        service.configure(configured, &caller()).unwrap_err().code,
        ErrorCode::ConfigurationConflict,
        "a real configuration edit must still refuse the old revision"
    );
}

#[test]
fn large_project_metadata_retains_bounded_pages_and_full_point_details() {
    let fixture = Fixture::new();
    initialize(&fixture.repository);
    fixture
        .database
        .transaction(|c| {
            c.execute(
                "UPDATE repositories SET registered_by_uid=$1",
                [caller().uid],
            )?;
            Ok(())
        })
        .unwrap();
    let service = ProjectService::new(
        fixture.database.clone(),
        Registry::new(fixture.database.clone()),
    );
    let status = configure(
        &service,
        &fixture,
        "Large",
        vec![source("project-alpha", &["project-alpha"])],
    );
    let project = status.project.project_id;
    let long = "A long accepted description. ".repeat(70);
    let long = long[..2000].to_string();
    fixture.database.transaction(|c|{
        for index in 0..70 {
            let name=format!("feature-{index:03}");let mut branch=empty_branch(&format!("branch-{index:03}"),"project-alpha",&name);branch.intent=Some(long.clone());branch.outcome=Some(long.clone());branch.owner=Some(long.clone());branch.next_action=Some(long.clone());branch.task_id=Some("spec".into());
            c.execute("INSERT INTO project_branches(project_id,branch_id,repository_id,branch_name,intent,outcome,owner,next_action,task_id,record_json,updated_at,updated_by) VALUES($1,$2,'project-alpha',$3,$4,$4,$4,$4,'spec',$5::jsonb,'t','fixture')",params![project,branch.branch_id,name,long,json(&branch)?])?;
            c.execute("INSERT INTO project_branch_observations(project_id,branch_id,physical_repository_id,local_present,observed_at) VALUES($1,$2,'project-alpha',true,'t')",params![project,branch.branch_id])?;
            let path=fixture._temporary.path().join(format!("copy-{index:03}"));let mut checkout=empty_checkout(&format!("checkout-{index:03}"),"project-alpha",path.to_str().unwrap(),format!("copy-{index:03}"),"source".into(),"worktree");checkout.branch_id=Some(branch.branch_id);checkout.branch_name=Some(name);checkout.intent=Some(long.clone());checkout.outcome=Some(long.clone());checkout.owner=Some(long.clone());checkout.next_action=Some(long.clone());checkout.task_id=Some("spec".into());
            c.execute("INSERT INTO project_checkouts(project_id,worktree_id,repository_id,physical_repository_id,absolute_path,role,branch_id,branch_name,intent,outcome,owner,next_action,task_id,record_json,updated_at,updated_by) VALUES($1,$2,'project-alpha','project-alpha',$3,'worktree',$4,$5,$6,$6,$6,$6,'spec',$7::jsonb,'t','fixture')",params![project,checkout.worktree_id,path.to_string_lossy().into_owned(),checkout.branch_id,checkout.branch_name,long,json(&checkout)?])?;
        }Ok(())
    }).unwrap();
    let status = service.status(reference(&project), &caller()).unwrap();
    assert!(encoded_len(&status) < 192 * 1024);
    assert!(!status.branches.is_empty());
    assert!(!status.checkouts.is_empty());
    assert!(status.next_branch_cursor.is_some());
    assert!(status.next_checkout_cursor.is_some());
    assert!(status.branches[0].details_omitted);
    let branch = service
        .branch(
            api::BranchItem {
                project_id: project.clone(),
                branch_id: status.branches[0].branch_id.clone(),
            },
            &caller(),
        )
        .unwrap();
    assert_eq!(branch.intent.as_deref(), Some(long.as_str()));
    assert!(!branch.details_omitted);
    let checkout = service
        .checkout(
            api::CheckoutItem {
                project_id: project.clone(),
                worktree_id: "checkout-000".into(),
            },
            &caller(),
        )
        .unwrap();
    assert_eq!(checkout.outcome.as_deref(), Some(long.as_str()));
    assert!(!checkout.details_omitted);
    let mut query = page(&project, 100);
    let mut seen = BTreeSet::new();
    loop {
        let result = service.branches(query.clone(), &caller()).unwrap();
        assert!(encoded_len(&result) < 192 * 1024);
        for branch in result.branches {
            assert!(seen.insert(branch.branch_id));
        }
        let Some(cursor) = result.next_cursor else {
            break;
        };
        query.cursor = Some(cursor);
    }
    assert_eq!(seen.len(), 70);
    let checkouts = service.checkouts(page(&project, 100), &caller()).unwrap();
    assert!(encoded_len(&checkouts) < 192 * 1024);
    assert!(!checkouts.checkouts.is_empty());
}

#[test]
fn refresh_rejects_a_foreign_repository_reusing_a_registered_path() {
    let fixture = Fixture::new();
    initialize(&fixture.repository);
    git(&fixture.repository, &["branch", "original-feature"]);
    let linked = fixture._temporary.path().join("identity-linked");
    git(
        &fixture.repository,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "linked-feature",
            linked.to_str().unwrap(),
        ],
    );
    fixture
        .database
        .transaction(|c| {
            c.execute(
                "UPDATE repositories SET registered_by_uid=$1",
                [caller().uid],
            )?;
            Ok(())
        })
        .unwrap();
    let service = ProjectService::new(
        fixture.database.clone(),
        Registry::new(fixture.database.clone()),
    );
    let status = configure(
        &service,
        &fixture,
        "Identity",
        vec![source("project-alpha", &["project-alpha"])],
    );
    let project = status.project.project_id;
    drain(&service, &project, "bind-native");
    let old = service.status(reference(&project), &caller()).unwrap();
    let linked_old = fixture._temporary.path().join("kept-linked");
    fs::rename(&linked, &linked_old).unwrap();
    initialize(&linked);
    git(&linked, &["branch", "foreign-linked"]);
    let linked_job = service
        .refresh(
            api::Refresh {
                project_id: project.clone(),
                idempotency_key: "reject-linked-replacement".into(),
                include_remote: false,
            },
            &caller(),
        )
        .unwrap();
    for _ in 0..10 {
        service.refresh_step(&mut Budget::new(Duration::from_secs(10), 256, 1024 * 1024));
    }
    assert_eq!(
        service
            .job(
                api::JobReference {
                    project_id: project.clone(),
                    job_id: linked_job.job_id
                },
                &caller()
            )
            .unwrap()
            .state,
        "partial"
    );
    assert!(
        service
            .status(reference(&project), &caller())
            .unwrap()
            .branches
            .iter()
            .all(|b| b.branch_name != "foreign-linked")
    );
    let kept = fixture._temporary.path().join("kept-original");
    fs::rename(&fixture.repository, &kept).unwrap();
    initialize(&fixture.repository);
    git(&fixture.repository, &["branch", "private-foreign"]);
    let job = service
        .refresh(
            api::Refresh {
                project_id: project.clone(),
                idempotency_key: "reject-replacement".into(),
                include_remote: false,
            },
            &caller(),
        )
        .unwrap();
    for _ in 0..5 {
        service.refresh_step(&mut Budget::new(Duration::from_secs(10), 256, 1024 * 1024));
    }
    let job = service
        .job(
            api::JobReference {
                project_id: project.clone(),
                job_id: job.job_id,
            },
            &caller(),
        )
        .unwrap();
    assert_eq!(job.state, "partial");
    let status = service.status(reference(&project), &caller()).unwrap();
    assert_eq!(status.repositories[0].refresh_state, "failed");
    assert!(
        status
            .branches
            .iter()
            .all(|b| b.branch_name != "private-foreign")
    );
    assert!(
        status
            .branches
            .iter()
            .any(|b| b.branch_name == "original-feature")
    );
    assert_eq!(
        status
            .branches
            .iter()
            .map(|b| &b.branch_id)
            .collect::<BTreeSet<_>>(),
        old.branches
            .iter()
            .map(|b| &b.branch_id)
            .collect::<BTreeSet<_>>()
    );
    assert!(kept.join("source.txt").is_file());
}

#[test]
fn measured_source_allocations_exclude_shared_git_and_declared_unmeasured_builds() {
    let fixture = Fixture::new();
    initialize(&fixture.repository);
    fixture
        .database
        .transaction(|c| {
            c.execute(
                "UPDATE repositories SET registered_by_uid=$1",
                [caller().uid],
            )?;
            Ok(())
        })
        .unwrap();
    let linked = fixture._temporary.path().join("measured-feature");
    git(
        &fixture.repository,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            linked.to_str().unwrap(),
        ],
    );
    fs::write(linked.join("local.bin"), vec![1u8; 16 * 1024]).unwrap();
    let service = ProjectService::new(
        fixture.database.clone(),
        Registry::new(fixture.database.clone()),
    );
    let status = configure(
        &service,
        &fixture,
        "Measured",
        vec![source("project-alpha", &["project-alpha"])],
    );
    let project = status.project.project_id;
    drain(&service, &project, "measure-real");
    let status = service.status(reference(&project), &caller()).unwrap();
    let shared = crate::storage::fs::measure(&fixture.repository.join(".git"))
        .unwrap()
        .bytes;
    let whole = crate::storage::fs::measure(&fixture.repository)
        .unwrap()
        .bytes
        + crate::storage::fs::measure(&linked).unwrap().bytes;
    assert_eq!(status.storage.shared_bytes, Some(shared));
    assert_eq!(status.storage.allocated_bytes, Some(whole));
    assert_eq!(status.storage.source_bytes, Some(whole - shared));
    assert_eq!(status.storage.unmeasured_count, 0);
    assert!(status.storage.measured_at.is_some());
    assert_eq!(
        status
            .checkouts
            .iter()
            .map(|c| c.local_bytes.unwrap())
            .sum::<u64>(),
        whole - shared
    );
    let buckets = fixture
        .database
        .read(|c| {
            Ok(c.query_row(
                "SELECT count(*) FROM project_storage WHERE project_id=$1 AND kind='shared'",
                [&project],
                |r| r.get::<_, u32>(0),
            )?)
        })
        .unwrap();
    assert_eq!(buckets, 1);
    let build = fixture._temporary.path().join("declared-build");
    fs::create_dir(&build).unwrap();
    fixture.database.transaction(|c|{c.execute("INSERT INTO project_roots(project_id,root_id,kind,label,absolute_path) VALUES($1,'build-root','build','Build',$2)",params![project,build.to_string_lossy().into_owned()])?;Ok(())}).unwrap();
    let declared = service.status(reference(&project), &caller()).unwrap();
    assert_eq!(declared.storage.allocated_bytes, None);
    assert_eq!(declared.storage.build_bytes, None);
    assert!(declared.storage.unmeasured_count > 0);
    let current = fixture._temporary.path().join("immutable-current");
    let rollback = fixture._temporary.path().join("immutable-rollback");
    for path in [&current, &rollback] {
        fs::create_dir(path).unwrap();
        fs::write(path.join("app.bin"), vec![7u8; 8192]).unwrap();
    }
    let current_bytes = crate::storage::fs::measure(&current).unwrap().bytes;
    let rollback_bytes = crate::storage::fs::measure(&rollback).unwrap().bytes;
    let primary = status
        .checkouts
        .iter()
        .find(|row| row.role == "primary")
        .unwrap();
    fixture.database.transaction(|c|{
        c.execute("INSERT INTO deployments(deployment_id,repository_id,worktree_id,name,source,spec_fingerprint,spec_json,state,current_generation,previous_generation,created_at,created_by_uid,client,updated_at) VALUES('measured-deployment','project-alpha',$1,'Web','checkout','fixture','{}','stopped',2,1,'t',$2,'fixture','t')",params![primary.worktree_id,caller().uid])?;
        for (number,path) in [(2,&current),(1,&rollback)]{c.execute("INSERT INTO generations(deployment_id,number,commit_hash,dirty,path,fingerprint,created_at,state) VALUES('measured-deployment',$1,'fixture',0,$2,'fixture','t','current')",params![number,path.to_string_lossy().into_owned()])?;}
        Ok(())
    }).unwrap();
    drain(&service, &project, "measure-generations");
    let measured = service.status(reference(&project), &caller()).unwrap();
    assert_eq!(
        measured.storage.build_bytes,
        Some(current_bytes + rollback_bytes)
    );
    assert_eq!(
        measured.storage.allocated_bytes,
        Some(whole + current_bytes + rollback_bytes)
    );
    let generations: Vec<_> = measured
        .checkouts
        .iter()
        .filter(|row| row.immutable_generation)
        .collect();
    assert_eq!(generations.len(), 2);
    for row in generations {
        assert_eq!(row.registration_state, "unregistered");
        assert_eq!(row.locations.len(), 1);
        assert_eq!(row.locations[0].kind, "generation");
        assert!(row.locations[0].allocated_bytes.is_some());
        assert!(!row.can_remove);
    }
    fs::remove_dir_all(&rollback).unwrap();
    drain(&service, &project, "measure-disappeared-generation");
    let missing = service.status(reference(&project), &caller()).unwrap();
    assert_eq!(missing.storage.build_bytes, Some(current_bytes));
    assert_eq!(missing.storage.allocated_bytes, Some(whole + current_bytes));
    let gone = missing
        .checkouts
        .iter()
        .find(|row| row.absolute_path.as_deref() == rollback.to_str())
        .unwrap();
    assert_eq!(gone.exists, Some(false));
    assert_eq!(gone.local_bytes, Some(0));
}

fn save_project_artifact(
    fixture: &Fixture,
    path: &Path,
    id: &str,
    repository: &str,
) -> crate::storage::model::Record {
    use crate::storage::model::{Locator, Record};
    use devcoordinator2_api::storage::{Artifact, Effect, Kind, Safety};
    fs::create_dir_all(path).unwrap();
    fs::write(path.join("output.bin"), vec![3u8; 16384]).unwrap();
    let measured = crate::storage::fs::measure(path).unwrap();
    let record = Record {
        update_sequence: 1,
        artifact: Artifact {
            artifact_id: id.into(),
            revision: 1,
            repository_id: Some(repository.into()),
            repository_name: None,
            group_id: None,
            group_name: None,
            accounting_id: format!("allocation-{id}"),
            name: id.into(),
            kind: Kind::BuildOutput,
            effect: Effect::Rebuildable,
            ownership: "explicit".into(),
            filesystem_id: None,
            allocated_bytes: Some(measured.bytes),
            last_used_at_ms: None,
            observed_since_ms: 1,
            verified_at_ms: Some(1),
            eligible_at_ms: None,
            safety: Safety::NeedsReview,
            reasons: vec![],
            protected: false,
            deletable: false,
            automatic_eligible: false,
            dependencies: vec![],
            aliases: vec![],
            removed_at_ms: None,
        },
        locator: Locator::Directory {
            path: path.into(),
            device: measured.device,
            inode: measured.inode,
            root: fixture._temporary.path().into(),
            git_root: None,
        },
        resource_key: format!("fs:{}:{}", measured.device, measured.inode),
        ancestor_keys: vec![],
        fingerprint: "fixture".into(),
        owner_deployment: None,
        last_activity_signature: "fixture".into(),
        discovered_effect: Effect::Rebuildable,
        disposal_approved: false,
        disposal_reason: None,
        blockers: vec![],
        recovery_lineage: None,
        recovery_verified: false,
        recovery_created_at_ms: None,
        private_aliases: vec![],
        retention_eligible: false,
        ignored_review: None,
    };
    fixture.database.transaction(|c|{c.execute("INSERT INTO storage_artifacts(artifact_id,repository_id,kind,revision,record_json,updated_at_ms) VALUES($1,$2,'build_output',1,$3::jsonb,1)",params![id,repository,json(&record)?])?;Ok(())}).unwrap();
    record
}

#[test]
fn artifact_associations_bind_retry_intent_ownership_and_real_external_bytes() {
    use api::artifacts::{Adopt, Detach, List};
    let fixture = Fixture::new();
    initialize(&fixture.repository);
    fixture
        .database
        .transaction(|c| {
            c.execute(
                "UPDATE repositories SET registered_by_uid=$1",
                [caller().uid],
            )?;
            Ok(())
        })
        .unwrap();
    let linked = fixture._temporary.path().join("artifact-feature");
    git(
        &fixture.repository,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            linked.to_str().unwrap(),
        ],
    );
    let service = ProjectService::new(
        fixture.database.clone(),
        Registry::new(fixture.database.clone()),
    );
    let initial = configure(
        &service,
        &fixture,
        "Artifacts",
        vec![source("project-alpha", &["project-alpha"])],
    );
    let project = initial.project.project_id;
    drain(&service, &project, "artifact-inventory");
    let snapshot = service.status(reference(&project), &caller()).unwrap();
    let primary = snapshot
        .checkouts
        .iter()
        .find(|row| row.role == "primary")
        .unwrap()
        .clone();
    let feature = snapshot
        .checkouts
        .iter()
        .find(|row| row.absolute_path.as_deref() == linked.to_str())
        .unwrap()
        .clone();
    let output = fixture._temporary.path().join("external-output");
    let record = save_project_artifact(&fixture, &output, "external-artifact", "project-alpha");
    let request = Adopt {
        project_id: project.clone(),
        worktree_id: primary.worktree_id.clone(),
        artifact_id: record.artifact.artifact_id.clone(),
        expected_checkout_revision: primary.revision,
        expected_artifact_revision: 1,
        expected_revision: 0,
        ownership: "exclusive".into(),
        idempotency_key: "adopt-first".into(),
    };
    let first = service.adopt_artifact(request.clone(), &caller()).unwrap();
    assert!(first.identity_valid);
    let mut changed = request.clone();
    changed.ownership = "shared".into();
    assert_eq!(
        service.adopt_artifact(changed, &caller()).unwrap_err().code,
        ErrorCode::ConfigurationConflict
    );
    let other = Adopt {
        worktree_id: feature.worktree_id.clone(),
        expected_checkout_revision: feature.revision,
        idempotency_key: "adopt-other".into(),
        ownership: "shared".into(),
        ..request.clone()
    };
    assert!(service.adopt_artifact(other.clone(), &caller()).is_err());
    let shared = service
        .adopt_artifact(
            Adopt {
                expected_checkout_revision: primary.revision + 1,
                expected_revision: first.revision,
                ownership: "shared".into(),
                idempotency_key: "make-shared".into(),
                ..request.clone()
            },
            &caller(),
        )
        .unwrap();
    let second = service.adopt_artifact(other, &caller()).unwrap();
    assert!(second.has_other_owners);
    assert_eq!(second.other_checkout_count, 1);
    assert!(
        service
            .adopt_artifact(
                Adopt {
                    expected_checkout_revision: primary.revision + 2,
                    expected_revision: shared.revision,
                    idempotency_key: "reject-exclusive-update".into(),
                    ..request.clone()
                },
                &caller()
            )
            .is_err()
    );
    let nested = linked.join("target");
    save_project_artifact(&fixture, &nested, "nested-artifact", "project-alpha");
    let reserved_plan = serde_json::json!({"checkout_removal":{"targets":[{"binding":{"project_id":project,"worktree_id":feature.worktree_id,"physical_repository_id":"project-alpha","path":linked}}]},"records":[]});
    fixture.database.transaction(|c|{
        c.execute("INSERT INTO storage_plans(plan_id,plan_json,created_at_ms,actor) VALUES('ownership-plan',$1::jsonb,1,'fixture')",[reserved_plan.to_string()])?;
        c.execute("INSERT INTO storage_jobs(job_id,kind,state,idempotency_key,request_sha256,actor,caller_uid,job_json,request_json,created_at_ms) VALUES('ownership-cleanup','cleanup','queued','ownership-cleanup','fixture','fixture',$1,$2::jsonb,'{}',1)",params![caller().uid,serde_json::json!({"plan_id":"ownership-plan"}).to_string()])?;
        Ok(())
    }).unwrap();
    assert_eq!(
        service
            .adopt_artifact(
                Adopt {
                    expected_checkout_revision: primary.revision + 2,
                    artifact_id: "nested-artifact".into(),
                    ownership: "shared".into(),
                    idempotency_key: "queued-nested-owner".into(),
                    ..request.clone()
                },
                &caller()
            )
            .unwrap_err()
            .code,
        ErrorCode::Busy,
        "an accepted removal reserves nested output even when adoption targets another checkout"
    );
    assert_eq!(
        service
            .detach_artifact(
                Detach {
                    project_id: project.clone(),
                    worktree_id: feature.worktree_id.clone(),
                    association_id: second.association_id.clone(),
                    expected_revision: second.revision,
                    idempotency_key: "queued-detach".into()
                },
                &caller()
            )
            .unwrap_err()
            .code,
        ErrorCode::Busy
    );
    fixture
        .database
        .transaction(|c| {
            c.execute(
                "UPDATE storage_jobs SET state='cancelled' WHERE job_id='ownership-cleanup'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    fs::remove_dir_all(&nested).unwrap();
    let detach = Detach {
        project_id: project.clone(),
        worktree_id: feature.worktree_id.clone(),
        association_id: second.association_id,
        expected_revision: second.revision,
        idempotency_key: "detach-other".into(),
    };
    let detached = service.detach_artifact(detach.clone(), &caller()).unwrap();
    assert!(!detached.active);
    assert_eq!(
        service.detach_artifact(detach, &caller()).unwrap().revision,
        detached.revision
    );
    // Native storage can retain distinct locator records for one allocation.
    // Keep their raw per-artifact accounting IDs different, just as production
    // candidates do before the storage read projection normalizes them.
    let alias = save_project_artifact(
        &fixture,
        &output,
        "external-artifact-alias",
        "project-alpha",
    );
    assert_eq!(alias.resource_key, record.resource_key);
    assert_ne!(alias.artifact.accounting_id, record.artifact.accounting_id);
    let feature_current = service
        .checkout(
            api::CheckoutItem {
                project_id: project.clone(),
                worktree_id: feature.worktree_id.clone(),
            },
            &caller(),
        )
        .unwrap();
    let alias_owner = service
        .adopt_artifact(
            Adopt {
                project_id: project.clone(),
                worktree_id: feature.worktree_id.clone(),
                artifact_id: alias.artifact.artifact_id,
                expected_checkout_revision: feature_current.revision,
                expected_artifact_revision: 1,
                expected_revision: 0,
                ownership: "shared".into(),
                idempotency_key: "adopt-shared-allocation-alias".into(),
            },
            &caller(),
        )
        .unwrap();
    assert!(alias_owner.has_other_owners);
    drain(&service, &project, "artifact-accounting");
    assert_eq!(
        service
            .adopt_artifact(request.clone(), &caller())
            .unwrap()
            .association_id,
        first.association_id,
        "an exact retry survives checkout observation revisions"
    );
    let measured = service.status(reference(&project), &caller()).unwrap();
    assert_eq!(
        measured.storage.build_bytes, record.artifact.allocated_bytes,
        "different records for the same native allocation must contribute bytes once"
    );
    assert_eq!(
        fixture
            .database
            .read(|c| Ok(c.query_row(
                "SELECT count(*) FROM project_storage WHERE project_id=$1 AND kind='build'",
                [&project],
                |row| row.get::<_, u32>(0)
            )?))
            .unwrap(),
        1
    );
    let shared_feature = measured
        .checkouts
        .iter()
        .find(|row| row.worktree_id == feature.worktree_id)
        .unwrap();
    assert_eq!(
        shared_feature.build_bytes, record.artifact.allocated_bytes,
        "each owner still displays its linked output"
    );
    service
        .detach_artifact(
            Detach {
                project_id: project.clone(),
                worktree_id: feature.worktree_id.clone(),
                association_id: alias_owner.association_id,
                expected_revision: alias_owner.revision,
                idempotency_key: "detach-allocation-alias".into(),
            },
            &caller(),
        )
        .unwrap();
    let primary = measured
        .checkouts
        .iter()
        .find(|row| row.worktree_id == primary.worktree_id)
        .unwrap();
    assert_eq!(primary.build_bytes, record.artifact.allocated_bytes);
    assert!(
        primary
            .locations
            .iter()
            .any(|location| location.artifact_id.as_deref() == Some("external-artifact"))
    );
    let moved = fixture._temporary.path().join("retained-old-output");
    fs::rename(&output, &moved).unwrap();
    fs::create_dir(&output).unwrap();
    let associations = service
        .artifact_associations(
            List {
                project_id: project.clone(),
                worktree_id: primary.worktree_id.clone(),
                cursor: None,
                limit: 50,
            },
            &caller(),
        )
        .unwrap();
    assert!(!associations.associations[0].identity_valid);
    assert_eq!(associations.associations[0].location.allocated_bytes, None);
    assert!(
        fixture
            .database
            .read(|c| artifacts::active_records(c, &project, &primary.worktree_id))
            .is_err()
    );
    drain(&service, &project, "replacement-output");
    let stale = service.status(reference(&project), &caller()).unwrap();
    assert_eq!(stale.storage.build_bytes, None);
    assert!(stale.storage.unmeasured_count > 0);
    let detached = service
        .detach_artifact(
            Detach {
                project_id: project.clone(),
                worktree_id: primary.worktree_id.clone(),
                association_id: first.association_id,
                expected_revision: shared.revision,
                idempotency_key: "detach-stale".into(),
            },
            &caller(),
        )
        .unwrap();
    assert!(
        !detached.active,
        "stale ownership can be detached without treating the replacement as owned"
    );
    drain(&service, &project, "remove-stale-accounting");
    assert_eq!(
        fixture
            .database
            .read(|c| Ok(c.query_row(
                "SELECT count(*) FROM project_storage WHERE project_id=$1 AND kind='build'",
                [&project],
                |r| r.get::<_, u32>(0)
            )?))
            .unwrap(),
        0
    );
    let race_output = fixture._temporary.path().join("racing-output");
    save_project_artifact(&fixture, &race_output, "racing-artifact", "project-alpha");
    let current = service.status(reference(&project), &caller()).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = current
        .checkouts
        .iter()
        .take(2)
        .enumerate()
        .map(|(index, checkout)| {
            let service = service.clone();
            let barrier = barrier.clone();
            let request = Adopt {
                project_id: project.clone(),
                worktree_id: checkout.worktree_id.clone(),
                artifact_id: "racing-artifact".into(),
                expected_checkout_revision: checkout.revision,
                expected_artifact_revision: 1,
                expected_revision: 0,
                ownership: "exclusive".into(),
                idempotency_key: format!("racing-owner-{index}"),
            };
            std::thread::spawn(move || {
                barrier.wait();
                service.adopt_artifact(request, &caller())
            })
        })
        .collect();
    let results: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(
                |result| matches!(result,Err(error) if error.code==ErrorCode::ConfigurationConflict)
            )
            .count(),
        1
    );
    let concurrent_output = fixture._temporary.path().join("refresh-concurrent-output");
    save_project_artifact(
        &fixture,
        &concurrent_output,
        "refresh-concurrent-artifact",
        "project-alpha",
    );
    let current = service.status(reference(&project), &caller()).unwrap();
    let target = &current.checkouts[0];
    let concurrent = Adopt {
        project_id: project.clone(),
        worktree_id: target.worktree_id.clone(),
        artifact_id: "refresh-concurrent-artifact".into(),
        expected_checkout_revision: target.revision,
        expected_artifact_revision: 1,
        expected_revision: 0,
        ownership: "exclusive".into(),
        idempotency_key: "refresh-concurrent-adoption".into(),
    };
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let refresh_thread = {
        let service = service.clone();
        let project = project.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            drain(&service, &project, "refresh-with-association-edit")
        })
    };
    barrier.wait();
    let adopted = service.adopt_artifact(concurrent.clone(), &caller());
    refresh_thread.join().unwrap();
    if let Err(error) = adopted {
        assert_eq!(
            error.code,
            ErrorCode::ConfigurationConflict,
            "a refreshed observation may require a newer revision; a lock-order failure is not a valid conflict"
        );
        let current = service.status(reference(&project), &caller()).unwrap();
        let target = current
            .checkouts
            .iter()
            .find(|row| row.worktree_id == concurrent.worktree_id)
            .unwrap();
        service
            .adopt_artifact(
                Adopt {
                    expected_checkout_revision: target.revision,
                    ..concurrent.clone()
                },
                &caller(),
            )
            .unwrap();
    }
    drain(&service, &project, "concurrent-association-accounted");
    let current = service.status(reference(&project), &caller()).unwrap();
    assert!(
        current
            .checkouts
            .iter()
            .find(|row| row.worktree_id == concurrent.worktree_id)
            .unwrap()
            .artifact_ids
            .iter()
            .any(|id| id == "refresh-concurrent-artifact")
    );
}
