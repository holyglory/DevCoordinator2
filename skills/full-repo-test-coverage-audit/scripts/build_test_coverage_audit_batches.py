#!/usr/bin/env python3
"""Build deterministic test-coverage audit batches."""

from __future__ import annotations

import argparse
import json
import shlex
import sys
import tempfile
import uuid
from dataclasses import asdict
from datetime import datetime, timezone
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
SKILL_DIR = SCRIPT_DIR.parent
REPO_ROOT = Path(__file__).resolve().parents[3]
if not (REPO_ROOT / "full_repo_harness" / "queue.py").is_file():
    raise RuntimeError("full-repo-test-coverage-audit must resolve from the canonical DevCoordinator2 skill link")
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

import full_repo_harness.queue as queue
from full_repo_harness import test_targets


ARTIFACT_OWNER = "full-repo-test-coverage-audit"
ARTIFACT_MARKER = ".full-repo-test-coverage-audit-artifacts.json"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Create full-repo test-coverage audit manifest and worker prompts.")
    parser.add_argument("--repo", default=".", help="Repository root to audit. Defaults to cwd.")
    parser.add_argument("--out", default=None, help="Output directory. Defaults outside the audited repo.")
    parser.add_argument("--batch-size", type=queue.positive_int, default=8, help="Maximum files per batch.")
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
    parser.add_argument("--include-assets", action="store_true")
    parser.add_argument("--run-id", type=queue.run_id_token, default=None)
    parser.add_argument("--exclude-glob", action="append", default=[])
    parser.add_argument("--include-file", action="append", default=[])
    parser.add_argument("--include-glob", action="append", default=[])
    parser.add_argument(
        "--coverage-report",
        action="append",
        default=[],
        help="Optional LCOV, Cobertura XML, coverage.py JSON, or Istanbul JSON evidence; repeat as needed.",
    )
    return parser.parse_args()


def utc_stamp() -> str:
    return datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")


def table_cell(value: object) -> str:
    return str(value).replace("\\", "\\\\").replace("|", "\\|").replace("\n", " ")


def unit_lines(entries: list[queue.AuditUnit]) -> str:
    lines: list[str] = []
    for entry in entries:
        if entry.start_line is not None:
            lines.append(
                f"- Unit `{entry.unit_id}`: `{entry.rel_path}` lines {entry.start_line}-{entry.end_line} "
                f"({entry.kind}, interface={str(entry.interface_relevant).lower()}, sha256=`{entry.sha256}`)"
            )
        elif entry.start_byte is not None:
            lines.append(
                f"- Unit `{entry.unit_id}`: `{entry.rel_path}` bytes {entry.start_byte}-{entry.end_byte} "
                f"({entry.kind}, interface={str(entry.interface_relevant).lower()}, sha256=`{entry.sha256}`)"
            )
        else:
            lines.append(
                f"- Unit `{entry.unit_id}`: `{entry.rel_path}` "
                f"({entry.kind}, {entry.size_bytes} bytes, interface={str(entry.interface_relevant).lower()}, sha256=`{entry.sha256}`)"
            )
    return "\n".join(lines)


