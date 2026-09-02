#!/usr/bin/env python3
"""Build deterministic UI implementation audit batches."""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import sys
import tempfile
import uuid
from collections import Counter
from dataclasses import asdict, dataclass, replace
from datetime import datetime, timezone
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
SKILL_DIR = SCRIPT_DIR.parent
REPO_ROOT = Path(__file__).resolve().parents[3]
if not (REPO_ROOT / "full_repo_harness" / "queue.py").is_file():
    raise RuntimeError("ui-implementation-audit must resolve from the canonical DevCoordinator2 skill link")
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

import full_repo_harness.queue as queue
import ui_implementation_gate as ui_gate


ARTIFACT_OWNER = "ui-implementation-audit"
ARTIFACT_MARKER = ".ui-implementation-audit-artifacts.json"
VISUAL_ASSET_EXTENSIONS = set(queue.UI_ASSET_EXTENSIONS) | {".pdf", ".svg"}
MOCKUP_TOKENS = {
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
}
MOCKUP_DIRS = {
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
}
REQUIREMENT_TOKENS = {
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
}
REQUIREMENT_EXTENSIONS = {".md", ".mdx", ".markdown", ".txt", ".json", ".jsonc", ".yaml", ".yml"}
REQUIREMENT_TEXT_RE = re.compile(
    r"\b(user journey|workflow|persona|acceptance criteria|screen sequence|primary action|responsive|mockup|wireframe|figma|visual design|ui requirement|ux requirement)\b",
    re.IGNORECASE,
)

INAPPLICABLE_EXIT = 3


@dataclass(frozen=True)
class VisualAsset:
    rel_path: str
    role: str
    size_bytes: int
    sha256: str
    evidence: str


@dataclass(frozen=True)
class RequirementSource:
    rel_path: str
    kind: str
    size_bytes: int
    sha256: str
    evidence: str


@dataclass(frozen=True)
class FormalConfigSource:
    rel_path: str
    sha256: str
    size_bytes: int


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Create UI implementation audit manifest and worker prompts.")
    parser.add_argument("--repo", default=".", help="Repository root to audit. Defaults to cwd.")
    parser.add_argument("--out", default=None, help="Output directory. Defaults outside the audited repo.")
    parser.add_argument("--batch-size", type=queue.positive_int, default=6, help="Maximum UI source files per batch.")
    parser.add_argument(
        "--max-batch-bytes",
        type=queue.positive_int,
        default=queue.DEFAULT_MAX_BATCH_BYTES,
        help="Maximum total file bytes per batch. Larger text files are split into ranges.",
    )
    parser.add_argument("--include-config", action=argparse.BooleanOptionalAction, default=True)
    parser.add_argument("--include-env", action="store_true")
    parser.add_argument("--include-generated", action="store_true")
    parser.add_argument("--include-vendor", action="store_true")
    parser.add_argument("--include-assets", action=argparse.BooleanOptionalAction, default=True)
    parser.add_argument("--run-id", type=queue.run_id_token, default=None)
    parser.add_argument("--exclude-glob", action="append", default=[])
    parser.add_argument("--include-file", action="append", default=[])
    parser.add_argument("--include-glob", action="append", default=[])
    parser.add_argument("--mockup", action="append", default=[], help="Repo-relative mockup/design asset to force into visual evidence.")
    parser.add_argument(
        "--split-visual-discovery",
        action="store_true",
        help="Use separate asset and visual-tooling workers for unusually large or specialized evidence inventories.",
    )
    parser.add_argument("--journey-file", action="append", default=[], help="Repo-relative journey/requirements file to force into evidence.")
    parser.add_argument(
        "--ui-platform",
        choices=("web", "native", "hybrid"),
        default=None,
        help="Required for a full audit: web, native, or hybrid product UI scope.",
    )
    parser.add_argument(
        "--formal-config",
        default=None,
        help="Repo-relative formal-web-ui-verification config. Required for web/hybrid formal evidence; omission remains a reported audit gap.",
    )
    parser.add_argument(
        "--implemented-ui-file",
        action="append",
        default=[],
        help=(
            "Repo-relative executable product UI source inspected by the lead. "
            "Repeat for additional evidence; at least one qualifying file is required."
        ),
    )
    parser.add_argument(
        "--implemented-ui-override",
        action="append",
        nargs=3,
        default=[],
        metavar=("PATH", "UI-KIND", "SOURCE-ANCHOR"),
        help=(
            "Exceptional evidence for an implemented UI framework the detector does not recognize. "
            "UI-KIND must be an imported/inherited/constructed UI type and SOURCE-ANCHOR a named "
            "screen/component definition whose body uses it."
        ),
    )
    parser.add_argument(
        "--eligibility-only",
        action="store_true",
        help="Check the implemented-UI gate, print JSON, and create no audit artifacts.",
    )
    return parser.parse_args()


def utc_stamp() -> str:
    return datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")


def isolated_worker_contract() -> str:
    return """Run this worker in a fresh isolated context and pass this complete prompt plus applicable project-ledger requirements. Do not prescribe or validate a reasoning-effort level; use the runtime/user-selected worker default. Do not rely on an inherited lead transcript. If workers cannot be spawned, the lead may perform the same bounded work through the documented manual fallback. Write the complete report to the exact path below and return only the bounded filename-bearing receipt."""


def load_formal_config(repo: Path, raw_path: str | None, ui_platform: str) -> FormalConfigSource | None:
    if ui_platform == "native":
        if raw_path:
            raise ValueError("--formal-config is valid only for web or hybrid audits")
        return None
    if not raw_path:
        return None
    rel_path = queue.validate_repo_relative_include(repo, raw_path)
    path = repo / rel_path
    if path.is_symlink() or not path.is_file():
        raise ValueError("--formal-config must name a regular non-symlink repository file")
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError(f"--formal-config must contain valid UTF-8 JSON: {error}") from error
    if not isinstance(payload, dict):
        raise ValueError("--formal-config must contain a JSON object")
    if not isinstance(payload.get("targets"), list) and not isinstance(payload.get("targetDefaults"), dict):
        raise ValueError("--formal-config must declare targets or targetDefaults")
    return FormalConfigSource(
        rel_path=rel_path,
        sha256=queue.sha256_file(path),
        size_bytes=path.stat().st_size,
    )


