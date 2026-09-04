//! Deterministic structural test targets and empirical coverage ingestion.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use quick_xml::Reader;
use quick_xml::encoding::Decoder;
use quick_xml::events::{BytesStart, Event};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::audit_ledger::read_bytes_nofollow;

struct FunctionPattern {
    suffix: &'static str,
    kind: &'static str,
    regex: Regex,
}

static FUNCTION_PATTERNS: LazyLock<Vec<FunctionPattern>> = LazyLock::new(|| {
    [
        (
            ".py",
            "function",
            r"^\s*(?:async\s+)?def\s+([A-Za-z_]\w*)\s*\(",
        ),
        (
            ".ts",
            "function",
            r"^\s*(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_$][\w$]*)\s*\(",
        ),
        (
            ".ts",
            "function",
            r"^\s*(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(?:async\s*)?(?:\([^)]*\)|[A-Za-z_$][\w$]*)\s*=>",
        ),
        (
            ".tsx",
            "component/function",
            r"^\s*(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_$][\w$]*)\s*\(",
        ),
        (
            ".tsx",
            "component/function",
            r"^\s*(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(?:async\s*)?(?:\([^)]*\)|[A-Za-z_$][\w$]*)\s*=>",
        ),
        (
            ".js",
            "function",
            r"^\s*(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_$][\w$]*)\s*\(",
        ),
        (
            ".js",
            "function",
            r"^\s*(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(?:async\s*)?(?:\([^)]*\)|[A-Za-z_$][\w$]*)\s*=>",
        ),
        (
            ".jsx",
            "component/function",
            r"^\s*(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_$][\w$]*)\s*\(",
        ),
        (
            ".swift",
            "method",
            r"^\s*(?:(?:public|private|internal|fileprivate|open|static|class|mutating)\s+)*(?:func|init)\s*([A-Za-z_]\w*)?\s*\(",
        ),
        (
            ".rs",
            "function",
            r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_]\w*)\s*\(",
        ),
        (
            ".go",
            "function",
            r"^\s*func\s+(?:\([^)]*\)\s*)?([A-Za-z_]\w*)\s*\(",
        ),
    ]
    .into_iter()
    .map(|(suffix, kind, expression)| FunctionPattern {
        suffix,
        kind,
        regex: Regex::new(expression).expect("constant function regex"),
    })
    .collect()
});

static UI_CONTROL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)<(button|a|input|select|textarea|form)\b[^>]*>([^<]{0,80})")
        .expect("constant UI-control regex")
});
static WHITESPACE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+").expect("constant whitespace regex"));

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AuditUnit {
    pub unit_id: String,
    pub rel_path: String,
    #[serde(default)]
    pub start_line: Option<usize>,
    #[serde(default)]
    pub end_line: Option<usize>,
    #[serde(default)]
    pub start_byte: Option<usize>,
    #[serde(default)]
    pub interface_relevant: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TestTarget {
    pub target_id: String,
    pub unit_id: String,
    pub rel_path: String,
    pub symbol: String,
    pub kind: String,
    pub line: usize,
    pub structural_basis: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CoverageLines {
    pub measured_lines: Vec<usize>,
    pub covered_lines: Vec<usize>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CoverageEvidence {
    pub evidence_id: String,
    pub path: String,
    pub sha256: String,
    pub format: String,
    pub files: BTreeMap<String, CoverageLines>,
}

#[derive(Clone, Debug, Default)]
struct MutableCoverage {
    measured: BTreeSet<usize>,
    covered: BTreeSet<usize>,
}

type CoverageIndex = BTreeMap<String, MutableCoverage>;

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to String cannot fail");
            output
        })
}

fn target_id(unit_id: &str, kind: &str, symbol: &str, line: usize) -> String {
    let value = format!("{unit_id}\0{kind}\0{symbol}\0{line}");
    format!("target-{}", &sha256_hex(value.as_bytes())[..16])
}