def render_batch_prompt(
    repo: Path,
    run_id: str,
    batch_id: int,
    total_batches: int,
    entries: list[queue.AuditUnit],
    target_records: list[dict],
    report_path: Path,
) -> str:
    target_rows = "\n".join(
        f"| {item['target_id']} | {item['unit_id']} | {item['rel_path']} | {item['symbol']} | {item['kind']} | {item['line']} | {item['structural_basis']} |"
        for item in target_records
    )
    return f"""# Full Repo Test Coverage Audit Batch {batch_id:03d}/{total_batches:03d}

Run ID: `{run_id}`
Repo root: `{repo}`
Batch ID: `batch_{batch_id:03d}`

{queue.artifact_delivery_contract(report_path)}
{queue.isolated_light_worker_contract()}

You are a low-effort worker auditing test coverage. Do not edit the audited repository; write only the exact audit artifact authorized above. Inspect every owned unit below and report whether reasonable behavior targets, intended features, UI elements, states, handlers, and journeys have meaningful tests.

## Files You Own

{unit_lines(entries)}

For ranged units, inspect the assigned range manually plus nearby imports/types/callers only as needed. In `File Coverage`, use the exact unit id.

## Structurally Discovered Targets You Must Map Exactly

| Target ID | Unit | File | Symbol/Behavior | Kind | Line | Discovery Basis |
| --- | --- | --- | --- | --- | ---: | --- |
{target_rows}

Every target id above needs exactly one inventory row. The structural scanner is a floor, not proof of completeness: add manually discovered targets with stable ids prefixed `manual-`, and explain them. Do not call structural source/test matching empirical coverage.

## Review Rules

- Identify reasonable test targets: exported/public functions, methods with behavior, reducers/hooks, API handlers, command/job entrypoints, domain services, intended feature behavior, UI element behavior, UI state transitions, validation logic, and non-trivial private helpers.
- Exclude only with rationale: types/interfaces, pure constants, generated code, static copy-only markup, trivial pass-throughs, and framework boilerplate with no repo-owned behavior.
- For each target, look for existing test files, test names, fixtures, snapshots, stories, e2e specs, or explicit absence.
- Assess happy path, invalid input, empty/null/boundary values, failure paths, async/concurrency, permissions, persistence, navigation, rollback, integration boundaries, required UI states, and feature completion when applicable.

## Required Report File

Write exactly these top-level headings in order to the report path above:

## Run ID
{run_id}

## Batch ID
batch_{batch_id:03d}

## Batch Summary
Briefly summarize what these files do.

## File Coverage
| Unit | Status | SHA-256 | Purpose |
| --- | --- | --- | --- |
| exact unit id | CHECKED | exact sha256 | one-line purpose |

## Test Target Inventory
| Target ID | Unit | File | Target | Kind | Disposition | Evidence Level | Existing Test Evidence | Scenario Assessment | Recommendation |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| exact target id | exact unit id | repo-relative file | exact symbol/behavior | function/component/control/unit-review | TESTED/UNTESTED/NOT_REASONABLE | EMPIRICAL/STRUCTURAL/MANUAL/NONE | real `test/path#test name`, `manual: evidence`, `None found`, or exclusion rationale | covered and missing cases | test direction or exclusion rationale |

## Coverage Findings
Use `No findings.` or one block per gap:

- Priority: P0/P1/P2/P3
- Files: repo-relative files owned by this batch
- Target ID: exact target id from the inventory
- Target: function, method, component, journey, API, job, or behavior
- Existing test evidence: concrete tests found or `None found`
- Missing scenarios/boundaries: concrete missing cases
- Suggested test direction: specific test type and expected assertion focus

## No Gap Notes
List targets or files with adequate coverage and why.

## Open Questions
List unresolved ambiguity for the lead, or `None.`
"""


def render_ui_prompt(
    repo: Path,
    run_id: str,
    entries: list[queue.FileEntry],
    report_path: Path,
) -> str:
    files = "\n".join(f"- `{item.rel_path}` ({item.kind}, sha256=`{item.sha256}`)" for item in entries)
    return f"""# UI And User Journey Test Coverage Audit

Run ID: `{run_id}`
Repo root: `{repo}`
Worker: `ui_test_coverage`

{queue.artifact_delivery_contract(report_path)}
{queue.isolated_light_worker_contract()}

Do not edit the audited repository; write only the exact audit artifact authorized above. Audit whether intended UI routes, controls, forms, states, UI elements, feature paths, and user journeys have meaningful component, integration, e2e, or visual coverage.

## Interface-Relevant Files

{files}

Write exactly these sections to the report path above:

## Run ID
{run_id}

## Worker
ui_test_coverage

## Journey/Test Sources
List route maps, docs, tests, stories, specs, or source files used.

## UI Coverage Checks
List routes/journeys/controls/states/features/UI elements checked and existing test evidence.

## Findings
Use `No findings.` or finding blocks with Priority, Files, Target, Existing test evidence, Missing scenarios/boundaries, Suggested test direction.

## Open Questions
List user-journey ambiguity or `None.`
"""