def table_cell(value: object) -> str:
    return str(value).replace("\\", "\\\\").replace("|", "\\|").replace("\n", " ")


def is_evidence_only_interface_path(rel_path: str) -> bool:
    return ui_gate.is_evidence_only_interface_path(rel_path)


def parse_implemented_ui_override(repo: Path, raw: list[str]) -> tuple[str, dict]:
    raw_path, ui_kind, source_anchor = raw
    rel_path = queue.validate_repo_relative_include(repo, raw_path)
    if not ui_kind.strip() or not source_anchor.strip():
        raise ValueError("--implemented-ui-override UI-KIND and SOURCE-ANCHOR must be non-empty")
    return rel_path, {"ui_kind": ui_kind, "source_anchor": source_anchor}


def implemented_ui_evidence_qualification(
    repo: Path,
    rel_path: str,
    entry: queue.FileEntry | None,
    override: dict | None,
) -> tuple[str | None, dict | None]:
    if entry is None or entry.kind == "source/ui-asset":
        return "not a collected executable source file", None
    return ui_gate.qualify_implementation_source(repo, rel_path, override)


def assess_implementation_gate(
    repo: Path,
    evidence_specs: dict[str, dict | None],
    interface_entries: list[queue.FileEntry],
) -> dict:
    entries_by_path = {item.rel_path: item for item in interface_entries}
    accepted: list[dict] = []
    rejected: list[dict] = []
    for rel_path in sorted(evidence_specs):
        entry = entries_by_path.get(rel_path)
        override = evidence_specs[rel_path]
        issue, qualification = implemented_ui_evidence_qualification(repo, rel_path, entry, override)
        if issue:
            rejected.append({"rel_path": rel_path, "reason": issue})
            continue
        accepted.append(
            {
                "rel_path": rel_path,
                "sha256": entry.sha256,
                "evidence": (
                    "lead-inspected via --implemented-ui-override"
                    if override is not None
                    else "lead-inspected via --implemented-ui-file"
                ),
                "qualification": qualification,
            }
        )
    if not evidence_specs:
        reason = "no repo-owned executable product UI implementation file was named"
    elif rejected:
        reason = "one or more named files do not prove an implemented target UI surface"
    elif not accepted:
        reason = "no named file proves an implemented target UI surface"
    else:
        reason = "at least one repo-owned substantive product UI surface is implemented"
    return {
        "schema_version": 2,
        "status": "passed" if accepted and not rejected else "not-applicable",
        "reason": reason,
        "evidence_files": accepted,
        "rejected_files": rejected,
    }


def is_visual_asset_candidate(rel_path: str) -> bool:
    suffix = Path(rel_path).suffix.lower()
    if suffix not in VISUAL_ASSET_EXTENSIONS:
        return False
    parts = {part.lower() for part in Path(rel_path).parts[:-1]}
    words = queue.filename_words(Path(rel_path).name)
    return bool(
        queue.is_ui_asset_path(rel_path)
        or parts & MOCKUP_DIRS
        or words & MOCKUP_TOKENS
        or parts & queue.UI_ASSET_DIRS
        or words & queue.UI_ASSET_NAME_TOKENS
    )


def visual_asset_role(rel_path: str, forced_mockups: set[str]) -> str | None:
    if rel_path in forced_mockups:
        return "mockup"
    if not is_visual_asset_candidate(rel_path):
        return None
    parts = {part.lower() for part in Path(rel_path).parts[:-1]}
    words = queue.filename_words(Path(rel_path).name)
    if parts & MOCKUP_DIRS or words & MOCKUP_TOKENS:
        return "mockup"
    return "ui-asset"


def collect_candidate_paths(
    repo: Path,
    include_generated: bool,
    include_vendor: bool,
    include_assets: bool,
    output_rel_dirs: list[str],
) -> set[str]:
    git_paths = queue.run_git_files(repo)
    if git_paths is None:
        paths = set(queue.walk_files(repo, include_generated, include_vendor))
    else:
        paths = set(git_paths)
        if include_assets:
            paths.update(queue.run_git_ignored_files(repo, include_generated, include_vendor, output_rel_dirs))
    return paths


def discover_visual_assets(
    repo: Path,
    include_generated: bool,
    include_vendor: bool,
    include_assets: bool,
    exclude_globs: list[str],
    output_rel_dirs: list[str],
    forced_mockups: set[str],
    collected_entries: list[queue.FileEntry],
) -> list[VisualAsset]:
    candidates = collect_candidate_paths(repo, include_generated, include_vendor, include_assets, output_rel_dirs)
    candidates.update(forced_mockups)
    candidates.update(item.rel_path for item in collected_entries if is_visual_asset_candidate(item.rel_path))
    assets: dict[str, VisualAsset] = {}
    for rel_path in sorted(candidates):
        path = repo / rel_path
        forced = rel_path in forced_mockups
        reason = (
            queue.excluded_by_output_dir(rel_path, output_rel_dirs)
            or queue.excluded_by_dir(rel_path, include_generated, include_vendor)
            or queue.matches_any_glob(rel_path, exclude_globs)
        )
        if reason and not forced:
            continue
        if not path.exists() or not path.is_file() or path.is_symlink():
            continue
        role = visual_asset_role(rel_path, forced_mockups)
        if role is None:
            continue
        try:
            size = path.stat().st_size
            digest = queue.sha256_file(path)
        except OSError:
            continue
        evidence = "forced by --mockup" if forced else ("mockup/design path or filename" if role == "mockup" else "UI asset path or filename")
        assets[rel_path] = VisualAsset(rel_path=rel_path, role=role, size_bytes=size, sha256=digest, evidence=evidence)
    return list(assets.values())