pub fn discover_targets(repo: &Path, units: &[AuditUnit]) -> Vec<TestTarget> {
    let mut records = Vec::new();
    let mut text_cache: HashMap<&str, Vec<String>> = HashMap::new();
    for unit in units {
        let lines = text_cache.entry(&unit.rel_path).or_insert_with(|| {
            let path = repo.join(&unit.rel_path);
            read_bytes_nofollow(&path, Some(repo))
                .ok()
                .flatten()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .map(|text| text.lines().map(str::to_owned).collect())
                .unwrap_or_default()
        });
        let suffix = Path::new(&unit.rel_path)
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| format!(".{}", value.to_lowercase()))
            .unwrap_or_default();
        let mut found = Vec::new();
        if !lines.is_empty() && unit.start_byte.is_none() {
            let first = unit.start_line.unwrap_or(1);
            let last = unit.end_line.unwrap_or(lines.len()).min(lines.len());
            if first <= last && first > 0 {
                for line_number in first..=last {
                    let line = &lines[line_number - 1];
                    for pattern in FUNCTION_PATTERNS
                        .iter()
                        .filter(|pattern| pattern.suffix == suffix)
                    {
                        if let Some(captures) = pattern.regex.captures(line) {
                            let symbol = captures
                                .get(1)
                                .map(|capture| capture.as_str())
                                .filter(|value| !value.is_empty())
                                .unwrap_or("init");
                            found.push((symbol.to_owned(), pattern.kind.to_owned(), line_number));
                        }
                    }
                    if unit.interface_relevant
                        && let Some(captures) = UI_CONTROL_RE.captures(line)
                    {
                        let tag = captures[1].to_lowercase();
                        let label = WHITESPACE_RE
                            .replace_all(
                                captures.get(2).map(|value| value.as_str()).unwrap_or(""),
                                " ",
                            )
                            .trim()
                            .to_owned();
                        let label = if label.is_empty() { tag.clone() } else { label };
                        found.push((
                            format!("{tag}:{label}"),
                            "ui-control".to_owned(),
                            line_number,
                        ));
                    }
                }
            }
        }
        let mut deduplicated = BTreeSet::new();
        for (symbol, kind, line) in &found {
            if !deduplicated.insert((symbol.clone(), kind.clone(), *line)) {
                continue;
            }
            records.push(TestTarget {
                target_id: target_id(&unit.unit_id, kind, symbol, *line),
                unit_id: unit.unit_id.clone(),
                rel_path: unit.rel_path.clone(),
                symbol: symbol.clone(),
                kind: kind.clone(),
                line: *line,
                structural_basis: format!(
                    "deterministic {} source scan",
                    if suffix.is_empty() { "file" } else { &suffix }
                ),
            });
        }
        if found.is_empty() {
            let symbol = format!("unit-review:{}", unit.unit_id);
            let line = unit.start_line.unwrap_or(1);
            records.push(TestTarget {
                target_id: target_id(&unit.unit_id, "unit-review", &symbol, line),
                unit_id: unit.unit_id.clone(),
                rel_path: unit.rel_path.clone(),
                symbol,
                kind: "unit-review".to_owned(),
                line,
                structural_basis: "no supported behavior symbol detected; explicit reviewed-target or not-reasonable decision required".to_owned(),
            });
        }
    }
    records
}

fn normalize_coverage_path(repo: &Path, raw: &str) -> Option<String> {
    let candidate = Path::new(raw);
    let canonical_repo = repo.canonicalize().ok()?;
    if candidate.is_absolute() {
        let canonical = candidate.canonicalize().ok()?;
        return canonical
            .strip_prefix(canonical_repo)
            .ok()
            .map(|path| path.to_string_lossy().replace('\\', "/"));
    }
    if candidate.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return None;
    }
    let normalized = raw.trim_start_matches(['.', '/']);
    if normalized.is_empty() {
        return None;
    }
    let path = repo.join(normalized);
    let metadata = path.symlink_metadata().ok()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return None;
    }
    Some(Path::new(normalized).to_string_lossy().replace('\\', "/"))
}

