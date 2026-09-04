//! Privacy and provenance guard for publishable repository artifacts.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::audit_ledger::{read_bytes_nofollow, validate_directory_nofollow};

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const PNG_METADATA_CHUNKS: &[[u8; 4]] = &[*b"tEXt", *b"zTXt", *b"iTXt", *b"eXIf", *b"tIME"];
const PORTABLE_USERS: &[&str] = &[
    "devcoordinator2",
    "developer",
    "example",
    "fixture",
    "one",
    "runner",
    "root",
    "someone",
    "test",
    "two",
    "user",
    "username",
];
const PORTABLE_AGENT_MARKERS: &[&str] = &[
    "fixture", "example", "test", "codex", "claude", "agent", "sample",
];
const GRAMMAR_WORDS: &[&str] = &["a", "and", "for", "or", "so", "the", "to", "with"];

static HOME_UNIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[^A-Za-z0-9])/(?:Users|home)/([A-Za-z0-9._-]+)(?:/|$)")
        .expect("home path regex")
});
static HOME_WINDOWS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^A-Za-z0-9])(?:[A-Z]:\\Users\\)([^\\/\s]+)(?:\\|$)")
        .expect("Windows home path regex")
});
static AGENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"--agent(?:=|\s+)["']?([A-Za-z][A-Za-z0-9._-]+)"#).expect("agent identity regex")
});
static SECRET_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"\bgh[pousr]_[A-Za-z0-9]{20,}\b",
        r"\bgithub_pat_[A-Za-z0-9_]{20,}\b",
        r"\bAKIA[0-9A-Z]{16}\b",
        r"\bsk-[A-Za-z0-9_-]{20,}\b",
        r"\bxox[baprs]-[A-Za-z0-9-]{20,}\b",
        r"-----BEGIN (?:RSA |OPENSSH |EC |DSA )?PRIVATE KEY-----",
        r"(?i)Authorization\s*:\s*Bearer\s+[A-Za-z0-9._~+/=<$-]{16,}",
    ]
    .into_iter()
    .map(|pattern| Regex::new(pattern).expect("secret regex"))
    .collect()
});
static ENV_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new( // public-artifact-guard: allow text-secret
        r#"\b(?:[A-Z][A-Z0-9_]*_)?(?:PASSWORD|PASSWD|SECRET|TOKEN|API_KEY|PRIVATE_KEY)\s*=\s*["']?([^\s"',;}{]{8,})"#,
    )
    .expect("environment secret regex")
});
static STRUCTURED_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new( // public-artifact-guard: allow text-secret
        r#"(?i)^\s*["']?(?:password|passwd|secret|token|api[_-]?key|private[_-]?key)["']?\s*:\s*["']?([^\s"',;}{]{8,})"#,
    )
    .expect("structured secret regex")
});

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Finding {
    pub rule: String,
    pub path: String,
    pub line: Option<usize>,
    pub detail: String,
}

type PngChunks = (u32, u32, Vec<([u8; 4], Vec<u8>)>);

fn finding(rule: &str, path: &Path, line: Option<usize>, detail: impl Into<String>) -> Finding {
    Finding {
        rule: rule.to_owned(),
        path: path.to_string_lossy().replace('\\', "/"),
        line,
        detail: detail.into(),
    }
}

fn snapshot_paths(repo: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    let mut stack = vec![repo.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in std::fs::read_dir(&directory).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let metadata = entry
                .path()
                .symlink_metadata()
                .map_err(|error| error.to_string())?;
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                stack.push(entry.path());
            } else if metadata.is_file() || metadata.file_type().is_symlink() {
                paths.push(
                    entry
                        .path()
                        .strip_prefix(repo)
                        .map_err(|error| error.to_string())?
                        .to_owned(),
                );
            }
        }
    }
    paths.sort();
    Ok(paths)
}

fn nul_paths(bytes: &[u8]) -> Vec<PathBuf> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|item| !item.is_empty())
        .map(|item| PathBuf::from(String::from_utf8_lossy(item).into_owned()))
        .collect()
}

pub fn publishable_paths(repo: &Path) -> Result<Vec<PathBuf>, String> {
    let probe = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|error| error.to_string())?;
    if !probe.status.success() {
        let git = repo.join(".git");
        if git.exists() || git.symlink_metadata().is_ok() {
            return Err(String::from_utf8_lossy(&probe.stderr).trim().to_owned());
        }
        return snapshot_paths(repo);
    }
    let top = String::from_utf8(probe.stdout)
        .map_err(|_| "could not decode repository root".to_owned())?;
    let top = PathBuf::from(top.trim())
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let repo = repo.canonicalize().map_err(|error| error.to_string())?;
    if top != repo {
        return Err("--repo must name the exact Git worktree root".to_owned());
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(nul_paths(&output.stdout))
}