def requirement_evidence_for_path(rel_path: str, path: Path, forced: bool) -> str | None:
    if forced:
        return "forced by --journey-file"
    suffix = path.suffix.lower()
    if suffix not in REQUIREMENT_EXTENSIONS:
        return None
    parts = {part.lower() for part in path.parts[:-1]}
    words = queue.filename_words(path.name)
    if words & REQUIREMENT_TOKENS or parts & {"docs", "documentation", "product", "requirements", "spec", "specs", "ux", "design"}:
        return "requirement-like path or filename"
    try:
        text = queue.read_initial_bytes(path, limit=400_000).decode("utf-8", errors="ignore")
    except OSError:
        return None
    if REQUIREMENT_TEXT_RE.search(text):
        return "journey or UI requirement terms in file"
    return None


def discover_requirement_sources(
    repo: Path,
    collected_entries: list[queue.FileEntry],
    forced_journey_files: set[str],
    include_generated: bool,
    include_vendor: bool,
    output_rel_dirs: list[str],
) -> list[RequirementSource]:
    candidates = {item.rel_path for item in collected_entries}
    candidates.update(forced_journey_files)
    candidates.update(
        rel_path
        for rel_path in collect_candidate_paths(repo, include_generated, include_vendor, True, output_rel_dirs)
        if Path(rel_path).suffix.lower() in REQUIREMENT_EXTENSIONS
    )
    records: dict[str, RequirementSource] = {}
    for rel_path in sorted(candidates):
        path = repo / rel_path
        if queue.excluded_by_output_dir(rel_path, output_rel_dirs) and rel_path not in forced_journey_files:
            continue
        if not path.exists() or not path.is_file() or path.is_symlink():
            continue
        evidence = requirement_evidence_for_path(rel_path, path, rel_path in forced_journey_files)
        if evidence is None:
            continue
        try:
            size = path.stat().st_size
            digest = queue.sha256_file(path)
        except OSError:
            continue
        kind = "forced-requirement" if rel_path in forced_journey_files else "requirement-candidate"
        records[rel_path] = RequirementSource(rel_path=rel_path, kind=kind, size_bytes=size, sha256=digest, evidence=evidence)
    return list(records.values())


def unit_lines(entries: list[queue.AuditUnit]) -> str:
    lines: list[str] = []
    for entry in entries:
        if entry.start_line is not None:
            location = f"lines {entry.start_line}-{entry.end_line}"
        elif entry.start_byte is not None:
            location = f"bytes {entry.start_byte}-{entry.end_byte}"
        else:
            location = f"{entry.size_bytes} bytes"
        lines.append(
            f"- Unit `{entry.unit_id}`: `{entry.rel_path}` {location} "
            f"({entry.kind}, sha256=`{entry.sha256}`)"
        )
    return "\n".join(lines)


def compact_asset_list(assets: list[VisualAsset], *, role: str | None = None, limit: int = 80) -> str:
    chosen = [item for item in assets if role is None or item.role == role]
    if not chosen:
        return "- None found."
    rows = [
        f"- `{item.rel_path}` ({item.role}, {item.size_bytes} bytes, sha256=`{item.sha256}`, evidence={item.evidence})"
        for item in chosen[:limit]
    ]
    if len(chosen) > limit:
        rows.append(f"- ... {len(chosen) - limit} more listed in manifest.json")
    return "\n".join(rows)


def compact_requirement_list(requirements: list[RequirementSource], limit: int = 80) -> str:
    if not requirements:
        return "- None found."
    rows = [
        f"- `{item.rel_path}` ({item.kind}, sha256=`{item.sha256}`, evidence={item.evidence})"
        for item in requirements[:limit]
    ]
    if len(requirements) > limit:
        rows.append(f"- ... {len(requirements) - limit} more listed in manifest.json")
    return "\n".join(rows)


def render_batch_prompt(
    repo: Path,
    run_id: str,
    batch_id: int,
    total_batches: int,
    entries: list[queue.AuditUnit],
    assets: list[VisualAsset],
    requirements: list[RequirementSource],
    report_path: Path,
) -> str:
    return f"""# UI Implementation Audit Batch {batch_id:03d}/{total_batches:03d}

Run ID: `{run_id}`
Repo root: `{repo}`
Batch ID: `batch_{batch_id:03d}`

{queue.artifact_delivery_contract(report_path)}
{isolated_worker_contract()}

You are a low-effort worker auditing interface source implementation. Do not edit the audited repository; write only the exact audit artifact authorized above. Inspect every owned unit below and compare source-defined UI behavior, visible text, layout, state handling, responsive intent, implementation paths, and test evidence against the mockup/assets, required UI elements, features, and journey requirements listed here and in `manifest.json`.

## Files You Own

{unit_lines(entries)}

For ranged units, inspect the assigned range manually plus nearby imports/types/callers/styles only as needed. In `File Coverage` and `UI Source Inventory`, use the exact unit id.

## Mockup And Asset Evidence

{compact_asset_list(assets)}

## Journey Requirement Evidence

{compact_requirement_list(requirements)}

## Review Rules

- Inventory every required visible label, control, field, menu, route link, toast, banner, empty/loading/error state, layout container, and visual/test evidence.
- Record source wiring references for handlers, state, navigation, API/persistence, permissions, validation, and missing state branches when the UI promises behavior. A path/symbol reference proves only that the source anchor exists; runtime or test evidence is required to prove the observable outcome.
- Compare implementation to mockup/journey evidence: hierarchy, density, spacing, imagery, typography intent, copy, responsiveness, required decision information, feature behavior, and test evidence.
- Flag source order, layout rules, or default state that plausibly elevate low-relevance settings, rare/admin controls, debug detail, or secondary metadata above journey-critical content. Leave rendered conclusions to the visual worker and formal evidence.
- Flag missing UI elements, unwired handlers, missing data/persistence paths, missing states, missing accessibility paths, and missing safe visual states or fixture paths when source implies heavy or production-only operations.

## Required Report File

Write exactly these top-level headings in order to the report path above:

## Run ID
{run_id}

## Batch ID
batch_{batch_id:03d}

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
"""