def render_visual_prompt(
    repo: Path,
    run_id: str,
    entries: list[queue.FileEntry],
    report_path: Path,
) -> str:
    files = "\n".join(f"- `{item.rel_path}` ({item.kind}, sha256=`{item.sha256}`)" for item in entries)
    return f"""# Visual And E2E Test Coverage Audit

Run ID: `{run_id}`
Repo root: `{repo}`
Worker: `visual_e2e_coverage`

{queue.artifact_delivery_contract(report_path)}
{queue.isolated_light_worker_contract()}

Do not edit the audited repository; write only the exact audit artifact authorized above. Identify visual/e2e tooling and assess whether high-frequency UI journeys, required screens, UI elements, and states can be checked safely in test or fixture mode.

## Interface-Relevant Files

{files}

For CLI, library, plugin, or skill packages with no repo-owned rendered UI, mark visual/e2e checks as `not applicable` with evidence.

Write exactly these sections to the report path above:

## Run ID
{run_id}

## Worker
visual_e2e_coverage

## Visual/E2E Tooling
List Playwright, Cypress, Storybook, browser/native preview, screenshots, or absence.

## Visual/E2E Coverage Checks
List journeys, screens, UI elements, and states checked or explain not applicable.

## Findings
Use `No findings.` or finding blocks with Priority, Files, Target, Existing test evidence, Missing scenarios/boundaries, Suggested test direction.

## Open Questions
List blockers or `None.`
"""