fn suppressed(line: &str, rule: &str) -> bool {
    line.contains(&format!("public-artifact-guard: allow {rule}"))
}

fn portable_username(value: &str) -> bool {
    let lowered = value.to_ascii_lowercase();
    PORTABLE_USERS.contains(&lowered.as_str())
        || GRAMMAR_WORDS.contains(&lowered.as_str())
        || PORTABLE_AGENT_MARKERS
            .iter()
            .any(|marker| lowered.contains(marker))
}

fn placeholder_secret(value: &str) -> bool {
    let normalized = value.trim().trim_matches(['"', '\'']);
    let lowered = normalized.to_ascii_lowercase();
    if [
        "redacted",
        "do-not-leak",
        "do_not_leak",
        "do-not-audit",
        "not-a-secret",
        "changeme",
    ]
    .contains(&lowered.as_str())
        || [
            "fixture-",
            "example-",
            "dummy-",
            "test-",
            "placeholder-",
            "do-not-audit-",
            "do-not-leak-",
            "do_not_leak_",
            "not-a-secret-",
        ]
        .iter()
        .any(|prefix| lowered.starts_with(prefix))
    {
        return true;
    }
    let variable = Regex::new(r"^\$[A-Za-z_][A-Za-z0-9_]*$").expect("variable regex");
    let braced = Regex::new(r"^\$\{[A-Za-z_][A-Za-z0-9_]*(?::[-+?][^}]*)?\}$")
        .expect("braced variable regex");
    let angle = Regex::new(r"^<[A-Za-z0-9 _./:-]+>$").expect("angle placeholder regex");
    variable.is_match(normalized) || braced.is_match(normalized) || angle.is_match(normalized)
}

