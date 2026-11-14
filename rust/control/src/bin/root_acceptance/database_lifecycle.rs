//! Real shared-fixture ownership, retained retry, and crash recovery.

use super::*;

pub(super) fn shared_ownership_and_recovery(world: &mut World) -> Result<(), String> {
    let (listener, gate) = database_case_gate(world)?;
    let command = command_json(&fixture_command(
        world,
        &[
            "postgres-query-gate",
            "SELECT 1",
            gate.to_str().ok_or("invalid gate path")?,
        ],
    ))?;
    world.write_config(&format!(
        r#"schema=2
[test.database]
timeout_seconds=120
[test.database.postgres]
image="postgres:16-alpine"
[[test.database.check]]
name="database-user"
tier="release"
command={command}
timeout_seconds=90
"#
    ))?;
    world.git(&["add", "."])?;
    world.git(&["commit", "-qm", "shared fixture ownership acceptance"])?;
    let mut previous = None;
    let mut first_request: Option<Value> = None;
    for scenario in ["normal", "legacy", "name-reuse", "restart"] {
        let started = world.start_fresh(
            json!({"path":world.repo,"test":"database"}),
            previous.as_deref(),
        )?;
        let run_id = test_run_id(&started)?.to_owned();
        let mut socket = await_database_case(&listener)?;
        let receipt_path = world
            .repo
            .join(".devcoordinator/test/containers/pending")
            .join(&run_id)
            .join("container-lifecycle.json");
        let records: Value = serde_json::from_slice(
            &fs::read(&receipt_path).map_err(|_| "fixture ownership receipt missing")?,
        )
        .map_err(|_| "fixture ownership receipt invalid")?;
        let records = records
            .as_array()
            .ok_or("fixture records must be an array")?;
        ensure!(
            records.len() == 1,
            "shared fixture did not retain exactly one owned container"
        );
        let record = &records[0];
        let id = record["container_id"]
            .as_str()
            .ok_or("fixture exact ID missing")?
            .to_owned();
        let name = record["name"]
            .as_str()
            .ok_or("fixture name missing")?
            .to_owned();
        let request =
            json!({"path":world.repo,"run_id":run_id,"check":record["check"],"container_id":id});
        if let Some(prior) = &first_request {
            let old = world.call("test.container.inspect", prior.clone())?;
            ensure!(
                data(&old)?["record"]["cleanup_status"] == "cleanup_complete",
                "replacement lost the earlier run's ownership receipt"
            );
        }
        if scenario == "legacy" {
            // Recreate the prior persisted contract: containers.json contains
            // the exact ID, but no typed receipt exists. Labels alone are not
            // sufficient for adoption.
            fs::remove_file(&receipt_path).map_err(|_| "cannot prepare legacy fixture receipt")?;
        }
        let live = world.call("test.container.inspect", request.clone())?;
        ensure!(
            data(&live)?["inspection"]["container_id"] == id,
            "supported inspection did not find the shared fixture"
        );
        let refused = world.call("test.container.retry", request.clone())?;
        ensure!(
            refused["ok"] == false,
            "retry deleted a container with an active fixture lease"
        );
        let mut replacement = None;
        if scenario == "name-reuse" {
            run_status("docker", &["rm", "-f", &id])?;
            let unrelated = command_stdout(
                "docker",
                &[
                    "create",
                    "--name",
                    &name,
                    "--label",
                    &format!("devcoordinator2.instance={}", world.unit_prefix),
                    "--label",
                    "devcoordinator2.purpose=test",
                    "postgres:16-alpine",
                ],
            )?;
            let unrelated = unrelated.trim().to_owned();
            ensure!(
                unrelated.len() == 64 && unrelated != id,
                "name replacement must have its own exact identity"
            );
            replacement = Some(unrelated);
        }
        let finished = if scenario == "restart" {
            world.stop_daemon(true)?;
            world.start_daemon(None, None, None)?;
            world.wait_run_status(&started, &["interrupted"], Duration::from_secs(30))?
        } else {
            socket.write_all(b"1").map_err(|error| error.to_string())?;
            world.wait_run_status(&started, &["passed", "failed"], Duration::from_secs(30))?
        };
        let observed = world.call("test.container.inspect", request.clone())?;
        let observed = data(&observed)?;
        ensure!(
            observed.get("inspection").is_none_or(Value::is_null),
            "removed exact fixture still has a live inspection"
        );
        if let Some(replacement) = replacement {
            ensure!(
                finished["status"] == "failed"
                    && observed["record"]["cleanup_status"] != "cleanup_complete",
                "reused name falsely completed cleanup"
            );
            let failed_retry = world.call("test.container.retry", request.clone())?;
            ensure!(
                data(&failed_retry)?["record"]["cleanup_status"] != "cleanup_complete",
                "retry ignored the reused name"
            );
            command_stdout("docker", &["inspect", "--format", "{{.Id}}", &replacement])?;
            run_status("docker", &["rm", "-f", &replacement])?;
        } else {
            ensure!(
                observed["record"]["cleanup_status"] == "cleanup_complete",
                "fixture cleanup did not retain exact absence proof"
            );
            ensure!(
                finished["status"]
                    == if scenario == "restart" {
                        "interrupted"
                    } else {
                        "passed"
                    },
                "normal fixture result changed during cleanup"
            );
        }
        let retry = world.call("test.container.retry", request.clone())?;
        ensure!(
            data(&retry)?["record"]["cleanup_status"] == "cleanup_complete",
            "retained fixture cleanup could not be retried"
        );
        let repeated = world.call("test.container.retry", request.clone())?;
        ensure!(
            data(&repeated)?["record"] == data(&retry)?["record"],
            "repeating successful cleanup changed its durable receipt"
        );
        if first_request.is_none() {
            first_request = Some(request);
        }
        previous = Some(run_id);
        drop(socket);
    }
    Ok(())
}

