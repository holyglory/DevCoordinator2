#!/usr/bin/env python3
"""Self-test for the cross-report findings merge tool."""

from __future__ import annotations

import hashlib
import json
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from full_repo_harness import merge_findings


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


# full-repo-audit heading form; shares a finding with batch_002 (same file + summary).
BATCH_1 = """## Run ID
run-1

## Batch ID
batch_001

## File-Level Findings
### P1 - Save button uses placeholder console-only behavior
- Files: `src/SaveButton.tsx`
- Evidence: onClick only calls console.log
- Expected behavior/standard: save changes durably
- Gap: no persistence.
- Suggested direction: wire the handler.

### P2 - Minor copy inconsistency
- Files: `src/Header.tsx`
- Evidence: title casing differs.
- Gap: cosmetic.

## No Finding Notes
- other files fine.
"""

# UI field-block form; the SaveButton finding is a duplicate that should merge.
BATCH_2 = """## Run ID
run-1

## Batch ID
batch_002

## Implementation Gap Findings
- Priority: P1
- Files: `src/SaveButton.tsx`
- Interface evidence: Save changes button
- Summary: Save button uses placeholder console-only behavior
- Evidence: onClick only calls console.log
- Expected behavior/standard: save changes durably
- Gap: no persistence.
- Suggested implementation direction: wire the handler.

- Priority: P0
- Files: `src/auth/login.ts`
- Evidence: password compared with ==
- Gap: auth bypass risk
"""

BATCH_3 = """## Run ID
run-1

## Batch ID
batch_003

## Coverage Findings
No findings.
"""

BATCH_4 = """## Run ID
run-1

## Batch ID
batch_004

## File-Level Findings
### P2 - This deliberately long implementation finding summary shares the same first eighty characters but ends in calculation path A
- Files: `src/calculate.py`
- Evidence: branch A returns a fixed value for all inputs.
- Gap: calculation A is not implemented.

### P2 - This deliberately long implementation finding summary shares the same first eighty characters but ends in persistence path B
- Files: `src/calculate.py`
- Evidence: branch B reports success without durable storage.
- Gap: persistence B is not implemented.

### P2 - Repeated summary and gap must retain distinct create evidence
- Files: `src/store.py`
- Evidence: create_item returns success without writing the record.
- Expected behavior/standard: persist the requested mutation.
- Gap: the mutation is not persisted.
- Suggested direction: implement and verify durable storage.

### P2 - Repeated summary and gap must retain distinct create evidence
- Files: `src/store.py`
- Evidence: delete_item returns success without deleting the record.
- Expected behavior/standard: persist the requested mutation.
- Gap: the mutation is not persisted.
- Suggested direction: implement and verify durable storage.
"""

LEAD = """## Run ID
run-1

## Worker
lead_reconciliation

## Findings
### P1 - Registration never reaches the scheduled worker
- Files: `src/jobs.py`, `src/worker.py`
- Evidence: register_job stores a name that worker dispatch never reads.
- Interface evidence: Not applicable
- Expected behavior/standard: registered jobs must be dispatched.
- Gap: the cross-file registration and dispatch contract is disconnected.
- Suggested direction: use one registry and exercise a real scheduled run.

## Open Questions
None.
"""

ROGUE = """## Findings
### P0 - Unverified injected report
- Files: `src/rogue.py`
- Evidence: this file is not manifest-authorized.
- Gap: this must never enter consolidation.
"""


