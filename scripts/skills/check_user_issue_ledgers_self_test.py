#!/usr/bin/env python3
"""Recall and precision tests for scoped user issue ledger validation."""

from __future__ import annotations

import importlib.util
import os
import tempfile
from pathlib import Path


SCRIPT = Path(__file__).with_name("check_user_issue_ledgers.py")
SPEC = importlib.util.spec_from_file_location("check_user_issue_ledgers", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("unable to load scoped user issue ledger checker")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)

UI = """# User Issue Ledger: UI

| ID | Applies to | Mistake pattern | Required behavior | Prevention and verification |
| --- | --- | --- | --- | --- |
| UIL-UI-001 | Collection destinations | A collection page put the creation form before the collection. | Show the real collection or honest state first. | Exercise populated, empty, and long-list creation flows at wide and narrow widths. |
| UIL-UI-002 | Long collection destinations | A create action appended its form below a long list. | Reveal a focused creation surface in the current viewport. | Trigger create after a long list and assert visibility, focus, save, and restored context. |
"""

PRICING = """# User Issue Ledger: Business logic / pricing

| ID | Applies to | Mistake pattern | Required behavior | Prevention and verification |
| --- | --- | --- | --- | --- |
| UIL-BUSINESS-LOGIC-PRICING-001 | Tax-inclusive invoice totals | An intermediate currency value was rounded before all line adjustments were applied. | Preserve specified precision until final currency rounding. | Compare boundary and high-precision cases against an independent reference, including a previously resolved invoice. |
"""

PERMISSIONS = """# User Issue Ledger: Business logic / permissions

| ID | Applies to | Mistake pattern | Required behavior | Prevention and verification |
| --- | --- | --- | --- | --- |
| UIL-BUSINESS-LOGIC-PERMISSIONS-001 | Approval actions | A user without approval authority was shown an enabled approval action. | Derive visibility and enabled state from the same enforced permission rule. | Exercise authorized and unauthorized users through both the UI and server action. |
"""

EVERYTHING = """# User Issue Ledger: Everything

| ID | Applies to | Mistake pattern | Required behavior | Prevention and verification |
| --- | --- | --- | --- | --- |
| UIL-EVERYTHING-001 | Collection pages | A creation form displaced the named collection. | Show the collection first. | Verify the first viewport. |
| UIL-EVERYTHING-002 | Release jobs | A retry duplicated a published artifact. | Make publication idempotent. | Replay the job after an injected timeout. |
"""


def messages(text: str) -> str:
    return "\n".join(MODULE.find_ledger_violations(text))


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def write_valid_tree(root: Path) -> None:
    root.mkdir()
    (root / "UI.md").write_text(UI, encoding="utf-8")
    business = root / "BusinessLogic"
    business.mkdir()
    (business / "Pricing.md").write_text(PRICING, encoding="utf-8")
    (business / "Permissions.md").write_text(PERMISSIONS, encoding="utf-8")