def render_mockup_asset_prompt(
    repo: Path,
    run_id: str,
    assets: list[VisualAsset],
    requirements: list[RequirementSource],
    report_path: Path,
) -> str:
    return f"""# UI Implementation Audit: Mockup And Asset Worker

Run ID: `{run_id}`
Repo root: `{repo}`
Worker: `mockup_asset_audit`

{queue.artifact_delivery_contract(report_path)}
{isolated_worker_contract()}

Do not edit the audited repository; write only the exact audit artifact authorized above. Inventory the design target from mockups/assets and journey requirement sources, including required screens, features, UI elements, states, implementation expectations, and test expectations. Use image-viewing tools when available for raster assets; otherwise describe the blocker and rely on filenames/nearby docs only as fallback.

## Mockup And Asset Inputs

{compact_asset_list(assets)}

## Journey Requirement Inputs

{compact_requirement_list(requirements)}

Write exactly these sections to the report path above:

## Run ID
{run_id}

## Worker
mockup_asset_audit

## Mockup/Asset Inputs
List each mockup/design/asset input used, including whether it was visually inspected.

## Journey Requirement Inputs
List requirement docs/source used and the journeys/screens they imply.

## Expected Screens And Visual Requirements
List expected screens, hierarchy, density, layout, typography, color, imagery, states, UI elements, feature behavior, implementation expectations, test expectations, and desktop/mobile requirements. Include the journey decision model for each important surface: primary goal, primary decision, required facts, warning/flag conditions, frequent actions, secondary/rare actions, and unconfirmed assumptions.

## Findings
Use `No findings.` or finding blocks with Priority, Files, Mockup/requirement evidence, Interface evidence, Expected behavior/standard, Gap, Suggested implementation direction. Use `Files: not-applicable` only for missing target assets or requirements.

## Open Questions
List missing mockups, unclear journeys, missing UI element/feature/test expectations, or `None.`
"""


def render_visual_tooling_prompt(
    repo: Path,
    run_id: str,
    ui_entries: list[queue.FileEntry],
    requirements: list[RequirementSource],
    report_path: Path,
) -> str:
    files = "\n".join(f"- `{item.rel_path}` ({item.kind}, sha256=`{item.sha256}`)" for item in ui_entries) or "- None."
    return f"""# UI Implementation Audit: Visual Tooling Worker

Run ID: `{run_id}`
Repo root: `{repo}`
Worker: `visual_tooling_audit`

{queue.artifact_delivery_contract(report_path)}
{isolated_worker_contract()}

Do not edit the audited repository; write only the exact audit artifact authorized above. Identify how to render the implemented UI safely for screenshot comparison and how required screens, UI elements, states, and visual tests can be exercised. Prefer Playwright, Cypress, Storybook, Vite/Next dev servers, browser MCP tools, native previews/simulators, test fixtures, mock data modes, or existing screenshot tests.

## Interface Source Files

{files}

## Journey Requirement Inputs

{compact_requirement_list(requirements)}

Write exactly these sections to the report path above:

## Run ID
{run_id}

## Worker
visual_tooling_audit

## Tooling Inventory
List exact detected tools/configs/scripts/routes/stories/specs or the absence of them.

## Safe Run Path
List exact commands, environment/test-mode requirements, and routes/screens to open; or explain why no safe render path exists.

## Desktop/Mobile Screenshot Plan
List desktop, native, and narrow mobile viewport checks to run, including target routes/screens, required UI elements/states, expected artifacts, rendered journey usefulness, readability/contrast evidence, and how to identify visible decision-driving content versus secondary/detail/debug/configuration content.

## Findings
Use `No findings.` or finding blocks with Priority, Files, Mockup/requirement evidence, Interface evidence, Expected behavior/standard, Gap, Suggested implementation direction.

## Open Questions
List blockers or `None.`
"""


