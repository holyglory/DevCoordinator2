//! Commit-stamped release build and installation planning.
//!
//! The canonical checkout remains the only source and binary tree. This
//! module proves a clean `main`, builds all release executables as the
//! checkout owner, verifies their embedded commit, records hashes in one
//! private manifest, and renders direct unit/link targets. Host mutation is
//! kept behind explicit command and filesystem boundaries for cutover tests.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use rustix::fs::{Mode, OFlags, open as unix_open};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const OUTPUT_CAP: usize = 64 * 1024;
const TEMPLATE_CAP: u64 = 256 * 1024;
const MANIFEST_SCHEMA: u8 = 1;
const BINARY_NAMES: &[&str] = &[
    "devcoordinator2",
    "devcoordinator2-executor",
    "devcoordinator2-tooling",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandRequest {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub environment: BTreeMap<OsString, OsString>,
    pub clear_environment: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

pub trait CommandRunner: Send + Sync {
    fn run(&self, request: &CommandRequest) -> Result<CommandOutput, String>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HostRunner;

impl CommandRunner for HostRunner {
    fn run(&self, request: &CommandRequest) -> Result<CommandOutput, String> {
        let mut command = Command::new(&request.program);
        command
            .args(&request.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if request.clear_environment {
            command.env_clear();
        }
        command.envs(&request.environment);
        let output = command
            .output()
            .map_err(|error| format!("cannot run {}: {error}", request.program.display()))?;
        Ok(CommandOutput {
            success: output.status.success(),
            stdout: bounded_output(output.stdout),
            stderr: bounded_output(output.stderr),
        })
    }
}

#[derive(Clone, Debug)]
pub struct BuildTools {
    pub cargo: PathBuf,
    pub setpriv: PathBuf,
}

impl Default for BuildTools {
    fn default() -> Self {
        Self {
            cargo: PathBuf::from("/usr/bin/cargo"),
            setpriv: PathBuf::from("/usr/bin/setpriv"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct BuildIdentity {
    pub uid: u32,
    pub gid: u32,
    pub home: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryReceipt {
    pub name: String,
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
    pub source_commit: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallManifest {
    pub schema: u8,
    pub product: String,
    pub package_version: String,
    pub source_root: String,
    pub source_commit: String,
    pub built_at: String,
    pub binaries: Vec<BinaryReceipt>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallationPlan {
    pub source_root: String,
    pub source_commit: String,
    pub manifest_path: String,
    pub daemon_unit_path: String,
    pub edge_unit_path: String,
    pub binary_links: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComposeAuthorization {
    pub repository_id: String,
    pub path: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexUsageSource {
    pub uid: u32,
    pub codex_home: String,
    pub executable: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalAccount {
    pub uid: u32,
    pub gid: u32,
    pub home: PathBuf,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ComposePolicy {
    schema: u8,
    authorizations: Vec<ComposeAuthorization>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UsagePolicy {
    schema: u8,
    sources: Vec<CodexUsageSource>,
}

pub fn validate_live_checkout<R: CommandRunner>(
    root: &Path,
    fetch: bool,
    runner: &R,
) -> Result<String, String> {
    let lexical = std::path::absolute(root)
        .map_err(|error| format!("cannot make live checkout absolute: {error}"))?;
    let canonical = lexical
        .canonicalize()
        .map_err(|error| format!("cannot resolve live checkout: {error}"))?;
    if lexical != canonical
        || lexical
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(true)
    {
        return Err(format!(
            "live checkout must be a real absolute directory: {}",
            lexical.display()
        ));
    }
    let top = git(&canonical, ["rev-parse", "--show-toplevel"], runner)?;
    let top = PathBuf::from(top.trim())
        .canonicalize()
        .map_err(|error| format!("cannot resolve Git worktree root: {error}"))?;
    if top != canonical {
        return Err("live checkout must be the Git worktree root".to_owned());
    }
    if fetch {
        git(&canonical, ["fetch", "origin", "main"], runner)?;
    }
    let branch = git(&canonical, ["branch", "--show-current"], runner)?;
    if branch.trim() != "main" {
        return Err(format!(
            "live checkout must be on main, found {}",
            if branch.trim().is_empty() {
                "detached HEAD"
            } else {
                branch.trim()
            }
        ));
    }
    let status = git(
        &canonical,
        ["status", "--porcelain", "--untracked-files=all"],
        runner,
    )?;
    if !status.trim().is_empty() {
        return Err("live checkout must be clean".to_owned());
    }
    let head = git(&canonical, ["rev-parse", "HEAD"], runner)?;
    let upstream = git(
        &canonical,
        ["rev-parse", "refs/remotes/origin/main"],
        runner,
    )?;
    let head = head.trim();
    if head != upstream.trim() {
        return Err("live checkout must exactly match fetched origin/main".to_owned());
    }
    validate_commit(head)?;
    Ok(head.to_owned())
}

pub fn checkout_build_identity(source_root: &Path) -> Result<BuildIdentity, String> {
    let manifest = source_root.join("Cargo.toml");
    let metadata = manifest
        .symlink_metadata()
        .map_err(|_| "Rust Cargo manifest has no valid non-root owner".to_owned())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.uid() == 0 {
        return Err("Rust Cargo manifest must be owned by a usable non-root writer".to_owned());
    }
    let (gid, home) = passwd_identity(metadata.uid())?;
    if !home.is_dir() {
        return Err("Rust Cargo manifest must be owned by a usable non-root writer".to_owned());
    }
    Ok(BuildIdentity {
        uid: metadata.uid(),
        gid,
        home,
    })
}

pub fn build_release_binaries<R: CommandRunner>(
    source_root: &Path,
    source_commit: &str,
    identity: &BuildIdentity,
    tools: &BuildTools,
    runner: &R,
) -> Result<Vec<BinaryReceipt>, String> {
    validate_commit(source_commit)?;
    validate_tool(&tools.cargo, "Cargo")?;
    validate_tool(&tools.setpriv, "setpriv")?;
    let manifest = source_root.join("Cargo.toml");
    validate_regular(&manifest, false)
        .map_err(|_| "Rust Cargo manifest is unavailable".to_owned())?;
    let request = CommandRequest {
        program: tools.setpriv.clone(),
        args: vec![
            format!("--reuid={}", identity.uid).into(),
            format!("--regid={}", identity.gid).into(),
            "--init-groups".into(),
            "--reset-env".into(),
            "--".into(),
            tools.cargo.as_os_str().to_owned(),
            "build".into(),
            "--locked".into(),
            "--release".into(),
            "--manifest-path".into(),
            manifest.as_os_str().to_owned(),
            "--workspace".into(),
        ],
        environment: BTreeMap::from([
            (OsString::from("HOME"), identity.home.as_os_str().to_owned()),
            (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
            (
                OsString::from("DEVCOORDINATOR2_SOURCE_COMMIT"),
                OsString::from(source_commit),
            ),
            (
                OsString::from("CARGO_TARGET_DIR"),
                source_root.join("target").into_os_string(),
            ),
        ]),
        clear_environment: true,
    };
    let output = runner.run(&request)?;
    if !output.success {
        return Err(format!(
            "Rust release build failed: {}",
            useful_failure(&output)
        ));
    }
    verify_release_binaries(source_root, source_commit, runner)
}

pub fn verify_release_binaries<R: CommandRunner>(
    source_root: &Path,
    source_commit: &str,
    runner: &R,
) -> Result<Vec<BinaryReceipt>, String> {
    let mut receipts = Vec::new();
    for name in BINARY_NAMES {
        let path = source_root.join("target/release").join(name);
        let metadata = validate_regular(&path, true).map_err(|error| {
            format!("Rust release binary {name} is unavailable or unsafe: {error}")
        })?;
        let output = runner.run(&CommandRequest {
            program: path.clone(),
            args: vec!["--source-commit".into()],
            environment: BTreeMap::new(),
            clear_environment: true,
        })?;
        if !output.success || output.stdout.trim() != source_commit {
            return Err(format!(
                "Rust release binary {name} does not embed source commit {source_commit}"
            ));
        }
        receipts.push(BinaryReceipt {
            name: (*name).to_owned(),
            path: path_text(&path)?,
            sha256: hash_file(&path)?,
            bytes: metadata.len(),
            source_commit: source_commit.to_owned(),
        });
    }
    Ok(receipts)
}

pub fn manifest(
    source_root: &Path,
    source_commit: &str,
    built_at: &str,
    binaries: Vec<BinaryReceipt>,
) -> Result<InstallManifest, String> {
    if binaries.len() != BINARY_NAMES.len()
        || BINARY_NAMES
            .iter()
            .any(|name| !binaries.iter().any(|binary| binary.name == *name))
    {
        return Err("installation manifest requires all release binaries".to_owned());
    }
    Ok(InstallManifest {
        schema: MANIFEST_SCHEMA,
        product: "devcoordinator2".to_owned(),
        package_version: env!("CARGO_PKG_VERSION").to_owned(),
        source_root: path_text(source_root)?,
        source_commit: source_commit.to_owned(),
        built_at: built_at.to_owned(),
        binaries,
    })
}

pub fn write_manifest(
    path: &Path,
    manifest: &InstallManifest,
    owner: (u32, u32),
) -> Result<(), String> {
    let mut payload = serde_json::to_vec_pretty(manifest)
        .map_err(|error| format!("cannot encode installation manifest: {error}"))?;
    payload.push(b'\n');
    atomic_file(path, &payload, 0o600, Some(owner))
}

pub fn read_and_verify_manifest<R: CommandRunner>(
    path: &Path,
    runner: &R,
) -> Result<InstallManifest, String> {
    read_and_verify_manifest_owned(path, runner, 0)
}

pub fn read_and_verify_manifest_owned<R: CommandRunner>(
    path: &Path,
    runner: &R,
    expected_uid: u32,
) -> Result<InstallManifest, String> {
    let file = unix_open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|error| format!("cannot open installation manifest: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect installation manifest: {error}"))?;
    if !metadata.is_file()
        || metadata.len() > 256 * 1024
        || metadata.mode() & 0o077 != 0
        || metadata.uid() != expected_uid
    {
        return Err(
            "installation manifest must be a root-owned private bounded regular file".to_owned(),
        );
    }
    let mut raw = Vec::new();
    file.take(256 * 1024 + 1)
        .read_to_end(&mut raw)
        .map_err(|error| format!("cannot read installation manifest: {error}"))?;
    let manifest: InstallManifest = serde_json::from_slice(&raw)
        .map_err(|error| format!("installation manifest is invalid: {error}"))?;
    if manifest.schema != MANIFEST_SCHEMA
        || manifest.product != "devcoordinator2"
        || manifest.package_version != env!("CARGO_PKG_VERSION")
    {
        return Err("installation manifest identity is unsupported".to_owned());
    }
    let verified = verify_release_binaries(
        Path::new(&manifest.source_root),
        &manifest.source_commit,
        runner,
    )?;
    if verified != manifest.binaries {
        return Err("installed release binaries no longer match their manifest".to_owned());
    }
    Ok(manifest)
}

pub fn installation_plan(
    manifest: &InstallManifest,
    manifest_path: &Path,
) -> Result<InstallationPlan, String> {
    let source_root = Path::new(&manifest.source_root);
    let links = BTreeMap::from([
        (
            "/usr/local/bin/devcoordinator2".to_owned(),
            path_text(&source_root.join("target/release/devcoordinator2"))?,
        ),
        (
            "/usr/local/bin/devcoordinator2-tooling".to_owned(),
            path_text(&source_root.join("target/release/devcoordinator2-tooling"))?,
        ),
    ]);
    Ok(InstallationPlan {
        source_root: manifest.source_root.clone(),
        source_commit: manifest.source_commit.clone(),
        manifest_path: path_text(manifest_path)?,
        daemon_unit_path: "/etc/systemd/system/devcoordinator2.service".to_owned(),
        edge_unit_path: "/etc/systemd/system/devcoordinator2-edge.service".to_owned(),
        binary_links: links,
    })
}

pub fn render_daemon_unit(source_root: &Path) -> Result<String, String> {
    let template = read_template(&source_root.join("deploy/devcoordinator2.service"))?;
    let binary = path_text(&source_root.join("target/release/devcoordinator2"))?;
    let rendered = template.replace(
        "/home/DevCoordinator2/target/release/devcoordinator2",
        &binary,
    );
    if rendered.contains("python") || !rendered.contains(&format!("ExecStart={binary} daemon")) {
        return Err("daemon unit does not select the verified Rust binary".to_owned());
    }
    Ok(rendered)
}

pub fn render_edge_unit(source_root: &Path, canary: bool) -> Result<String, String> {
    let template = read_template(&source_root.join("deploy/devcoordinator2-edge.service"))?;
    let script = path_text(&source_root.join("edge/devcoordinator2-edge.mjs"))?;
    let mut rendered = template.replace(
        "/home/DevCoordinator2/edge/devcoordinator2-edge.mjs",
        &script,
    );
    if canary {
        rendered = rendered
            .lines()
            .filter(|line| {
                ![
                    "LoadCredential=tls",
                    "LoadCredential=oidc",
                    "Environment=EDGE_TLS_CERT",
                    "Environment=EDGE_OIDC_CLIENT",
                    "AmbientCapabilities",
                    "CapabilityBoundingSet",
                ]
                .iter()
                .any(|prefix| line.starts_with(prefix))
            })
            .collect::<Vec<_>>()
            .join("\n");
        rendered.push('\n');
    }
    if !rendered.contains(&format!("ExecStart=/usr/bin/node {script}")) {
        return Err("edge unit does not select the canonical Node source".to_owned());
    }
    Ok(rendered)
}

pub fn replace_direct_link(destination: &Path, source: &Path) -> Result<(), String> {
    if destination
        .symlink_metadata()
        .is_ok_and(|metadata| !metadata.file_type().is_symlink())
    {
        return Err(format!(
            "refusing to replace non-symlink managed path: {}",
            destination.display()
        ));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| "managed link has no parent".to_owned())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create managed link directory: {error}"))?;
    let name = destination
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| "managed link name is invalid".to_owned())?;
    let temporary = parent.join(format!(".{name}.tmp"));
    if let Ok(metadata) = temporary.symlink_metadata() {
        if !metadata.file_type().is_symlink() {
            return Err("refusing to replace non-symlink staging path".to_owned());
        }
        std::fs::remove_file(&temporary)
            .map_err(|error| format!("cannot clear managed link staging path: {error}"))?;
    }
    symlink(source, &temporary).map_err(|error| format!("cannot create managed link: {error}"))?;
    std::fs::rename(&temporary, destination)
        .map_err(|error| format!("cannot activate managed link: {error}"))?;
    Ok(())
}

pub fn write_if_absent(
    path: &Path,
    content: &str,
    mode: u32,
    owner: (u32, u32),
) -> Result<bool, String> {
    match path.symlink_metadata() {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err("refusing non-regular existing installation target".to_owned())
        }
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            atomic_file(path, content.as_bytes(), mode, Some(owner))?;
            Ok(true)
        }
        Err(error) => Err(format!("cannot inspect installation target: {error}")),
    }
}

pub fn ensure_env_value(path: &Path, key: &str, value: &str) -> Result<bool, String> {
    validate_env_pair(key, value)?;
    let (text, metadata) = read_optional_regular(path)?;
    let prefix = format!("{key}=");
    let matches = text
        .lines()
        .filter(|line| line.trim().starts_with(&prefix))
        .collect::<Vec<_>>();
    let desired = format!("{key}={value}");
    if !matches.is_empty() {
        if matches != [desired.as_str()] {
            return Err(format!("{key} already has a different installed value"));
        }
        return Ok(false);
    }
    let suffix = if text.is_empty() || text.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let updated = format!("{text}{suffix}{desired}\n");
    let mode = metadata
        .as_ref()
        .map_or(0o640, |metadata| metadata.mode() & 0o777);
    let owner = metadata
        .as_ref()
        .map(|metadata| (metadata.uid(), metadata.gid()));
    atomic_file(path, updated.as_bytes(), mode, owner)?;
    Ok(true)
}

pub fn set_env_value(path: &Path, key: &str, value: &str) -> Result<bool, String> {
    validate_env_pair(key, value)?;
    let (text, metadata) = read_optional_regular(path)?;
    let prefix = format!("{key}=");
    let mut lines = text.lines().map(str::to_owned).collect::<Vec<_>>();
    let indexes = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim().starts_with(&prefix))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if indexes.len() > 1 {
        return Err(format!("{key} appears more than once"));
    }
    let desired = format!("{key}={value}");
    if indexes
        .first()
        .is_some_and(|index| lines[*index] == desired)
    {
        return Ok(false);
    }
    if let Some(index) = indexes.first() {
        lines[*index] = desired;
    } else {
        lines.push(desired);
    }
    let updated = format!("{}\n", lines.join("\n"));
    let mode = metadata
        .as_ref()
        .map_or(0o640, |metadata| metadata.mode() & 0o777);
    let owner = metadata
        .as_ref()
        .map(|metadata| (metadata.uid(), metadata.gid()));
    atomic_file(path, updated.as_bytes(), mode, owner)?;
    Ok(true)
}

pub fn compose_env_authorizations<R: CommandRunner>(
    specifications: &[String],
    runner: &R,
) -> Result<Vec<ComposeAuthorization>, String> {
    let mut entries = BTreeSet::new();
    for specification in specifications {
        let (repository_text, relative) = specification.split_once('=').ok_or_else(|| {
            "--compose-env-authorization must be REPOSITORY=RELATIVE_PATH".to_owned()
        })?;
        if repository_text.is_empty() || relative.is_empty() {
            return Err("--compose-env-authorization must be REPOSITORY=RELATIVE_PATH".to_owned());
        }
        validate_relative_file(relative)?;
        let repository = Path::new(repository_text)
            .canonicalize()
            .map_err(|error| format!("cannot resolve Compose repository: {error}"))?;
        let identity = git(
            &repository,
            [
                "rev-parse",
                "--path-format=absolute",
                "--show-toplevel",
                "--git-common-dir",
            ],
            runner,
        )?;
        let lines = identity.lines().collect::<Vec<_>>();
        if lines.len() != 2 || Path::new(lines[1]).file_name() != Some(OsStr::new(".git")) {
            return Err("unsupported repository layout".to_owned());
        }
        let worktree = Path::new(lines[0])
            .canonicalize()
            .map_err(|error| format!("cannot resolve worktree: {error}"))?;
        let common_root = Path::new(lines[1])
            .canonicalize()
            .map_err(|error| format!("cannot resolve Git common directory: {error}"))?
            .parent()
            .ok_or_else(|| "Git common directory has no repository root".to_owned())?
            .to_owned();
        let candidate = worktree.join(relative);
        let metadata = candidate
            .symlink_metadata()
            .map_err(|error| format!("cannot inspect Compose environment file: {error}"))?;
        let resolved = candidate
            .canonicalize()
            .map_err(|error| format!("cannot resolve Compose environment file: {error}"))?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || (resolved != worktree && !resolved.starts_with(&worktree))
        {
            return Err("unsafe Compose environment file".to_owned());
        }
        let ignored = run_git_raw(
            &worktree,
            ["check-ignore", "--quiet", "--", relative],
            runner,
        )?;
        if !ignored.success {
            return Err("Compose environment file must be ignored".to_owned());
        }
        let mut digest = Sha256::new();
        digest.update(b"devcoordinator2.repository\0");
        digest.update(common_root.as_os_str().as_encoded_bytes());
        let entry = ComposeAuthorization {
            repository_id: format!("r{}", &hex(&digest.finalize())[..16]),
            path: relative.to_owned(),
        };
        if !entries.insert(entry) {
            return Err("duplicate Compose environment authorization".to_owned());
        }
    }
    Ok(entries.into_iter().collect())
}

pub fn merge_compose_env_allowlist(
    path: &Path,
    additions: &[ComposeAuthorization],
    owner: (u32, u32),
) -> Result<bool, String> {
    let existing = match read_optional_regular(path)? {
        (text, Some(_)) => {
            let policy: ComposePolicy = serde_json::from_str(&text).map_err(|error| {
                format!("cannot preserve Compose environment allowlist: {error}")
            })?;
            if policy.schema != 1 {
                return Err(
                    "cannot preserve Compose environment allowlist: wrong schema".to_owned(),
                );
            }
            policy.authorizations
        }
        (_, None) => Vec::new(),
    };
    let merged = existing
        .into_iter()
        .chain(additions.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let policy = ComposePolicy {
        schema: 1,
        authorizations: merged,
    };
    merge_policy(path, &policy, 0o640, owner)
}

pub fn codex_usage_sources<F>(
    accounts: &[String],
    mut resolve: F,
) -> Result<Vec<CodexUsageSource>, String>
where
    F: FnMut(&str) -> Result<LocalAccount, String>,
{
    let mut sources = BTreeSet::new();
    for name in accounts {
        let account = resolve(name)?;
        let codex_home = account.home.join(".codex");
        let executable = account.home.join(".local/bin/codex");
        let executable_ok = executable
            .symlink_metadata()
            .is_ok_and(|metadata| !metadata.file_type().is_symlink() && metadata.is_file());
        if !codex_home.is_dir() || !executable_ok {
            return Err(format!("Codex usage source is not installed for {name}"));
        }
        sources.insert(CodexUsageSource {
            uid: account.uid,
            codex_home: path_text(&codex_home)?,
            executable: path_text(&executable)?,
        });
    }
    Ok(sources.into_iter().collect())
}

pub fn merge_codex_usage_sources(
    path: &Path,
    additions: &[CodexUsageSource],
    owner: (u32, u32),
) -> Result<bool, String> {
    let existing = match read_optional_regular(path)? {
        (text, Some(_)) => {
            let policy: UsagePolicy = serde_json::from_str(&text)
                .map_err(|error| format!("cannot preserve Codex usage source policy: {error}"))?;
            if policy.schema != 1 {
                return Err("cannot preserve Codex usage source policy: wrong schema".to_owned());
            }
            policy.sources
        }
        (_, None) => Vec::new(),
    };
    let mut by_uid = BTreeMap::new();
    for source in existing.into_iter().chain(additions.iter().cloned()) {
        by_uid.insert(source.uid, source);
    }
    let policy = UsagePolicy {
        schema: 1,
        sources: by_uid.into_values().collect(),
    };
    merge_policy(path, &policy, 0o600, owner)
}

pub fn account_by_name(name: &str) -> Result<LocalAccount, String> {
    let passwd = std::fs::read_to_string("/etc/passwd")
        .map_err(|error| format!("cannot read local account registry: {error}"))?;
    for line in passwd.lines() {
        let fields = line.split(':').collect::<Vec<_>>();
        if fields.len() >= 7 && fields[0] == name {
            return Ok(LocalAccount {
                uid: fields[2]
                    .parse()
                    .map_err(|_| format!("local account {name} has an invalid uid"))?,
                gid: fields[3]
                    .parse()
                    .map_err(|_| format!("local account {name} has an invalid gid"))?,
                home: PathBuf::from(fields[5]),
            });
        }
    }
    Err(format!("local account {name} does not exist"))
}

fn git<R, I, S>(root: &Path, args: I, runner: &R) -> Result<String, String>
where
    R: CommandRunner,
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = run_git_raw(root, args, runner)?;
    if !output.success {
        return Err(format!("Git preflight failed: {}", useful_failure(&output)));
    }
    Ok(output.stdout)
}

fn run_git_raw<R, I, S>(root: &Path, args: I, runner: &R) -> Result<CommandOutput, String>
where
    R: CommandRunner,
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command_args = vec![
        "-c".into(),
        "safe.directory=*".into(),
        "-C".into(),
        root.as_os_str().to_owned(),
    ];
    command_args.extend(args.into_iter().map(|arg| arg.as_ref().to_owned()));
    runner.run(&CommandRequest {
        program: PathBuf::from("/usr/bin/git"),
        args: command_args,
        environment: BTreeMap::from([(OsString::from("PATH"), OsString::from("/usr/bin:/bin"))]),
        clear_environment: true,
    })
}

fn validate_env_pair(key: &str, value: &str) -> Result<(), String> {
    if key.is_empty()
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        || value.contains(['\n', '\r', '\0'])
    {
        return Err("environment entry is invalid".to_owned());
    }
    Ok(())
}

fn validate_relative_file(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 512
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains(['\\', '\0'])
        || value
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(
            "Compose environment authorization path must be normalized relative".to_owned(),
        );
    }
    Ok(())
}

fn read_optional_regular(path: &Path) -> Result<(String, Option<std::fs::Metadata>), String> {
    let metadata = match path.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((String::new(), None));
        }
        Err(error) => return Err(format!("cannot inspect installed policy: {error}")),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 256 * 1024 {
        return Err("installed policy must be a bounded regular non-symlink file".to_owned());
    }
    let file = unix_open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|error| format!("cannot open installed policy: {error}"))?;
    let mut text = String::new();
    file.take(256 * 1024 + 1)
        .read_to_string(&mut text)
        .map_err(|error| format!("cannot read installed policy: {error}"))?;
    if text.len() > 256 * 1024 {
        return Err("installed policy exceeds 256 KiB".to_owned());
    }
    Ok((text, Some(metadata)))
}

fn merge_policy<T: Serialize>(
    path: &Path,
    policy: &T,
    mode: u32,
    owner: (u32, u32),
) -> Result<bool, String> {
    let mut payload = serde_json::to_vec_pretty(policy)
        .map_err(|error| format!("cannot encode installed policy: {error}"))?;
    payload.push(b'\n');
    if let (text, Some(metadata)) = read_optional_regular(path)?
        && serde_json::from_str::<Value>(&text).ok()
            == serde_json::from_slice::<Value>(&payload).ok()
    {
        let changed = metadata.uid() != owner.0
            || metadata.gid() != owner.1
            || metadata.mode() & 0o777 != mode;
        if changed {
            let file = OpenOptions::new()
                .write(true)
                .open(path)
                .map_err(|error| format!("cannot open installed policy metadata: {error}"))?;
            file.set_permissions(std::fs::Permissions::from_mode(mode))
                .map_err(|error| format!("cannot set installed policy mode: {error}"))?;
            if (metadata.uid(), metadata.gid()) != owner
                // SAFETY: `file` owns a valid descriptor for the complete call.
                && unsafe {
                    libc::fchown(
                        std::os::fd::AsRawFd::as_raw_fd(&file),
                        owner.0,
                        owner.1,
                    )
                } != 0
            {
                return Err(format!(
                    "cannot set installed policy owner: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
        return Ok(changed);
    }
    atomic_file(path, &payload, mode, Some(owner))?;
    Ok(true)
}

fn validate_tool(path: &Path, name: &str) -> Result<(), String> {
    validate_regular(path, true)
        .map(|_| ())
        .map_err(|error| format!("required {name} tool is unavailable: {error}"))
}

fn validate_regular(path: &Path, executable: bool) -> Result<std::fs::Metadata, String> {
    let metadata = path
        .symlink_metadata()
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "{} is not a regular non-symlink file",
            path.display()
        ));
    }
    if executable && metadata.mode() & 0o100 == 0 {
        return Err(format!("{} is not executable", path.display()));
    }
    Ok(metadata)
}

fn read_template(path: &Path) -> Result<String, String> {
    validate_regular(path, false)?;
    let file = unix_open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|error| format!("cannot open unit template: {error}"))?;
    let mut text = String::new();
    file.take(TEMPLATE_CAP + 1)
        .read_to_string(&mut text)
        .map_err(|error| format!("cannot read unit template: {error}"))?;
    if text.len() as u64 > TEMPLATE_CAP {
        return Err("unit template exceeds 256 KiB".to_owned());
    }
    Ok(text)
}

fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = unix_open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|error| format!("cannot open release binary for hashing: {error}"))?;
    let before = file
        .metadata()
        .map_err(|error| format!("cannot inspect release binary: {error}"))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("cannot hash release binary: {error}"))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    let after = file
        .metadata()
        .map_err(|error| format!("cannot recheck release binary: {error}"))?;
    if file_identity(&before) != file_identity(&after) {
        return Err("release binary changed while it was hashed".to_owned());
    }
    Ok(hex(&digest.finalize()))
}

fn atomic_file(
    path: &Path,
    payload: &[u8],
    mode: u32,
    owner: Option<(u32, u32)>,
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "installation target has no parent".to_owned())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create installation directory: {error}"))?;
    let mut random = [0u8; 6];
    getrandom::fill(&mut random)
        .map_err(|error| format!("cannot create installation temporary identity: {error}"))?;
    let temporary = parent.join(format!(".install-{}.tmp", hex(&random)));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("cannot create installation temporary: {error}"))?;
        file.set_permissions(std::fs::Permissions::from_mode(mode))
            .map_err(|error| format!("cannot set installation mode: {error}"))?;
        if let Some((uid, gid)) = owner
            && file
                .metadata()
                .map(|metadata| (metadata.uid(), metadata.gid()) != (uid, gid))
                .unwrap_or(true)
        {
            // SAFETY: `file` owns a valid descriptor for the complete call.
            if unsafe { libc::fchown(std::os::fd::AsRawFd::as_raw_fd(&file), uid, gid) } != 0 {
                return Err(format!(
                    "cannot set installation owner: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
        file.write_all(payload)
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("cannot persist installation file: {error}"))?;
        std::fs::rename(&temporary, path)
            .map_err(|error| format!("cannot activate installation file: {error}"))?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("cannot sync installation directory: {error}"))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

fn passwd_identity(uid: u32) -> Result<(u32, PathBuf), String> {
    let passwd = std::fs::read_to_string("/etc/passwd")
        .map_err(|error| format!("cannot read local account registry: {error}"))?;
    for line in passwd.lines() {
        let fields = line.split(':').collect::<Vec<_>>();
        if fields.len() >= 7 && fields[2].parse::<u32>().ok() == Some(uid) {
            let gid = fields[3]
                .parse::<u32>()
                .map_err(|_| "build account group is invalid".to_owned())?;
            return Ok((gid, PathBuf::from(fields[5])));
        }
    }
    Err("Rust Cargo manifest has no valid non-root owner".to_owned())
}

fn validate_commit(value: &str) -> Result<(), String> {
    if matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err("source commit must be a complete lowercase Git object ID".to_owned())
    }
}

fn path_text(path: &Path) -> Result<String, String> {
    if !path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::CurDir | Component::ParentDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!(
            "path must be normalized and absolute: {}",
            path.display()
        ));
    }
    path.as_os_str()
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| "installation paths must be valid UTF-8".to_owned())
}

fn file_identity(metadata: &std::fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}

fn bounded_output(bytes: Vec<u8>) -> String {
    let start = bytes.len().saturating_sub(OUTPUT_CAP);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

fn useful_failure(output: &CommandOutput) -> String {
    let value = if output.stderr.trim().is_empty() {
        output.stdout.trim()
    } else {
        output.stderr.trim()
    };
    if value.is_empty() {
        "command exited nonzero".to_owned()
    } else {
        value
            .chars()
            .rev()
            .take(2_000)
            .collect::<String>()
            .chars()
            .rev()
            .collect()
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(value, "{byte:02x}").expect("string write");
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::process::Command;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct FakeRunner {
        requests: Arc<Mutex<Vec<CommandRequest>>>,
        outputs: Arc<Mutex<VecDeque<CommandOutput>>>,
    }

    impl FakeRunner {
        fn with_outputs(outputs: Vec<CommandOutput>) -> Self {
            Self {
                requests: Arc::default(),
                outputs: Arc::new(Mutex::new(outputs.into())),
            }
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, request: &CommandRequest) -> Result<CommandOutput, String> {
            self.requests.lock().unwrap().push(request.clone());
            self.outputs
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| "unexpected command".to_owned())
        }
    }

    fn success(stdout: &str) -> CommandOutput {
        CommandOutput {
            success: true,
            stdout: stdout.to_owned(),
            stderr: String::new(),
        }
    }

    #[test]
    fn live_checkout_requires_clean_current_main_at_upstream() {
        let temporary = tempfile::tempdir().unwrap();
        let remote = temporary.path().join("remote.git");
        assert!(
            Command::new("git")
                .args(["init", "--bare", "-q"])
                .arg(&remote)
                .status()
                .unwrap()
                .success()
        );
        let root = temporary.path().join("live");
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .arg(&root)
                .status()
                .unwrap()
                .success()
        );
        for args in [
            vec!["config", "user.name", "fixture"],
            vec!["config", "user.email", "fixture@example.invalid"],
        ] {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&root)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        std::fs::write(root.join("tracked.txt"), "one\n").unwrap();
        for args in [
            vec!["add", "tracked.txt"],
            vec!["commit", "-q", "-m", "initial"],
            vec!["branch", "-M", "main"],
            vec!["remote", "add", "origin", remote.to_str().unwrap()],
            vec!["push", "-q", "-u", "origin", "main"],
        ] {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&root)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let head = validate_live_checkout(&root, true, &HostRunner).unwrap();
        assert!(matches!(head.len(), 40 | 64));
        std::fs::write(root.join("dirty.txt"), "dirty").unwrap();
        assert!(
            validate_live_checkout(&root, false, &HostRunner)
                .unwrap_err()
                .contains("must be clean")
        );
    }

    #[test]
    fn build_runs_as_checkout_owner_and_verifies_every_binary_commit() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path();
        std::fs::write(source.join("Cargo.toml"), "[workspace]\n").unwrap();
        let tools = BuildTools {
            cargo: source.join("cargo"),
            setpriv: source.join("setpriv"),
        };
        for tool in [&tools.cargo, &tools.setpriv] {
            std::fs::write(tool, "tool").unwrap();
            std::fs::set_permissions(tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let commit = "a".repeat(40);
        let release = source.join("target/release");
        std::fs::create_dir_all(&release).unwrap();
        for name in BINARY_NAMES {
            let path = release.join(name);
            std::fs::write(&path, name).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let runner = FakeRunner::with_outputs(
            std::iter::once(success(""))
                .chain(BINARY_NAMES.iter().map(|_| success(&commit)))
                .collect(),
        );
        let identity = BuildIdentity {
            uid: 1000,
            gid: 1000,
            home: source.to_owned(),
        };
        let receipts = build_release_binaries(source, &commit, &identity, &tools, &runner).unwrap();
        assert_eq!(receipts.len(), 3);
        let requests = runner.requests.lock().unwrap();
        assert_eq!(requests[0].program, tools.setpriv);
        assert!(requests[0].args.contains(&OsString::from("--workspace")));
        assert_eq!(
            requests[0]
                .environment
                .get(OsStr::new("DEVCOORDINATOR2_SOURCE_COMMIT")),
            Some(&OsString::from(&commit))
        );
        assert!(
            requests[1..]
                .iter()
                .all(|request| request.args == [OsString::from("--source-commit")])
        );
    }

    #[test]
    fn manifest_hash_unit_and_direct_links_are_exact() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source");
        std::fs::create_dir_all(source.join("deploy")).unwrap();
        std::fs::create_dir_all(source.join("edge")).unwrap();
        std::fs::create_dir_all(source.join("target/release")).unwrap();
        std::fs::write(
            source.join("deploy/devcoordinator2.service"),
            "[Service]\nExecStart=/home/DevCoordinator2/target/release/devcoordinator2 daemon\n",
        )
        .unwrap();
        std::fs::write(
            source.join("deploy/devcoordinator2-edge.service"),
            "[Service]\nExecStart=/usr/bin/node /home/DevCoordinator2/edge/devcoordinator2-edge.mjs\nProtectHome=read-only\nReadOnlyPaths=/home/DevCoordinator2\n",
        )
        .unwrap();
        let commit = "b".repeat(40);
        let mut outputs = Vec::new();
        for name in BINARY_NAMES {
            let path = source.join("target/release").join(name);
            std::fs::write(&path, name).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
            outputs.push(success(&commit));
        }
        let runner = FakeRunner::with_outputs(outputs);
        let binaries = verify_release_binaries(&source, &commit, &runner).unwrap();
        let manifest = manifest(&source, &commit, "2026-09-04T00:00:00Z", binaries).unwrap();
        let manifest_path = temporary.path().join("install-manifest.json");
        let uid = rustix::process::getuid().as_raw();
        let gid = rustix::process::getgid().as_raw();
        write_manifest(&manifest_path, &manifest, (uid, gid)).unwrap();
        let runner =
            FakeRunner::with_outputs(BINARY_NAMES.iter().map(|_| success(&commit)).collect());
        assert_eq!(
            read_and_verify_manifest_owned(&manifest_path, &runner, uid).unwrap(),
            manifest
        );
        assert_eq!(
            std::fs::metadata(&manifest_path).unwrap().mode() & 0o777,
            0o600
        );
        let daemon = render_daemon_unit(&source).unwrap();
        assert!(daemon.contains(&format!(
            "ExecStart={}/target/release/devcoordinator2 daemon",
            source.display()
        )));
        assert!(!daemon.contains("python"));
        let edge = render_edge_unit(&source, false).unwrap();
        assert!(edge.contains("ProtectHome=read-only"));
        let plan = installation_plan(&manifest, &manifest_path).unwrap();
        assert_eq!(
            plan.binary_links["/usr/local/bin/devcoordinator2"],
            source
                .join("target/release/devcoordinator2")
                .display()
                .to_string()
        );
        let destination = temporary.path().join("bin/devcoordinator2");
        replace_direct_link(&destination, &source.join("target/release/devcoordinator2")).unwrap();
        assert_eq!(
            std::fs::read_link(destination).unwrap(),
            source.join("target/release/devcoordinator2")
        );
        let changed = source.join("target/release/devcoordinator2");
        std::fs::write(&changed, "changed").unwrap();
        std::fs::set_permissions(&changed, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runner =
            FakeRunner::with_outputs(BINARY_NAMES.iter().map(|_| success(&commit)).collect());
        assert!(
            read_and_verify_manifest_owned(&manifest_path, &runner, uid)
                .unwrap_err()
                .contains("no longer match")
        );
    }

    #[test]
    fn manifest_detects_binary_tampering_and_links_refuse_regular_targets() {
        let temporary = tempfile::tempdir().unwrap();
        let destination = temporary.path().join("tool");
        std::fs::write(&destination, "owned").unwrap();
        assert!(
            replace_direct_link(&destination, Path::new("/source"))
                .unwrap_err()
                .contains("non-symlink")
        );
    }

    #[test]
    fn environment_updates_are_atomic_idempotent_and_conflict_aware() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("instance.env");
        std::fs::write(&path, "A=1\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        assert!(ensure_env_value(&path, "B", "2").unwrap());
        assert!(!ensure_env_value(&path, "B", "2").unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "A=1\nB=2\n");
        assert!(
            ensure_env_value(&path, "B", "3")
                .unwrap_err()
                .contains("different installed value")
        );
        assert!(set_env_value(&path, "B", "3").unwrap());
        assert!(!set_env_value(&path, "B", "3").unwrap());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "A=1\nB=3\n");
    }

    #[test]
    fn private_policies_merge_without_dropping_existing_accounts() {
        let temporary = tempfile::tempdir().unwrap();
        let uid = rustix::process::getuid().as_raw();
        let gid = rustix::process::getgid().as_raw();
        let compose = temporary.path().join("compose.json");
        std::fs::write(
            &compose,
            serde_json::to_vec(&ComposePolicy {
                schema: 1,
                authorizations: vec![ComposeAuthorization {
                    repository_id: format!("r{}", "a".repeat(16)),
                    path: "a.env".to_owned(),
                }],
            })
            .unwrap(),
        )
        .unwrap();
        assert!(
            merge_compose_env_allowlist(
                &compose,
                &[ComposeAuthorization {
                    repository_id: format!("r{}", "b".repeat(16)),
                    path: "b.env".to_owned(),
                }],
                (uid, gid),
            )
            .unwrap()
        );
        let policy: ComposePolicy =
            serde_json::from_slice(&std::fs::read(&compose).unwrap()).unwrap();
        assert_eq!(policy.authorizations.len(), 2);
        assert_eq!(std::fs::metadata(&compose).unwrap().mode() & 0o777, 0o640);

        let usage = temporary.path().join("usage.json");
        let first = CodexUsageSource {
            uid: 1000,
            codex_home: "/home/one/.codex".to_owned(),
            executable: "/home/one/.local/bin/codex".to_owned(),
        };
        assert!(
            merge_codex_usage_sources(&usage, std::slice::from_ref(&first), (uid, gid)).unwrap()
        );
        assert!(
            merge_codex_usage_sources(
                &usage,
                &[CodexUsageSource {
                    uid: 1001,
                    codex_home: "/home/two/.codex".to_owned(),
                    executable: "/home/two/.local/bin/codex".to_owned(),
                }],
                (uid, gid),
            )
            .unwrap()
        );
        let policy: UsagePolicy = serde_json::from_slice(&std::fs::read(&usage).unwrap()).unwrap();
        assert_eq!(policy.sources.len(), 2);
        assert_eq!(policy.sources[0], first);
        assert_eq!(std::fs::metadata(&usage).unwrap().mode() & 0o777, 0o600);
    }

    #[test]
    fn compose_authorization_binds_one_ignored_regular_file_to_git_identity() {
        let temporary = tempfile::tempdir().unwrap();
        let repository = temporary.path().join("repo");
        std::fs::create_dir(&repository).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(&repository)
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(repository.join(".gitignore"), "private/dev.env\n").unwrap();
        std::fs::create_dir(repository.join("private")).unwrap();
        std::fs::write(repository.join("private/dev.env"), "VALUE=not-read\n").unwrap();
        let entries = compose_env_authorizations(
            &[format!("{}=private/dev.env", repository.display())],
            &HostRunner,
        )
        .unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].repository_id.starts_with('r'));
        assert_eq!(entries[0].path, "private/dev.env");
        assert!(
            compose_env_authorizations(
                &[format!("{}=../outside.env", repository.display())],
                &HostRunner,
            )
            .unwrap_err()
            .contains("normalized relative")
        );
    }

    #[test]
    fn codex_source_resolution_uses_only_the_account_default_paths() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        std::fs::write(home.join(".local/bin/codex"), "binary").unwrap();
        let sources = codex_usage_sources(&["developer".to_owned()], |_| {
            Ok(LocalAccount {
                uid: 1234,
                gid: 1234,
                home: home.clone(),
            })
        })
        .unwrap();
        assert_eq!(
            sources,
            [CodexUsageSource {
                uid: 1234,
                codex_home: home.join(".codex").display().to_string(),
                executable: home.join(".local/bin/codex").display().to_string(),
            }]
        );
    }
}
