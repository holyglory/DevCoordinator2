#!/usr/bin/env python3
"""Smoke tests for the full-repo-test-coverage-audit harness."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
BUILD = ROOT / "scripts" / "build_test_coverage_audit_batches.py"
VERIFY = ROOT / "scripts" / "verify_test_coverage_audit_results.py"
TIMEOUT = int(os.environ.get("FULL_REPO_TEST_COVERAGE_AUDIT_SELF_TEST_TIMEOUT", "30"))


def write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def run(args: list[str], *, expect: int = 0) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(args, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=TIMEOUT)
    if result.returncode != expect:
        raise AssertionError(
            f"Expected exit {expect} from {' '.join(args)}, got {result.returncode}\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    return result


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def make_ui_fixture(repo: Path) -> None:
    write(
        repo / "src" / "math.ts",
        """
export function clamp(value: number, min: number, max: number): number {
  if (min > max) throw new Error("invalid range");
  if (value < min) return min;
  if (value > max) return max;
  return value;
}

export async function loadUser(id: string, fetcher: (id: string) => Promise<{ id: string }>) {
  if (!id) throw new Error("id required");
  return fetcher(id);
}
""".strip()
        + "\n",
    )
    write(
        repo / "src" / "App.tsx",
        """
export function App() {
  return (
    <main>
      <button onClick={() => console.log("save")}>Save profile</button>
      <p role="status">Ready</p>
    </main>
  );
}
""".strip()
        + "\n",
    )
    write(
        repo / "tests" / "math.test.ts",
        """
import { clamp } from "../src/math";

test("clamp returns in-range values", () => {
  expect(clamp(2, 1, 3)).toBe(2);
});
""".strip()
        + "\n",
    )
    write(repo / "package.json", '{"scripts":{"test":"vitest"},"devDependencies":{"vitest":"latest"}}\n')


def make_cli_fixture(repo: Path) -> None:
    write(
        repo / "src" / "tool.py",
        """
def normalize_name(value: str) -> str:
    if not value:
        raise ValueError("value required")
    return value.strip().lower()
""".strip()
        + "\n",
    )


def complete_ledger(out: Path, manifest: dict) -> None:
    ledger_path = out / "effort_ledger.json"
    ledger = json.loads(ledger_path.read_text(encoding="utf-8"))
    ledger["subagent_capability_check"]["status"] = "completed"
    ledger["subagent_capability_check"]["spawn_tool"] = "self-test"
    ledger["subagent_capability_check"]["can_set_reasoning_effort"] = True
    ledger["lead_effort"]["status"] = "completed"
    ledger["lead_effort"]["actual_reasoning_effort"] = "high"
    ledger["lead_effort"]["agent_id"] = "self-test-lead"
    ledger["lead_effort"]["runtime_provenance"] = "self-test fixture"
    ledger["lead_effort"]["evidence"] = "self-test fixture"
    for worker in ledger["batch_workers"]:
        worker["status"] = "completed"
        worker["agent_id"] = "self-test"
        worker["actual_reasoning_effort"] = "low"
        worker["runtime_provenance"] = "self-test fixture"
    for key in ("ui_test_coverage_worker", "visual_e2e_coverage_worker"):
        if ledger[key]["status"] == "pending":
            ledger[key]["status"] = "completed"
            ledger[key]["agent_id"] = "self-test"
            ledger[key]["actual_reasoning_effort"] = "low"
            ledger[key]["runtime_provenance"] = "self-test fixture"
    if ledger["pruned_directory_review"]["status"] == "pending":
        ledger["pruned_directory_review"]["status"] = "completed"
    ledger_path.write_text(json.dumps(ledger, indent=2, sort_keys=True), encoding="utf-8")


def assert_ledger_mutation_fails(out: Path, tmp: Path, label: str, mutate) -> None:
    target = tmp / label
    shutil.copytree(out, target)
    ledger_path = target / "effort_ledger.json"
    ledger = json.loads(ledger_path.read_text(encoding="utf-8"))
    mutate(ledger)
    ledger_path.write_text(json.dumps(ledger, indent=2, sort_keys=True), encoding="utf-8")
    result = verify(target, expect=1)
    check("effort_ledger" in result.stdout or "runtime_provenance" in result.stdout or "agent_id" in result.stdout, f"{label} should fail effort ledger verification")


def batch_report(
    manifest: dict,
    batch: dict,
    *,
    out_of_scope_file: str | None = None,
    omit_inventory: bool = False,
    omit_target_symbol: str | None = None,
) -> str:
    unit_rows = []
    inventory_rows = []
    finding_blocks = []
    unit_by_id = {unit["unit_id"]: unit for unit in manifest["coverage_units"]}
    for unit_id in batch["coverage_units"]:
        unit = unit_by_id[unit_id]
        unit_rows.append(f"| {unit_id} | CHECKED | {unit['sha256']} | Source/test coverage unit |")
    targets = manifest["test_coverage_audit"]["target_inventory"]
    for target in targets:
        if target["unit_id"] not in batch["coverage_units"] or target["symbol"] == omit_target_symbol:
            continue
        if target["symbol"] == "clamp":
            disposition = "TESTED"
            covered_lines = {
                line
                for record in manifest["test_coverage_audit"].get("empirical_coverage", [])
                for line in record.get("files", {}).get("src/math.ts", {}).get("covered_lines", [])
            }
            level = "EMPIRICAL" if target["line"] in covered_lines else "STRUCTURAL"
            evidence = "tests/math.test.ts#clamp returns in-range values"
            assessment = "Happy path exists; invalid range and boundary scenarios are missing"
            recommendation = "Add boundary and thrown-error unit tests"
            finding_blocks.append(
                f"""- Priority: P1