def write_effort_ledger(out_dir: Path, manifest: dict) -> None:
    coverage = manifest.get("test_coverage_audit", {})
    ui_required = bool(coverage.get("ui_required"))
    ledger = {
        "run_id": manifest["run_id"],
        "repo_root": manifest["repo_root"],
        "audit_kind": "test-coverage",
        "provenance_scope": "lead-recorded runtime ledger",
        "effort_verification_scope": "ledger-recorded",
        "subagent_capability_check": {
            "status": "pending",
            "spawn_tool": None,
            "can_set_reasoning_effort": None,
            "notes": "",
        },
        "lead_effort": {
            "required_reasoning_effort": "high-or-higher",
            "actual_reasoning_effort": None,
            "status": "pending",
            "agent_id": None,
            "runtime_provenance": None,
            "evidence": "",
        },
        "fallback": {"status": "not-started", "reason": ""},
        "ui_test_coverage_worker": {
            "status": "pending" if ui_required else "not-applicable",
            "prompt": coverage.get("ui_prompt"),
            "required_reasoning_effort": "low" if ui_required else None,
            "report": coverage.get("ui_report"),
            "agent_id": None,
            "actual_reasoning_effort": None,
            "runtime_provenance": None,
        },
        "visual_e2e_coverage_worker": {
            "status": "pending" if ui_required else "not-applicable",
            "prompt": coverage.get("visual_prompt"),
            "required_reasoning_effort": "low" if ui_required else None,
            "report": coverage.get("visual_report"),
            "agent_id": None,
            "actual_reasoning_effort": None,
            "runtime_provenance": None,
        },
        "batch_workers": [
            {
                "batch_id": batch["id"],
                "status": "pending",
                "prompt": batch["prompt"],
                "required_reasoning_effort": "low",
                "report": f"reports/{batch['id']}.md",
                "agent_id": None,
                "actual_reasoning_effort": None,
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
    queue.write_json(out_dir / "effort_ledger.json", ledger)


def write_completion_marker(out_dir: Path, manifest: dict) -> None:
    marker = {
        "run_id": manifest["run_id"],
        "phase": "queue_generated",
        "audit_verified": False,
        "audit_kind": "test-coverage",
        "manifest": "manifest.json",
        "audit_index": "audit_index.md",
        "effort_ledger": "effort_ledger.json",
        "excluded_files": "excluded_files.json",
        "reports_dir": "reports",
        "logs_dir": "logs",
        "final_report": "final-report.md",
        "ownership_marker": ARTIFACT_MARKER,
        "batch_count": manifest["batch_count"],
        "source_file_count": manifest["source_file_count"],
        "marker_semantics": "Queue artifacts were generated; worker reports and effort ledger still require verifier completion.",
    }
    queue.write_json(out_dir / "queue_complete.json", marker)


def render_index(repo: Path, out_dir: Path, manifest: dict) -> str:
    rows = "\n".join(
        f"| {table_cell(batch['id'])} | `{table_cell(batch['prompt'])}` | {batch['file_count']} | {batch['coverage_unit_count']} | {batch['interface_file_count']} | {table_cell(batch['purpose'])} |"
        for batch in manifest["batches"]
    )
    coverage = manifest["test_coverage_audit"]
    extras = (
        f"- UI test coverage worker prompt: `{coverage['ui_prompt']}` -> `{coverage['ui_report']}`\n"
        f"- Visual/e2e coverage worker prompt: `{coverage['visual_prompt']}` -> `{coverage['visual_report']}`"
        if coverage["ui_required"]
        else "- No UI or visual/e2e coverage prompts were generated because no interface-relevant files were queued."
    )
    return f"""# Full Repo Test Coverage Audit Index

Repo root: `{repo}`
Output directory: `{out_dir}`
Run ID: `{manifest['run_id']}`
Audit kind: `test-coverage`

Source files queued: **{manifest['source_file_count']}**
Coverage units queued: **{manifest['coverage_unit_count']}**
Batches: **{manifest['batch_count']}**
Scope warnings: **{manifest['scope_warning_count']}**

## Dispatch

1. Fill `effort_ledger.json` as workers are assigned.
2. Dispatch one fresh isolated worker per batch prompt with runtime/user-selected effort. Pass the entire prompt plus applicable project-ledger requirements.
3. Workers write complete reports to their prompt-declared `reports/` paths and return only bounded filename-bearing `REPORT_SAVED` receipts; never request the report body through the worker response.
4. Dispatch UI/visual workers when listed below.
5. Run verifier: `{manifest['verifier_command']}`
6. Redirect verbose command output to `logs/`, write the complete synthesis to `final-report.md`, and return only a compact outcome/counts/verifier/artifact summary to chat.

{extras}

## Batches

| Batch | Prompt | Files | Units | Interface Files | Purpose |
| --- | --- | ---: | ---: | ---: | --- |
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
    empirical_coverage: list[dict],
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

    target_inventory = test_targets.discover_targets(repo, units)
    targets_by_unit: dict[str, list[dict]] = {}
    for target in target_inventory:
        targets_by_unit.setdefault(target["unit_id"], []).append(target)

    batch_records: list[dict] = []
    all_paths: list[str] = []
    all_units: list[str] = []
    for index, batch in enumerate(batches, start=1):
        prompt = f"batch_{index:03d}.md"
        batch_targets = [target for item in batch for target in targets_by_unit.get(item.unit_id, [])]
        (out_dir / prompt).write_text(
            render_batch_prompt(
                repo,
                run_id,
                index,
                len(batches),
                batch,
                batch_targets,
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
                "interface_file_count": sum(1 for item in batch if item.interface_relevant),
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
    duplicate_units = sorted(unit for unit in unit_ids if all_units.count(unit) > 1)
    extra_units = sorted(set(all_units) - set(unit_ids))
    scope_warnings = [item for item in excluded if item.get("scope_warning")]
    pruned_hints = [item for item in excluded if item.get("entry_type") == "directory" and item.get("contains_source_like_samples")]
    interface_entries = [item for item in entries if item.interface_relevant]
    ui_required = bool(interface_entries)
    if ui_required:
        (out_dir / "ui_test_coverage_audit.md").write_text(
            render_ui_prompt(
                repo,
                run_id,
                interface_entries,
                reports_dir / "ui_test_coverage_audit.md",
            ),
            encoding="utf-8",
        )
        (out_dir / "visual_e2e_coverage_audit.md").write_text(
            render_visual_prompt(
                repo,
                run_id,
                interface_entries,
                reports_dir / "visual_e2e_coverage_audit.md",
            ),
            encoding="utf-8",
        )

    verifier_args = [
        sys.executable,
        str(Path(__file__).resolve().with_name("verify_test_coverage_audit_results.py")),
        "--manifest",
        str(out_dir / "manifest.json"),
        "--reports",
        str(reports_dir),
    ]
    generated_artifacts = [
        "audit_index.md",
        "effort_ledger.json",
        "excluded_files.json",
        "manifest.json",
        "queue_complete.json",
        "final-report.md",
        "logs",
        *(["ui_test_coverage_audit.md", "visual_e2e_coverage_audit.md"] if ui_required else []),
        *([archived_reports_name] if archived_reports_name else []),
        *[batch["prompt"] for batch in batch_records],
    ]
    manifest = {
        "repo_root": str(repo),
        "run_id": run_id,
        "audit_kind": "test-coverage",
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "reports_dir": str(reports_dir),
        "logs_dir": str(logs_dir),
        "final_report": str(out_dir / "final-report.md"),
        "archived_reports_dir": archived_reports_dir,
        "artifact_marker": str(out_dir / ARTIFACT_MARKER),
        "effort_ledger": str(out_dir / "effort_ledger.json"),
        "generated_artifacts": generated_artifacts,
        "verifier_command": " ".join(shlex.quote(arg) for arg in verifier_args),
        "verifier_args": verifier_args,
        "source_file_count": len(entries),
        "interface_file_count": len(interface_entries),
        "scope_warning_count": len(scope_warnings),
        "pruned_directory_review_hint_count": len(pruned_hints),
        "excluded_file_count": len(excluded),
        "excluded_files_sha256": queue.canonical_json_sha256(excluded),
        "batch_count": len(batch_records),
        "source_files": [asdict(item) for item in entries],
        "coverage_unit_count": len(units),
        "coverage_units": [asdict(item) for item in units],
        "batches": batch_records,
        "test_coverage_audit": {
            "ui_required": ui_required,
            "interface_files": [item.rel_path for item in interface_entries],
            "ui_prompt": "ui_test_coverage_audit.md" if ui_required else None,
            "ui_report": "reports/ui_test_coverage_audit.md" if ui_required else None,
            "visual_prompt": "visual_e2e_coverage_audit.md" if ui_required else None,
            "visual_report": "reports/visual_e2e_coverage_audit.md" if ui_required else None,
            "target_count": len(target_inventory),
            "target_inventory": target_inventory,
            "empirical_coverage": empirical_coverage,
            "coverage_claim_scope": (
                "empirical line evidence supplied and structurally bound to target lines"
                if empirical_coverage
                else "structural/manual audit only; no runtime coverage evidence supplied"
            ),
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
    write_effort_ledger(out_dir, manifest)
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
        else Path(tempfile.gettempdir()) / "full-repo-test-coverage-audit" / (repo.name or "repo") / f"{utc_stamp()}-{run_id[:8]}"
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
        include_files,
        args.include_glob,
        output_rel_dirs,
    )
    units = queue.audit_units_for(repo, entries, args.max_batch_bytes)
    try:
        empirical_coverage = test_targets.ingest_coverage_reports(repo, args.coverage_report)
    except (OSError, UnicodeDecodeError, json.JSONDecodeError, ValueError) as error:
        print(f"Could not ingest coverage report: {error}", file=sys.stderr)
        return 2
    batches = queue.batch_files(units, args.batch_size, args.max_batch_bytes)
    try:
        write_outputs(repo, out_dir, entries, excluded, units, batches, run_id, empirical_coverage)
    except ValueError as exc:
        print(str(exc), file=sys.stderr)
        return 2
    print(f"Wrote {len(batches)} test coverage batches covering {len(entries)} source files to {out_dir}")
    print(f"Excluded {len(excluded)} files; see {out_dir / 'excluded_files.json'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
