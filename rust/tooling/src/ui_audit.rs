//! UI implementation audit queue, evidence import, verifier, and completion gate.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::audit_common::sha256_file;
use crate::audit_ledger::{
    create_directory_all_nofollow, read_bytes_nofollow, validate_directory_nofollow,
    write_bytes_nofollow,
};
use crate::audit_queue::{self, ArtifactOwnership, AuditUnit, CollectOptions, FileEntry};
use crate::ui_gate::{self, ExplicitUiBasis};

pub const ARTIFACT_OWNER: &str = "ui-implementation-audit";
pub const ARTIFACT_MARKER: &str = ".ui-implementation-audit-artifacts.json";
pub const INAPPLICABLE_EXIT: u8 = 3;

pub(crate) const UX_CRITERIA: &[&str] = &[
    "journey-first",
    "step-necessity",
    "surface-purpose",
    "copy-purpose",
    "product-language",
    "project-guidelines",
    "context-inheritance",
    "contextual-actions",
    "compact-choices",
    "progressive-disclosure",
    "overlay-behavior",
    "generated-results",
    "contextual-help",
];

pub(crate) const CONTEXT_CRITERIA: &[&str] = &[
    "context-inheritance",
    "contextual-actions",
    "compact-choices",
    "progressive-disclosure",
    "overlay-behavior",
    "generated-results",
    "contextual-help",
];

pub(crate) const INTERACTION_SCENARIOS: &[&str] = &[
    "completion",
    "cancellation",
    "validation-error",
    "recovery",
    "persistence",
    "loading",
    "empty",
    "long-content",
];

pub(crate) const UPDATE_SCENARIOS: &[&str] = &[
    "update-startup",
    "update-periodic",
    "update-download",
    "update-ready",
    "update-restart",
    "update-recovery",
];