def main() -> int:
    tmp = Path(tempfile.mkdtemp(prefix="merge-findings-self-test-"))
    try:
        reports = tmp / "reports"
        reports.mkdir()
        (reports / "batch_001.md").write_text(BATCH_1, encoding="utf-8")
        (reports / "batch_002.md").write_text(BATCH_2, encoding="utf-8")
        (reports / "batch_003.md").write_text(BATCH_3, encoding="utf-8")
        (reports / "batch_004.md").write_text(BATCH_4, encoding="utf-8")
        (reports / "lead_reconciliation.md").write_text(LEAD, encoding="utf-8")
        (reports / "rogue.md").write_text(ROGUE, encoding="utf-8")

        manifest = {
            "batches": [{"id": f"batch_{index:03d}"} for index in range(1, 5)],
            "journey_audit": {"required": False},
            "lead_reconciliation": {
                "required": True,
                "report": "reports/lead_reconciliation.md",
            },
        }
        report_names = merge_findings.manifest_report_names(manifest)
        result = merge_findings.merge_findings(reports, report_names=report_names)

        check(result["reports_scanned"] == 5, f"expected 5 reports scanned, got {result['reports_scanned']}")
        check(result["raw_findings"] == 9, f"expected 9 raw findings, got {result['raw_findings']}")
        # SaveButton finding appears in two reports and must collapse to one.
        check(result["unique_findings"] == 8, f"expected 8 unique findings, got {result['unique_findings']}")
        check(result["ignored_unverified_reports"] == ["rogue.md"], "unverified Markdown must be ignored")
        check(set(result["report_sha256"]) == set(report_names), "every authorized report must be hash-bound")

        by_summary = {normalize(item["summary"]): item for item in result["findings"]}
        save = next(item for item in result["findings"] if "src/SaveButton.tsx" in item["files"])
        check(sorted(set(save["sources"])) == ["batch_001.md", "batch_002.md"], f"SaveButton finding should cite both reports: {save['sources']}")
        check(save["priority"] == "P1", f"SaveButton merged priority should be P1: {save['priority']}")
        check(save["suggested_direction"] == "wire the handler.", "merged findings should preserve implementation direction")
        check(save["candidate_id"].startswith("FRA-C-"), "merged findings should receive stable candidate ids")

        priorities = [item["priority"] for item in result["findings"]]
        check(priorities == sorted(priorities, key=lambda p: {"P0": 0, "P1": 1, "P2": 2, "P3": 3}[p]), f"findings must be ranked by priority: {priorities}")
        check(result["findings"][0]["priority"] == "P0", "most severe finding must sort first")
        check(result["priority_counts"] == {"P0": 1, "P1": 2, "P2": 5, "P3": 0}, f"unexpected counts: {result['priority_counts']}")
        calculate = [item for item in result["findings"] if "src/calculate.py" in item["files"]]
        check(len(calculate) == 2, "distinct findings with the same long prefix must not be truncated into one")
        store = [item for item in result["findings"] if "src/store.py" in item["files"]]
        check(len(store) == 2, "same summary/gap with distinct evidence must remain separate candidates")
        check(any("src/jobs.py" in item["files"] for item in result["findings"]), "lead-only finding must be merged")

        unicode_create = {
            "files": ["src/store.py"],
            "summary": "Сохранение не работает",
            "evidence": "Нет записи в хранилище",
            "expected_behavior": "Сохранить запись",
            "gap": "Нет записи",
            "suggested_direction": "Добавить сохранение",
        }
        unicode_delete = {
            **unicode_create,
            "summary": "Удаление не работает",
            "evidence": "Нет удаления из хранилища",
            "expected_behavior": "Удалить запись",
            "gap": "Нет удаления",
            "suggested_direction": "Добавить удаление",
        }
        check(
            merge_findings.candidate_id(unicode_create) != merge_findings.candidate_id(unicode_delete),
            "distinct non-ASCII findings must not collapse during normalization",
        )
        equality_guard = {
            **unicode_create,
            "summary": "Incorrect authorization guard",
            "evidence": "guard uses x == y",
        }
        inequality_guard = {
            **equality_guard,
            "evidence": "guard uses x != y",
        }
        check(
            merge_findings.dedupe_key(equality_guard) != merge_findings.dedupe_key(inequality_guard)
            and merge_findings.candidate_id(equality_guard) != merge_findings.candidate_id(inequality_guard),
            "code-significant operators and punctuation must remain part of finding identity",
        )

        try:
            merge_findings.merge_findings(reports, report_names=[*report_names, "missing.md"])
        except ValueError:
            pass
        else:
            raise AssertionError("missing manifest-authorized reports must fail closed")

        manifest_path = tmp / "manifest.json"
        manifest.update(
            {
                "run_id": "run-1",
                "repo_root": "/tmp/repo",
                "reports_dir": str(reports.resolve()),
            }
        )
        manifest_bytes = (json.dumps(manifest, indent=2) + "\n").encode("utf-8")
        manifest_path.write_bytes(manifest_bytes)
        receipt_path = tmp / "verification_receipt.json"
        receipt = {
            "schema_version": 1,
            "audit_kind": "full-repo-audit",
            "run_id": "run-1",
            "repo_root": "/tmp/repo",
            "manifest_sha256": hashlib.sha256(manifest_bytes).hexdigest(),
            "reports_dir": str(reports.resolve()),
            "report_sha256": result["report_sha256"],
            "verifier_result_sha256": "b" * 64,
        }
        receipt_path.write_text(json.dumps(receipt), encoding="utf-8")
        cli_json = tmp / "cli.json"
        check(
            merge_findings.main(
                [
                    "--reports",
                    str(reports),
                    "--manifest",
                    str(manifest_path),
                    "--verification-receipt",
                    str(receipt_path),
                    "--json-out",
                    str(cli_json),
                ]
            )
            == 0,
            "manifest mode must accept an exact verifier-bound report set",
        )
        check(cli_json.is_file(), "verified manifest-mode merge must publish requested output")

        original_batch = (reports / "batch_001.md").read_text(encoding="utf-8")
        (reports / "batch_001.md").write_text(ROGUE, encoding="utf-8")
        check(
            merge_findings.main(
                [
                    "--reports",
                    str(reports),
                    "--manifest",
                    str(manifest_path),
                    "--verification-receipt",
                    str(receipt_path),
                ]
            )
            == 2,
            "authorized report replacement after verification must be rejected",
        )
        (reports / "batch_001.md").write_text(original_batch, encoding="utf-8")

        alternate = tmp / "alternate-reports"
        alternate.mkdir()
        for name in report_names:
            (alternate / name).write_bytes((reports / name).read_bytes())
        check(
            merge_findings.main(
                [
                    "--reports",
                    str(alternate),
                    "--manifest",
                    str(manifest_path),
                    "--verification-receipt",
                    str(receipt_path),
                ]
            )
            == 2,
            "manifest mode must reject an alternate report root with matching basenames",
        )
        linked_reports = tmp / "linked-reports"
        linked_reports.symlink_to(reports, target_is_directory=True)
        check(
            merge_findings.main(
                [
                    "--reports",
                    str(linked_reports),
                    "--manifest",
                    str(manifest_path),
                    "--verification-receipt",
                    str(receipt_path),
                ]
            )
            == 2,
            "manifest mode must reject a symlinked report root",
        )

        original_reader = merge_findings.read_stable_regular_file
        mutated = False

        def mutate_manifest_after_receipt(path: Path, label: str):
            nonlocal mutated
            value = original_reader(path, label)
            if label == "verification receipt" and not mutated:
                mutated = True
                changed = {**manifest, "run_id": "run-2"}
                manifest_path.write_text(json.dumps(changed), encoding="utf-8")
            return value

        merge_findings.read_stable_regular_file = mutate_manifest_after_receipt
        try:
            check(
                merge_findings.main(
                    [
                        "--reports",
                        str(reports),
                        "--manifest",
                        str(manifest_path),
                        "--verification-receipt",
                        str(receipt_path),
                    ]
                )
                == 2,
                "manifest changes between selection and publication must be rejected",
            )
        finally:
            merge_findings.read_stable_regular_file = original_reader
            manifest_path.write_bytes(manifest_bytes)

        projection = merge_findings.render_completion_ledger_projection(
            result,
            run_id="run-1",
            repo_root="/tmp/repo",
            manifest_sha256="a" * 64,
        )
        check(projection["review_status"] == "pending", "ledger projection must require lead review")
        check(len(projection["candidates"]) == result["unique_findings"], "projection must preserve every candidate")
        check(
            all(item["disposition"] == "pending" and not item["ledger_row"]["verification"] for item in projection["candidates"]),
            "raw findings must not be directly publishable as completion rows",
        )

        markdown = merge_findings.render_markdown(result)
        check("## P0" in markdown and "auth bypass" in markdown.lower(), "markdown must render the P0 finding")
        check("`src/SaveButton.tsx`" in markdown, "markdown must cite merged finding files")

        print("merge-findings self-test ok")
        return 0
    finally:
        import shutil

        shutil.rmtree(tmp, ignore_errors=True)


def normalize(value: str) -> str:
    return value.strip().lower()


if __name__ == "__main__":
    raise SystemExit(main())
