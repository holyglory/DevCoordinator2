//! Real Git and authority fixture for the Project Explorer browser journey.
use std::{path::Path, process::Command};
use devcoordinator2_api::ClientKind;
use devcoordinator2_control::{access::Caller, control_plane::ControlPlane, daemon::OperationExecutor, database::Database};
use serde_json::{json, Value};

fn git(root: &Path, arguments: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let result = Command::new("git").arg("-C").arg(root).args(arguments)
        .env("GIT_AUTHOR_NAME", "Explorer fixture").env("GIT_AUTHOR_EMAIL", "fixture@example.test")
        .env("GIT_COMMITTER_NAME", "Explorer fixture").env("GIT_COMMITTER_EMAIL", "fixture@example.test")
        .output()?;
    if !result.status.success() { return Err("fixture Git operation failed".into()); }
    Ok(String::from_utf8(result.stdout)?.trim().to_owned())
}
fn repo(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(path)?;
    git(path, &["init", "-q", "-b", "main"])?;
    std::fs::write(path.join("README.md"), "Project Explorer integration fixture\n")?;
    git(path, &["add", "README.md"])?; git(path, &["commit", "-q", "-m", "Initial source"])?;
    Ok(())
}
pub fn seed(root: &Path, plane: &ControlPlane, database: &Database) -> Result<Value, Box<dyn std::error::Error>> {
    let source = root.join("projects/hdlripper"); let build = root.join("build/projects/hdlripper");
    let primary = source.join("repos/core/primary"); let docs = source.join("repos/docs/primary");
    let feature = source.join("repos/core/worktrees/clock-divider-drag");
    repo(&primary)?; repo(&docs)?; std::fs::create_dir_all(&build)?;
    let remote = root.join("upstream.git"); std::fs::create_dir_all(&remote)?;
    git(&remote, &["init", "--bare", "-q", "-b", "main"])?;
    git(&primary, &["remote", "add", "origin", remote.to_str().ok_or("fixture path")?])?;
    git(&primary, &["push", "-q", "-u", "origin", "main"])?;
    git(&primary, &["worktree", "add", "-q", "-b", "codex/clock-divider-drag", feature.to_str().ok_or("fixture path")?])?;
    std::fs::write(feature.join("drag.txt"), "Drag preserves attached signals and labels.\n")?;
    git(&feature, &["add", "drag.txt"])?; git(&feature, &["commit", "-q", "-m", "Preserve connections while dragging"])?;
    git(&feature, &["push", "-q", "-u", "origin", "HEAD"])?;
    git(&primary, &["branch", "codex/waveform-export"])?;
    std::fs::write(primary.join("unsaved-notes.txt"), "Dirty primary work must survive cleanup.\n")?;
    // SAFETY: getuid/getgid have no memory preconditions.
    let (uid,gid) = unsafe { (libc::getuid(), libc::getgid()) };
    if uid == 0 { return Err("Project browser fixture must run as a non-root execution account".into()); }
    let caller = Caller { via_edge:false,pid:std::process::id(),uid,gid,client_kind:ClientKind::Other,model:None,effort:None,client_session:None,work:None,identity:None };
    let registered = plane.registry().register(&primary,uid,gid)?;
    let feature_record = plane.registry().register(&feature,uid,gid)?;
    let docs_record = plane.registry().register(&docs,uid,gid)?;
    let configured = plane.execute("project.configure",json!({"expected_revision":0,"name":"HDL Ripper","slug":"hdlripper","roots":[{"kind":"source","label":"Source","path":source},{"kind":"build","label":"Build","path":build}],"repositories":[{"repository_id":registered.repository_id,"name":"core","default_branch":"main","physical_repository_ids":[registered.repository_id]},{"repository_id":docs_record.repository_id,"name":"docs","default_branch":"main","physical_repository_ids":[docs_record.repository_id]}]}),&caller)?;
    let project_id = configured.get("project").and_then(|p|p.get("project_id")).or_else(||configured.get("project_id")).and_then(Value::as_str).ok_or("configure lacks project ID")?.to_owned();
    let rid = registered.repository_id.clone(); let did = docs_record.repository_id.clone(); let wid = registered.worktree_id.clone();
    database.transaction(move|c|{
        for (id,email) in [("u3333333333333333","operator@example.test"),("u4444444444444444","outsider@example.test")] {
            c.execute("INSERT INTO users(user_id,email,administrator,created_at,created_by) VALUES($1,$2,0,'now','fixture')",devcoordinator2_control::database::params![id,email])?;
        }
        c.execute("INSERT INTO deployments(deployment_id,repository_id,worktree_id,name,source,spec_fingerprint,spec_json,state,created_at,created_by_uid,client,updated_at) VALUES('d3333333333333333',$1,$2,'fixture-access','checkout','fixture','{}','stopped','now',$3,'other','now')",devcoordinator2_control::database::params![rid,wid,uid])?;
        c.execute("INSERT INTO deployments(deployment_id,repository_id,worktree_id,name,source,spec_fingerprint,spec_json,state,created_at,created_by_uid,client,updated_at) VALUES('d4444444444444444',$1,$2,'docs-access','checkout','fixture','{}','stopped','now',$3,'other','now')",devcoordinator2_control::database::params![did,docs_record.worktree_id,uid])?;
        for dep in ["d3333333333333333","d4444444444444444"] {
            for (u,role) in [("u2222222222222222","viewer"),("u3333333333333333","operator")] {c.execute("INSERT INTO grants VALUES($1,$2,$3,'now','fixture')",devcoordinator2_control::database::params![u,dep,role])?;}
        }
        Ok(())
    })?;
    let job=plane.execute("project.refresh",json!({"project_id":project_id,"include_remote":false,"idempotency_key":"fixture-seed-refresh"}),&caller)?;
    run_jobs(plane)?;
    let status=plane.execute("project.status",json!({"project_id":project_id}),&caller)?;
    if let Some(branch)=status["branches"].as_array().and_then(|rows|rows.iter().find(|b|b["branch_name"]=="codex/clock-divider-drag")) {
        plane.execute("project.branch.update",json!({"project_id":project_id,"branch_id":branch["branch_id"],"expected_revision":branch["revision"],"intent":"Make clock dividers easier to reposition","outcome":"Dragging preserves connections and labels","owner":"UI agent","lifecycle":"ready"}),&caller)?;
    }
    Ok(json!({"project_id":project_id,"repository_id":registered.repository_id,"feature_worktree_id":feature_record.worktree_id,"primary":primary,"feature":feature,"build":build,"refresh_job":job}))
}
pub fn run_jobs(plane: &ControlPlane) -> Result<(), Box<dyn std::error::Error>> {
    use devcoordinator2_control::maintenance::{Budget, JobStatus};
    let deadline=std::time::Instant::now()+std::time::Duration::from_secs(45);
    for _ in 0..256 {
        let mut budget=Budget::new(std::time::Duration::from_secs(5),256,262144);
        let result=plane.project().refresh_step(&mut budget);
        if let JobStatus::Failed{..}=result.status{return Err("fixture project refresh failed".into());}
        let pending=plane.database().read(|connection|Ok(connection.query_row("SELECT count(*) FROM project_jobs WHERE state IN('queued','running')",[],|row|row.get::<_,u64>(0))?))?;
        if pending==0{return Ok(());}
        if std::time::Instant::now()>=deadline{return Err("fixture project refresh deadline".into());}
    }
    Err("fixture project refresh step bound".into())
}