def main() -> int:
    check(not MODULE.find_ledger_violations(UI), "realistic UI ledger must pass")
    check(not MODULE.find_ledger_violations(PRICING), "nested business perspective must pass")
    check(not MODULE.find_ledger_violations(PERMISSIONS), "second business perspective must pass")
    escaped = UI.replace("real collection", "real A \\| B collection")
    check(not MODULE.find_ledger_violations(escaped), "escaped pipes must pass")
    check(
        "duplicates an existing applicability" not in messages(UI),
        "distinct related UI mistakes must not be merged by inference",
    )

    cases = (
        (UI.replace("# User Issue Ledger: UI", "# Incident History"), "line 1"),
        (UI.replace("\n\n| ID", "\n| ID"), "blank line"),
        (UI.replace("| Applies to |", "| Area |"), "table header"),
        ("\n".join(UI.splitlines()[:4]) + "\n", "at least one issue row"),
        (UI + "Narrative: this happened on Tuesday.\n", "pipe-delimited"),
        (UI.replace("| UIL-UI-002 |", "| UIL-UI-001 |"), "duplicates ID"),
        (UI.replace("| UIL-UI-002 |", "| UI-2 |"), "UIL-<SCOPE>-NNN"),
        (
            UI.replace(
                "| UIL-UI-002 | Long collection destinations |",
                "| UIL-UI-002 |  |",
            ),
            "empty 'Applies to'",
        ),
        (
            UI.replace(
                "| UIL-UI-002 | Long collection destinations | A create action appended its form below a long list. |",
                "| UIL-UI-002 | Collection destinations | A collection page put the creation form before the collection! |",
            ),
            "duplicates an existing applicability and mistake pattern",
        ),
        (UI.replace("restored context.", "restored<br>context."), "one compact table cell"),
    )
    for broken, expected in cases:
        check(expected in messages(broken), f"must catch {expected!r}")

    with tempfile.TemporaryDirectory(prefix="user-issue-ledgers-") as raw:
        base = Path(raw).resolve()
        absent = base / "Absent"
        check(not MODULE.audit_ledgers(absent), "an absent ledger tree must pass")

        valid = base / "Valid"
        write_valid_tree(valid)
        check(not MODULE.audit_ledgers(valid), "multiple scoped ledgers must pass")

        catch_all = base / "CatchAll"
        catch_all.mkdir()
        (catch_all / "Everything.md").write_text(EVERYTHING, encoding="utf-8")
        check(
            "scope is too broad" in "\n".join(MODULE.audit_ledgers(catch_all)),
            "one catch-all file must not combine UI and automation patterns under a generic namespace",
        )

        broad_business = base / "BroadBusiness"
        broad_business.mkdir()
        (broad_business / "BusinessLogic.md").write_text(
            PRICING.replace(
                "# User Issue Ledger: Business logic / pricing",
                "# User Issue Ledger: Business logic",
            ).replace(
                "UIL-BUSINESS-LOGIC-PRICING-001",
                "UIL-BUSINESS-LOGIC-001",
            ),
            encoding="utf-8",
        )
        check(
            "bounded business-logic perspective" in "\n".join(MODULE.audit_ledgers(broad_business)),
            "business logic must be split by perspective rather than stored in one broad file",
        )

        wrong_title = base / "WrongTitle"
        wrong_title.mkdir()
        (wrong_title / "UI.md").write_text(
            UI.replace("# User Issue Ledger: UI", "# User Issue Ledger: Automation"),
            encoding="utf-8",
        )
        check(
            "title scope must match" in "\n".join(MODULE.audit_ledgers(wrong_title)),
            "a ledger title must remain bound to its path",
        )

        wrong_nested_id = base / "WrongNestedId"
        write_valid_tree(wrong_nested_id)
        (wrong_nested_id / "BusinessLogic" / "Pricing.md").write_text(
            PRICING.replace("UIL-BUSINESS-LOGIC-PRICING-001", "UIL-BUSINESS-PRICING-001"),
            encoding="utf-8",
        )
        check(
            "UIL-BUSINESS-LOGIC-PRICING-<NNN>" in "\n".join(MODULE.audit_ledgers(wrong_nested_id)),
            "nested business perspectives must own their complete ID namespace",
        )

        empty = base / "Empty"
        empty.mkdir()
        check("at least one scoped ledger" in "\n".join(MODULE.audit_ledgers(empty)), "empty tree must fail")

        duplicate_id = base / "DuplicateId"
        write_valid_tree(duplicate_id)
        duplicate_text = PRICING.replace("UIL-BUSINESS-LOGIC-PRICING-001", "UIL-UI-001")
        (duplicate_id / "BusinessLogic" / "Pricing.md").write_text(duplicate_text, encoding="utf-8")
        check("duplicates global ID" in "\n".join(MODULE.audit_ledgers(duplicate_id)), "global IDs must be unique")

        duplicate_pattern = base / "DuplicatePattern"
        write_valid_tree(duplicate_pattern)
        copied = UI.replace("# User Issue Ledger: UI", "# User Issue Ledger: Automation").replace(
            "UIL-UI-001", "UIL-AUTOMATION-001"
        ).replace("UIL-UI-002", "UIL-AUTOMATION-002")
        (duplicate_pattern / "Automation.md").write_text(copied, encoding="utf-8")
        check(
            "keep one narrow owner" in "\n".join(MODULE.audit_ledgers(duplicate_pattern)),
            "cross-ledger duplicate patterns must fail",
        )

        invalid_name = base / "InvalidName"
        write_valid_tree(invalid_name)
        (invalid_name / "mixed ledger.md").write_text(UI, encoding="utf-8")
        check("UpperCamelCase" in "\n".join(MODULE.audit_ledgers(invalid_name)), "ambiguous filenames must fail")

        final_link = base / "FinalLink"
        write_valid_tree(final_link)
        (final_link / "Linked.md").symlink_to(final_link / "UI.md")
        check("must not be symlinks" in "\n".join(MODULE.audit_ledgers(final_link)), "file symlink must fail")

        linked_parent = base / "LinkedParent"
        linked_parent.mkdir()
        (linked_parent / "UI.md").write_text(UI, encoding="utf-8")
        root_link = base / "RootLink"
        root_link.symlink_to(linked_parent, target_is_directory=True)
        check("symlink" in "\n".join(MODULE.audit_ledgers(root_link)), "root symlink must fail")

        intermediate = base / "Intermediate"
        intermediate.mkdir()
        (intermediate / "UI.md").write_text(UI, encoding="utf-8")
        (intermediate / "BusinessLogic").symlink_to(valid / "BusinessLogic", target_is_directory=True)
        check("must not be symlinks" in "\n".join(MODULE.audit_ledgers(intermediate)), "directory symlink must fail")

        traversal_error = base / "TraversalError"
        write_valid_tree(traversal_error)
        moved_business = base / "MovedBusiness"
        original_lstat = MODULE.Path.lstat
        nested_replaced = False

        def lstat_then_replace_nested_directory(path: Path, *args, **kwargs):
            nonlocal nested_replaced
            metadata = original_lstat(path, *args, **kwargs)
            if path == traversal_error / "BusinessLogic" and not nested_replaced:
                nested_replaced = True
                path.rename(moved_business)
            return metadata

        MODULE.Path.lstat = lstat_then_replace_nested_directory
        try:
            traversal_error_messages = "\n".join(MODULE.audit_ledgers(traversal_error))
        finally:
            MODULE.Path.lstat = original_lstat
        check(
            "cannot traverse scoped-ledger directory" in traversal_error_messages,
            "a disappeared nested directory must fail closed even when another ledger remains",
        )

        traversal = "\n".join(MODULE.audit_ledgers(valid / "BusinessLogic" / ".."))
        check("parent traversal" in traversal, "operator parent traversal must fail")

        fifo_tree = base / "Fifo"
        fifo_tree.mkdir()
        os.mkfifo(fifo_tree / "Pipe.md")
        check("regular files" in "\n".join(MODULE.audit_ledgers(fifo_tree)), "FIFO must fail")

        invalid_utf8 = base / "InvalidUtf8"
        invalid_utf8.mkdir()
        (invalid_utf8 / "Broken.md").write_bytes(b"\xff\xfe")
        check("UTF-8" in "\n".join(MODULE.audit_ledgers(invalid_utf8)), "invalid UTF-8 must fail")

        raced = base / "Raced"
        write_valid_tree(raced)
        raced_ui = raced / "UI.md"
        moved = base / "MovedRaced"
        original_read = MODULE.secure_reader.os.read
        replaced = False

        def read_after_parent_replacement(descriptor: int, size: int):
            nonlocal replaced
            if not replaced:
                replaced = True
                raced.rename(moved)
                raced.mkdir()
                raced_ui.write_text(UI, encoding="utf-8")
            return original_read(descriptor, size)

        MODULE.secure_reader.os.read = read_after_parent_replacement
        try:
            raced_messages = "\n".join(MODULE.audit_ledgers(raced))
        finally:
            MODULE.secure_reader.os.read = original_read
        check("parent path changed" in raced_messages, "parent replacement race must fail closed")

    print("scoped user issue ledger checker self-test ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