def render_visual_comparison_prompt(
    repo: Path,
    run_id: str,
    assets: list[VisualAsset],
    requirements: list[RequirementSource],
    ui_platform: str,
    formal_config: FormalConfigSource | None,
    report_path: Path,
) -> str:
    formal_config_text = (
        f"`{formal_config.rel_path}` (sha256=`{formal_config.sha256}`)"
        if formal_config
        else "missing — formal web verification must be BLOCKED and reported as a finding"
    )
    return f"""# UI Implementation Audit: Visual Comparison Worker

Run ID: `{run_id}`
Repo root: `{repo}`
Worker: `visual_comparison_audit`
Declared UI platform: `{ui_platform}`
Formal web config: {formal_config_text}

{queue.artifact_delivery_contract(report_path)}
{isolated_worker_contract()}

Authorized visual evidence manifest: `{(report_path.parent.parent / 'visual_evidence.json').resolve()}`.
Screenshot, formal-verifier, journey-evidence, changed-review queue, decision, and manual-review artifacts may be written only beneath the same
audit-output directory and must be registered in that manifest.

Do not edit the audited repository; write only the exact audit artifacts authorized above. Use available screenshot-capable tooling to compare the implemented UI against mockups/assets, required UI elements, feature behavior, tests, and user journey requirements. Prefer safe test/fixture/preview mode. If the UI cannot be rendered, create desktop and mobile `BLOCKED` rows with concrete tool/route evidence and report the missing visual harness as a finding.

For native captures, add normal `screenshot` or `native-snapshot` records to `visual_evidence.json`. For web evidence, do not transcribe formal artifacts by hand. Run the formal verifier only with the manifest-bound config above, complete changed-image review, then invoke `scripts/import_formal_web_evidence.py` with the audit root, audit run id, formal report, journey-evidence manifest, review queue, and manual-review manifest. The importer registers the formal report, ordered journey bundle, screenshot pairs, queue, and review manifest and rejects path/hash/run mismatches.

For platform `web`, formal browser evidence is required. For `native`, formal web evidence is not applicable and native screenshots/snapshots are required. For `hybrid`, provide both the formal web evidence chain and native captures. Run all deterministic checks and other automatic tests before manual image review. Then read the formal verifier's `review-queue.json`: open only each queued cell's initial-viewport and full-page images, never carried unchanged images. Record `pass`, `gap`, or `blocked` decisions, finalize them with `formal_web_ui_review.py`, and preserve carried prior gaps as findings without reopening their screenshots. Pixel/hash drift is integrity evidence only.

Before visual comparison, define the journey decision model and required UI element set. A visual check is not clear merely because it matches a mockup, has correct data, or avoids overflow. Each rendered viewport must support the primary journey decision unless the surface is itself primarily a data-entry form. If settings, filters, menus, target/configuration blocks, raw/debug detail, explanatory copy, or other low-relevance content dominates the visible surface while the primary decision is unclear or buried, report a journey-usability finding. Also run the interaction checklist for every rendered/source-inferred viewport that contains badges, flags, expandable rows, scrollable details, message streams, tool/result blocks, copy controls, navigation rows, or icon-only controls: badge-detail, row-hit-target, navigation-cursor, transient-disclosure, disclosure-scrollbar, icon-meaning, stable-expansion-width, hover-copy, status-summary, and message-metadata.

## Mockup And Asset Evidence

{compact_asset_list(assets)}

## Journey Requirement Evidence

{compact_requirement_list(requirements)}

Write exactly these sections to the report path above:

## Run ID
{run_id}

## Worker
visual_comparison_audit

## Mockup And Asset Inventory
List the assets and requirements actually used, their role, and any material ambiguity. Keep this concise; use `None.` when no mockup is available.

## Visual Tooling
Name the safe render/capture path and desktop/mobile viewport plan, or the concrete blocker.

## Journey Decision Model
| Surface | Primary user goal | Primary decision | Required facts | Warning/flag conditions | Frequent actions | Secondary/rare actions | Unconfirmed assumptions |
| --- | --- | --- | --- | --- | --- | --- | --- |
| route/screen | user goal | decision the user must make | facts needed for the decision | warning or flag conditions | common action(s) | occasional/rare/admin/config actions | assumptions needing confirmation |

## Rendered Journey Usability
| Platform | Viewport | Decision supported | Visible decision-driving content | Visible secondary/detail content | Detail access pattern | Readability/contrast evidence | Layout quality result | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| web/native | desktop/native/mobile | supported decision or blocker | facts/actions/warnings visible | details/settings/debug/config visible | inline/expander/menu/detail route/blocked | screenshot/tool/DOM/viewport evidence | PASS/GAP/BLOCKED/NOT_APPLICABLE | screenshot/tool/DOM/viewport evidence |

For relevant rows, include these exact checklist labels in `Detail access pattern` or `Evidence`: `badge-detail`, `row-hit-target`, `navigation-cursor`, `transient-disclosure`, `disclosure-scrollbar`, `icon-meaning`, `stable-expansion-width`, `hover-copy`, `status-summary`, `message-metadata`.

## Visual Comparison Checks
| Platform | Journey | Viewport | Route/Screen | Mockup/Requirement | Implementation Screenshot/Tool Evidence | Differences | Result |
| --- | --- | --- | --- | --- | --- | --- | --- |
| web/native | journey or screen | desktop/mobile/native | route/screen/story | asset or requirement | tool command plus `evidence:<id>`, or concrete blocker | visual/responsive differences | MATCHED/GAP/BLOCKED/NOT_APPLICABLE |

## Formal Evidence
For web/hybrid, cite the imported `formal-web-verifier`, `journey-evidence`, `review-queue`, and `manual-review` evidence ids and summarize formal coverage, critical findings, pending/current decisions, carried gaps, visible scrollbars, and palette risks. For native, write exactly `Formal Web UI verification not applicable to declared native platform.`

## Findings
Use `No findings.` or finding blocks with Priority, Files, Mockup/requirement evidence, Interface evidence, Expected behavior/standard, Gap, Suggested implementation direction. Start with `Interaction checklist: badge-detail=<pass/gap/blocked/not-applicable>; row-hit-target=<...>; navigation-cursor=<...>; transient-disclosure=<...>; disclosure-scrollbar=<...>; icon-meaning=<...>; stable-expansion-width=<...>; hover-copy=<...>; status-summary=<...>; message-metadata=<...>.` If screenshot production is blocked, include a finding that names the missing safe visual path. If a required element/state is absent, content is overloaded/crowded/unreadable, low-relevance detail dominates while the primary decision is unclear or buried, or any interaction checklist item is gap/blocked, include a finding.

## Open Questions
List visual blockers, missing mockups, unclear routes, or `None.`
"""


def write_execution_ledger(out_dir: Path, manifest: dict) -> None:
    ui_required = bool(manifest.get("ui_implementation_audit", {}).get("visual_required"))
    audit = manifest.get("ui_implementation_audit", {})
    ledger = {
        "run_id": manifest["run_id"],
        "repo_root": manifest["repo_root"],
        "audit_kind": "ui-implementation",
        "provenance_scope": "lead-recorded runtime ledger",
        "worker_capability_check": {
            "status": "pending",
            "spawn_tool": None,
            "notes": "",
        },
        "lead": {
            "status": "pending",
            "agent_id": None,
            "runtime_provenance": None,
        },
        "fallback": {"status": "not-started", "reason": ""},
        "mockup_asset_worker": {
            "status": "pending" if audit.get("mockup_asset_prompt") else "not-applicable",
            "prompt": audit.get("mockup_asset_prompt"),
            "report": audit.get("mockup_asset_report"),
            "agent_id": None,
            "runtime_provenance": None,
        },
        "visual_tooling_worker": {
            "status": "pending" if audit.get("visual_tooling_prompt") else "not-applicable",
            "prompt": audit.get("visual_tooling_prompt"),
            "report": audit.get("visual_tooling_report"),
            "agent_id": None,
            "runtime_provenance": None,
        },
        "visual_comparison_worker": {
            "status": "pending" if ui_required else "not-applicable",
            "prompt": "visual_comparison_audit.md" if ui_required else None,
            "report": "reports/visual_comparison_audit.md" if ui_required else None,
            "agent_id": None,
            "runtime_provenance": None,
        },
        "batch_workers": [
            {
                "batch_id": batch["id"],
                "status": "pending",
                "prompt": batch["prompt"],
                "report": f"reports/{batch['id']}.md",
                "agent_id": None,
                "runtime_provenance": None,
                "fallback": False,
            }
            for batch in manifest["batches"]
        ],
        "pruned_directory_review": {
            "status": "pending" if manifest.get("pruned_directory_review_hint_count") else "not-applicable",
            "hint_count": manifest.get("pruned_directory_review_hint_count", 0),
            "decisions": [],
        },
    }
    queue.write_json(out_dir / "execution_ledger.json", ledger)