pub(super) fn assert_live_template(world: &World, id: &str) -> Result<Value, String> {
    let labels: Value = serde_json::from_str(&command_stdout(
        "docker",
        &["inspect", "--format", "{{json .Config.Labels}}", id],
    )?)
    .map_err(|_| "template ownership labels invalid")?;
    let request = json!({"path":world.repo,"run_id":labels["devcoordinator2.run"],"check":labels["devcoordinator2.check"],"container_id":id});
    let inspected = world.call("test.container.inspect", request.clone())?;
    ensure!(
        data(&inspected)?["record"]["container_id"] == id,
        "template lacks supported durable ownership"
    );
    let refused = world.call("test.container.retry", request.clone())?;
    ensure!(
        refused["ok"] == false,
        "retry removed a template still owned by the pool"
    );
    Ok(request)
}

pub(super) fn retain_start_response(
    world: &World,
    stage: &str,
    response: &Value,
) -> Result<(), String> {
    ensure!(
        stage
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "invalid database start evidence stage"
    );
    let run_id = test_run_id(response)?;
    let root = world.harness.work_root.join("evidence");
    match fs::create_dir(&root) {
        Ok(()) => fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(&root).map_err(|error| error.to_string())?;
            ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "database evidence directory changed"
            );
        }
        Err(error) => return Err(error.to_string()),
    }
    let case = world
        .base
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("invalid database case identity")?;
    let payload = serde_json::to_vec_pretty(
        &json!({"schema":1,"stage":stage,"run_id":run_id,"response":response}),
    )
    .map_err(|error| error.to_string())?;
    ensure!(
        payload.len() <= 64 * 1024,
        "database start evidence exceeded its bound"
    );
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(root.join(format!("{case}-{stage}.json")))
        .map_err(|error| error.to_string())?;
    file.write_all(&payload)
        .and_then(|()| file.sync_all())
        .map_err(|error| error.to_string())
}