pub fn scan_text(rel_path: &Path, text: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (offset, line) in text.lines().enumerate() {
        let line_number = offset + 1;
        for pattern in [&*HOME_UNIX, &*HOME_WINDOWS] {
            if let Some(capture) = pattern.captures(line)
                && !portable_username(capture.get(1).unwrap().as_str())
                && !suppressed(line, "text-private-home")
            {
                findings.push(finding(
                    "text-private-home",
                    rel_path,
                    Some(line_number),
                    "literal private home path",
                ));
                break;
            }
        }
        if let Some(capture) = AGENT.captures(line)
            && !portable_username(capture.get(1).unwrap().as_str())
            && !suppressed(line, "text-literal-username")
        {
            findings.push(finding(
                "text-literal-username",
                rel_path,
                Some(line_number),
                "literal operating-system or agent identity",
            ));
        }
        if suppressed(line, "text-secret") {
            continue;
        }
        let literal_secret = SECRET_PATTERNS.iter().any(|pattern| {
            pattern.find(line).is_some_and(|matched| {
                let value = matched.as_str();
                !(value.to_ascii_lowercase().contains("bearer <")
                    || value.to_ascii_lowercase().contains("bearer $"))
            })
        });
        let assignment = ENV_SECRET
            .captures(line)
            .or_else(|| STRUCTURED_SECRET.captures(line))
            .and_then(|capture| capture.get(1))
            .map(|value| value.as_str());
        if literal_secret || assignment.is_some_and(|value| !placeholder_secret(value)) {
            findings.push(finding(
                "text-secret",
                rel_path,
                Some(line_number),
                "credential-like literal; value withheld",
            ));
        }
    }
    findings
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn png_chunks(data: &[u8]) -> Result<PngChunks, String> {
    if !data.starts_with(PNG_SIGNATURE) {
        return Err("invalid PNG signature".to_owned());
    }
    let mut offset = PNG_SIGNATURE.len();
    let mut chunks = Vec::new();
    let mut dimensions = None;
    let mut saw_end = false;
    while offset < data.len() {
        if offset + 12 > data.len() {
            return Err("truncated PNG chunk".to_owned());
        }
        let length = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = data[offset + 4..offset + 8].try_into().unwrap();
        let start = offset + 8;
        let end = start
            .checked_add(length)
            .ok_or_else(|| "PNG chunk length overflow".to_owned())?;
        let crc_end = end + 4;
        if crc_end > data.len() {
            return Err("truncated PNG payload".to_owned());
        }
        let payload = data[start..end].to_vec();
        let expected = u32::from_be_bytes(data[end..crc_end].try_into().unwrap());
        let mut checked = kind.to_vec();
        checked.extend_from_slice(&payload);
        if expected != crc32(&checked) {
            return Err(format!(
                "invalid CRC for {} chunk",
                String::from_utf8_lossy(&kind)
            ));
        }
        if &kind == b"IHDR" {
            if length != 13 {
                return Err("invalid IHDR length".to_owned());
            }
            dimensions = Some((
                u32::from_be_bytes(payload[0..4].try_into().unwrap()),
                u32::from_be_bytes(payload[4..8].try_into().unwrap()),
            ));
        }
        chunks.push((kind, payload));
        if &kind == b"IEND" {
            if crc_end != data.len() {
                return Err("bytes found after IEND".to_owned());
            }
            saw_end = true;
            break;
        }
        offset = crc_end;
    }
    if !saw_end {
        return Err("PNG is missing IHDR or IEND".to_owned());
    }
    let (width, height) = dimensions.ok_or_else(|| "PNG is missing IHDR or IEND".to_owned())?;
    Ok((width, height, chunks))
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn scan_png(
    repo: &Path,
    rel_path: &Path,
    publishable: &BTreeSet<String>,
    linked_data: Option<Vec<u8>>,
) -> Vec<Finding> {
    let path = repo.join(rel_path);
    let data = match linked_data {
        Some(data) => data,
        None => match read_bytes_nofollow(&path, Some(repo)) {
            Ok(Some(data)) => data,
            Ok(None) => {
                return vec![finding("png-invalid", rel_path, None, "PNG is missing")];
            }
            Err(error) => {
                return vec![finding("png-invalid", rel_path, None, error.to_string())];
            }
        },
    };
    let (width, height, chunks) = match png_chunks(&data) {
        Ok(parsed) => parsed,
        Err(error) => return vec![finding("png-invalid", rel_path, None, error)],
    };
    let mut findings = Vec::new();
    let metadata = chunks
        .iter()
        .filter(|(kind, _)| PNG_METADATA_CHUNKS.contains(kind))
        .map(|(kind, _)| String::from_utf8_lossy(kind).into_owned())
        .collect::<BTreeSet<_>>();
    if !metadata.is_empty() {
        findings.push(finding(
            "png-sensitive-metadata",
            rel_path,
            None,
            format!(
                "publishable PNG contains unnecessary metadata chunks: {}",
                metadata.into_iter().collect::<Vec<_>>().join(", ")
            ),
        ));
    }
    let provenance_rel = format!("{}.provenance.json", rel_path.to_string_lossy());
    let provenance_path = repo.join(&provenance_rel);
    if !publishable.contains(&provenance_rel)
        || !provenance_path
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
    {
        findings.push(finding(
            "png-missing-provenance",
            rel_path,
            None,
            "publishable PNG lacks a publishable provenance sidecar",
        ));
        return findings;
    }
    let provenance = read_bytes_nofollow(&provenance_path, Some(repo))
        .ok()
        .flatten()
        .and_then(|data| serde_json::from_slice::<Value>(&data).ok())
        .and_then(|value| value.as_object().cloned());
    let Some(provenance) = provenance else {
        findings.push(finding(
            "png-invalid-provenance",
            rel_path,
            None,
            "invalid provenance sidecar",
        ));
        return findings;
    };
    if provenance.get("schema_version") != Some(&json!(1))
        || provenance.get("artifact_type") != Some(&json!("test-fixture-snapshot"))
        || provenance.get("source") != Some(&json!("isolated-test-fixture"))
    {
        findings.push(finding(
            "png-invalid-provenance",
            rel_path,
            None,
            "provenance is not an isolated test-fixture snapshot",
        ));
    }
    for key in ["fixture_id", "generator"] {
        if provenance
            .get(key)
            .and_then(Value::as_str)
            .is_none_or(|value| value.trim().is_empty())
        {
            findings.push(finding(
                "png-invalid-provenance",
                rel_path,
                None,
                format!("provenance is missing {key}"),
            ));
        }
    }
    if provenance.get("sha256") != Some(&json!(sha256_hex(&data)))
        || provenance.get("width") != Some(&json!(width))
        || provenance.get("height") != Some(&json!(height))
    {
        findings.push(finding(
            "png-provenance-mismatch",
            rel_path,
            None,
            "PNG hash or dimensions do not match provenance",
        ));
    }
    findings
}

fn symlink_target(path: &Path) -> Option<PathBuf> {
    path.canonicalize().ok().or_else(|| {
        let target = std::fs::read_link(path).ok()?;
        let target = if target.is_absolute() {
            target
        } else {
            path.parent()?.join(target)
        };
        let mut normalized = PathBuf::new();
        for component in target.components() {
            match component {
                Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
                Component::RootDir => normalized.push(Path::new("/")),
                Component::CurDir => {}
                Component::ParentDir => {
                    normalized.pop();
                }
                Component::Normal(part) => normalized.push(part),
            }
        }
        Some(normalized)
    })
}

pub fn scan(repo: &Path, allow_internal_symlinks: bool) -> Result<Value, String> {
    let repo = validate_directory_nofollow(repo).map_err(|error| error.to_string())?;
    let paths = publishable_paths(&repo)?;
    let publishable = paths
        .iter()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .collect::<BTreeSet<_>>();
    let mut findings = BTreeSet::new();
    let mut scanned = 0usize;
    for rel_path in paths {
        let path = repo.join(&rel_path);
        let metadata = match path.symlink_metadata() {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if metadata.file_type().is_symlink() {
            scanned += 1;
            let target = symlink_target(&path).unwrap_or_default();
            if !target.starts_with(&repo) {
                findings.insert(finding(
                    "publishable-external-symlink",
                    &rel_path,
                    None,
                    "publishable symlink resolves outside the repository",
                ));
                continue;
            }
            if !allow_internal_symlinks {
                findings.insert(finding(
                    "publishable-symlink",
                    &rel_path,
                    None,
                    "publishable symlinks are disabled; copy the artifact or opt in to internal links",
                ));
                continue;
            }
        }
        if !path.is_file() {
            continue;
        }
        if !metadata.file_type().is_symlink() {
            scanned += 1;
        }
        if path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("png"))
        {
            let linked_data = metadata
                .file_type()
                .is_symlink()
                .then(|| std::fs::read(&path).ok())
                .flatten();
            findings.extend(scan_png(&repo, &rel_path, &publishable, linked_data));
            continue;
        }
        let data = if metadata.file_type().is_symlink() {
            std::fs::read(&path).map_err(|error| error.to_string())
        } else {
            read_bytes_nofollow(&path, Some(&repo))
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "publishable file disappeared".to_owned())
        };
        let data = match data {
            Ok(data) => data,
            Err(error) => {
                findings.insert(finding("artifact-read-error", &rel_path, None, error));
                continue;
            }
        };
        if data.contains(&0) {
            continue;
        }
        let Ok(text) = String::from_utf8(data) else {
            continue;
        };
        findings.extend(scan_text(&rel_path, &text));
    }
    let findings = findings.into_iter().collect::<Vec<_>>();
    Ok(json!({
        "ok":findings.is_empty(),"scanned":scanned,
        "finding_count":findings.len(),"findings":findings,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, content: impl AsRef<[u8]>) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn git(repo: &Path, arguments: &[&str]) {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(arguments)
                .status()
                .unwrap()
                .success()
        );
    }

    fn chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        output.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        output.extend_from_slice(kind);
        output.extend_from_slice(payload);
        let mut checked = kind.to_vec();
        checked.extend_from_slice(payload);
        output.extend_from_slice(&crc32(&checked).to_be_bytes());
        output
    }

    fn png(metadata: Option<&str>) -> Vec<u8> {
        let mut bytes = PNG_SIGNATURE.to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&1_u32.to_be_bytes());
        ihdr.extend_from_slice(&1_u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes.extend(chunk(b"IHDR", &ihdr));
        if let Some(metadata) = metadata {
            let mut text = b"Source\0".to_vec();
            text.extend_from_slice(metadata.as_bytes());
            bytes.extend(chunk(b"tEXt", &text));
        }
        bytes.extend(chunk(b"IDAT", b"fixture"));
        bytes.extend(chunk(b"IEND", b""));
        bytes
    }

    fn provenance(data: &[u8], digest: Option<String>) -> Value {
        json!({
            "schema_version":1,"artifact_type":"test-fixture-snapshot",
            "source":"isolated-test-fixture","fixture_id":"neutral-ops-v1",
            "generator":"FixtureRenderer","width":1,"height":1,
            "sha256":digest.unwrap_or_else(||sha256_hex(data)),
        })
    }

    #[test]
    fn guard_preserves_private_text_png_symlink_and_snapshot_contracts() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        let private_mac = format!("/{}/{}.operator/Projects/customer", "Users", "real");
        let private_linux = format!("/{}/{}operator/work/customer", "home", "real");
        let private_windows = format!("C:{}Users{}{}operator{}work", "\\", "\\", "real", "\\");
        let literal_secret = format!("ghp_{}", "A".repeat(36));
        let dollar_secret = format!("realprod{}LeakedPass123", '$');
        write(
            &repo.join("docs/unsafe.md"),
            format!(
                "Local: {private_mac}\nLinux: {private_linux}\nWindows: {private_windows}\nCommand: tool --agent {}.operator\nAuthorization: Bearer {literal_secret}\nPOSTGRES_PASSWORD=production-value-4Hh7s91x\nDEPLOY_TOKEN={dollar_secret}\n",
                "real"
            ),
        );
        write(
            &repo.join("docs/safe.md"),
            "Use $HOME/.codex or ~/.codex.\n/Users/<username>/src /home/example/src\n--agent $USER --agent fixture-agent\nAuthorization: Bearer <contents-of-token>\nPOSTGRES_PASSWORD=${POSTGRES_PASSWORD}\nAPI_TOKEN=fixture-token\n",
        );
        let external = directory.path().join("external.md");
        write(&external, "safe external content");
        write(&repo.join("docs/internal.md"), "safe internal content");
        std::os::unix::fs::symlink(&external, repo.join("docs/external-link.md")).unwrap();
        std::os::unix::fs::symlink("internal.md", repo.join("docs/internal-link.md")).unwrap();
        let unsafe_png = png(Some(&private_mac));
        write(&repo.join("artifacts/unsafe-metadata.png"), &unsafe_png);
        let missing = png(None);
        write(&repo.join("artifacts/missing.png"), &missing);
        let forged = png(None);
        write(&repo.join("artifacts/forged.png"), &forged);
        crate::audit_queue::write_json(
            &repo.join("artifacts/forged.png.provenance.json"),
            &provenance(&forged, Some("0".repeat(64))),
        )
        .unwrap();
        let safe = png(None);
        write(&repo.join("artifacts/safe.png"), &safe);
        crate::audit_queue::write_json(
            &repo.join("artifacts/safe.png.provenance.json"),
            &provenance(&safe, None),
        )
        .unwrap();
        write(&repo.join(".gitignore"), "ignored/\n");
        git(&repo, &["add", "."]);
        write(
            &repo.join("docs/untracked-private.md"),
            format!("Untracked: {private_mac}\n"),
        );
        write(&repo.join("artifacts/untracked.png"), png(None));
        write(
            &repo.join("ignored/private.md"),
            format!("Ignored: {private_mac}\n"),
        );

        let report = scan(&repo, false).unwrap();
        assert_eq!(report["ok"], false);
        let rules = report["findings"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["rule"].as_str())
            .collect::<BTreeSet<_>>();
        for rule in [
            "text-private-home",
            "text-literal-username",
            "text-secret",
            "png-sensitive-metadata",
            "png-missing-provenance",
            "png-provenance-mismatch",
            "publishable-external-symlink",
            "publishable-symlink",
        ] {
            assert!(rules.contains(rule), "{rule}: {report:#}");
        }
        assert!(!report.to_string().contains("ignored/private.md"));
        assert!(!report.to_string().contains("docs/safe.md"));
        assert!(!report.to_string().contains("artifacts/safe.png\""));

        for relative in [
            "docs/unsafe.md",
            "artifacts/unsafe-metadata.png",
            "artifacts/missing.png",
            "artifacts/forged.png",
            "artifacts/forged.png.provenance.json",
            "docs/untracked-private.md",
            "artifacts/untracked.png",
            "docs/external-link.md",
        ] {
            std::fs::remove_file(repo.join(relative)).unwrap();
        }
        assert_eq!(scan(&repo, true).unwrap()["ok"], true);
        std::fs::remove_file(repo.join("docs/internal-link.md")).unwrap();
        assert_eq!(scan(&repo, false).unwrap()["ok"], true);

        std::fs::remove_dir_all(repo.join(".git")).unwrap();
        std::fs::remove_dir_all(repo.join("ignored")).unwrap();
        write(
            &repo.join("docs/snapshot-private.md"),
            format!("Snapshot: {private_linux}\n"),
        );
        let snapshot = scan(&repo, false).unwrap();
        assert!(snapshot.to_string().contains("snapshot-private.md"));
        std::fs::remove_file(repo.join("docs/snapshot-private.md")).unwrap();
        assert_eq!(scan(&repo, false).unwrap()["ok"], true);
    }
}