fn add_line(index: &mut CoverageIndex, rel_path: Option<&str>, line: usize, hits: i64) {
    let Some(rel_path) = rel_path.filter(|_| line > 0) else {
        return;
    };
    let row = index.entry(rel_path.to_owned()).or_default();
    row.measured.insert(line);
    if hits > 0 {
        row.covered.insert(line);
    }
}

fn parse_lcov(repo: &Path, text: &str) -> CoverageIndex {
    let mut index = CoverageIndex::new();
    let mut current = None;
    for raw in text.lines() {
        if let Some(path) = raw.strip_prefix("SF:") {
            current = normalize_coverage_path(repo, path.trim());
        } else if let Some(values) = raw.strip_prefix("DA:") {
            if let Some(path) = current.as_deref() {
                let values = values.split(',').collect::<Vec<_>>();
                if values.len() >= 2
                    && let (Ok(line), Ok(hits)) =
                        (values[0].parse::<usize>(), values[1].parse::<i64>())
                {
                    add_line(&mut index, Some(path), line, hits);
                }
            }
        } else if raw == "end_of_record" {
            current = None;
        }
    }
    index
}

fn xml_attribute(
    start: &BytesStart<'_>,
    name: &[u8],
    decoder: Decoder,
) -> Result<Option<String>, String> {
    for attribute in start.attributes().with_checks(true) {
        let attribute =
            attribute.map_err(|error| format!("invalid coverage XML attribute: {error}"))?;
        if attribute.key.as_ref() == name {
            return attribute
                .decode_and_unescape_value(decoder)
                .map(|value| Some(value.into_owned()))
                .map_err(|error| format!("invalid coverage XML value: {error}"));
        }
    }
    Ok(None)
}

fn add_xml_line(
    index: &mut CoverageIndex,
    current: Option<&str>,
    line: &BytesStart<'_>,
    decoder: Decoder,
) -> Result<(), String> {
    let number =
        xml_attribute(line, b"number", decoder)?.and_then(|value| value.parse::<usize>().ok());
    let hits = xml_attribute(line, b"hits", decoder)?
        .unwrap_or_else(|| "0".to_owned())
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .map(|value| value as i64);
    if let (Some(number), Some(hits)) = (number, hits) {
        add_line(index, current, number, hits);
    }
    Ok(())
}

