//! Real systemd/PG cutover using only synthetic, marker-owned authority.
//! This admission proof is deliberately invalid for ordinary activation.
use super::*;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

#[path = "rehearsal/host.rs"]
mod host;
const KIND: &str = "devcoordinator2-isolated-installer-rehearsal";
const PREFIX: &str = "devcoordinator2-rustint-installer-";

#[derive(Clone, Debug)]
pub struct Config {
    pub candidate_manifest: PathBuf,
    pub prior_manifest: PathBuf,
    pub work_root: PathBuf,
    pub report: PathBuf,
    pub postgres_bin: PathBuf,
    pub postgres_account: String,
    pub caller_uid: u32,
    pub caller_gid: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Marker {
    schema: u8,
    kind: String,
    fixture_only: bool,
    root: PathBuf,
    namespace: String,
    source_commit: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub schema: u8,
    pub fixture_only: bool,
    pub status: String,
    pub source_commit: String,
    pub prior_source_commit: String,
    pub namespace: String,
    pub checks: Vec<String>,
    pub cleanup_complete: bool,
    pub error: Option<String>,
    pub report_path: PathBuf,
}

fn exact(path: &Path) -> bool {
    path.is_absolute()
        && !path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
}
fn contained(path: &Path, root: &Path) -> bool {
    if !exact(path) || !path.starts_with(root) || path == root {
        return false;
    }
    for ancestor in path.ancestors() {
        match ancestor.symlink_metadata() {
            Ok(metadata) => {
                return !metadata.file_type().is_symlink()
                    && ancestor
                        .canonicalize()
                        .is_ok_and(|physical| physical == ancestor && physical.starts_with(root));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return false,
        }
    }
    false
}
fn marker(root: &Path, owner: u32) -> Result<Marker, String> {
    if !exact(root)
        || root.parent() != Some(Path::new("/var/lib"))
        || !root
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(PREFIX))
        || root
            .canonicalize()
            .map_err(|_| "fixture root is unavailable")?
            != root
    {
        return Err("installer rehearsal requires its exact /var/lib namespace root".into());
    }
    let metadata = root
        .symlink_metadata()
        .map_err(|_| "fixture root ownership is unavailable")?;
    if metadata.uid() != owner || metadata.mode() & 0o022 != 0 {
        return Err("fixture root is not privately installation-owned".into());
    }
    let marker: Marker =
        serde_json::from_value(finalize::private(&root.join("ownership.json"), owner)?)
            .map_err(|_| "installer fixture ownership marker is invalid")?;
    if marker.schema != 1
        || marker.kind != KIND
        || !marker.fixture_only
        || marker.root != root
        || !marker.namespace.starts_with(PREFIX)
        || !marker
            .namespace
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("installer fixture ownership is unverified".into());
    }
    Ok(marker)
}

pub(super) fn service_root(config: &HostCutoverConfig) -> Result<PathBuf, String> {
    let root = config
        .database_path
        .parent()
        .and_then(Path::parent)
        .ok_or("fixture service root is unavailable")?;
    let owned = marker(root, 0)?;
    if config.database_path != root.join("state/authority.sqlite3")
        || config.runtime_dir != root.join("run")
        || config.instance_env != root.join("etc/instance.env")
        || !config
            .daemon_unit
            .starts_with(&format!("{}-", owned.namespace))
        || !config
            .edge_unit
            .starts_with(&format!("{}-", owned.namespace))
    {
        return Err("ephemeral services require the marker-bound fixture layout".into());
    }
    Ok(root.to_path_buf())
}

pub(super) fn verify_fixture_inputs(
    config: &HostCutoverConfig,
    candidate: &InstallManifest,
    owner: u32,
) -> Result<String, String> {
    let proof = finalize::private(&config.candidate_acceptance, owner)?;
    let root = PathBuf::from(
        proof["work_root"]
            .as_str()
            .ok_or("fixture proof has no owned root")?,
    );
    let owned = marker(&root, owner)?;
    if proof["schema"] != 1
        || proof["fixture_only"] != true
        || proof["scope"] != KIND
        || proof["result"] != "inputs_verified"
        || proof["source_commit"] != candidate.source_commit
        || owned.source_commit != candidate.source_commit
        || !config.ephemeral_services
        || !config.canary
    {
        return Err("test-only installer inputs are not a release acceptance receipt".into());
    }
    let hashes = proof["binaries"]
        .as_object()
        .ok_or("fixture proof has no exact binaries")?;
    if hashes.len() != candidate.binaries.len()
        || candidate.binaries.iter().any(|binary| {
            hashes.get(&binary.name).and_then(Value::as_str) != Some(binary.sha256.as_str())
        })
    {
        return Err("installer fixture binaries changed".into());
    }
    for path in [
        &config.candidate_manifest,
        &config.candidate_acceptance,
        &config.installed_manifest,
        &config.source_cache_dir,
        &config.tmpfiles_path,
        &config.transaction_dir,
        &config.runtime_dir,
        &config.socket_path,
        &config.database_path,
        &config.edge_env,
        &config.cli_link,
        &config.tooling_link,
        &config.instance_env,
        &config.edge_state_dir,
        &config.postgres.data_dir,
        &config.postgres.socket_dir,
        &config.postgres.config_path,
    ] {
        if !contained(path, &root) {
            return Err("fixture cannot select installed or shared state".into());
        }
    }
    for (path, name) in [
        (&config.daemon_unit_path, &config.daemon_unit),
        (&config.edge_unit_path, &config.edge_unit),
        (&config.postgres.unit_path, &config.postgres.unit),
    ] {
        if !name.starts_with(&format!("{}-", owned.namespace))
            || path != &Path::new("/run/systemd/system").join(name)
        {
            return Err("fixture cannot select installed or shared units".into());
        }
    }
    if config.tests_slice_unit_path
        != Path::new("/run/systemd/system").join(format!("{}-tests.slice", owned.namespace))
    {
        return Err("fixture test slice is outside its namespace".into());
    }
    let values = crate::instance::parse_env(&config.instance_env);
    let bridge = root.join("bridge");
    let bindings: [(&str, &Path); 4] = [
        ("DEVCOORDINATOR2_SOCKET", &config.socket_path),
        (
            "DEVCOORDINATOR2_STATE_DIR",
            config.database_path.parent().unwrap(),
        ),
        ("DEVCOORDINATOR2_SANDBOX_BRIDGE_DIR", &bridge),
        (
            "DEVCOORDINATOR2_DATABASE_CONFIG",
            &config.postgres.config_path,
        ),
    ];
    for (key, path) in bindings {
        if values.get(key).map(PathBuf::from).as_deref() != Some(path) {
            return Err("fixture daemon environment selects another authority".into());
        }
    }
    if values.get("DEVCOORDINATOR2_UNIT_PREFIX") != Some(&format!("{}-test", owned.namespace)) {
        return Err("fixture runtime ownership prefix is unverified".into());
    }
    install::hash_file(&config.candidate_acceptance)
}