- Files: src/math.ts
- Target ID: {target['target_id']}
- Target: clamp
- Existing test evidence: tests/math.test.ts#clamp returns in-range values
- Missing scenarios/boundaries: invalid min/max ordering, lower boundary, upper boundary
- Suggested test direction: add unit tests that assert thrown errors and boundary return values"""
            )
        elif target["kind"] == "unit-review":
            disposition = "NOT_REASONABLE"
            level = "MANUAL"
            evidence = "Not reasonable: supporting fixture or test/config file has no independently executable behavior target"
            assessment = "Reviewed structurally as support-only in this fixture"
            recommendation = "No direct target test; behavior is covered through owning source targets"
        else:
            disposition = "UNTESTED"
            level = "NONE"
            evidence = "None found"
            assessment = "No bound test path or runtime evidence; success and failure scenarios are absent"
            recommendation = "Add focused unit/component coverage with observable assertions"
            finding_blocks.append(
                f"""- Priority: P1
- Files: {target['rel_path']}
- Target ID: {target['target_id']}
- Target: {target['symbol']}
- Existing test evidence: None found
- Missing scenarios/boundaries: happy path, invalid input, failure behavior, and relevant state transitions
- Suggested test direction: add focused unit/component tests with observable result assertions"""
            )
        inventory_rows.append(
            f"| {target['target_id']} | {target['unit_id']} | {target['rel_path']} | {target['symbol']} | {target['kind']} | {disposition} | {level} | {evidence} | {assessment} | {recommendation} |"
        )
    findings = "\n\n".join(finding_blocks) if finding_blocks else "No findings."
    if out_of_scope_file:
        target_id = next(iter(target["target_id"] for target in targets if target["unit_id"] in batch["coverage_units"]), "target-missing")
        findings = f"""- Priority: P1
- Files: {out_of_scope_file}
- Target ID: {target_id}
- Target: out-of-scope target
- Existing test evidence: None found
- Missing scenarios/boundaries: finding is intentionally outside the batch
- Suggested test direction: keep findings scoped to owned files"""
    inventory = "\n".join(inventory_rows) if not omit_inventory else ""
    return f"""## Run ID
{manifest['run_id']}

## Batch ID
{batch['id']}

## Batch Summary
Fixture batch used by the self-test.

## File Coverage
| Unit | Status | SHA-256 | Purpose |
| --- | --- | --- | --- |
{chr(10).join(unit_rows)}

## Test Target Inventory
| Target ID | Unit | File | Target | Kind | Disposition | Evidence Level | Existing Test Evidence | Scenario Assessment | Recommendation |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
{inventory}

## Coverage Findings
{findings}

## No Gap Notes
Supporting files were reviewed and either mapped to a finding or marked as non-targets.