def write_completion_marker(out_dir: Path, manifest: dict) -> None:
    marker = {
        "run_id": manifest["run_id"],
        "phase": "queue_generated",
        "audit_verified": False,
        "audit_kind": "ui-implementation",
        "manifest": "manifest.json",
        "audit_index": "audit_index.md",
        "execution_ledger": "execution_ledger.json",
        "excluded_files": "excluded_files.json",
        "reports_dir": "reports",
        "logs_dir": "logs",
        "final_report": "final-report.md",
        "ownership_marker": ARTIFACT_MARKER,
        "batch_count": manifest["batch_count"],
        "source_file_count": manifest["source_file_count"],
        "marker_semantics": "Queue artifacts were generated; worker reports and execution ledger still require verifier completion.",
    }
    queue.write_json(out_dir / "queue_complete.json", marker)


def render_index(repo: Path, out_dir: Path, manifest: dict) -> str:
    rows = "\n".join(
        f"| {table_cell(batch['id'])} | `{table_cell(batch['prompt'])}` | {batch['file_count']} | {batch['coverage_unit_count']} | {table_cell(batch['purpose'])} |"
        for batch in manifest["batches"]
    )
    if not rows:
        rows = "| None | None | 0 | 0 | No interface source files queued |"
    audit = manifest["ui_implementation_audit"]
    visual_lines = []
    if audit.get("mockup_asset_prompt"):
        visual_lines.append(f"- Mockup/assets prompt: `{audit['mockup_asset_prompt']}` -> `{audit['mockup_asset_report']}`")
    if audit.get("visual_tooling_prompt"):
        visual_lines.append(f"- Visual tooling prompt: `{audit['visual_tooling_prompt']}` -> `{audit['visual_tooling_report']}`")
    if audit.get("visual_comparison_prompt"):
        visual_lines.append(f"- Visual comparison prompt: `{audit['visual_comparison_prompt']}` -> `{audit['visual_comparison_report']}`")
    visual_prompts = "\n".join(visual_lines) or "- No visual worker prompt was generated."
    return f"""# UI Implementation Audit Index

Repo root: `{repo}`
Output directory: `{out_dir}`
Run ID: `{manifest['run_id']}`
Audit kind: `ui-implementation`

Interface source files queued: **{manifest['source_file_count']}**
Coverage units queued: **{manifest['coverage_unit_count']}**
Batches: **{manifest['batch_count']}**
Visual assets found: **{audit['visual_asset_count']}**
Mockup assets found: **{audit['mockup_asset_count']}**
Requirement sources found: **{audit['requirement_source_count']}**
Declared UI platform: **{audit['ui_platform']}**
Formal web config: **{audit['formal_config']['rel_path'] if audit.get('formal_config') else 'missing/not applicable'}**
Scope warnings: **{manifest['scope_warning_count']}**

## Dispatch

1. Fill `execution_ledger.json` as workers are assigned.
2. Dispatch one fresh isolated worker per batch prompt. Use the runtime/user-selected worker effort and pass the entire prompt plus applicable project-ledger requirements.
3. Workers write complete reports to their prompt-declared `reports/` paths and return only bounded filename-bearing `REPORT_SAVED` receipts; never request the report body through the worker response.
4. Dispatch visual workers when listed below.
5. The lead reviews visual evidence, writes the complete synthesis to `final-report.md`, and keeps verbose output in `logs/`.
6. Run verifier: `{manifest['verifier_command']}`
7. Return only a compact outcome/counts/verifier/artifact summary to chat.

{visual_prompts}

## Batches

| Batch | Prompt | Files | Units | Purpose |
| --- | --- | ---: | ---: | --- |
{rows}
"""