fn envelope(protocol: u8, operation: &str, params: Value) -> Value {
    serde_json::json!({"protocol":protocol,"id":"installer-rehearsal","client":{"kind":"other"},"operation":operation,"params":params})
}
fn request(socket: &Path, value: &Value) -> Result<Value, String> {
    let mut stream =
        UnixStream::connect(socket).map_err(|_| "fixture daemon connection is unavailable")?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|_| "cannot bound fixture response")?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| "cannot bound fixture request")?;
    let mut encoded = serde_json::to_vec(value).map_err(|_| "cannot encode fixture request")?;
    encoded.push(b'\n');
    stream
        .write_all(&encoded)
        .map_err(|_| "fixture request failed")?;
    let mut response = Vec::new();
    let mut reader = std::io::BufReader::new(stream).take(2 * 1024 * 1024 + 1);
    std::io::BufRead::read_until(&mut reader, b'\n', &mut response)
        .map_err(|_| "fixture response failed")?;
    if response.len() > 2 * 1024 * 1024 {
        return Err("fixture response exceeded its bound".into());
    }
    serde_json::from_slice(&response).map_err(|_| "fixture response is invalid".into())
}
fn data(value: Value) -> Result<Value, String> {
    if value["ok"] == true {
        Ok(value["data"].clone())
    } else {
        Err(format!(
            "synthetic fixture operation refused ({})",
            value["error"]["code"].as_str().unwrap_or("unknown")
        ))
    }
}
pub fn request_stdin(socket: &Path) -> Result<Value, String> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "fixture client input is unavailable")?;
    if bytes.len() > 1024 * 1024 {
        return Err("fixture client input exceeds its bound".into());
    }
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| "fixture client input is invalid")?;
    request(socket, &value)
}

pub fn run(config: Config) -> Result<Report, String> {
    if rustix::process::geteuid().as_raw() != 0 || config.caller_uid == 0 || config.caller_gid == 0
    {
        return Err(
            "installer rehearsal requires root with explicit non-root fixture ownership".into(),
        );
    }
    if config.report.symlink_metadata().is_ok()
        || !exact(&config.report)
        || !exact(&config.work_root)
        || !(config.report.starts_with("/tmp") || config.report.starts_with("/var/tmp"))
    {
        return Err("rehearsal requires a new report and exact new fixture root".into());
    }
    let candidate =
        install::read_and_verify_manifest(&config.candidate_manifest, &install::HostRunner)?;
    if candidate.source_commit != devcoordinator2_control::SOURCE_COMMIT {
        return Err("rehearsal executable must be built from the exact candidate source".into());
    }
    let prior = install::read_and_verify_prior_manifest_owned(
        &config.prior_manifest,
        &install::HostRunner,
        0,
    )?;
    if prior.binaries.len() != 3 {
        return Err("the one-time SQLite rehearsal requires the explicit prior schema-34/v2 three-binary bundle".into());
    }
    if candidate.source_root != prior.source_root {
        return Err(
            "rehearsal source identities belong to different canonical repositories".into(),
        );
    }
    let mut fixture = host::Fixture::prepare(&config, &candidate, &prior)?;
    let mut report = Report {
        schema: 1,
        fixture_only: true,
        status: "failed".into(),
        source_commit: candidate.source_commit.clone(),
        prior_source_commit: prior.source_commit.clone(),
        namespace: fixture.marker.namespace.clone(),
        checks: Vec::new(),
        cleanup_complete: false,
        error: None,
        report_path: config.report.clone(),
    };
    let result = fixture.exercise(&mut report);
    let cleanup = fixture.cleanup();
    report.cleanup_complete = cleanup.is_ok();
    match (result, cleanup) {
        (Ok(()), Ok(())) => report.status = "passed".into(),
        (result, cleanup) => {
            report.error = Some(format!(
                "{}{}",
                result.err().unwrap_or_default(),
                cleanup
                    .err()
                    .map(|error| format!("; cleanup: {error}"))
                    .unwrap_or_default()
            ))
        }
    }
    install::atomic_file(
        &config.report,
        &serde_json::to_vec_pretty(&report).map_err(|_| "cannot retain fixture report")?,
        0o600,
        Some((0, 0)),
    )?;
    if report.status != "passed" {
        return Err(format!(
            "isolated installer rehearsal failed; private report retained at {}",
            config.report.display()
        ));
    }
    Ok(report)
}