## Open Questions
None.
"""


def write_complete_reports(
    out: Path,
    *,
    out_of_scope: bool = False,
    omit_inventory: bool = False,
    omit_target_symbol: str | None = None,
) -> dict:
    manifest = json.loads((out / "manifest.json").read_text(encoding="utf-8"))
    complete_ledger(out, manifest)
    reports = out / "reports"
    reports.mkdir(exist_ok=True)
    all_files = [item["rel_path"] for item in manifest["source_files"]]
    fallback_out_of_scope = next((path for path in all_files if path != manifest["batches"][0]["files"][0]), "outside.ts")
    for index, batch in enumerate(manifest["batches"]):
        out_file = fallback_out_of_scope if out_of_scope and index == 0 else None
        write(
            reports / f"{batch['id']}.md",
            batch_report(
                manifest,
                batch,
                out_of_scope_file=out_file,
                omit_inventory=omit_inventory and index == 0,
                omit_target_symbol=omit_target_symbol,
            ),
        )
    coverage = manifest.get("test_coverage_audit", {})
    if coverage.get("ui_required"):
        write(
            reports / "ui_test_coverage_audit.md",
            f"""## Run ID
{manifest['run_id']}

## Worker
ui_test_coverage

## Journey/Test Sources
src/App.tsx and tests/math.test.ts.

## UI Coverage Checks
Save profile button has no component or e2e test evidence.

## Findings
- Priority: P1
- Files: src/App.tsx
- Target: Save profile button
- Existing test evidence: None found
- Missing scenarios/boundaries: click behavior, status update, failure state
- Suggested test direction: add component or e2e test for the save path

## Open Questions
None.
""",
        )
        write(
            reports / "visual_e2e_coverage_audit.md",
            f"""## Run ID
{manifest['run_id']}

## Worker
visual_e2e_coverage

## Visual/E2E Tooling
No Playwright, Cypress, or Storybook config found in fixture.

## Visual/E2E Coverage Checks
No safe visual harness exists for the UI fixture.

## Findings
- Priority: P2
- Files: src/App.tsx
- Target: profile screen visual state
- Existing test evidence: None found
- Missing scenarios/boundaries: desktop and mobile render checks
- Suggested test direction: add a lightweight component visual test or e2e smoke