def write_outputs(
    repo: Path,
    out_dir: Path,
    entries: list[queue.FileEntry],
    excluded: list[dict],
    units: list[queue.AuditUnit],
    batches: list[list[queue.AuditUnit]],
    run_id: str,
    visual_assets: list[VisualAsset],
    requirements: list[RequirementSource],
    non_interface_source_count: int,
    implementation_gate: dict,
    split_visual_discovery: bool,
    ui_platform: str,
    formal_config: FormalConfigSource | None,
) -> None:
    queue.ARTIFACT_OWNER = ARTIFACT_OWNER
    queue.ARTIFACT_MARKER = ARTIFACT_MARKER
    queue.validate_generated_artifact_tokens(entries, units)
    marker = queue.ensure_output_dir_safe(out_dir, repo)
    out_dir.mkdir(parents=True, exist_ok=True)
    if marker is None:
        queue.write_ownership_marker(out_dir, repo, [])
        marker = queue.read_ownership_marker(out_dir)
    queue.clean_generated_artifacts(out_dir, marker)
    reports_dir = out_dir / "reports"
    archived_reports_name = None
    archived_reports_dir = None
    if reports_dir.exists() and any(reports_dir.iterdir()):
        archive = out_dir / f"reports.stale.{utc_stamp()}"
        suffix = 1
        while archive.exists():
            suffix += 1
            archive = out_dir / f"reports.stale.{utc_stamp()}.{suffix}"
        reports_dir.rename(archive)
        archived_reports_name = archive.name
        archived_reports_dir = str(archive)
    reports_dir.mkdir(exist_ok=True)
    logs_dir = out_dir / "logs"
    logs_dir.mkdir(exist_ok=True)

    batch_records: list[dict] = []
    all_paths: list[str] = []
    all_units: list[str] = []
    for index, batch in enumerate(batches, start=1):
        prompt = f"batch_{index:03d}.md"
        (out_dir / prompt).write_text(
            render_batch_prompt(
                repo,
                run_id,
                index,
                len(batches),
                batch,
                visual_assets,
                requirements,
                reports_dir / prompt,
            ),
            encoding="utf-8",
        )
        paths = sorted({item.rel_path for item in batch})
        unit_ids = [item.unit_id for item in batch]
        all_paths.extend(paths)
        all_units.extend(unit_ids)
        batch_records.append(
            {
                "id": f"batch_{index:03d}",
                "prompt": prompt,
                "report": f"reports/{prompt}",
                "file_count": len(paths),
                "coverage_unit_count": len(batch),
                "interface_file_count": len(paths),
                "byte_count": sum(item.size_bytes for item in batch),
                "files": paths,
                "coverage_units": unit_ids,
                "purpose": queue.purpose_for(batch),
            }
        )

    source_paths = [item.rel_path for item in entries]
    unit_ids = [item.unit_id for item in units]
    missing_paths = sorted(set(source_paths) - set(all_paths))
    extra_paths = sorted(set(all_paths) - set(source_paths))
    missing_units = sorted(set(unit_ids) - set(all_units))
    duplicate_units = sorted(unit for unit, count in Counter(all_units).items() if count > 1)
    extra_units = sorted(set(all_units) - set(unit_ids))
    scope_warnings = [item for item in excluded if item.get("scope_warning")]
    pruned_hints = [item for item in excluded if item.get("entry_type") == "directory" and item.get("contains_source_like_samples")]
    visual_required = True

    if split_visual_discovery:
        (out_dir / "mockup_asset_audit.md").write_text(
            render_mockup_asset_prompt(
                repo,
                run_id,
                visual_assets,
                requirements,
                reports_dir / "mockup_asset_audit.md",
            ),
            encoding="utf-8",
        )
        (out_dir / "visual_tooling_audit.md").write_text(
            render_visual_tooling_prompt(
                repo,
                run_id,
                entries,
                requirements,
                reports_dir / "visual_tooling_audit.md",
            ),
            encoding="utf-8",
        )
    if visual_required:
        (out_dir / "visual_comparison_audit.md").write_text(
            render_visual_comparison_prompt(
                repo,
                run_id,
                visual_assets,
                requirements,
                ui_platform,
                formal_config,
                reports_dir / "visual_comparison_audit.md",
            ),
            encoding="utf-8",
        )
        queue.write_json(out_dir / "visual_evidence.json", {"schema_version": 1, "run_id": run_id, "artifacts": []})

    verifier_args = [
        sys.executable,
        str(Path(__file__).resolve().with_name("verify_ui_implementation_audit_results.py")),
        "--manifest",
        str(out_dir / "manifest.json"),
        "--reports",
        str(reports_dir),
    ]
    generated_artifacts = [
        "audit_index.md",
        "execution_ledger.json",
        "excluded_files.json",
        "manifest.json",
        "queue_complete.json",
        "final-report.md",
        "logs",
        *(["mockup_asset_audit.md", "visual_tooling_audit.md"] if split_visual_discovery else []),
        *(["visual_comparison_audit.md", "visual_evidence.json"] if visual_required else []),
        *([archived_reports_name] if archived_reports_name else []),
        *[batch["prompt"] for batch in batch_records],
    ]
    manifest = {
        "repo_root": str(repo),
        "run_id": run_id,
        "audit_kind": "ui-implementation",
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "reports_dir": str(reports_dir),
        "logs_dir": str(logs_dir),
        "final_report": str(out_dir / "final-report.md"),
        "archived_reports_dir": archived_reports_dir,
        "artifact_marker": str(out_dir / ARTIFACT_MARKER),
        "execution_ledger": str(out_dir / "execution_ledger.json"),
        "generated_artifacts": generated_artifacts,
        "verifier_command": " ".join(shlex.quote(arg) for arg in verifier_args),
        "verifier_args": verifier_args,
        "source_file_count": len(entries),
        "interface_file_count": len(entries),
        "non_interface_source_count": non_interface_source_count,
        "scope_warning_count": len(scope_warnings),
        "pruned_directory_review_hint_count": len(pruned_hints),
        "excluded_file_count": len(excluded),
        "excluded_files_sha256": queue.canonical_json_sha256(excluded),
        "batch_count": len(batch_records),
        "source_files": [asdict(item) for item in entries],
        "coverage_unit_count": len(units),
        "coverage_units": [asdict(item) for item in units],
        "batches": batch_records,
        "ui_implementation_audit": {
            "ui_platform": ui_platform,
            "formal_config": asdict(formal_config) if formal_config else None,
            "visual_required": visual_required,
            "visual_worker_mode": "split" if split_visual_discovery else "combined",
            "implementation_gate": implementation_gate,
            "source_selection": "Interface-defining non-asset source plus explicit lead-confirmed implementation evidence is queued.",
            "visual_asset_count": len(visual_assets),
            "mockup_asset_count": sum(1 for item in visual_assets if item.role == "mockup"),
            "requirement_source_count": len(requirements),
            "visual_assets": [asdict(item) for item in visual_assets],
            "requirement_sources": [asdict(item) for item in requirements],
            "mockup_asset_prompt": "mockup_asset_audit.md" if split_visual_discovery else None,
            "mockup_asset_report": "reports/mockup_asset_audit.md" if split_visual_discovery else None,
            "visual_tooling_prompt": "visual_tooling_audit.md" if split_visual_discovery else None,
            "visual_tooling_report": "reports/visual_tooling_audit.md" if split_visual_discovery else None,
            "visual_comparison_prompt": "visual_comparison_audit.md" if visual_required else None,
            "visual_comparison_report": "reports/visual_comparison_audit.md" if visual_required else None,
        },
        "coverage_invariants": {
            "unique_batched_file_count": len(set(all_paths)),
            "unique_batched_unit_count": len(set(all_units)),
            "missing_from_batches": missing_paths,
            "duplicates_in_batches": queue.duplicate_whole_file_paths_for_batches(batches),
            "extra_in_batches": extra_paths,
            "missing_units_from_batches": missing_units,
            "duplicate_units_in_batches": duplicate_units,
            "extra_units_in_batches": extra_units,
            "all_coverage_units_queued_exactly_once": not missing_units and not duplicate_units and not extra_units,
            "all_source_files_queued_exactly_once": not missing_paths and not extra_paths and not missing_units and not duplicate_units and not extra_units,
        },
        "scope_warnings": scope_warnings,
        "pruned_directory_review_hints": pruned_hints,
    }
    queue.write_json(out_dir / "manifest.json", manifest)
    queue.write_json(out_dir / "excluded_files.json", excluded)
    (out_dir / "audit_index.md").write_text(render_index(repo, out_dir, manifest), encoding="utf-8")
    write_execution_ledger(out_dir, manifest)
    queue.write_ownership_marker(out_dir, repo, generated_artifacts)
    write_completion_marker(out_dir, manifest)