const MOCKUP_TOKENS: &[&str] = &[
    "comp",
    "design",
    "figma",
    "flow",
    "journey",
    "mockup",
    "prototype",
    "screen",
    "screenshot",
    "spec",
    "ui",
    "ux",
    "wire",
    "wireframe",
];
const MOCKUP_DIRS: &[&str] = &[
    "design",
    "designs",
    "figma",
    "flow",
    "flows",
    "mockup",
    "mockups",
    "prototype",
    "prototypes",
    "screen",
    "screens",
    "screenshot",
    "screenshots",
    "spec",
    "specs",
    "ux",
    "wireframe",
    "wireframes",
];
const REQUIREMENT_TOKENS: &[&str] = &[
    "acceptance",
    "design",
    "flow",
    "journey",
    "mockup",
    "persona",
    "prd",
    "product",
    "requirements",
    "route",
    "scenario",
    "screen",
    "spec",
    "story",
    "ui",
    "ux",
    "workflow",
];
const REQUIREMENT_EXTENSIONS: &[&str] = &[
    ".md",
    ".mdx",
    ".markdown",
    ".txt",
    ".json",
    ".jsonc",
    ".yaml",
    ".yml",
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VisualAsset {
    pub rel_path: String,
    pub role: String,
    pub size_bytes: usize,
    pub sha256: String,
    pub evidence: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RequirementSource {
    pub rel_path: String,
    pub kind: String,
    pub size_bytes: usize,
    pub sha256: String,
    pub evidence: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FormalConfigSource {
    pub rel_path: String,
    pub sha256: String,
    pub size_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct BuildOptions {
    pub repo: PathBuf,
    pub out: PathBuf,
    pub run_id: String,
    pub generated_at: String,
    pub archive_stamp: String,
    pub verifier_program: PathBuf,
    pub batch_size: usize,
    pub max_batch_bytes: usize,
    pub collection: CollectOptions,
    pub forced_mockups: BTreeSet<String>,
    pub forced_journey_files: BTreeSet<String>,
    pub implementation_evidence: BTreeMap<String, Option<ExplicitUiBasis>>,
    pub split_visual_discovery: bool,
    pub ui_platform: String,
    pub formal_config: Option<String>,
}

fn ownership() -> ArtifactOwnership {
    let mut owner = ArtifactOwnership {
        owner: ARTIFACT_OWNER.to_owned(),
        marker_name: ARTIFACT_MARKER.to_owned(),
        ..ArtifactOwnership::default()
    };
    owner.known_generated_artifacts.extend(
        [
            "execution_ledger.json",
            "mockup_asset_audit.md",
            "visual_tooling_audit.md",
            "visual_comparison_audit.md",
        ]
        .map(str::to_owned),
    );
    owner
}

fn suffix(rel_path: &str) -> String {
    Path::new(rel_path)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{}", value.to_ascii_lowercase()))
        .unwrap_or_default()
}

fn parent_parts(rel_path: &str) -> BTreeSet<String> {
    Path::new(rel_path)
        .parent()
        .into_iter()
        .flat_map(Path::components)
        .filter_map(|component| component.as_os_str().to_str())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn read_prefix(path: &Path, anchor: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let bytes = read_bytes_nofollow(path, Some(anchor))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("file does not exist: {}", path.display()))?;
    Ok(bytes[..bytes.len().min(limit)].to_vec())
}

pub fn load_formal_config(
    repo: &Path,
    raw_path: Option<&str>,
    ui_platform: &str,
) -> Result<Option<FormalConfigSource>, String> {
    if ui_platform == "native" {
        if raw_path.is_some() {
            return Err("--formal-config is valid only for web or hybrid audits".to_owned());
        }
        return Ok(None);
    }
    let Some(raw_path) = raw_path else {
        return Ok(None);
    };
    let rel_path = audit_queue::validate_repo_relative_include(repo, raw_path)?;
    let path = repo.join(&rel_path);
    let bytes = read_bytes_nofollow(&path, Some(repo))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            "--formal-config must name a regular non-symlink repository file".to_owned()
        })?;
    let payload = crate::audit_findings::strict_json_object(&bytes, "--formal-config")
        .map_err(|error| format!("--formal-config must contain valid UTF-8 JSON: {error}"))?;
    if !payload.get("targets").is_some_and(Value::is_array)
        && !payload.get("targetDefaults").is_some_and(Value::is_object)
    {
        return Err("--formal-config must declare targets or targetDefaults".to_owned());
    }
    Ok(Some(FormalConfigSource {
        rel_path,
        sha256: sha256_file(&path)?,
        size_bytes: bytes.len(),
    }))
}

pub fn assess_implementation_gate(
    repo: &Path,
    evidence: &BTreeMap<String, Option<ExplicitUiBasis>>,
    entries: &[FileEntry],
) -> Value {
    let by_path = entries
        .iter()
        .map(|entry| (entry.rel_path.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let mut accepted = Vec::new();
    let mut rejected = Vec::new();
    for (rel_path, basis) in evidence {
        let result = by_path
            .get(rel_path.as_str())
            .filter(|entry| entry.kind != "source/ui-asset")
            .ok_or_else(|| "not a collected executable source file".to_owned())
            .and_then(|_| ui_gate::qualify_implementation_source(repo, rel_path, basis.as_ref()));
        match result {
            Ok(qualification) => {
                let entry = by_path[rel_path.as_str()];
                accepted.push(json!({
                    "rel_path":rel_path,
                    "sha256":entry.sha256,
                    "evidence":if basis.is_some() {"lead-inspected via --implemented-ui-override"} else {"lead-inspected via --implemented-ui-file"},
                    "qualification":qualification,
                }));
            }
            Err(reason) => rejected.push(json!({"rel_path":rel_path,"reason":reason})),
        }
    }
    let (status, reason) = if evidence.is_empty() {
        (
            "not-applicable",
            "no repo-owned executable product UI implementation file was named",
        )
    } else if !rejected.is_empty() {
        (
            "not-applicable",
            "one or more named files do not prove an implemented target UI surface",
        )
    } else if accepted.is_empty() {
        (
            "not-applicable",
            "no named file proves an implemented target UI surface",
        )
    } else {
        (
            "passed",
            "at least one repo-owned substantive product UI surface is implemented",
        )
    };
    json!({
        "schema_version":2,
        "status":status,
        "reason":reason,
        "evidence_files":accepted,
        "rejected_files":rejected,
    })
}

fn visual_asset_candidate(rel_path: &str) -> bool {
    let mut extensions = audit_queue::UI_ASSET_EXTENSIONS.to_vec();
    extensions.extend([".pdf", ".svg"]);
    if !extensions.contains(&suffix(rel_path).as_str()) {
        return false;
    }
    let parts = parent_parts(rel_path);
    let words = audit_queue::filename_words(
        Path::new(rel_path)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default(),
    );
    audit_queue::is_ui_asset_path(rel_path)
        || parts
            .iter()
            .any(|part| MOCKUP_DIRS.contains(&part.as_str()))
        || words
            .iter()
            .any(|word| MOCKUP_TOKENS.contains(&word.as_str()))
        || parts
            .iter()
            .any(|part| audit_queue::UI_ASSET_DIRS.contains(&part.as_str()))
        || words
            .iter()
            .any(|word| audit_queue::UI_ASSET_NAME_TOKENS.contains(&word.as_str()))
}

fn visual_asset_role(rel_path: &str, forced: &BTreeSet<String>) -> Option<&'static str> {
    if forced.contains(rel_path) {
        return Some("mockup");
    }
    if !visual_asset_candidate(rel_path) {
        return None;
    }
    let parts = parent_parts(rel_path);
    let words = audit_queue::filename_words(
        Path::new(rel_path)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default(),
    );
    if parts
        .iter()
        .any(|part| MOCKUP_DIRS.contains(&part.as_str()))
        || words
            .iter()
            .any(|word| MOCKUP_TOKENS.contains(&word.as_str()))
    {
        Some("mockup")
    } else {
        Some("ui-asset")
    }
}

fn collect_candidate_paths(
    repo: &Path,
    include_generated: bool,
    include_vendor: bool,
    include_assets: bool,
    output_rel_dirs: &[String],
) -> BTreeSet<String> {
    let mut paths = audit_queue::run_git_files(repo)
        .unwrap_or_else(|| audit_queue::walk_files(repo, include_generated, include_vendor))
        .into_iter()
        .collect::<BTreeSet<_>>();
    if include_assets {
        paths.extend(audit_queue::run_git_ignored_files(
            repo,
            include_generated,
            include_vendor,
            output_rel_dirs,
        ));
    }
    paths
}

fn discover_visual_assets(
    options: &BuildOptions,
    collected_entries: &[FileEntry],
) -> Vec<VisualAsset> {
    let mut candidates = collect_candidate_paths(
        &options.repo,
        options.collection.include_generated,
        options.collection.include_vendor,
        options.collection.include_assets,
        &options.collection.output_rel_dirs,
    );
    candidates.extend(options.forced_mockups.iter().cloned());
    candidates.extend(
        collected_entries
            .iter()
            .filter(|entry| visual_asset_candidate(&entry.rel_path))
            .map(|entry| entry.rel_path.clone()),
    );
    candidates
        .into_iter()
        .filter_map(|rel_path| {
            let forced = options.forced_mockups.contains(&rel_path);
            let excluded =
                audit_queue::excluded_by_output_dir(&rel_path, &options.collection.output_rel_dirs)
                    .or_else(|| {
                        audit_queue::excluded_by_dir(
                            &rel_path,
                            options.collection.include_generated,
                            options.collection.include_vendor,
                        )
                    })
                    .or_else(|| {
                        audit_queue::matches_any_glob(&rel_path, &options.collection.exclude_globs)
                    });
            if excluded.is_some() && !forced {
                return None;
            }
            let role = visual_asset_role(&rel_path, &options.forced_mockups)?;
            let path = options.repo.join(&rel_path);
            let bytes = read_bytes_nofollow(&path, Some(&options.repo))
                .ok()
                .flatten()?;
            Some(VisualAsset {
                rel_path,
                role: role.to_owned(),
                size_bytes: bytes.len(),
                sha256: sha256_file(&path).ok()?,
                evidence: if forced {
                    "forced by --mockup"
                } else if role == "mockup" {
                    "mockup/design path or filename"
                } else {
                    "UI asset path or filename"
                }
                .to_owned(),
            })
        })
        .collect()
}

fn requirement_evidence(repo: &Path, rel_path: &str, forced: bool) -> Option<&'static str> {
    if forced {
        return Some("forced by --journey-file");
    }
    if !REQUIREMENT_EXTENSIONS.contains(&suffix(rel_path).as_str()) {
        return None;
    }
    let path = Path::new(rel_path);
    let parts = parent_parts(rel_path);
    let words = audit_queue::filename_words(
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default(),
    );
    if words
        .iter()
        .any(|word| REQUIREMENT_TOKENS.contains(&word.as_str()))
        || parts.iter().any(|part| {
            [
                "docs",
                "documentation",
                "product",
                "requirements",
                "spec",
                "specs",
                "ux",
                "design",
            ]
            .contains(&part.as_str())
        })
    {
        return Some("requirement-like path or filename");
    }
    let bytes = read_prefix(&repo.join(rel_path), repo, 400_000).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    Regex::new(r"(?i)\b(?:user journey|workflow|persona|acceptance criteria|screen sequence|primary action|responsive|mockup|wireframe|figma|visual design|ui requirement|ux requirement)\b")
        .expect("requirement regex")
        .is_match(&text)
        .then_some("journey or UI requirement terms in file")
}

fn discover_requirement_sources(
    options: &BuildOptions,
    collected_entries: &[FileEntry],
) -> Vec<RequirementSource> {
    let mut candidates = collected_entries
        .iter()
        .map(|entry| entry.rel_path.clone())
        .collect::<BTreeSet<_>>();
    candidates.extend(options.forced_journey_files.iter().cloned());
    candidates.extend(
        collect_candidate_paths(
            &options.repo,
            options.collection.include_generated,
            options.collection.include_vendor,
            true,
            &options.collection.output_rel_dirs,
        )
        .into_iter()
        .filter(|rel_path| REQUIREMENT_EXTENSIONS.contains(&suffix(rel_path).as_str())),
    );
    candidates
        .into_iter()
        .filter_map(|rel_path| {
            let forced = options.forced_journey_files.contains(&rel_path);
            if audit_queue::excluded_by_output_dir(&rel_path, &options.collection.output_rel_dirs)
                .is_some()
                && !forced
            {
                return None;
            }
            let evidence = requirement_evidence(&options.repo, &rel_path, forced)?;
            let path = options.repo.join(&rel_path);
            let bytes = read_bytes_nofollow(&path, Some(&options.repo))
                .ok()
                .flatten()?;
            Some(RequirementSource {
                rel_path,
                kind: if forced {
                    "forced-requirement"
                } else {
                    "requirement-candidate"
                }
                .to_owned(),
                size_bytes: bytes.len(),
                sha256: sha256_file(&path).ok()?,
                evidence: evidence.to_owned(),
            })
        })
        .collect()
}

fn unit_lines(entries: &[AuditUnit]) -> String {
    entries
        .iter()
        .map(|entry| {
            let location = if let Some(start) = entry.start_line {
                format!("lines {start}-{}", entry.end_line.unwrap_or(start))
            } else if let Some(start) = entry.start_byte {
                format!("bytes {start}-{}", entry.end_byte.unwrap_or(start))
            } else {
                format!("{} bytes", entry.size_bytes)
            };
            format!(
                "- Unit `{}`: `{}` {} ({}, sha256=`{}`)",
                entry.unit_id, entry.rel_path, location, entry.kind, entry.sha256
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn compact_assets(assets: &[VisualAsset], role: Option<&str>) -> String {
    let chosen = assets
        .iter()
        .filter(|asset| role.is_none_or(|role| asset.role == role))
        .collect::<Vec<_>>();
    if chosen.is_empty() {
        return "- None found.".to_owned();
    }
    let mut rows = chosen
        .iter()
        .take(80)
        .map(|item| {
            format!(
                "- `{}` ({}, {} bytes, sha256=`{}`, evidence={})",
                item.rel_path, item.role, item.size_bytes, item.sha256, item.evidence
            )
        })
        .collect::<Vec<_>>();
    if chosen.len() > 80 {
        rows.push(format!(
            "- ... {} more listed in manifest.json",
            chosen.len() - 80
        ));
    }
    rows.join("\n")
}

fn compact_requirements(requirements: &[RequirementSource]) -> String {
    if requirements.is_empty() {
        return "- None found.".to_owned();
    }
    let mut rows = requirements
        .iter()
        .take(80)
        .map(|item| {
            format!(
                "- `{}` ({}, sha256=`{}`, evidence={})",
                item.rel_path, item.kind, item.sha256, item.evidence
            )
        })
        .collect::<Vec<_>>();
    if requirements.len() > 80 {
        rows.push(format!(
            "- ... {} more listed in manifest.json",
            requirements.len() - 80
        ));
    }
    rows.join("\n")
}

fn isolated_worker_contract() -> &'static str {
    "Run this worker in a fresh isolated context and pass this complete prompt plus applicable project-ledger requirements. Do not prescribe or validate a reasoning-effort level; use the runtime/user-selected worker default. Do not rely on an inherited lead transcript. If workers cannot be spawned, the lead may perform the same bounded work through the documented manual fallback. Write the complete report to the exact path below and return only the bounded filename-bearing receipt."
}

fn render_batch_prompt(
    options: &BuildOptions,
    batch_id: usize,
    total_batches: usize,
    entries: &[AuditUnit],
    assets: &[VisualAsset],
    requirements: &[RequirementSource],
    report_path: &Path,
) -> Result<String, String> {
    Ok(format!(
        r#"# UI Implementation Audit Batch {batch_id:03}/{total_batches:03}

Run ID: `{run_id}`
Repo root: `{repo}`
Batch ID: `batch_{batch_id:03}`

{delivery}
{isolation}

You are a worker auditing interface source implementation. Do not edit the audited repository; write only the exact audit artifact authorized above. Inspect every owned unit below and compare source-defined UI behavior, visible text, layout, state handling, responsive intent, implementation paths, and test evidence against the mockup/assets, required UI elements, features, and journey requirements listed here and in `manifest.json`.

## Files You Own

{units}

For ranged units, inspect the assigned range manually plus nearby imports/types/callers/styles only as needed. In `File Coverage` and `UI Source Inventory`, use the exact unit id.

## Mockup And Asset Evidence

{assets}

## Journey Requirement Evidence

{requirements}

## Review Rules

- Inventory every visible label, control, field, menu, route link, toast, banner, empty/loading/error state, layout container, and visual/test evidence, including additions not justified by requirements. Include conditional supporting copy and all separate pages, tabs, modes, and dialogs so the visual worker can reconcile its surface and copy inventory.
- Record source wiring references for handlers, state, navigation, API/persistence, permissions, validation, and missing state branches when the UI promises behavior. A path/symbol reference proves only that the source anchor exists; runtime or test evidence is required to prove the observable outcome.
- Compare implementation to mockup/journey evidence: hierarchy, density, spacing, imagery, typography intent, copy, responsiveness, required decision information, feature behavior, and test evidence.
- Flag source order, layout rules, or default state that plausibly elevate low-relevance settings, rare/admin controls, debug detail, or secondary metadata above journey-critical content. Leave rendered conclusions to the visual worker and formal evidence.
- Flag implementation commentary, redundant explanations, repeated entry, and separate surfaces justified only by data structures. Record relevant product requirements, effective UI guidelines, and glossary sources; matching a mockup does not excuse unnecessary user effort. Do not classify necessary guidance or genuinely user-facing technical tasks as leakage.
- Flag missing UI elements, unwired handlers, missing data/persistence paths, missing states, missing accessibility paths, and missing safe visual states or fixture paths when source implies heavy or production-only operations.

## Required Report File

Write exactly these top-level headings in order to the report path above:

## Run ID
{run_id}

## Batch ID
batch_{batch_id:03}

## Batch Summary
Briefly summarize the UI surfaces these files define.

## File Coverage
| Unit | Status | SHA-256 | Purpose |
| --- | --- | --- | --- |
| exact unit id | CHECKED | exact sha256 | one-line UI purpose |

## UI Source Inventory
| Unit | File | Surface | Visible Element | Source Evidence | Expected Behavior | Actual Implementation | Handler Reference | Backend/API Reference | Permission Reference | Persistence Reference | Test Reference | Responsive/State Notes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| exact unit id | repo-relative file | screen/component/style/message catalog | label/control/state/layout | source line/copy/style evidence | mockup/journey/feature/test expectation or inferred standard | implemented/missing path | `path#symbol`, `missing`, or `not-applicable: rationale` | same structured form | same structured form | same structured form | real `test-path#test-name`, `missing`, or justified not-applicable | desktop/mobile/state notes |

## Mockup And Journey Alignment
Explain how the owned UI source aligns or conflicts with the listed mockups/assets, required UI elements, features, tests, and journey requirements. Mention missing target evidence if no relevant mockup or journey exists.

## Implementation Gap Findings
Use `No findings.` or one block per gap:

- Priority: P0/P1/P2/P3
- Files: repo-relative files owned by this batch
- Mockup/requirement evidence: asset, journey doc, route, or explicit absence
- Interface evidence: source file, visible text, handler, style, or state
- Expected behavior/standard: expected visual, journey, feature, UI element, implementation, or test behavior
- Gap: concrete mismatch, missing element, unwired path, or missing test evidence
- Suggested implementation direction: specific fix direction

## No Gap Notes
List units or UI behaviors that look aligned and why.

## Open Questions
List ambiguity for the lead, or `None.`
"#,
        batch_id = batch_id,
        total_batches = total_batches,
        run_id = options.run_id,
        repo = options.repo.display(),
        delivery = audit_queue::artifact_delivery_contract(report_path)?,
        isolation = isolated_worker_contract(),
        units = unit_lines(entries),
        assets = compact_assets(assets, None),
        requirements = compact_requirements(requirements),
    ))
}

fn render_split_prompt(
    options: &BuildOptions,
    worker: &str,
    report_path: &Path,
    assets: &[VisualAsset],
    requirements: &[RequirementSource],
    entries: &[FileEntry],
) -> Result<String, String> {
    let (title, body, sections) = if worker == "mockup_asset_audit" {
        (
            "Mockup And Asset Worker",
            format!(
                "Inventory the design target from mockups/assets and journey requirement sources, including required screens, features, UI elements, states, implementation expectations, and test expectations. Use image-viewing tools when available.\n\n## Mockup And Asset Inputs\n\n{}\n\n## Journey Requirement Inputs\n\n{}",
                compact_assets(assets, None),
                compact_requirements(requirements)
            ),
            "## Mockup/Asset Inputs\nList each input used and whether it was visually inspected.\n\n## Journey Requirement Inputs\nList sources and implied journeys.\n\n## Expected Screens And Visual Requirements\nList the journey decision model, screens, states, layout, UI elements, behavior, implementation, tests, and responsive expectations.",
        )
    } else {
        let files = entries
            .iter()
            .map(|entry| {
                format!(
                    "- `{}` ({}, sha256=`{}`)",
                    entry.rel_path, entry.kind, entry.sha256
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        (
            "Visual Tooling Worker",
            format!(
                "Identify how to render the implemented UI safely and how required screens, elements, states, and visual tests can be exercised.\n\n## Interface Source Files\n\n{}\n\n## Journey Requirement Inputs\n\n{}",
                if files.is_empty() { "- None." } else { &files },
                compact_requirements(requirements)
            ),
            "## Tooling Inventory\nList exact tools, configs, scripts, routes, stories, specs, or their absence.\n\n## Safe Run Path\nList exact commands, test-mode requirements, and routes/screens or the blocker.\n\n## Desktop/Mobile Screenshot Plan\nList desktop, native, and mobile checks, artifacts, usability, readability, and content hierarchy evidence.",
        )
    };
    Ok(format!(
        "# UI Implementation Audit: {title}\n\nRun ID: `{run_id}`\nRepo root: `{repo}`\nWorker: `{worker}`\n\n{delivery}\n{isolation}\n\nDo not edit the audited repository; write only the exact audit artifact authorized above. {body}\n\nWrite exactly these sections to the report path above:\n\n## Run ID\n{run_id}\n\n## Worker\n{worker}\n\n{sections}\n\n## Findings\nUse `No findings.` or complete UI implementation finding blocks.\n\n## Open Questions\nList blockers or `None.`\n",
        title = title,
        run_id = options.run_id,
        repo = options.repo.display(),
        worker = worker,
        delivery = audit_queue::artifact_delivery_contract(report_path)?,
        isolation = isolated_worker_contract(),
        body = body,
        sections = sections,
    ))
}

fn render_visual_comparison_prompt(
    options: &BuildOptions,
    assets: &[VisualAsset],
    requirements: &[RequirementSource],
    formal_config: Option<&FormalConfigSource>,
    report_path: &Path,
) -> Result<String, String> {
    let config = formal_config.map_or_else(
        || "missing — formal web verification must be BLOCKED and reported as a finding".to_owned(),
        |config| format!("`{}` (sha256=`{}`)", config.rel_path, config.sha256),
    );
    Ok(format!(
        r#"# UI Implementation Audit: Visual Comparison Worker

Run ID: `{run_id}`
Repo root: `{repo}`
Worker: `visual_comparison_audit`
Declared UI platform: `{platform}`
Formal web config: {config}

{delivery}
{isolation}

Authorized visual evidence manifest: `{evidence}`.
Screenshot, formal-verifier, journey-evidence, changed-review queue, decision, and manual-review artifacts may be written only beneath the same audit-output directory and must be registered in that manifest.

Do not edit the audited repository; write only the exact audit artifacts authorized above. Use screenshot-capable tooling to compare the implemented UI against mockups/assets, required UI elements, feature behavior, tests, and user journey requirements. If the UI cannot be rendered, create desktop and mobile `BLOCKED` rows with concrete tool/route evidence and report the missing visual harness as a finding.

For native captures, add `screenshot` or `native-snapshot` records to `visual_evidence.json`. For web evidence, do not transcribe formal artifacts by hand. Run the formal verifier only with the manifest-bound config, complete changed-image review, then invoke `devcoordinator2-tooling audit ui-implementation import-formal` with the audit root, audit run id, formal report, journey-evidence manifest, review-queue, and manual-review manifest. The importer registers the formal report, ordered journey bundle, screenshot pairs, queue, and review manifest and rejects path/hash/run mismatches.

For platform `web`, formal browser evidence is required. For `native`, formal web evidence is not applicable and native screenshots/snapshots are required. For `hybrid`, provide both. Run deterministic checks before manual image review. Read `review-queue.json`: open only each queued cell's initial-viewport and full-page images, never carried unchanged images. Record decisions and finalize them with `devcoordinator2-tooling formal-ui review`.

Derive the complete journey inventory from user requirements before mapping existing screens; include requested but missing journeys, give each a stable Journey ID, and record requirement evidence and unresolved assumptions. Reconcile it against every source worker's surface and conditional-copy inventory. Do not infer completeness merely from the screens that exist. Every rendered viewport must support the current user task, including data-entry forms.

Observe each journey from its real starting situation through its completed outcome, including applicable cancellation and recovery. In Observed path enumerate the actual actions, decisions, navigation, waits, repeated entry, and backtracking; assess each step's necessity in the step-necessity review. Distinguish actual execution from an inferred or unavailable path. A screenshot or source anchor alone does not prove saving, processing, or completion. Do not claim measured efficiency gains without measurement.

For each Journey ID assess every required criterion: {ux_criteria}. Use the effective universal and project UI guidelines, relevant glossary, and confirmed product requirements; name the applicable source in Guideline source. The criteria mean: journey-first checks user outcomes rather than implementation structure; step-necessity challenges avoidable effort and compares a simpler valid path; surface-purpose justifies each separate destination or mode; copy-purpose asks whether each status/helper/explanation belongs at that point; product-language excludes development commentary unless reviewing it is the user's actual task; project-guidelines checks relevant project-specific expectations and vocabulary. Do not treat mockup fidelity as an exemption from these checks.

List exact surface names separated by semicolons in each flow row. Provide a surface-purpose row for every surface and copy-purpose rows for every supporting-text item on that surface, including conditional text. If none exists, provide an evidenced NOT_APPLICABLE row for that surface. Other criteria may use Surface=all. Keep each item separate so omissions and proposed removals are reviewable. User benefit explains the user's action, decision, understanding, or error prevention, not the implementation. Consider clearer labels, defaults, placement, or simpler behavior before adding copy. Preserve necessary guidance, domain terminology, and justified separate tasks; do not impose click quotas or collapse everything into one screen.

Use PASS, GAP, or BLOCKED for flow results; guideline rows may also use justified NOT_APPLICABLE, except journey-first, step-necessity, and surface-purpose. Every row needs an observation, reason, and registered evidence:<id>, or an exact blocker when evidence cannot be obtained. GAP/BLOCKED rows must reference complete findings by Finding ID; PASS/NOT_APPLICABLE rows use Finding=none. Add a unique `- Finding ID: UX-001` after `- Priority` in referenced finding blocks. Link multiple IDs with semicolons. A flow cannot PASS while one of its guideline assessments remains GAP/BLOCKED. Missing evidence is not a pass. These are reviewer-owned judgments, not questions to ask the user for every item; the verifier checks coverage and consistency, not subjective correctness.

For every surface assess the concrete control criteria individually: inherit known project/parent values and show infrequently changed context as clickable text rather than permanent full-size selectors; keep actions beside their object, including useful empty-state actions; use direct one-click icon-and-label choices for small option sets; reveal optional fields on demand without losing values or focus; make dropdowns overlay content rather than stretch forms; keep live generated results consistent with changed inputs and saved values, including pending and error states; put detailed explanations and examples behind small contextual help buttons without hiding essential guidance. Name every affected control in Item. PASS requires observed runtime behavior; use evidenced, reasoned NOT_APPLICABLE rows where no such control exists. A blanket Surface=all or unnamed passing item cannot cover these criteria. Reconcile with the source inventory; preserve useful help and legitimate selectors instead of imposing click quotas.

Derive the UI Configuration Contract from supported product configurations and requirement sources before selecting rendered evidence. Record exact platform/theme/viewport/input combinations and applicable Journey IDs, separated by semicolons; do not derive scope from the screenshots available. Platform is web, desktop:<target>, or native:<target>. Desktop app configurations name their Update journey; other configurations use none. Every journey must appear in at least one configuration. If support is unknown, use an explicitly unknown configuration and BLOCKED interactions/build review with a finding, not an invented supported configuration.

For each declared journey/configuration pair, record completion, cancellation, validation-error, recovery, persistence, loading, empty, and long-content. Scenarios that genuinely do not apply need a reasoned NOT_APPLICABLE row; completion cannot be waived. Exercise that configuration's input method, actual viewport, and supported theme rather than assuming another cell proves it. Desktop Update journeys additionally require update-startup, update-periodic, update-download, update-ready, update-restart, and update-recovery, using an isolated test installation. Verify background checks/downloads, readiness, user-triggered restart with unsaved-work handling, and failure recovery. PASS requires trace, video, or ordered journey evidence; screenshots and aggregate formal reports do not establish interaction completion. Name the exact evidence segment/cell and expected observable result.

Use desktop:<target> for installed desktop applications; native:<target> is for other native targets. Desktop update checks/downloads must not interrupt ongoing work; a small caption-area Update button appears only after download and installs/restarts on user activation. Native/desktop execution requires a trace or video record with platform metadata exactly matching the declared target, such as desktop:linux-x64. Imported browser journey evidence cannot substitute for native execution.

Rendered Build Review binds every configuration to the actual URL or native package/launch target, expected source/build snapshot, and observed snapshot. A stale or inaccessible target cannot PASS. Preserve unknown identity as BLOCKED. Do not place development identifiers in normal product UI merely to expose them to an audit; use existing build/deployment evidence.

Design Decision Review concerns the current supplied design target and available options only. Do not check whether mockups existed before implementation, compare creation dates, reconstruct historical mockups, or fail solely because historical mockups are absent. Applicability is alternatives, approved-design, routine-fix, or unavailable. For alternatives, review three named materially different options, the selected one, concrete layout/hierarchy/interaction distinctions, and the recorded user selection or explicit autonomous authority. Approved designs and routine fixes do not need three new proposals. Unavailable design material is a reasoned NOT_APPLICABLE design review, not fabricated fidelity proof; the rest of the UX audit continues. Reference confirmed Coordinator decisions without introducing new approval rounds. Reuse one design review across its named journeys.

Bind policy, requirement, glossary, and design references to their effective revision or immutable Coordinator ref, retaining bounded exports as audit evidence when necessary. Existing manifest hashes bind repository inputs. Never count a registered record's existence as proof of the reviewer's semantic judgment. Preserve the four new tables in final synthesis with their exact configuration, snapshot, and evidence bindings. Do not turn this audit into a delivery watchdog or resource-accounting workflow, or require a full audit before preliminary delivery.

Run the interaction checklist: badge-detail, row-hit-target, navigation-cursor, transient-disclosure, disclosure-scrollbar, icon-meaning, stable-expansion-width, hover-copy, status-summary, and message-metadata.

## Mockup And Asset Evidence

{assets}

## Journey Requirement Evidence

{requirements}

Write exactly these sections to the report path above:

## Run ID
{run_id}

## Worker
visual_comparison_audit

## Mockup And Asset Inventory
List evidence used or `None.`

## Visual Tooling
Name the safe render/capture path and viewport plan, or the concrete blocker.

## Journey Decision Model
| Journey ID | Requirement evidence | Surface | Primary user goal | Primary decision | Required facts | Warning/flag conditions | Frequent actions | Secondary/rare actions | Unconfirmed assumptions |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |

## Journey Flow Review
| Journey ID | Starting situation | Intended outcome | Observed path | Surfaces | Outcome evidence | Unnecessary effort | Simpler alternative | Result | Reason | Evidence | Finding |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |

## UX Guideline Review
| Journey ID | Criterion | Guideline source | Surface | Item | User benefit | Observation | Result | Reason | Evidence | Finding |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |

## UI Configuration Contract
| Config ID | Platform | Theme | Viewport | Input mode | Journey IDs | Update journey | Requirement source |
| --- | --- | --- | --- | --- | --- | --- | --- |

## Interaction Coverage
| Journey ID | Config ID | Scenario | Target | Expected result | Observation | Result | Reason | Evidence | Finding |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |

## Rendered Build Review
| Config ID | Target | Expected snapshot | Observed snapshot | Result | Reason | Evidence | Finding |
| --- | --- | --- | --- | --- | --- | --- | --- |

## Design Decision Review
| Review ID | Journey IDs | Applicability | Design target | Options | Selection | Authority | Distinctions | Result | Reason | Evidence | Finding |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |

## Rendered Journey Usability
| Platform | Viewport | Decision supported | Visible decision-driving content | Visible secondary/detail content | Detail access pattern | Readability/contrast evidence | Layout quality result | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |

For relevant rows, include every exact checklist label above in `Detail access pattern` or `Evidence`.

## Visual Comparison Checks
| Platform | Journey | Viewport | Route/Screen | Mockup/Requirement | Implementation Screenshot/Tool Evidence | Differences | Result |
| --- | --- | --- | --- | --- | --- | --- | --- |

## Formal Evidence
For web/hybrid, cite imported `formal-web-verifier`, `journey-evidence`, `review-queue`, and `manual-review` evidence ids. For native, write exactly `Formal Web UI verification not applicable to declared native platform.`

## Findings
Use exactly `No findings.` when there are no gaps; otherwise provide complete finding blocks, including Finding ID for every referenced UX finding. Put all interaction checklist labels and their pass/gap/blocked/not-applicable outcomes in the rendered usability or comparison rows, not before the no-findings sentinel.

## Open Questions
List visual blockers, missing mockups, unclear routes, or `None.`
"#,
        run_id = options.run_id,
        repo = options.repo.display(),
        platform = options.ui_platform,
        config = config,
        delivery = audit_queue::artifact_delivery_contract(report_path)?,
        isolation = isolated_worker_contract(),
        evidence = options.out.join("visual_evidence.json").display(),
        assets = compact_assets(assets, None),
        requirements = compact_requirements(requirements),
        ux_criteria = UX_CRITERIA.join(", "),
    ))
}

fn shell_quote(value: &str) -> String {
    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&byte))
    {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn write_text(path: &Path, text: &str) -> Result<(), String> {
    write_bytes_nofollow(path, text.as_bytes(), 0o600).map_err(|error| error.to_string())
}

fn archive_reports(out: &Path, stamp: &str) -> Result<Option<PathBuf>, String> {
    let reports = out.join("reports");
    let populated = reports
        .read_dir()
        .ok()
        .is_some_and(|mut entries| entries.next().is_some());
    if !populated {
        create_directory_all_nofollow(&reports, 0o700).map_err(|error| error.to_string())?;
        return Ok(None);
    }
    validate_directory_nofollow(&reports).map_err(|error| error.to_string())?;
    let mut suffix = 1usize;
    let mut archive = out.join(format!("reports.stale.{stamp}"));
    while archive.exists() {
        suffix += 1;
        archive = out.join(format!("reports.stale.{stamp}.{suffix}"));
    }
    std::fs::rename(&reports, &archive)
        .map_err(|error| format!("cannot archive {}: {error}", reports.display()))?;
    create_directory_all_nofollow(&reports, 0o700).map_err(|error| error.to_string())?;
    Ok(Some(archive))
}

fn execution_ledger(manifest: &Value) -> Value {
    let audit = &manifest["ui_implementation_audit"];
    let visual_required = audit["visual_required"].as_bool().unwrap_or(false);
    let batch_workers = manifest["batches"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|batch| {
            json!({
                "batch_id":batch["id"],"status":"pending","prompt":batch["prompt"],
                "report":batch["report"],"agent_id":Value::Null,
                "runtime_provenance":Value::Null,"fallback":false,
            })
        })
        .collect::<Vec<_>>();
    let worker = |prompt_key: &str, report_key: &str| {
        let applicable = audit[prompt_key].is_string();
        json!({
            "status":if applicable {"pending"} else {"not-applicable"},
            "prompt":audit[prompt_key],"report":audit[report_key],
            "agent_id":Value::Null,"runtime_provenance":Value::Null,
        })
    };
    json!({
        "run_id":manifest["run_id"],"repo_root":manifest["repo_root"],
        "audit_kind":"ui-implementation","provenance_scope":"lead-recorded runtime ledger",
        "worker_capability_check":{"status":"pending","spawn_tool":Value::Null,"notes":""},
        "lead":{"status":"pending","agent_id":Value::Null,"runtime_provenance":Value::Null},
        "fallback":{"status":"not-started","reason":""},
        "mockup_asset_worker":worker("mockup_asset_prompt","mockup_asset_report"),
        "visual_tooling_worker":worker("visual_tooling_prompt","visual_tooling_report"),
        "visual_comparison_worker":{
            "status":if visual_required {"pending"} else {"not-applicable"},
            "prompt":if visual_required {json!("visual_comparison_audit.md")} else {Value::Null},
            "report":if visual_required {json!("reports/visual_comparison_audit.md")} else {Value::Null},
            "agent_id":Value::Null,"runtime_provenance":Value::Null,
        },
        "batch_workers":batch_workers,
        "pruned_directory_review":{
            "status":if manifest["pruned_directory_review_hint_count"].as_u64().unwrap_or(0)>0 {"pending"} else {"not-applicable"},
            "hint_count":manifest["pruned_directory_review_hint_count"],"decisions":[],
        },
    })
}

fn queue_marker(manifest: &Value) -> Value {
    json!({
        "run_id":manifest["run_id"],"phase":"queue_generated","audit_verified":false,
        "audit_kind":"ui-implementation","manifest":"manifest.json","audit_index":"audit_index.md",
        "execution_ledger":"execution_ledger.json","excluded_files":"excluded_files.json",
        "reports_dir":"reports","logs_dir":"logs","final_report":"final-report.md",
        "ownership_marker":ARTIFACT_MARKER,"batch_count":manifest["batch_count"],
        "source_file_count":manifest["source_file_count"],
        "marker_semantics":"Queue artifacts were generated; worker reports and execution ledger still require verifier completion.",
    })
}

fn render_index(options: &BuildOptions, manifest: &Value) -> String {
    let rows = manifest["batches"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|batch| {
            format!(
                "| {} | `{}` | {} | {} | {} |",
                batch["id"].as_str().unwrap_or(""),
                batch["prompt"].as_str().unwrap_or(""),
                batch["file_count"],
                batch["coverage_unit_count"],
                batch["purpose"].as_str().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let audit = &manifest["ui_implementation_audit"];
    let mut visual = Vec::new();
    for (label, prompt_key, report_key) in [
        (
            "Mockup/assets",
            "mockup_asset_prompt",
            "mockup_asset_report",
        ),
        (
            "Visual tooling",
            "visual_tooling_prompt",
            "visual_tooling_report",
        ),
        (
            "Visual comparison",
            "visual_comparison_prompt",
            "visual_comparison_report",
        ),
    ] {
        if let (Some(prompt), Some(report)) =
            (audit[prompt_key].as_str(), audit[report_key].as_str())
        {
            visual.push(format!("- {label} prompt: `{prompt}` -> `{report}`"));
        }
    }
    format!(
        "# UI Implementation Audit Index\n\nRepo root: `{}`\nOutput directory: `{}`\nRun ID: `{}`\nAudit kind: `ui-implementation`\n\nInterface source files queued: **{}**\nCoverage units queued: **{}**\nBatches: **{}**\nVisual assets found: **{}**\nMockup assets found: **{}**\nRequirement sources found: **{}**\nDeclared UI platform: **{}**\nFormal web config: **{}**\nScope warnings: **{}**\n\n## Dispatch\n\n1. Fill `execution_ledger.json` as workers are assigned.\n2. Dispatch one fresh isolated worker per batch prompt with the runtime/user-selected worker effort and complete prompt.\n3. Workers write reports and return bounded filename-bearing `REPORT_SAVED` receipts.\n4. Dispatch visual workers below.\n5. The lead writes `final-report.md` and keeps verbose output in `logs/`.\n6. Run verifier: `{}`\n7. Return only a compact outcome/counts/verifier/artifact summary.\n\n{}\n\n## Batches\n\n| Batch | Prompt | Files | Units | Purpose |\n| --- | --- | ---: | ---: | --- |\n{}\n",
        options.repo.display(),
        options.out.display(),
        options.run_id,
        manifest["source_file_count"],
        manifest["coverage_unit_count"],
        manifest["batch_count"],
        audit["visual_asset_count"],
        audit["mockup_asset_count"],
        audit["requirement_source_count"],
        audit["ui_platform"].as_str().unwrap_or(""),
        audit["formal_config"]["rel_path"]
            .as_str()
            .unwrap_or("missing/not applicable"),
        manifest["scope_warning_count"],
        manifest["verifier_command"].as_str().unwrap_or(""),
        if visual.is_empty() {
            "- No visual worker prompt was generated.".to_owned()
        } else {
            visual.join("\n")
        },
        if rows.is_empty() {
            "| None | None | 0 | 0 | No interface source files queued |".to_owned()
        } else {
            rows
        },
    ) + "\n## Journey and UX completion\n\nPreserve the complete Journey Decision Model, Journey Flow Review, UX Guideline Review, UI Configuration Contract, Interaction Coverage, Rendered Build Review, and Design Decision Review tables in final-report.md. Keep exact journey/configuration identities, targets, snapshots, and evidence bindings; retain GAP/BLOCKED results and include linked Finding ID blocks in the Implementation Plan. Review each surface, supporting-copy item, and contextual control, including conditional states. A matching mockup does not exempt avoidable user effort, and absent historical mockups do not prove a process failure. Verifier success proves complete audit artifacts, not product readiness.\n"
}

pub fn collect_and_assess_gate(
    repo: &Path,
    collection_options: &CollectOptions,
    evidence: &BTreeMap<String, Option<ExplicitUiBasis>>,
) -> (audit_queue::FileCollection, Value) {
    let collection = audit_queue::collect_files(repo, collection_options);
    let gate = assess_implementation_gate(repo, evidence, &collection.entries);
    (collection, gate)
}

pub fn build(options: &BuildOptions) -> Result<Value, String> {
    audit_queue::run_id_token(&options.run_id)?;
    if !["web", "native", "hybrid"].contains(&options.ui_platform.as_str()) {
        return Err("--ui-platform must be web, native, or hybrid".to_owned());
    }
    let (collection, implementation_gate) = collect_and_assess_gate(
        &options.repo,
        &options.collection,
        &options.implementation_evidence,
    );
    if implementation_gate["status"] != "passed" {
        return Err(format!(
            "UI implementation audit is not applicable: {}",
            implementation_gate["reason"]
                .as_str()
                .unwrap_or("gate failed")
        ));
    }
    let formal_config = load_formal_config(
        &options.repo,
        options.formal_config.as_deref(),
        &options.ui_platform,
    )?;
    let evidence_paths = implementation_gate["evidence_files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["rel_path"].as_str())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let automatic_paths = collection
        .entries
        .iter()
        .filter(|entry| {
            entry.interface_relevant
                && entry.kind != "source/ui-asset"
                && !visual_asset_candidate(&entry.rel_path)
                && !ui_gate::is_evidence_only_interface_path(&entry.rel_path)
        })
        .map(|entry| entry.rel_path.clone())
        .collect::<BTreeSet<_>>();
    let entries = collection
        .entries
        .iter()
        .filter(|entry| {
            automatic_paths.contains(&entry.rel_path) || evidence_paths.contains(&entry.rel_path)
        })
        .cloned()
        .map(|mut entry| {
            if evidence_paths.contains(&entry.rel_path) {
                entry.interface_relevant = true;
            }
            entry
        })
        .collect::<Vec<_>>();
    let visual_assets = discover_visual_assets(options, &collection.entries);
    let requirements = discover_requirement_sources(options, &collection.entries);
    let units = audit_queue::audit_units_for(&options.repo, &entries, options.max_batch_bytes);
    let batches = audit_queue::batch_files(&units, options.batch_size, options.max_batch_bytes)?;
    audit_queue::validate_generated_artifact_tokens(&entries, &units)?;

    let owner = ownership();
    let previous_marker = audit_queue::ensure_output_dir_safe(&options.out, &options.repo, &owner)?;
    create_directory_all_nofollow(&options.out, 0o700).map_err(|error| error.to_string())?;
    if previous_marker.is_none() {
        audit_queue::write_ownership_marker(
            &options.out,
            &options.repo,
            &[],
            &options.generated_at,
            &owner,
        )?;
    }
    audit_queue::clean_generated_artifacts(&options.out, previous_marker.as_ref(), &owner)?;
    let archive = archive_reports(&options.out, &options.archive_stamp)?;
    let reports = options.out.join("reports");
    let logs = options.out.join("logs");
    create_directory_all_nofollow(&logs, 0o700).map_err(|error| error.to_string())?;

    let mut batch_records = Vec::new();
    let mut batched_paths = BTreeSet::new();
    let mut batched_units = BTreeSet::new();
    let mut duplicate_units = BTreeSet::new();
    for (offset, batch) in batches.iter().enumerate() {
        let index = offset + 1;
        let prompt = format!("batch_{index:03}.md");
        let report = format!("reports/{prompt}");
        write_text(
            &options.out.join(&prompt),
            &render_batch_prompt(
                options,
                index,
                batches.len(),
                batch,
                &visual_assets,
                &requirements,
                &options.out.join(&report),
            )?,
        )?;
        let paths = batch
            .iter()
            .map(|unit| unit.rel_path.clone())
            .collect::<BTreeSet<_>>();
        for unit in batch {
            batched_paths.insert(unit.rel_path.clone());
            if !batched_units.insert(unit.unit_id.clone()) {
                duplicate_units.insert(unit.unit_id.clone());
            }
        }
        batch_records.push(json!({
            "id":format!("batch_{index:03}"),"prompt":prompt,"report":report,
            "file_count":paths.len(),"coverage_unit_count":batch.len(),
            "interface_file_count":paths.len(),
            "byte_count":batch.iter().map(|unit|unit.size_bytes).sum::<usize>(),
            "files":paths,"coverage_units":batch.iter().map(|unit|unit.unit_id.clone()).collect::<Vec<_>>(),
            "purpose":audit_queue::purpose_for(batch),
        }));
    }

    if options.split_visual_discovery {
        write_text(
            &options.out.join("mockup_asset_audit.md"),
            &render_split_prompt(
                options,
                "mockup_asset_audit",
                &reports.join("mockup_asset_audit.md"),
                &visual_assets,
                &requirements,
                &entries,
            )?,
        )?;
        write_text(
            &options.out.join("visual_tooling_audit.md"),
            &render_split_prompt(
                options,
                "visual_tooling_audit",
                &reports.join("visual_tooling_audit.md"),
                &visual_assets,
                &requirements,
                &entries,
            )?,
        )?;
    }
    write_text(
        &options.out.join("visual_comparison_audit.md"),
        &render_visual_comparison_prompt(
            options,
            &visual_assets,
            &requirements,
            formal_config.as_ref(),
            &reports.join("visual_comparison_audit.md"),
        )?,
    )?;
    audit_queue::write_json(
        &options.out.join("visual_evidence.json"),
        &json!({"schema_version":1,"run_id":options.run_id,"artifacts":[]}),
    )?;

    let verifier_args = vec![
        options.verifier_program.to_string_lossy().into_owned(),
        "audit".to_owned(),
        "ui-implementation".to_owned(),
        "verify".to_owned(),
        "--manifest".to_owned(),
        options
            .out
            .join("manifest.json")
            .to_string_lossy()
            .into_owned(),
        "--reports".to_owned(),
        reports.to_string_lossy().into_owned(),
    ];
    let mut generated = vec![
        "audit_index.md",
        "execution_ledger.json",
        "excluded_files.json",
        "manifest.json",
        "queue_complete.json",
        "final-report.md",
        "logs",
        "visual_comparison_audit.md",
        "visual_evidence.json",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    if options.split_visual_discovery {
        generated.extend([
            "mockup_asset_audit.md".to_owned(),
            "visual_tooling_audit.md".to_owned(),
        ]);
    }
    if let Some(name) = archive
        .as_ref()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
    {
        generated.push(name);
    }
    generated.extend(
        batch_records
            .iter()
            .filter_map(|batch| batch["prompt"].as_str().map(str::to_owned)),
    );
    let source_paths = entries
        .iter()
        .map(|entry| entry.rel_path.clone())
        .collect::<BTreeSet<_>>();
    let unit_ids = units
        .iter()
        .map(|unit| unit.unit_id.clone())
        .collect::<BTreeSet<_>>();
    let missing_paths = source_paths
        .difference(&batched_paths)
        .cloned()
        .collect::<Vec<_>>();
    let extra_paths = batched_paths
        .difference(&source_paths)
        .cloned()
        .collect::<Vec<_>>();
    let missing_units = unit_ids
        .difference(&batched_units)
        .cloned()
        .collect::<Vec<_>>();
    let extra_units = batched_units
        .difference(&unit_ids)
        .cloned()
        .collect::<Vec<_>>();
    let scope_warnings = collection
        .excluded
        .iter()
        .filter(|item| item["scope_warning"] == true)
        .cloned()
        .collect::<Vec<_>>();
    let pruned_hints = collection
        .excluded
        .iter()
        .filter(|item| {
            item["entry_type"] == "directory" && item["contains_source_like_samples"] == true
        })
        .cloned()
        .collect::<Vec<_>>();
    let mockup_count = visual_assets
        .iter()
        .filter(|asset| asset.role == "mockup")
        .count();
    let all_units_once =
        missing_units.is_empty() && extra_units.is_empty() && duplicate_units.is_empty();
    let all_files_once = missing_paths.is_empty() && extra_paths.is_empty() && all_units_once;
    let excluded_digest =
        audit_queue::canonical_json_sha256(&Value::Array(collection.excluded.clone()))?;
    let formal_config_value = formal_config
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or(Value::Null);
    let ui_audit = json!({
        "ui_platform":options.ui_platform,"formal_config":formal_config_value,
        "visual_required":true,
        "visual_worker_mode":if options.split_visual_discovery {"split"} else {"combined"},
        "implementation_gate":implementation_gate,
        "source_selection":"Interface-defining non-asset source plus explicit lead-confirmed implementation evidence is queued.",
        "visual_asset_count":visual_assets.len(),"mockup_asset_count":mockup_count,
        "requirement_source_count":requirements.len(),"visual_assets":visual_assets,
        "requirement_sources":requirements,
        "mockup_asset_prompt":if options.split_visual_discovery {json!("mockup_asset_audit.md")} else {Value::Null},
        "mockup_asset_report":if options.split_visual_discovery {json!("reports/mockup_asset_audit.md")} else {Value::Null},
        "visual_tooling_prompt":if options.split_visual_discovery {json!("visual_tooling_audit.md")} else {Value::Null},
        "visual_tooling_report":if options.split_visual_discovery {json!("reports/visual_tooling_audit.md")} else {Value::Null},
        "visual_comparison_prompt":"visual_comparison_audit.md",
        "visual_comparison_report":"reports/visual_comparison_audit.md",
    });
    let invariants = json!({
        "unique_batched_file_count":batched_paths.len(),"unique_batched_unit_count":batched_units.len(),
        "missing_from_batches":missing_paths,
        "duplicates_in_batches":audit_queue::duplicate_whole_file_paths_for_batches(&batches),
        "extra_in_batches":extra_paths,"missing_units_from_batches":missing_units,
        "duplicate_units_in_batches":duplicate_units,"extra_units_in_batches":extra_units,
        "all_coverage_units_queued_exactly_once":all_units_once,
        "all_source_files_queued_exactly_once":all_files_once,
    });
    let manifest = json!({
        "repo_root":options.repo.to_string_lossy(),"run_id":options.run_id,
        "audit_kind":"ui-implementation","generated_at":options.generated_at,
        "reports_dir":reports.to_string_lossy(),"logs_dir":logs.to_string_lossy(),
        "final_report":options.out.join("final-report.md").to_string_lossy(),
        "archived_reports_dir":archive.as_ref().map(|path|path.to_string_lossy().into_owned()),
        "artifact_marker":options.out.join(ARTIFACT_MARKER).to_string_lossy(),
        "execution_ledger":options.out.join("execution_ledger.json").to_string_lossy(),
        "generated_artifacts":generated,
        "verifier_command":verifier_args.iter().map(|value|shell_quote(value)).collect::<Vec<_>>().join(" "),
        "verifier_args":verifier_args,
        "source_file_count":entries.len(),"interface_file_count":entries.len(),
        "non_interface_source_count":collection.entries.len().saturating_sub(entries.len()),
        "scope_warning_count":scope_warnings.len(),
        "pruned_directory_review_hint_count":pruned_hints.len(),
        "excluded_file_count":collection.excluded.len(),"excluded_files_sha256":excluded_digest,
        "batch_count":batch_records.len(),"source_files":entries,"coverage_unit_count":units.len(),
        "coverage_units":units,"batches":batch_records,
        "ui_implementation_audit":ui_audit,"coverage_invariants":invariants,
        "scope_warnings":scope_warnings,"pruned_directory_review_hints":pruned_hints,
    });
    audit_queue::write_json(&options.out.join("manifest.json"), &manifest)?;
    audit_queue::write_json(
        &options.out.join("excluded_files.json"),
        &Value::Array(collection.excluded),
    )?;
    write_text(
        &options.out.join("audit_index.md"),
        &render_index(options, &manifest),
    )?;
    audit_queue::write_json(
        &options.out.join("execution_ledger.json"),
        &execution_ledger(&manifest),
    )?;
    audit_queue::write_ownership_marker(
        &options.out,
        &options.repo,
        manifest["generated_artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>()
            .as_slice(),
        &options.generated_at,
        &owner,
    )?;
    audit_queue::write_json(
        &options.out.join("queue_complete.json"),
        &queue_marker(&manifest),
    )?;
    Ok(manifest)
}

fn load_json_object(path: &Path, root: &Path, label: &str) -> Result<Map<String, Value>, String> {
    let data = read_bytes_nofollow(path, Some(root))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("{label} must be a regular non-symlink JSON file"))?;
    crate::audit_findings::strict_json_object(&data, label)
        .map_err(|error| format!("{label} must contain valid UTF-8 JSON: {error}"))
}

fn confined_file(root: &Path, path: &Path, label: &str) -> Result<(PathBuf, String), String> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let absolute = if candidate.is_absolute() {
        candidate
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join(candidate)
    };
    read_bytes_nofollow(&absolute, Some(root))
        .map_err(|error| format!("{label} must be an existing regular non-symlink file: {error}"))?
        .ok_or_else(|| format!("{label} must be an existing regular non-symlink file"))?;
    let relative = absolute
        .strip_prefix(root)
        .map_err(|_| format!("{label} must remain inside the audit output"))?
        .to_string_lossy()
        .replace('\\', "/");
    Ok((absolute, relative))
}

fn safe_evidence_id(value: &str) -> String {
    let mut folded = Regex::new(r"[^A-Za-z0-9_-]+")
        .expect("safe id regex")
        .replace_all(value, "-")
        .trim_matches('-')
        .to_owned();
    if folded.is_empty()
        || !folded
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic())
    {
        folded = format!(
            "cell-{}",
            if folded.is_empty() {
                "unknown"
            } else {
                &folded
            }
        );
    }
    folded.chars().take(40).collect()
}

fn positive_u64(value: Option<&Value>) -> u64 {
    value
        .and_then(Value::as_u64)
        .or_else(|| {
            value
                .and_then(Value::as_i64)
                .filter(|value| *value > 0)
                .map(|value| value as u64)
        })
        .unwrap_or(1)
}

fn screenshot_record(root: &Path, page: &Map<String, Value>, role: &str) -> Result<Value, String> {
    let screenshot = page
        .get("screenshots")
        .and_then(Value::as_object)
        .and_then(|screenshots| screenshots.get(role))
        .and_then(Value::as_object)
        .ok_or_else(|| {
            format!(
                "formal page {} lacks {role} screenshot evidence",
                page.get("cellId").and_then(Value::as_str).unwrap_or("")
            )
        })?;
    let raw_path = screenshot
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let (path, relative) = confined_file(root, Path::new(raw_path), &format!("{role} screenshot"))?;
    let actual = sha256_file(&path)?;
    if screenshot.get("sha256") != Some(&json!(actual)) {
        return Err(format!(
            "{role} screenshot hash does not match the formal report"
        ));
    }
    let viewport = page.get("viewport").and_then(Value::as_object);
    let target = page.get("target").and_then(Value::as_object);
    let route = page
        .get("finalPath")
        .or_else(|| page.get("requestedPath"))
        .and_then(Value::as_str)
        .unwrap_or("/");
    let cell = page.get("cellId").and_then(Value::as_str).unwrap_or("cell");
    Ok(json!({
        "id":format!("formal-{}-{}",safe_evidence_id(cell),role.to_ascii_lowercase()),
        "kind":"screenshot","path":relative,"sha256":actual,"mime":"image/png",
        "route":route,
        "state":target.and_then(|value|value.get("stateName")).and_then(Value::as_str).unwrap_or("base"),
        "viewport":{
            "width":positive_u64(viewport.and_then(|value|value.get("width")).or_else(||screenshot.get("width"))),
            "height":positive_u64(viewport.and_then(|value|value.get("height")).or_else(||screenshot.get("height"))),
            "label":viewport.and_then(|value|value.get("name")).and_then(Value::as_str).unwrap_or(role),
        },
        "captured_by":"formal-web-ui-verification",
        "width":positive_u64(screenshot.get("width")),"height":positive_u64(screenshot.get("height")),
    }))
}

fn json_evidence_record(
    root: &Path,
    path: &Path,
    record_id: &str,
    kind: &str,
    formal: Option<&Map<String, Value>>,
) -> Result<Value, String> {
    let (resolved, relative) = confined_file(root, path, record_id)?;
    let mut record = json!({
        "id":record_id,"kind":kind,"path":relative,"sha256":sha256_file(&resolved)?,
        "mime":"application/json","captured_by":"formal-web-ui-verification evidence importer",
    });
    if kind == "formal-web-verifier" {
        let first = formal
            .and_then(|value| value.get("pages"))
            .and_then(Value::as_array)
            .and_then(|pages| pages.iter().find_map(Value::as_object));
        let viewport = first
            .and_then(|page| page.get("viewport"))
            .and_then(Value::as_object);
        record["route"] = json!("multiple formal web targets");
        record["state"] = json!("multiple formal web states");
        record["viewport"] = json!({
            "width":positive_u64(viewport.and_then(|value|value.get("width"))),
            "height":positive_u64(viewport.and_then(|value|value.get("height"))),
            "label":"formal web route/state/viewport set",
        });
    }
    Ok(record)
}

fn merge_evidence_records(existing: &[Value], imported: &[Value]) -> Result<Vec<Value>, String> {
    let mut by_id = BTreeMap::new();
    for item in existing {
        let id = item
            .as_object()
            .and_then(|item| item.get("id"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                "existing visual evidence contains invalid or duplicate ids".to_owned()
            })?;
        if by_id.insert(id.to_owned(), item.clone()).is_some() {
            return Err("existing visual evidence contains invalid or duplicate ids".to_owned());
        }
    }
    for item in imported {
        let id = item["id"]
            .as_str()
            .ok_or_else(|| "imported visual evidence lacks an id".to_owned())?;
        if let Some(previous) = by_id.get(id)
            && previous != item
        {
            return Err(format!("visual evidence id collision: {id}"));
        }
        by_id.insert(id.to_owned(), item.clone());
    }
    Ok(by_id.into_values().collect())
}

#[derive(Clone, Debug)]
pub struct ImportFormalOptions {
    pub audit_root: PathBuf,
    pub run_id: String,
    pub formal_report: PathBuf,
    pub journey_evidence: PathBuf,
    pub review_queue: PathBuf,
    pub manual_review: PathBuf,
}

pub fn import_formal_evidence(options: &ImportFormalOptions) -> Result<Value, String> {
    let root = validate_directory_nofollow(&options.audit_root)
        .map_err(|error| format!("audit root is unsafe: {error}"))?;
    let manifest_path = root.join("visual_evidence.json");
    let original = read_bytes_nofollow(&manifest_path, Some(&root))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "visual_evidence.json must be a regular non-symlink JSON file".to_owned())?;
    let mut manifest =
        crate::audit_findings::strict_json_object(&original, "visual_evidence.json")?;
    if manifest.get("schema_version") != Some(&json!(1))
        || manifest.get("run_id") != Some(&json!(options.run_id))
    {
        return Err("visual evidence manifest schema or audit run id does not match".to_owned());
    }
    let (report_path, _) = confined_file(&root, &options.formal_report, "formal report")?;
    let (journey_path, _) =
        confined_file(&root, &options.journey_evidence, "formal journey evidence")?;
    let (queue_path, _) = confined_file(&root, &options.review_queue, "formal review queue")?;
    let (review_path, _) = confined_file(&root, &options.manual_review, "formal manual review")?;
    let report = load_json_object(&report_path, &root, "formal report")?;
    let journey = load_json_object(&journey_path, &root, "formal journey evidence")?;
    let queue = load_json_object(&queue_path, &root, "formal review queue")?;
    let review = load_json_object(&review_path, &root, "formal manual review")?;
    let formal_run_id = report
        .get("runId")
        .and_then(Value::as_str)
        .filter(|_| report.get("schemaVersion") == Some(&json!(2)))
        .ok_or_else(|| "formal report must use schemaVersion 2 and contain runId".to_owned())?;
    if journey.get("kind") != Some(&json!("formal-web-ui-journey-evidence"))
        || journey.get("runId") != Some(&json!(formal_run_id))
    {
        return Err("journey evidence does not belong to the formal report".to_owned());
    }
    if queue.get("kind") != Some(&json!("formal-web-ui-review-queue"))
        || queue.get("runId") != Some(&json!(formal_run_id))
    {
        return Err("review queue does not belong to the formal report".to_owned());
    }
    if review.get("kind") != Some(&json!("formal-web-ui-manual-review"))
        || review.get("reviewedRunId") != Some(&json!(formal_run_id))
    {
        return Err("manual review does not belong to the formal report".to_owned());
    }
    if review.get("reportSha256") != Some(&json!(sha256_file(&report_path)?)) {
        return Err("manual review does not bind the formal report bytes".to_owned());
    }
    if review.get("reviewQueueSha256") != Some(&json!(sha256_file(&queue_path)?)) {
        return Err("manual review does not bind the review queue bytes".to_owned());
    }
    let checked_pages = report
        .get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter(|page| page.get("outcome") == Some(&json!("checked")))
        .collect::<Vec<_>>();
    if checked_pages.is_empty() {
        return Err("formal report contains no checked pages to import".to_owned());
    }
    let mut imported = vec![
        json_evidence_record(
            &root,
            &report_path,
            "formal-web",
            "formal-web-verifier",
            Some(&report),
        )?,
        json_evidence_record(
            &root,
            &journey_path,
            "formal-journey-evidence",
            "journey-evidence",
            None,
        )?,
        json_evidence_record(
            &root,
            &queue_path,
            "formal-review-queue",
            "review-queue",
            None,
        )?,
        json_evidence_record(
            &root,
            &review_path,
            "formal-manual-review",
            "manual-review",
            None,
        )?,
    ];
    for page in checked_pages {
        imported.push(screenshot_record(&root, page, "viewport")?);
        imported.push(screenshot_record(&root, page, "fullPage")?);
    }
    let existing = manifest
        .get("artifacts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    manifest.insert(
        "artifacts".to_owned(),
        Value::Array(merge_evidence_records(&existing, &imported)?),
    );
    audit_queue::write_json(&manifest_path, &Value::Object(manifest))?;
    let (_, issues) =
        crate::audit_evidence::validate_visual_evidence_manifest(&root, &options.run_id, true);
    if !issues.is_empty() {
        write_bytes_nofollow(&manifest_path, &original, 0o600)
            .map_err(|error| error.to_string())?;
        return Err(format!(
            "imported evidence failed validation: {}",
            serde_json::to_string(&issues.into_iter().take(5).collect::<Vec<_>>())
                .map_err(|error| error.to_string())?
        ));
    }
    Ok(json!({
        "ok":true,"auditRunId":options.run_id,"formalRunId":formal_run_id,
        "importedIds":imported.iter().map(|item|item["id"].clone()).collect::<Vec<_>>(),
        "evidenceManifest":manifest_path.to_string_lossy(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _directory: tempfile::TempDir,
        repo: PathBuf,
        out: PathBuf,
    }

    fn write(path: &Path, value: impl AsRef<[u8]>) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, value).unwrap();
    }

    fn git(repo: &Path, args: &[&str]) {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }

    fn fixture() -> Fixture {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path().join("repo");
        let out = directory.path().join("out");
        write(
            &repo.join("src/App.tsx"),
            "export function Dashboard() { return <main><h1>Operations Dashboard</h1><button onClick={resolve}>Resolve incident</button></main>; }",
        );
        write(
            &repo.join("src/styles.css"),
            ".dashboard { display: grid; gap: 16px; }",
        );
        write(
            &repo.join("src/server.ts"),
            "export function status() { return 200; }",
        );
        write(
            &repo.join("docs/journeys.md"),
            "# Dashboard User Journey\nGoal: review urgent incidents.\nPrimary action: Resolve incident.\nResponsive requirement: mobile keeps the incident first.\nAcceptance criteria: desktop and mobile screenshots match the mockup.\n",
        );
        write(&repo.join("design/mockups/dashboard-mobile.png"), b"png");
        write(&repo.join("public/logo.png"), b"logo");
        write(
            &repo.join("formal-web-ui.json"),
            br#"{"targets":[{"route":"/"}]}"#,
        );
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "-A"]);
        Fixture {
            _directory: directory,
            repo,
            out,
        }
    }

    fn options(fixture: &Fixture, split: bool) -> BuildOptions {
        BuildOptions {
            repo: fixture.repo.clone(),
            out: fixture.out.clone(),
            run_id: "ui-selftest-run".to_owned(),
            generated_at: "2026-09-04T00:00:00Z".to_owned(),
            archive_stamp: "20260904T000000Z".to_owned(),
            verifier_program: PathBuf::from("/usr/local/bin/devcoordinator2-tooling"),
            batch_size: 6,
            max_batch_bytes: 60_000,
            collection: CollectOptions {
                include_config: true,
                include_assets: true,
                include_files: BTreeSet::from(["src/App.tsx".to_owned()]),
                ..Default::default()
            },
            forced_mockups: BTreeSet::new(),
            forced_journey_files: BTreeSet::new(),
            implementation_evidence: BTreeMap::from([("src/App.tsx".to_owned(), None)]),
            split_visual_discovery: split,
            ui_platform: "web".to_owned(),
            formal_config: Some("formal-web-ui.json".to_owned()),
        }
    }

    #[test]
    fn builds_bound_ui_queue_and_rust_verifier_contract() {
        let fixture = fixture();
        let options = options(&fixture, false);
        let manifest = build(&options).unwrap();
        assert_eq!(manifest["audit_kind"], "ui-implementation");
        assert_eq!(
            manifest["source_file_count"], 2,
            "{}",
            manifest["source_files"]
        );
        assert_eq!(manifest["ui_implementation_audit"]["ui_platform"], "web");
        assert_eq!(
            manifest["ui_implementation_audit"]["formal_config"]["rel_path"],
            "formal-web-ui.json"
        );
        assert_eq!(
            manifest["ui_implementation_audit"]["visual_worker_mode"],
            "combined"
        );
        assert_eq!(
            manifest["ui_implementation_audit"]["implementation_gate"]["status"],
            "passed"
        );
        assert!(
            manifest["ui_implementation_audit"]["mockup_asset_count"]
                .as_u64()
                .unwrap()
                >= 1
        );
        assert!(
            manifest["ui_implementation_audit"]["requirement_source_count"]
                .as_u64()
                .unwrap()
                >= 1
        );
        assert!(
            manifest["verifier_args"]
                .as_array()
                .unwrap()
                .iter()
                .all(|value| !value.as_str().unwrap().contains("python"))
        );
        let prompt = std::fs::read_to_string(fixture.out.join("batch_001.md")).unwrap();
        for token in [
            "fresh isolated context",
            "runtime/user-selected worker default",
            "REPORT_SAVED",
            "filename=batch_001.md;",
            "REPORT_NOT_SAVED path=<exact-report-path>; reason=<short-reason>",
            "at or below 80 tokens",
        ] {
            assert!(prompt.contains(token), "{token}");
        }
        assert!(!prompt.contains("## Journey Decision Model"));
        let visual =
            std::fs::read_to_string(fixture.out.join("visual_comparison_audit.md")).unwrap();
        for token in [
            "review-queue.json",
            "devcoordinator2-tooling formal-ui review",
            "open only each queued cell",
            "initial-viewport and full-page",
            "## Journey Decision Model",
            "## Journey Flow Review",
            "## UX Guideline Review",
            "## UI Configuration Contract",
            "## Interaction Coverage",
            "## Rendered Build Review",
            "## Design Decision Review",
            "## Rendered Journey Usability",
            "Do not check whether mockups existed before implementation",
            "Approved designs and routine fixes do not need three new proposals",
            "do not derive scope from the screenshots available",
            "Imported browser journey evidence cannot substitute for native execution",
            "clickable text rather than permanent full-size selectors",
            "dropdowns overlay content rather than stretch forms",
            "small caption-area Update button appears only after download",
        ] {
            assert!(visual.contains(token), "{token}");
        }
        for criterion in UX_CRITERIA {
            assert!(visual.contains(criterion), "{criterion}");
        }
        for scenario in INTERACTION_SCENARIOS.iter().chain(UPDATE_SCENARIOS) {
            assert!(visual.contains(scenario), "{scenario}");
        }
        let index = std::fs::read_to_string(fixture.out.join("audit_index.md")).unwrap();
        for section in [
            "UI Configuration Contract",
            "Interaction Coverage",
            "Rendered Build Review",
            "Design Decision Review",
            "absent historical mockups do not prove a process failure",
        ] {
            assert!(index.contains(section), "{section}");
        }
    }

    #[test]
    fn split_mode_archives_reports_and_rejects_inapplicable_sources() {
        let fixture = fixture();
        let first = build(&options(&fixture, true)).unwrap();
        assert_eq!(
            first["ui_implementation_audit"]["visual_worker_mode"],
            "split"
        );
        assert!(fixture.out.join("mockup_asset_audit.md").is_file());
        write(&fixture.out.join("reports/old.md"), "old");
        let second = build(&options(&fixture, true)).unwrap();
        assert!(second["archived_reports_dir"].is_string());

        let mut none = options(&fixture, false);
        none.out = fixture._directory.path().join("no-evidence");
        none.implementation_evidence.clear();
        assert!(build(&none).unwrap_err().contains("not applicable"));

        write(
            &fixture.repo.join("src/Starter.tsx"),
            "export function App(){ return <main><h1>Vite + React</h1><p>Edit src/App.tsx and save to test HMR</p></main>; }",
        );
        let mut starter = options(&fixture, false);
        starter.out = fixture._directory.path().join("starter");
        starter.collection.include_files = BTreeSet::from(["src/Starter.tsx".to_owned()]);
        starter.implementation_evidence = BTreeMap::from([("src/Starter.tsx".to_owned(), None)]);
        assert!(build(&starter).unwrap_err().contains("not applicable"));
    }

    #[test]
    fn formal_evidence_import_is_hash_bound_idempotent_and_atomic() {
        const PNG: &[u8] = &[
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
        ];
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let artifacts = root.join("artifacts");
        std::fs::create_dir(&artifacts).unwrap();
        write(&artifacts.join("viewport.png"), PNG);
        write(&artifacts.join("full.png"), PNG);
        let image_sha = sha256_file(&artifacts.join("viewport.png")).unwrap();
        let source = "a".repeat(64);
        let intent = "b".repeat(64);
        let queue = json!({
            "schemaVersion":1,"kind":"formal-web-ui-review-queue","runId":"formal-1",
            "entries":[{"reviewCellKey":"cell-1","sourceFingerprint":source,
                "intentFingerprint":intent,"screenshots":{
                    "viewport":{"sha256":image_sha},"fullPage":{"sha256":image_sha}}}]
        });
        audit_queue::write_json(&artifacts.join("queue.json"), &queue).unwrap();
        let queue_sha = sha256_file(&artifacts.join("queue.json")).unwrap();
        let page = json!({
            "cellId":"cell-1","outcome":"checked","metrics":{"visibleScrollbars":[]},
            "screenshots":{
                "viewport":{"path":artifacts.join("viewport.png"),"mime":"image/png","sha256":image_sha,"width":1,"height":1},
                "fullPage":{"path":artifacts.join("full.png"),"mime":"image/png","sha256":image_sha,"width":1,"height":1}},
            "review":{"reviewCellKey":"cell-1","sourceFingerprint":source,"intentFingerprint":intent},
            "viewport":{"name":"mobile","width":390,"height":844},
            "requestedPath":"/dashboard","finalPath":"/dashboard","target":{"stateName":"base"},
        });
        let formal = json!({
            "schemaVersion":2,"runId":"formal-1","pages":[page],"findings":[],
            "coverage":{"checkedPages":1,"failed":false},
            "review":{"queueSha256":queue_sha,"cells":[]},
        });
        audit_queue::write_json(&artifacts.join("formal.json"), &formal).unwrap();
        let formal_sha = sha256_file(&artifacts.join("formal.json")).unwrap();
        let journey = json!({
            "schemaVersion":1,"kind":"formal-web-ui-journey-evidence","runId":"formal-1",
            "governedRunId":Value::Null,"governedCheck":Value::Null,
            "cells":[{"cellId":"cell-1","targetName":"Dashboard","stateName":"base","outcome":"checked",
                "screenshots":{
                    "viewport":{"path":"viewport.png","sha256":image_sha},
                    "fullPage":{"path":"full.png","sha256":image_sha}}}],
        });
        audit_queue::write_json(&artifacts.join("journey.json"), &journey).unwrap();
        let review = json!({
            "schemaVersion":1,"kind":"formal-web-ui-manual-review","reviewedRunId":"formal-1",
            "reportSha256":formal_sha,"reviewQueueSha256":queue_sha,
            "decisions":[{"reviewCellKey":"cell-1","decision":"pass","note":"",
                "sourceFingerprint":source,"intentFingerprint":intent,
                "screenshots":{"viewportSha256":image_sha,"fullPageSha256":image_sha}}],
        });
        audit_queue::write_json(&artifacts.join("review.json"), &review).unwrap();
        audit_queue::write_json(
            &root.join("visual_evidence.json"),
            &json!({"schema_version":1,"run_id":"audit-1","artifacts":[]}),
        )
        .unwrap();
        let options = ImportFormalOptions {
            audit_root: root.to_owned(),
            run_id: "audit-1".to_owned(),
            formal_report: artifacts.join("formal.json"),
            journey_evidence: artifacts.join("journey.json"),
            review_queue: artifacts.join("queue.json"),
            manual_review: artifacts.join("review.json"),
        };
        let imported = import_formal_evidence(&options).unwrap();
        assert_eq!(imported["importedIds"].as_array().unwrap().len(), 6);
        let first = std::fs::read(root.join("visual_evidence.json")).unwrap();
        import_formal_evidence(&options).unwrap();
        assert_eq!(
            first,
            std::fs::read(root.join("visual_evidence.json")).unwrap()
        );

        audit_queue::write_json(
            &root.join("visual_evidence.json"),
            &json!({"schema_version":1,"run_id":"audit-1","artifacts":[]}),
        )
        .unwrap();
        let empty = std::fs::read(root.join("visual_evidence.json")).unwrap();
        let mut broken = journey;
        broken["cells"][0]["screenshots"]["viewport"]["sha256"] = json!("c".repeat(64));
        audit_queue::write_json(&artifacts.join("journey.json"), &broken).unwrap();
        assert!(
            import_formal_evidence(&options)
                .unwrap_err()
                .contains("failed validation")
        );
        assert_eq!(
            empty,
            std::fs::read(root.join("visual_evidence.json")).unwrap()
        );
    }
}