fn parse_xml(repo: &Path, data: &[u8]) -> Result<CoverageIndex, String> {
    let mut reader = Reader::from_reader(data);
    reader.config_mut().trim_text(false);
    let mut index = CoverageIndex::new();
    let mut current_class = None;
    let mut class_depth = None;
    let mut depth = 0usize;
    loop {
        match reader
            .read_event()
            .map_err(|error| format!("coverage XML is invalid: {error}"))?
        {
            Event::Start(start) => {
                depth += 1;
                if start.name().as_ref() == b"class" {
                    current_class = xml_attribute(&start, b"filename", reader.decoder())?
                        .and_then(|path| normalize_coverage_path(repo, &path));
                    class_depth = Some(depth);
                } else if start.name().as_ref() == b"line" {
                    add_xml_line(
                        &mut index,
                        current_class.as_deref(),
                        &start,
                        reader.decoder(),
                    )?;
                }
            }
            Event::Empty(start) => {
                if start.name().as_ref() == b"line" {
                    add_xml_line(
                        &mut index,
                        current_class.as_deref(),
                        &start,
                        reader.decoder(),
                    )?;
                }
            }
            Event::End(_) => {
                if class_depth == Some(depth) {
                    current_class = None;
                    class_depth = None;
                }
                depth = depth.saturating_sub(1);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(index)
}

fn parse_json(repo: &Path, payload: &serde_json::Map<String, Value>) -> (String, CoverageIndex) {
    let mut index = CoverageIndex::new();
    if let Some(files) = payload.get("files").and_then(Value::as_object) {
        for (raw_path, value) in files {
            let Some(row) = value.as_object() else {
                continue;
            };
            let rel_path = normalize_coverage_path(repo, raw_path);
            let executed = row
                .get("executed_lines")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_u64)
                .filter_map(|line| usize::try_from(line).ok())
                .collect::<BTreeSet<_>>();
            let missing = row
                .get("missing_lines")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_u64)
                .filter_map(|line| usize::try_from(line).ok())
                .collect::<BTreeSet<_>>();
            for line in executed.union(&missing) {
                add_line(
                    &mut index,
                    rel_path.as_deref(),
                    *line,
                    i64::from(executed.contains(line)),
                );
            }
        }
        return ("coverage.py-json".to_owned(), index);
    }
    for (raw_path, value) in payload {
        let Some(row) = value.as_object() else {
            continue;
        };
        let (Some(statement_map), Some(hits)) = (
            row.get("statementMap").and_then(Value::as_object),
            row.get("s").and_then(Value::as_object),
        ) else {
            continue;
        };
        let rel_path = normalize_coverage_path(repo, raw_path);
        for (statement_id, location) in statement_map {
            let Some(line) = location
                .get("start")
                .and_then(|value| value.get("line"))
                .and_then(Value::as_u64)
                .and_then(|line| usize::try_from(line).ok())
            else {
                continue;
            };
            let Some(hits) = hits.get(statement_id).and_then(Value::as_i64).or(Some(0)) else {
                continue;
            };
            add_line(&mut index, rel_path.as_deref(), line, hits);
        }
    }
    ("istanbul-json".to_owned(), index)
}

fn freeze_index(index: CoverageIndex) -> BTreeMap<String, CoverageLines> {
    index
        .into_iter()
        .map(|(path, lines)| {
            (
                path,
                CoverageLines {
                    measured_lines: lines.measured.into_iter().collect(),
                    covered_lines: lines.covered.into_iter().collect(),
                },
            )
        })
        .collect()
}

pub fn ingest_coverage_reports(
    repo: &Path,
    raw_paths: &[PathBuf],
) -> Result<Vec<CoverageEvidence>, String> {
    let mut records = Vec::new();
    for raw_path in raw_paths {
        let candidate = if raw_path.is_absolute() {
            raw_path.clone()
        } else {
            repo.join(raw_path)
        };
        let data = read_bytes_nofollow(&candidate, None)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("coverage report not found: {}", candidate.display()))?;
        let path = candidate
            .canonicalize()
            .map_err(|error| format!("cannot resolve coverage report: {error}"))?;
        let text = std::str::from_utf8(&data)
            .map_err(|error| format!("coverage report is not UTF-8: {error}"))?;
        let suffix = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_lowercase();
        let trimmed = text.trim_start();
        let (format, index) =
            if suffix == "info" || trimmed.starts_with("TN:") || text.contains("\nSF:") {
                ("lcov".to_owned(), parse_lcov(repo, text))
            } else if suffix == "xml"
                || trimmed.starts_with("<?xml")
                || trimmed.starts_with("<coverage")
            {
                ("cobertura-xml".to_owned(), parse_xml(repo, &data)?)
            } else {
                let payload: Value = serde_json::from_slice(&data)
                    .map_err(|error| format!("coverage JSON is invalid: {error}"))?;
                let payload = payload.as_object().ok_or_else(|| {
                    format!("coverage JSON must be an object: {}", path.display())
                })?;
                parse_json(repo, payload)
            };
        let path_text = path.to_string_lossy().into_owned();
        records.push(CoverageEvidence {
            evidence_id: format!("coverage-{}", &sha256_hex(path_text.as_bytes())[..12]),
            path: path_text,
            sha256: sha256_hex(&data),
            format,
            files: freeze_index(index),
        });
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_discovery_finds_functions_controls_and_review_fallbacks() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("screen.tsx"),
            "export function Screen() {\n  return <button> Save   now </button>;\n}\n",
        )
        .unwrap();
        let units = vec![
            AuditUnit {
                unit_id: "unit-a".to_owned(),
                rel_path: "screen.tsx".to_owned(),
                start_line: None,
                end_line: None,
                start_byte: None,
                interface_relevant: true,
            },
            AuditUnit {
                unit_id: "unit-b".to_owned(),
                rel_path: "screen.tsx".to_owned(),
                start_line: Some(1),
                end_line: Some(2),
                start_byte: Some(0),
                interface_relevant: true,
            },
        ];
        let targets = discover_targets(directory.path(), &units);
        assert_eq!(targets.len(), 3);
        assert_eq!(targets[0].symbol, "Screen");
        assert_eq!(targets[0].target_id, "target-3202da2fab17d888");
        assert_eq!(targets[1].symbol, "button:Save now");
        assert_eq!(targets[1].target_id, "target-558f6d612049b79b");
        assert_eq!(targets[2].kind, "unit-review");
        assert_eq!(targets[2].target_id, "target-7e73774ece6898af");
        assert!(
            targets
                .iter()
                .all(|target| target.target_id.starts_with("target-"))
        );
    }

    #[test]
    fn ingests_lcov_cobertura_coveragepy_and_istanbul() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let source = repo.join("app.rs");
        std::fs::write(&source, "one\ntwo\nthree\n").unwrap();
        let lcov = repo.join("coverage.info");
        std::fs::write(&lcov, "TN:\nSF:app.rs\nDA:1,1\nDA:2,0\nend_of_record\n").unwrap();
        let xml = repo.join("coverage.xml");
        std::fs::write(
            &xml,
            "<coverage><classes><class filename=\"app.rs\"><lines><line number=\"2\" hits=\"1\"/><line number=\"3\" hits=\"0\"/></lines></class></classes></coverage>",
        )
        .unwrap();
        let coveragepy = repo.join("coverage-python.json");
        std::fs::write(
            &coveragepy,
            r#"{"files":{"app.rs":{"executed_lines":[1],"missing_lines":[2]}}}"#,
        )
        .unwrap();
        let istanbul = repo.join("coverage-istanbul.json");
        std::fs::write(
            &istanbul,
            r#"{"app.rs":{"statementMap":{"0":{"start":{"line":3}}},"s":{"0":1}}}"#,
        )
        .unwrap();
        let records = ingest_coverage_reports(&repo, &[lcov, xml, coveragepy, istanbul]).unwrap();
        assert_eq!(
            records
                .iter()
                .map(|record| record.format.as_str())
                .collect::<Vec<_>>(),
            ["lcov", "cobertura-xml", "coverage.py-json", "istanbul-json"]
        );
        assert_eq!(records[0].files["app.rs"].measured_lines, [1, 2]);
        assert_eq!(records[0].files["app.rs"].covered_lines, [1]);
        assert_eq!(records[1].files["app.rs"].covered_lines, [2]);
        assert_eq!(records[3].files["app.rs"].covered_lines, [3]);
    }

    #[test]
    fn coverage_paths_cannot_escape_or_follow_symlinked_reports() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        std::fs::write(repo.join("app.rs"), "fn main() {}\n").unwrap();
        assert_eq!(normalize_coverage_path(&repo, "../outside.rs"), None);
        let report = repo.join("coverage.info");
        std::fs::write(&report, "TN:\nSF:app.rs\nDA:1,1\n").unwrap();
        let link = repo.join("linked.info");
        std::os::unix::fs::symlink(report, &link).unwrap();
        assert!(ingest_coverage_reports(&repo, &[link]).is_err());
    }
}