def main() -> int:
    queue.ARTIFACT_OWNER = ARTIFACT_OWNER
    queue.ARTIFACT_MARKER = ARTIFACT_MARKER
    args = parse_args()
    repo = Path(args.repo).expanduser().resolve()
    if not repo.exists() or not repo.is_dir():
        print(f"Repo path is not a directory: {repo}", file=sys.stderr)
        return 2
    run_id = args.run_id or uuid.uuid4().hex
    out_dir = (
        Path(args.out).expanduser().resolve()
        if args.out
        else Path(tempfile.gettempdir()) / "ui-implementation-audit" / (repo.name or "repo") / f"{utc_stamp()}-{run_id[:8]}"
    )
    output_rel_dirs: list[str] = []
    output_rel_dir = queue.relative_dir_if_child(repo, out_dir)
    if output_rel_dir == "":
        print("--out cannot be the repository root; choose a dedicated audit output directory.", file=sys.stderr)
        return 2
    if output_rel_dir is not None:
        output_rel_dirs.append(output_rel_dir)
    for owned in queue.discover_owned_output_dirs(repo, args.include_generated, args.include_vendor):
        if owned not in output_rel_dirs:
            output_rel_dirs.append(owned)
    try:
        include_files = {queue.validate_repo_relative_include(repo, raw) for raw in args.include_file}
        forced_mockups = {queue.validate_repo_relative_include(repo, raw) for raw in args.mockup}
        forced_journey_files = {queue.validate_repo_relative_include(repo, raw) for raw in args.journey_file}
        implemented_ui_files = {queue.validate_repo_relative_include(repo, raw) for raw in args.implemented_ui_file}
        implemented_ui_overrides: dict[str, dict] = {}
        for raw in args.implemented_ui_override:
            rel_path, basis = parse_implemented_ui_override(repo, raw)
            if rel_path in implemented_ui_overrides:
                raise ValueError(f"duplicate --implemented-ui-override path: {rel_path}")
            implemented_ui_overrides[rel_path] = basis
        overlap = implemented_ui_files & set(implemented_ui_overrides)
        if overlap:
            raise ValueError(
                "name each implementation path with only one evidence mode; duplicated: "
                + ", ".join(sorted(overlap))
            )
        implementation_evidence = {rel_path: None for rel_path in implemented_ui_files}
        implementation_evidence.update(implemented_ui_overrides)
    except ValueError as exc:
        print(str(exc), file=sys.stderr)
        return 2

    entries, excluded, _tracked_deletions = queue.collect_files(
        repo,
        args.include_config,
        args.include_env,
        args.include_generated,
        args.include_vendor,
        args.include_assets,
        args.exclude_glob,
        include_files | set(implementation_evidence),
        args.include_glob,
        output_rel_dirs,
    )
    automatic_interface_entries = [
        item
        for item in entries
        if item.interface_relevant
        and item.kind != "source/ui-asset"
        and not is_visual_asset_candidate(item.rel_path)
        and not is_evidence_only_interface_path(item.rel_path)
    ]
    implementation_gate = assess_implementation_gate(repo, implementation_evidence, entries)
    if args.eligibility_only:
        print(json.dumps(implementation_gate, indent=2, sort_keys=True))
    if implementation_gate["status"] != "passed":
        if not args.eligibility_only:
            print(
                "UI implementation audit is not applicable: " + implementation_gate["reason"],
                file=sys.stderr,
            )
            for item in implementation_gate["rejected_files"]:
                print(f"- {item['rel_path']}: {item['reason']}", file=sys.stderr)
        return INAPPLICABLE_EXIT
    if args.eligibility_only:
        return 0
    if args.ui_platform is None:
        print("--ui-platform is required for a full UI implementation audit", file=sys.stderr)
        return 2
    try:
        formal_config = load_formal_config(repo, args.formal_config, args.ui_platform)
    except ValueError as exc:
        print(str(exc), file=sys.stderr)
        return 2
    evidence_paths = {item["rel_path"] for item in implementation_gate["evidence_files"]}
    automatic_paths = {item.rel_path for item in automatic_interface_entries}
    interface_entries = [
        replace(item, interface_relevant=True)
        if item.rel_path in evidence_paths and not item.interface_relevant
        else item
        for item in entries
        if item.rel_path in automatic_paths or item.rel_path in evidence_paths
    ]
    visual_assets = discover_visual_assets(
        repo,
        args.include_generated,
        args.include_vendor,
        args.include_assets,
        args.exclude_glob,
        output_rel_dirs,
        forced_mockups,
        entries,
    )
    requirements = discover_requirement_sources(
        repo,
        entries,
        forced_journey_files,
        args.include_generated,
        args.include_vendor,
        output_rel_dirs,
    )
    non_interface_source_count = len(entries) - len(interface_entries)
    units = queue.audit_units_for(repo, interface_entries, args.max_batch_bytes)
    batches = queue.batch_files(units, args.batch_size, args.max_batch_bytes)
    try:
        write_outputs(
            repo,
            out_dir,
            interface_entries,
            excluded,
            units,
            batches,
            run_id,
            visual_assets,
            requirements,
            non_interface_source_count,
            implementation_gate,
            args.split_visual_discovery,
            args.ui_platform,
            formal_config,
        )
    except ValueError as exc:
        print(str(exc), file=sys.stderr)
        return 2
    print(f"Wrote {len(batches)} UI implementation batches covering {len(interface_entries)} interface source files to {out_dir}")
    print(f"Found {len(visual_assets)} visual assets and {len(requirements)} requirement sources")
    print(f"Excluded {len(excluded)} files; see {out_dir / 'excluded_files.json'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