## Open Questions
None.
""",
        )
    return manifest


def build(repo: Path, out: Path, *extra: str) -> dict:
    run([sys.executable, str(BUILD), "--repo", str(repo), "--out", str(out), "--run-id", "selftest-run", *extra])
    return json.loads((out / "manifest.json").read_text(encoding="utf-8"))


def verify(out: Path, *, expect: int = 0) -> subprocess.CompletedProcess[str]:
    return run([sys.executable, str(VERIFY), "--manifest", str(out / "manifest.json"), "--reports", str(out / "reports")], expect=expect)


def main() -> int:
    tmp = Path(tempfile.mkdtemp(prefix="full-repo-test-coverage-audit-self-test-"))
    try:
        repository_root = ROOT.parents[1]
        if str(repository_root) not in sys.path:
            sys.path.insert(0, str(repository_root))
        from full_repo_harness import verify_common
        from full_repo_harness import test_targets as target_tools

        escaped_rows = verify_common.parse_markdown_table_dicts(
            "| Unit | File | Target |\n"
            "| --- | --- | --- |\n"
            "| src/a.ts | src/a.ts | handles A \\| B branch |\n"
        )
        check(escaped_rows[0]["target"] == "handles A | B branch", "escaped Markdown table pipes should stay inside a cell")

        fixture = tmp / "ui-fixture"
        make_ui_fixture(fixture)
        coverage_formats = tmp / "coverage-formats"
        write(
            coverage_formats / "coverage.xml",
            '<coverage><packages><package><classes><class filename="src/math.ts"><lines><line number="1" hits="1"/><line number="8" hits="0"/></lines></class></classes></package></packages></coverage>',
        )
        write(
            coverage_formats / "coverage.json",
            json.dumps({"files": {"src/math.ts": {"executed_lines": [1], "missing_lines": [8]}}}),
        )
        write(
            coverage_formats / "istanbul.json",
            json.dumps(
                {
                    str(fixture / "src" / "math.ts"): {
                        "statementMap": {"0": {"start": {"line": 1}, "end": {"line": 1}}},
                        "s": {"0": 1},
                    }
                }
            ),
        )
        parsed_formats = target_tools.ingest_coverage_reports(
            fixture,
            [
                str(coverage_formats / "coverage.xml"),
                str(coverage_formats / "coverage.json"),
                str(coverage_formats / "istanbul.json"),
            ],
        )
        check(
            {record["format"] for record in parsed_formats} == {"cobertura-xml", "coverage.py-json", "istanbul-json"},
            "all advertised empirical JSON/XML formats should parse",
        )
        out = tmp / "out"
        manifest = build(fixture, out)
        check(manifest["audit_kind"] == "test-coverage", "manifest should record test-coverage audit kind")
        check("test_coverage_audit" in manifest, "manifest should include test_coverage_audit block")
        check(manifest["logs_dir"] == str((out / "logs").resolve()), "manifest should record cold logs_dir")
        check(manifest["final_report"] == str((out / "final-report.md").resolve()), "manifest should record final report path")
        check((out / "logs").is_dir(), "builder should create cold logs directory")
        marker = json.loads((out / ".full-repo-test-coverage-audit-artifacts.json").read_text(encoding="utf-8"))
        check("logs" in marker["generated_artifacts"] and "final-report.md" in marker["generated_artifacts"], "ownership marker should reserve cold logs and final report")
        for batch in manifest["batches"]:
            prompt_text = (out / batch["prompt"]).read_text(encoding="utf-8")
            check(str((out / batch["report"]).resolve()) in prompt_text, "batch prompt should contain exact report path")
            for token in (
                "fresh isolated context",
                "runtime/user-selected effort",
                "REPORT_SAVED",
                f"filename={Path(batch['report']).name};",
                "REPORT_NOT_SAVED path=<exact-report-path>; reason=<short-reason>",
                "at or below 80 tokens",
            ):
                check(token in prompt_text, f"batch prompt should enforce context-light artifact delivery: {token}")
        coverage_meta = manifest["test_coverage_audit"]
        for prompt_key, report_key in (("ui_prompt", "ui_report"), ("visual_prompt", "visual_report")):
            prompt_text = (out / coverage_meta[prompt_key]).read_text(encoding="utf-8")
            check(str((out / coverage_meta[report_key]).resolve()) in prompt_text, f"{prompt_key} should contain exact report path")
            check(
                "fresh isolated context" in prompt_text and "REPORT_SAVED" in prompt_text,
                f"{prompt_key} should use a compact isolated-worker contract",
            )
            check(
                f"filename={Path(coverage_meta[report_key]).name};" in prompt_text,
                f"{prompt_key} receipt should name its saved report filename",
            )
        skill_contract = (ROOT / "SKILL.md").read_text(encoding="utf-8")
        check("fresh isolated workers" in skill_contract, "skill must require isolated workers")
        check(
            "runtime/user-selected" in skill_contract,
            "skill must leave worker effort to the runtime or user",
        )
        for forbidden in ("Codex", "Claude", "fork_turns"):
            check(forbidden not in skill_contract, f"shared skill must remain neutral: {forbidden}")
        check(
            "final-report.md" in skill_contract and "filename-bearing `REPORT_SAVED`" in skill_contract,
            "skill must keep complete results in cold artifacts with filename-bearing receipts",
        )
        agent_metadata = (ROOT / "agents" / "openai.yaml").read_text(encoding="utf-8")
        check(
            "\npolicy:\n  allow_implicit_invocation: false\n" in f"\n{agent_metadata}",
            "full-repo-test-coverage-audit metadata must disable implicit invocation",
        )
        check(
            any(
                line.strip().startswith("default_prompt:") and "$full-repo-test-coverage-audit" in line
                for line in agent_metadata.splitlines()
            ),
            "full-repo-test-coverage-audit default prompt must explicitly invoke $full-repo-test-coverage-audit",
        )
        omitted_target_out = tmp / "omitted-target-out"
        build(fixture, omitted_target_out)
        write_complete_reports(omitted_target_out, omit_target_symbol="loadUser")
        omitted_target_result = verify(omitted_target_out, expect=1)
        check("loadUser" in omitted_target_result.stdout, "omitting a real exported target must fail verification")
        write_complete_reports(out)
        result = verify(out)
        check("ok: true" in result.stdout, "complete report should verify")

        manual_target_out = tmp / "manual-target-out"
        shutil.copytree(out, manual_target_out)
        manual_manifest = json.loads((manual_target_out / "manifest.json").read_text(encoding="utf-8"))
        app_unit = next(unit["unit_id"] for unit in manual_manifest["coverage_units"] if unit["rel_path"] == "src/App.tsx")
        manual_report = next(
            path
            for path in (manual_target_out / "reports").glob("batch_*.md")
            if "src/App.tsx" in path.read_text(encoding="utf-8")
        )
        manual_row = f"| manual-focus-check | {app_unit} | src/App.tsx | keyboard focus walkthrough | manual-journey | TESTED | MANUAL | manual: keyboard walkthrough exercised tab order and activation in fixture mode | focus and activation worked in the recorded walkthrough | retain repeatable keyboard acceptance steps |"
        manual_report.write_text(
            manual_report.read_text(encoding="utf-8").replace("\n\n## Coverage Findings", f"\n{manual_row}\n\n## Coverage Findings"),
            encoding="utf-8",
        )
        manual_target_result = verify(manual_target_out)
        check("ok: true" in manual_target_result.stdout, "honest manually discovered targets should be accepted alongside deterministic targets")

        invalid_test_reference_out = tmp / "invalid-test-reference-out"
        shutil.copytree(out, invalid_test_reference_out)
        report_with_reference = next((invalid_test_reference_out / "reports").glob("batch_*.md"))
        report_with_reference.write_text(
            report_with_reference.read_text(encoding="utf-8").replace(
                "tests/math.test.ts#clamp returns in-range values",
                "tests/math.test.ts#invented test symbol",
            ),
            encoding="utf-8",
        )
        invalid_test_reference_result = verify(invalid_test_reference_out, expect=1)
        check("test symbol/name is absent" in invalid_test_reference_result.stdout, "invented test symbols must fail verification")

        weak_exclusion_out = tmp / "weak-exclusion-out"
        shutil.copytree(out, weak_exclusion_out)
        weak_exclusion_report = next((weak_exclusion_out / "reports").glob("batch_*.md"))
        weak_exclusion_report.write_text(
            weak_exclusion_report.read_text(encoding="utf-8").replace(
                "Not reasonable: supporting fixture or test/config file has no independently executable behavior target",
                "Not reasonable: trivial",
                1,
            ),
            encoding="utf-8",
        )
        weak_exclusion_result = verify(weak_exclusion_out, expect=1)
        check("NOT_REASONABLE requires" in weak_exclusion_result.stdout, "weak not-reasonable rationales must fail")

        mislabeled_empirical_out = tmp / "mislabeled-empirical-out"
        shutil.copytree(out, mislabeled_empirical_out)
        mislabeled_report = next((mislabeled_empirical_out / "reports").glob("batch_*.md"))
        mislabeled_report.write_text(mislabeled_report.read_text(encoding="utf-8").replace("| STRUCTURAL |", "| EMPIRICAL |", 1), encoding="utf-8")
        mislabeled_result = verify(mislabeled_empirical_out, expect=1)
        check("not backed by a supplied coverage report" in mislabeled_result.stdout, "structural evidence must not be mislabeled empirical")

        coverage_file = tmp / "runtime evidence" / "lcov.info"
        write(
            coverage_file,
            f"TN:self-test\nSF:{fixture / 'src' / 'math.ts'}\nDA:1,1\nDA:8,0\nend_of_record\n",
        )
        empirical_out = tmp / "empirical-out"
        build(fixture, empirical_out, "--coverage-report", str(coverage_file))
        write_complete_reports(empirical_out)
        empirical_result = verify(empirical_out)
        check("ok: true" in empirical_result.stdout, "valid LCOV evidence should support an EMPIRICAL target claim")
        write(coverage_file, coverage_file.read_text(encoding="utf-8") + "# changed\n")
        stale_empirical_result = verify(empirical_out, expect=1)
        check("coverage evidence hash changed" in stale_empirical_result.stdout, "changed empirical evidence must fail hash verification")

        missing_report_out = tmp / "missing-report-out"
        shutil.copytree(out, missing_report_out)
        first_report = next((missing_report_out / "reports").glob("batch_*.md"))
        first_report.unlink()
        missing_result = verify(missing_report_out, expect=1)
        check("missing_reports" in missing_result.stdout, "missing batch report should fail verification")

        weak_ledger_out = tmp / "weak-ledger-out"
        shutil.copytree(out, weak_ledger_out)
        weak_ledger = json.loads((weak_ledger_out / "effort_ledger.json").read_text(encoding="utf-8"))
        weak_ledger["batch_workers"][0]["actual_reasoning_effort"] = None
        (weak_ledger_out / "effort_ledger.json").write_text(json.dumps(weak_ledger, indent=2, sort_keys=True), encoding="utf-8")
        weak_ledger_result = verify(weak_ledger_out, expect=1)
        check("actual_reasoning_effort" in weak_ledger_result.stdout, "missing worker effort should fail verification")
        assert_ledger_mutation_fails(
            out,
            tmp,
            "weak-capability-ledger-out",
            lambda ledger: ledger["subagent_capability_check"].update({"can_set_reasoning_effort": None}),
        )
        assert_ledger_mutation_fails(
            out,
            tmp,
            "weak-lead-provenance-out",
            lambda ledger: ledger["lead_effort"].update({"runtime_provenance": ""}),
        )
        assert_ledger_mutation_fails(
            out,
            tmp,
            "weak-worker-agent-out",
            lambda ledger: ledger["batch_workers"][0].update({"agent_id": ""}),
        )
        assert_ledger_mutation_fails(
            out,
            tmp,
            "weak-ui-provenance-out",
            lambda ledger: ledger["ui_test_coverage_worker"].update({"runtime_provenance": ""}),
        )

        missing_inventory_out = tmp / "missing-inventory-out"
        manifest = build(fixture, missing_inventory_out)
        write_complete_reports(missing_inventory_out, omit_inventory=True)
        missing_inventory_result = verify(missing_inventory_out, expect=1)
        check("Test Target Inventory" in missing_inventory_result.stdout, "missing target inventory should fail verification")

        out_of_scope_out = tmp / "out-of-scope-out"
        manifest = build(fixture, out_of_scope_out, "--batch-size", "1")
        write_complete_reports(out_of_scope_out, out_of_scope=True)
        out_of_scope_result = verify(out_of_scope_out, expect=1)
        check("out_of_scope" in out_of_scope_result.stdout, "out-of-batch finding should fail verification")

        stale_out = tmp / "stale-out"
        build(fixture, stale_out)
        write_complete_reports(stale_out)
        write(fixture / "src" / "math.ts", (fixture / "src" / "math.ts").read_text(encoding="utf-8") + "\n// changed\n")
        stale_result = verify(stale_out, expect=1)
        check("current_hash_mismatches" in stale_result.stdout, "stale source hash should fail verification")

        scope_fixture = tmp / "scope-fixture"
        make_cli_fixture(scope_fixture)
        scope_out = tmp / "scope-out"
        build(scope_fixture, scope_out, "--exclude-glob", "src/tool.py")
        complete_ledger(scope_out, json.loads((scope_out / "manifest.json").read_text(encoding="utf-8")))
        scope_result = verify(scope_out, expect=1)
        check("unresolved scope warnings" in scope_result.stdout, "scope warning should fail verification")

        cli_fixture = tmp / "cli-fixture"
        make_cli_fixture(cli_fixture)
        cli_out = tmp / "cli-out"
        cli_manifest = build(cli_fixture, cli_out)
        check(not cli_manifest["test_coverage_audit"]["ui_required"], "CLI fixture should not require visual reports")
        write_complete_reports(cli_out)
        cli_result = verify(cli_out)
        check("ok: true" in cli_result.stdout, "non-UI fixture should verify without visual reports")

        stale_output_fixture = tmp / "stale-output-fixture"
        make_cli_fixture(stale_output_fixture)
        stale_owned_dir = stale_output_fixture / "old-test-output"
        stale_owned_dir.mkdir(parents=True)
        write(
            stale_owned_dir / ".full-repo-test-coverage-audit-artifacts.json",
            json.dumps(
                {
                    "owned_by": "full-repo-test-coverage-audit",
                    "repo_root": str(stale_output_fixture.resolve()),
                    "generated_artifacts": ["manifest.json"],
                },
                indent=2,
            ),
        )
        write(stale_owned_dir / "manifest.json", '{"stale": true}\n')
        stale_owned_out = tmp / "stale-owned-out"
        stale_owned_manifest = build(stale_output_fixture, stale_owned_out)
        queued_files = {item["rel_path"] for item in stale_owned_manifest["source_files"]}
        check(
            "old-test-output/.full-repo-test-coverage-audit-artifacts.json" not in queued_files,
            "owned stale test-coverage output directories should not be queued",
        )

        print("self-test ok")
        return 0
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
