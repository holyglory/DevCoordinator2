#!/usr/bin/env python3
"""Recall, precision, crash-recovery, and portability tests for policy deployment."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any, Callable, Dict, Iterable, Optional, Tuple


SCRIPT = Path(__file__).with_name("manage_global_policy.py").resolve()
SPEC = importlib.util.spec_from_file_location("manage_global_policy", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("unable to load global-policy manager")
MANAGER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MANAGER)


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def expect_error(
    function: Callable[..., Any],
    *args: Any,
    contains: Optional[str] = None,
    **kwargs: Any,
) -> str:
    try:
        function(*args, **kwargs)
    except MANAGER.PolicyManagerError as error:
        text = str(error)
        if contains is not None:
            check(contains.casefold() in text.casefold(), f"expected {contains!r} in {text!r}")
        return text
    raise AssertionError(f"{function.__name__} unexpectedly succeeded")


def make_repository(base: Path, name: str = "repo", content: str = "# Universal Agent Instructions\n") -> Tuple[Path, Path]:
    repository = base / name
    directory = repository / "reference" / "universal"
    directory.mkdir(parents=True)
    source = directory / "AGENTS.md"
    source.write_text(content, encoding="utf-8")
    return repository, source


def make_roots(base: Path, name: str) -> Tuple[Path, Path, Path]:
    target = base / f"{name}-target"
    state = base / f"{name}-state"
    target.mkdir()
    state.mkdir()
    return target, state, state / "transaction"


def plan_one(
    repository: Path,
    state: Path,
    target: Path,
    *,
    role: str = "codex",
    wrapper: bool = False,
) -> Tuple[Path, Dict[str, Any], str]:
    transaction = state / f"transaction-{time.time_ns()}"
    arguments: Dict[str, Iterable[Path]] = {}
    if wrapper:
        arguments["claude_wrapper_targets"] = [target]
    elif role == "claude":
        arguments["claude_targets"] = [target]
    else:
        arguments["codex_targets"] = [target]
    plan, digest = MANAGER.create_plan(repository, transaction, **arguments)
    return transaction, plan, digest


def assert_direct(path: Path, source: Path) -> None:
    check(path.is_symlink(), f"expected direct symlink: {path}")
    check(os.readlink(path) == str(source), f"wrong link text at {path}")
    check(path.resolve(strict=True) == source, f"wrong resolved link target at {path}")


def journal(transaction: Path) -> Dict[str, Any]:
    return json.loads((transaction / MANAGER.JOURNAL_NAME).read_text(encoding="utf-8"))


def replace_json(path: Path, value: Dict[str, Any], *, mode: int = 0o600) -> None:
    if os.name != "nt":
        os.chmod(path, 0o600)
    MANAGER._atomic_bytes_write(path, MANAGER._canonical_json(value), mode)


def can_symlink(base: Path) -> bool:
    source = base / "symlink-capability-source"
    link = base / "symlink-capability-link"
    source.write_text("x", encoding="utf-8")
    try:
        os.symlink(str(source), link)
    except OSError:
        return False
    else:
        link.unlink()
        return True


def test_windows_serialization_and_neutral_context() -> None:
    raw = r"c:\Users\Zoë Smith\DevCoordinator2\reference\universal\AGENTS.md"
    expected = "C:/Users/Zoë Smith/DevCoordinator2/reference/universal/AGENTS.md"
    check(MANAGER.serialize_windows_import_path(raw) == expected, "Windows path serialization drifted")
    wrapper = MANAGER.claude_wrapper_bytes(raw, windows=True)
    check(wrapper == ("@" + expected + "\n").encode("utf-8"), "Windows wrapper bytes drifted")
    check(wrapper.count(b"\n") == 1 and wrapper.startswith(b"@"), "wrapper must remain one import line")
    expect_error(MANAGER.serialize_windows_import_path, r"\\server\share\AGENTS.md", contains="UNC")
    expect_error(MANAGER.serialize_windows_import_path, r"relative\AGENTS.md", contains="absolute")
    expect_error(MANAGER.serialize_windows_import_path, "C:\\bad\npath", contains="control")

    positive_authority = re.compile(
        r"(?i)\b(?:you|the agent|agent)\s+(?:are|is)\s+authorized\b|"
        r"\bauthorized\s+to\s+act\b|\bgrants?\s+(?:the\s+agent\s+)?authority\b"
    )
    text = wrapper.decode("utf-8")
    check(not positive_authority.search(text), "generated startup wrapper asserted authority")
    command = MANAGER.render_shell_command(
        [r"C:\Program Files\Python\python.exe", r"C:\DevCoordinator2\manage_global_policy.py", "apply"],
        windows=True,
    )
    check(command.startswith("& 'C:\\Program Files\\Python\\python.exe' "), "PowerShell call operator missing")


def test_exact_install_verify_and_rollback(base: Path, symlinks: bool) -> None:
    repository, source = make_repository(base, "exact-repo")
    codex_root = base / "exact-codex"
    claude_root = base / "exact-claude"
    state = base / "exact-state"
    codex_root.mkdir()
    claude_root.mkdir()
    state.mkdir()
    codex = codex_root / "AGENTS.md"
    claude = claude_root / "CLAUDE.md"
    codex.write_text("PRIVATE EXISTING CODEX POLICY\n", encoding="utf-8")
    os.chmod(codex, 0o640)
    timestamp = 1_700_000_000_123_456_789
    os.utime(codex, ns=(timestamp, timestamp))
    if hasattr(os, "setxattr"):
        try:
            os.setxattr(codex, "user.devcoordinator2-test", b"preserve")
        except OSError:
            pass
    legacy = base / "legacy claude policy.md"
    legacy.write_text("legacy\n", encoding="utf-8")

    if symlinks:
        os.symlink("../legacy claude policy.md", claude)
        before_claude = MANAGER.snapshot_path(claude)
        plan, digest = MANAGER.create_plan(
            repository,
            state / "transaction",
            codex_targets=[codex],
            claude_targets=[claude],
        )
    else:
        claude.write_text("PRIVATE EXISTING CLAUDE POLICY\n", encoding="utf-8")
        before_claude = MANAGER.snapshot_path(claude)
        plan, digest = MANAGER.create_plan(
            repository,
            state / "transaction",
            claude_wrapper_targets=[claude],
        )
    before_codex = MANAGER.snapshot_path(codex)
    serialized = json.dumps(plan, sort_keys=True)
    check("PRIVATE EXISTING" not in serialized, "review plan leaked prior policy contents")
    rendered = MANAGER.render_plan(plan, digest)
    check("runtime_activation: not-observed" in rendered, "plan overstated runtime activation")
    check("--plan-digest" in rendered and digest in rendered, "plan did not bind manual review")

    MANAGER.apply_transaction(state / "transaction", digest)
    if symlinks:
        assert_direct(codex, source)
        assert_direct(claude, source)
    else:
        expected = MANAGER.claude_wrapper_bytes(source, windows=True)
        check(claude.read_bytes() == expected, "native Windows Claude wrapper bytes differ")
    verified = MANAGER.verify_transaction(state / "transaction", digest)
    check(verified["filesystem_state"] == "installed", "installed filesystem state was not verified")
    check(verified["runtime_activation"] == "not-observed", "filesystem verify overstated activation")

    # A normal update of the canonical file is exactly why the runtime entry is
    # a link/import rather than a copied mirror.  It must not block topology
    # verification or restoring the prior runtime-owned file.
    replacement = source.with_suffix(".new")
    replacement.write_text("# Universal Agent Instructions\n\nUpdated.\n", encoding="utf-8")
    os.replace(replacement, source)
    later = MANAGER.verify_transaction(state / "transaction", digest)
    check(later["canonical_source_changed_since_plan"] is True, "source update was not reported")

    MANAGER.rollback_transaction(state / "transaction", digest)
    check(MANAGER.snapshot_path(codex) == before_codex, "regular file was not restored exactly")
    check(MANAGER.snapshot_path(claude) == before_claude, "Claude prior state was not restored exactly")
    check(codex.read_text(encoding="utf-8") == "PRIVATE EXISTING CODEX POLICY\n", "prior bytes changed")
    if hasattr(os, "getxattr"):
        try:
            value = os.getxattr(codex, "user.devcoordinator2-test")
        except OSError:
            value = None
        if value is not None:
            check(value == b"preserve", "extended metadata was not preserved by rename rollback")
    again = MANAGER.rollback_transaction(state / "transaction", digest)
    check(again["filesystem_state"] == "rolled-back", "rollback was not idempotent")


def test_retain_and_unrelated_sibling(base: Path) -> None:
    repository, source = make_repository(base, "retain-repo")
    target_root, state, _ = make_roots(base, "retain")
    target = target_root / "AGENTS.md"
    sibling = target_root / "unrelated.txt"
    os.symlink(str(source), target)
    sibling.write_bytes(b"do not touch\x00sibling")
    before = MANAGER.snapshot_path(target)
    transaction, plan, digest = plan_one(repository, state, target)
    check(plan["entries"][0]["action"] == "retain", "exact direct link should be retained")
    MANAGER.apply_transaction(transaction, digest)
    check(MANAGER.snapshot_path(target) == before, "retained link was needlessly replaced")
    check(sibling.read_bytes() == b"do not touch\x00sibling", "unrelated sibling was changed")
    MANAGER.rollback_transaction(transaction, digest)
    check(MANAGER.snapshot_path(target) == before, "retained link changed during rollback")


def test_source_and_target_drift(base: Path) -> None:
    repository, source = make_repository(base, "drift-repo")
    target_root, state, _ = make_roots(base, "drift")
    target = target_root / "AGENTS.md"
    target.write_text("old\n", encoding="utf-8")
    before = MANAGER.snapshot_path(target)
    transaction, _, digest = plan_one(repository, state, target)
    source.write_text("# Universal Agent Instructions\nchanged in place\n", encoding="utf-8")
    expect_error(MANAGER.apply_transaction, transaction, digest, contains="changed after planning")
    check(MANAGER.snapshot_path(target) == before, "source drift mutated the target")

    repository2, source2 = make_repository(base, "swap-repo")
    target_root2, state2, _ = make_roots(base, "swap")
    target2 = target_root2 / "AGENTS.md"
    target2.write_text("old swap\n", encoding="utf-8")
    before2 = MANAGER.snapshot_path(target2)
    transaction2, _, digest2 = plan_one(repository2, state2, target2)
    moved = source2.with_suffix(".moved")
    os.replace(source2, moved)
    source2.write_bytes(moved.read_bytes())
    expect_error(MANAGER.apply_transaction, transaction2, digest2, contains="changed after planning")
    check(MANAGER.snapshot_path(target2) == before2, "source inode swap mutated the target")

    repository3, _ = make_repository(base, "target-drift-repo")
    target_root3, state3, _ = make_roots(base, "target-drift")
    target3 = target_root3 / "AGENTS.md"
    target3.write_text("planned\n", encoding="utf-8")
    transaction3, _, digest3 = plan_one(repository3, state3, target3)
    target3.write_text("external newer state\n", encoding="utf-8")
    expect_error(MANAGER.apply_transaction, transaction3, digest3, contains="drifted")
    check(target3.read_text(encoding="utf-8") == "external newer state\n", "target drift was overwritten")


def test_parent_swap_and_transaction_tamper(base: Path) -> None:
    repository, _ = make_repository(base, "parent-swap-repo")
    target_root, state, _ = make_roots(base, "parent-swap")
    target = target_root / "AGENTS.md"
    target.write_text("before\n", encoding="utf-8")
    transaction, _, digest = plan_one(repository, state, target)
    old_parent = target_root.with_name(target_root.name + "-old")
    os.replace(target_root, old_parent)
    target_root.mkdir()
    (target_root / "AGENTS.md").write_text("replacement parent\n", encoding="utf-8")
    expect_error(MANAGER.apply_transaction, transaction, digest, contains="identity changed")
    check((target_root / "AGENTS.md").read_text(encoding="utf-8") == "replacement parent\n", "replacement parent was touched")
    check((old_parent / "AGENTS.md").read_text(encoding="utf-8") == "before\n", "original parent was touched")

    repository2, _ = make_repository(base, "tamper-repo")
    target_root2, state2, _ = make_roots(base, "tamper")
    target2 = target_root2 / "AGENTS.md"
    transaction2, plan2, digest2 = plan_one(repository2, state2, target2)
    plan_path = transaction2 / MANAGER.PLAN_NAME
    tampered = json.loads(plan_path.read_text(encoding="utf-8"))
    tampered["entries"][0]["destination"] = str(base / "outside" / "AGENTS.md")
    replace_json(plan_path, tampered, mode=0o400)
    expect_error(MANAGER.apply_transaction, transaction2, digest2, contains="digest")
    check(not target2.exists(), "old-digest plan tamper changed the target")

    # Even if a caller supplies a new digest for edited JSON, derived adjacent
    # paths cannot be redirected through traversal or to an unrelated sibling.
    modified = dict(plan2)
    modified["entries"] = [dict(plan2["entries"][0])]
    modified["entries"][0]["backup"] = str(target_root2 / ".." / "stolen")
    payload = MANAGER._canonical_json(modified)
    new_digest = hashlib.sha256(payload).hexdigest()
    replace_json(plan_path, modified, mode=0o400)
    journal_value = journal(transaction2)
    journal_value["plan_sha256"] = new_digest
    replace_json(transaction2 / MANAGER.JOURNAL_NAME, journal_value)
    expect_error(MANAGER.apply_transaction, transaction2, new_digest, contains="backup")


def test_unsafe_inputs(base: Path, symlinks: bool) -> None:
    repository, source = make_repository(base, "unsafe-repo")
    target_root, state, _ = make_roots(base, "unsafe")
    expect_error(
        MANAGER.create_plan,
        repository,
        state / "bad-name-transaction",
        codex_targets=[target_root / "wrong.md"],
        contains="AGENTS.md",
    )
    target = target_root / "AGENTS.md"
    expect_error(
        MANAGER.create_plan,
        repository,
        state / "duplicate-transaction",
        codex_targets=[target, target],
        contains="duplicate",
    )
    expect_error(
        MANAGER.create_plan,
        repository,
        state / "inside-transaction",
        codex_targets=[repository / "AGENTS.md"],
        contains="outside",
    )
    target.mkdir()
    expect_error(
        MANAGER.create_plan,
        repository,
        state / "directory-transaction",
        codex_targets=[target],
        contains="directory",
    )
    target.rmdir()
    original = target_root / "hardlink-source"
    original.write_text("hard-linked\n", encoding="utf-8")
    try:
        os.link(original, target)
    except OSError:
        pass
    else:
        expect_error(
            MANAGER.create_plan,
            repository,
            state / "hardlink-transaction",
            codex_targets=[target],
            contains="hard-linked",
        )
        target.unlink()

    if os.name != "nt":
        expect_error(
            MANAGER.create_plan,
            repository,
            state / "wrapper-transaction",
            claude_wrapper_targets=[target_root / "CLAUDE.md"],
            contains="native Windows",
        )
    if symlinks:
        alias = base / "unsafe-repo-alias"
        os.symlink(str(repository), alias)
        expect_error(
            MANAGER.create_plan,
            alias,
            state / "repo-alias-transaction",
            codex_targets=[target],
            contains="real directory",
        )
        external = base / "external-policy.md"
        external.write_text(source.read_text(encoding="utf-8"), encoding="utf-8")
        source.unlink()
        os.symlink(str(external), source)
        expect_error(
            MANAGER.create_plan,
            repository,
            state / "source-link-transaction",
            codex_targets=[target],
            contains="real regular file",
        )


def test_backup_collision_and_no_fallback(base: Path) -> None:
    repository, _ = make_repository(base, "collision-repo")
    target_root, state, _ = make_roots(base, "collision")
    target = target_root / "AGENTS.md"
    target.write_text("exact old state\n", encoding="utf-8")
    before = MANAGER.snapshot_path(target)
    transaction, plan, digest = plan_one(repository, state, target)
    backup = Path(plan["entries"][0]["backup"])
    backup.write_text("unrelated collision\n", encoding="utf-8")
    expect_error(MANAGER.apply_transaction, transaction, digest, contains="not empty")
    check(MANAGER.snapshot_path(target) == before, "backup collision changed target")
    check(backup.read_text(encoding="utf-8") == "unrelated collision\n", "collision was deleted")

    repository2, _ = make_repository(base, "fallback-repo")
    target_root2, state2, _ = make_roots(base, "fallback")
    target2 = target_root2 / "AGENTS.md"
    target2.write_text("preserve when symlink unavailable\n", encoding="utf-8")
    before2 = MANAGER.snapshot_path(target2)
    transaction2, _, digest2 = plan_one(repository2, state2, target2)
    original_symlink = MANAGER.os.symlink
    try:
        def fail_symlink(*args: Any, **kwargs: Any) -> None:
            raise OSError("simulated missing symlink capability")

        MANAGER.os.symlink = fail_symlink
        expect_error(MANAGER.apply_transaction, transaction2, digest2, contains="no copy")
    finally:
        MANAGER.os.symlink = original_symlink
    check(MANAGER.snapshot_path(target2) == before2, "failed direct link did not restore original")
    check(not target2.read_bytes().startswith(b"@"), "Codex silently fell back to an import wrapper")


def test_atomic_boundary_collisions(base: Path, symlinks: bool) -> None:
    repository, _ = make_repository(base, "boundary-repo")
    target_root, state, _ = make_roots(base, "boundary")
    target = target_root / ("AGENTS.md" if symlinks else "CLAUDE.md")
    target.write_text("reviewed prior state\n", encoding="utf-8")
    before = MANAGER.snapshot_path(target)

    transaction, plan, digest = plan_one(repository, state, target, wrapper=not symlinks)
    entry = plan["entries"][0]
    backup = Path(entry["backup"])
    original_move = MANAGER._move_noreplace_in_parent
    injected = False

    def collide_at_backup(lock: Any, source_name: str, destination_name: str) -> None:
        nonlocal injected
        if not injected and destination_name == backup.name:
            injected = True
            (lock.path / destination_name).write_text("external backup collision\n", encoding="utf-8")
        original_move(lock, source_name, destination_name)

    MANAGER._move_noreplace_in_parent = collide_at_backup
    try:
        expect_error(MANAGER.apply_transaction, transaction, digest, contains="collision")
    finally:
        MANAGER._move_noreplace_in_parent = original_move
    check(MANAGER.snapshot_path(target) == before, "backup-boundary collision overwrote target")
    check(backup.read_text(encoding="utf-8") == "external backup collision\n", "backup collision was lost")
    backup.unlink()
    MANAGER.rollback_transaction(transaction, digest)

    transaction2, plan2, digest2 = plan_one(repository, state, target, wrapper=not symlinks)
    entry2 = plan2["entries"][0]
    temporary2 = Path(entry2["temporary"])
    backup2 = Path(entry2["backup"])
    injected = False

    def collide_at_install(lock: Any, source_name: str, destination_name: str) -> None:
        nonlocal injected
        if not injected and source_name == temporary2.name and destination_name == target.name:
            injected = True
            (lock.path / destination_name).write_text("external install collision\n", encoding="utf-8")
        original_move(lock, source_name, destination_name)

    MANAGER._move_noreplace_in_parent = collide_at_install
    try:
        expect_error(MANAGER.apply_transaction, transaction2, digest2, contains="collision")
    finally:
        MANAGER._move_noreplace_in_parent = original_move
    check(target.read_text(encoding="utf-8") == "external install collision\n", "install collision was overwritten")
    check(MANAGER.snapshot_path(backup2) == before, "prior state was not preserved after install collision")
    check(MANAGER._matches_desired(temporary2, entry2), "prepared desired object was lost on collision")
    target.unlink()
    MANAGER.rollback_transaction(transaction2, digest2)
    check(MANAGER.snapshot_path(target) == before, "collision recovery did not restore exact prior state")


def test_rollback_retention_drift_is_preserved(base: Path, symlinks: bool) -> None:
    repository, _ = make_repository(base, "cleanup-repo")
    target_root, state, _ = make_roots(base, "cleanup")
    target = target_root / ("AGENTS.md" if symlinks else "CLAUDE.md")
    target.write_text("cleanup prior\n", encoding="utf-8")
    before = MANAGER.snapshot_path(target)
    transaction, plan, digest = plan_one(repository, state, target, wrapper=not symlinks)
    MANAGER.apply_transaction(transaction, digest)
    temporary = Path(plan["entries"][0]["temporary"])

    def replace_before_retention(point: str, entry_id: str) -> None:
        if point == "before-rollback-retain":
            check(temporary.exists() or temporary.is_symlink(), "retention fixture expected captured desired")
            temporary.unlink()
            temporary.write_text("external retained-name replacement\n", encoding="utf-8")

    expect_error(
        MANAGER.rollback_transaction,
        transaction,
        digest,
        fault_hook=replace_before_retention,
        contains="drifted and was preserved",
    )
    check(MANAGER.snapshot_path(target) == before, "prior target was not restored before cleanup drift")
    check(
        temporary.read_text(encoding="utf-8") == "external retained-name replacement\n",
        "rollback deleted a writer replacement at its retention boundary",
    )
    temporary.unlink()
    MANAGER.rollback_transaction(transaction, digest)
    check(MANAGER.snapshot_path(target) == before, "rollback did not recover after cleanup drift removal")


def _crash_apply(transaction: Path, digest: str, point: str) -> subprocess.CompletedProcess[str]:
    code = (
        "import importlib.util, os, pathlib\n"
        f"path=pathlib.Path({str(SCRIPT)!r})\n"
        "spec=importlib.util.spec_from_file_location('crash_policy_manager', path)\n"
        "module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module)\n"
        f"wanted={point!r}\n"
        "def fault(point, entry):\n"
        "    if point == wanted: os._exit(73)\n"
        f"module.apply_transaction(pathlib.Path({str(transaction)!r}), {digest!r}, fault_hook=fault)\n"
    )
    environment = dict(os.environ)
    environment["PYTHONDONTWRITEBYTECODE"] = "1"
    return subprocess.run(
        [sys.executable, "-c", code],
        text=True,
        capture_output=True,
        env=environment,
        timeout=20,
    )


def _crash_rollback(transaction: Path, digest: str, point: str) -> subprocess.CompletedProcess[str]:
    code = (
        "import importlib.util, os, pathlib\n"
        f"path=pathlib.Path({str(SCRIPT)!r})\n"
        "spec=importlib.util.spec_from_file_location('crash_policy_manager', path)\n"
        "module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module)\n"
        f"wanted={point!r}\n"
        "def fault(point, entry):\n"
        "    if point == wanted: os._exit(74)\n"
        f"module.rollback_transaction(pathlib.Path({str(transaction)!r}), {digest!r}, fault_hook=fault)\n"
    )
    environment = dict(os.environ)
    environment["PYTHONDONTWRITEBYTECODE"] = "1"
    return subprocess.run(
        [sys.executable, "-c", code],
        text=True,
        capture_output=True,
        env=environment,
        timeout=20,
    )


def test_crash_recovery(base: Path, symlinks: bool) -> None:
    points = ("before-backup", "after-backup", "after-temp-create", "after-install", "before-commit")
    for index, point in enumerate(points):
        case = base / f"crash-{index}"
        case.mkdir()
        repository, _ = make_repository(case)
        target_root, state, _ = make_roots(case, "runtime")
        target = target_root / ("AGENTS.md" if symlinks else "CLAUDE.md")
        target.write_text(f"before {point}\n", encoding="utf-8")
        os.chmod(target, 0o640)
        before = MANAGER.snapshot_path(target)
        transaction, _, digest = plan_one(repository, state, target, wrapper=not symlinks)
        result = _crash_apply(transaction, digest, point)
        check(result.returncode == 73, f"crash fixture did not stop at {point}: {result.stderr}")
        MANAGER.rollback_transaction(transaction, digest)
        check(MANAGER.snapshot_path(target) == before, f"crash recovery was not exact at {point}")
        MANAGER.verify_transaction(transaction, digest)


def test_rollback_crash_recovery(base: Path, symlinks: bool) -> None:
    points = (
        "before-rollback-capture",
        "after-rollback-capture",
        "before-rollback-restore",
        "after-rollback-restore",
        "before-rollback-retain",
        "after-rollback-retain",
    )
    for index, point in enumerate(points):
        case = base / f"rollback-crash-{index}"
        case.mkdir()
        repository, _ = make_repository(case)
        target_root, state, _ = make_roots(case, "runtime")
        target = target_root / ("AGENTS.md" if symlinks else "CLAUDE.md")
        target.write_text(f"before {point}\n", encoding="utf-8")
        os.chmod(target, 0o640)
        before = MANAGER.snapshot_path(target)
        transaction, _, digest = plan_one(repository, state, target, wrapper=not symlinks)
        MANAGER.apply_transaction(transaction, digest)
        result = _crash_rollback(transaction, digest, point)
        check(result.returncode == 74, f"rollback crash fixture did not stop at {point}: {result.stderr}")
        MANAGER.rollback_transaction(transaction, digest)
        check(MANAGER.snapshot_path(target) == before, f"rollback crash recovery was not exact at {point}")
        MANAGER.verify_transaction(transaction, digest)

    missing_points = (
        "after-rollback-capture",
        "before-rollback-retain",
        "after-rollback-retain",
    )
    for index, point in enumerate(missing_points):
        case = base / f"missing-rollback-crash-{index}"
        case.mkdir()
        repository, _ = make_repository(case)
        target_root, state, _ = make_roots(case, "runtime")
        target = target_root / ("AGENTS.md" if symlinks else "CLAUDE.md")
        transaction, _, digest = plan_one(repository, state, target, wrapper=not symlinks)
        MANAGER.apply_transaction(transaction, digest)
        result = _crash_rollback(transaction, digest, point)
        check(result.returncode == 74, f"missing-target rollback crash missed {point}: {result.stderr}")
        MANAGER.rollback_transaction(transaction, digest)
        check(not target.exists() and not target.is_symlink(), f"missing target was not restored at {point}")
        MANAGER.verify_transaction(transaction, digest)

    if symlinks:
        case = base / "multi-entry-rollback-crash"
        case.mkdir()
        repository, _ = make_repository(case)
        first_root, state, _ = make_roots(case, "first")
        second_root = case / "second-target"
        second_root.mkdir()
        first = first_root / "AGENTS.md"
        second = second_root / "CLAUDE.md"
        first.write_text("first prior\n", encoding="utf-8")
        second.write_text("second prior\n", encoding="utf-8")
        first_before = MANAGER.snapshot_path(first)
        second_before = MANAGER.snapshot_path(second)
        transaction = state / "multi-transaction"
        _, digest = MANAGER.create_plan(
            repository,
            transaction,
            codex_targets=[first],
            claude_targets=[second],
        )
        MANAGER.apply_transaction(transaction, digest)
        result = _crash_rollback(transaction, digest, "after-rollback-restore")
        check(result.returncode == 74, f"multi-entry rollback crash fixture missed boundary: {result.stderr}")
        MANAGER.rollback_transaction(transaction, digest)
        check(MANAGER.snapshot_path(first) == first_before, "multi-entry crash missed first restoration")
        check(MANAGER.snapshot_path(second) == second_before, "multi-entry crash missed second restoration")
        MANAGER.verify_transaction(transaction, digest)


def test_partial_failure_and_all_or_nothing_rollback(base: Path) -> None:
    repository, source = make_repository(base, "multi-repo")
    first_root = base / "multi-first"
    second_root = base / "multi-second"
    state = base / "multi-state"
    first_root.mkdir()
    second_root.mkdir()
    state.mkdir()
    first = first_root / "AGENTS.md"
    second = second_root / "CLAUDE.md"
    first.write_text("first before\n", encoding="utf-8")
    second.write_text("second before\n", encoding="utf-8")
    first_before = MANAGER.snapshot_path(first)
    second_before = MANAGER.snapshot_path(second)
    plan, digest = MANAGER.create_plan(
        repository,
        state / "auto-rollback",
        codex_targets=[first],
        claude_targets=[second],
    )
    second_id = next(entry["id"] for entry in plan["entries"] if entry["role"] == "claude")

    def fail_second(point: str, entry_id: str) -> None:
        if point == "after-backup" and entry_id == second_id:
            raise RuntimeError("simulated second-target failure")

    expect_error(
        MANAGER.apply_transaction,
        state / "auto-rollback",
        digest,
        fault_hook=fail_second,
        contains="rollback succeeded",
    )
    check(MANAGER.snapshot_path(first) == first_before, "first target not restored after later failure")
    check(MANAGER.snapshot_path(second) == second_before, "second target not restored after failure")

    # A separate successful apply demonstrates that rollback preflights all
    # targets before changing any of them.
    plan2, digest2 = MANAGER.create_plan(
        repository,
        state / "blocked-rollback",
        codex_targets=[first],
        claude_targets=[second],
    )
    MANAGER.apply_transaction(state / "blocked-rollback", digest2)
    assert_direct(first, source)
    assert_direct(second, source)
    second.unlink()
    second.write_text("external post-apply drift\n", encoding="utf-8")
    expect_error(
        MANAGER.rollback_transaction,
        state / "blocked-rollback",
        digest2,
        contains="drift blocks",
    )
    assert_direct(first, source)
    check(second.read_text(encoding="utf-8") == "external post-apply drift\n", "drift was overwritten")
    second.unlink()
    os.symlink(str(source), second)
    MANAGER.rollback_transaction(state / "blocked-rollback", digest2)
    check(MANAGER.snapshot_path(first) == first_before, "first target not restored after drift correction")
    check(MANAGER.snapshot_path(second) == second_before, "second target not restored after drift correction")


def _apply_process(transaction: Path, digest: str) -> subprocess.Popen[str]:
    environment = dict(os.environ)
    environment["PYTHONDONTWRITEBYTECODE"] = "1"
    return subprocess.Popen(
        [
            sys.executable,
            str(SCRIPT),
            "apply",
            "--transaction-dir",
            str(transaction),
            "--plan-digest",
            digest,
        ],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=environment,
    )


def test_concurrent_stale_plans(base: Path, symlinks: bool) -> None:
    repository, _ = make_repository(base, "concurrent-repo")
    first_root = base / "concurrent-a"
    second_root = base / "concurrent-b"
    state = base / "concurrent-state"
    first_root.mkdir()
    second_root.mkdir()
    state.mkdir()
    if symlinks:
        first = first_root / "AGENTS.md"
        second = second_root / "CLAUDE.md"
        kwargs_one = {"codex_targets": [first], "claude_targets": [second]}
        kwargs_two = {"codex_targets": [first], "claude_targets": [second]}
    else:
        first = first_root / "CLAUDE.md"
        second = second_root / "CLAUDE.md"
        kwargs_one = {"claude_wrapper_targets": [first, second]}
        kwargs_two = {"claude_wrapper_targets": [second, first]}
    _, digest_one = MANAGER.create_plan(repository, state / "one", **kwargs_one)
    _, digest_two = MANAGER.create_plan(repository, state / "two", **kwargs_two)
    one = _apply_process(state / "one", digest_one)
    two = _apply_process(state / "two", digest_two)
    out_one, err_one = one.communicate(timeout=20)
    out_two, err_two = two.communicate(timeout=20)
    codes = sorted([one.returncode, two.returncode])
    check(codes == [0, 2], f"concurrent stale plans were not serialized: {codes}; {err_one}; {err_two}")
    winner = (state / "one", digest_one) if one.returncode == 0 else (state / "two", digest_two)
    loser = (state / "two", digest_two) if one.returncode == 0 else (state / "one", digest_one)
    check("status: applied" in (out_one + out_two), "successful concurrent apply was not reported")
    check("drifted" in (err_one + err_two), "stale concurrent plan did not report target drift")
    MANAGER.rollback_transaction(*winner)
    MANAGER.rollback_transaction(*loser)
    check(not first.exists() and not second.exists(), "concurrent plans did not restore missing targets")


def test_transaction_symlink_rejected(base: Path, symlinks: bool) -> None:
    if not symlinks:
        return
    repository, _ = make_repository(base, "transaction-link-repo")
    target_root, state, _ = make_roots(base, "transaction-link")
    target = target_root / "AGENTS.md"
    transaction, _, digest = plan_one(repository, state, target)
    moved = transaction.with_name(transaction.name + "-real")
    os.replace(transaction, moved)
    os.symlink(str(moved), transaction)
    expect_error(MANAGER.apply_transaction, transaction, digest, contains="real directory")


def test_transaction_swap_during_apply_rolls_back(base: Path, symlinks: bool) -> None:
    repository, _ = make_repository(base, "transaction-swap-repo")
    target_root, state, _ = make_roots(base, "transaction-swap")
    target = target_root / ("AGENTS.md" if symlinks else "CLAUDE.md")
    target.write_text("transaction swap prior state\n", encoding="utf-8")
    before = MANAGER.snapshot_path(target)
    transaction, _, digest = plan_one(repository, state, target, wrapper=not symlinks)
    moved = transaction.with_name(transaction.name + "-moved")
    swapped = False

    def swap_after_backup(point: str, entry_id: str) -> None:
        nonlocal swapped
        if point == "after-backup" and not swapped:
            os.rename(transaction, moved)
            transaction.mkdir()
            swapped = True

    try:
        expect_error(
            MANAGER.apply_transaction,
            transaction,
            digest,
            fault_hook=swap_after_backup,
            contains="transaction directory identity changed",
        )
        check(MANAGER.snapshot_path(target) == before, "transaction swap stranded a mutated runtime target")
        check(journal(moved)["status"] == "apply-failed-rolled-back", "detached journal missed rollback")
    finally:
        if transaction.exists():
            transaction.rmdir()
        if moved.exists():
            os.rename(moved, transaction)
    MANAGER.verify_transaction(transaction, digest)


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="global-policy-manager-self-test-") as temporary:
        # macOS commonly returns /var while /var is a system symlink.  The
        # product correctly rejects caller-supplied symlink components; tests
        # canonicalize only their own temporary root before deriving fixtures.
        root = Path(temporary).resolve()
        symlinks = can_symlink(root)
        test_windows_serialization_and_neutral_context()
        test_exact_install_verify_and_rollback(root / "exact", symlinks)
        if symlinks:
            (root / "retain").mkdir()
            test_retain_and_unrelated_sibling(root / "retain")
        (root / "drifts").mkdir()
        test_source_and_target_drift(root / "drifts")
        (root / "tamper").mkdir()
        test_parent_swap_and_transaction_tamper(root / "tamper")
        (root / "unsafe").mkdir()
        test_unsafe_inputs(root / "unsafe", symlinks)
        (root / "collision").mkdir()
        test_backup_collision_and_no_fallback(root / "collision")
        (root / "boundary-collisions").mkdir()
        test_atomic_boundary_collisions(root / "boundary-collisions", symlinks)
        (root / "rollback-cleanup").mkdir()
        test_rollback_retention_drift_is_preserved(root / "rollback-cleanup", symlinks)
        (root / "crashes").mkdir()
        test_crash_recovery(root / "crashes", symlinks)
        (root / "rollback-crashes").mkdir()
        test_rollback_crash_recovery(root / "rollback-crashes", symlinks)
        if symlinks:
            (root / "multi").mkdir()
            test_partial_failure_and_all_or_nothing_rollback(root / "multi")
        (root / "concurrent").mkdir()
        test_concurrent_stale_plans(root / "concurrent", symlinks)
        (root / "transaction-link").mkdir()
        test_transaction_symlink_rejected(root / "transaction-link", symlinks)
        (root / "transaction-swap").mkdir()
        test_transaction_swap_during_apply_rolls_back(root / "transaction-swap", symlinks)

    platform_note = MANAGER.host_family()
    evidence = "native" if platform_note in {"windows", "macos", "linux", "wsl"} else "portable"
    print(
        "global policy manager self-test ok "
        f"({evidence} {platform_note}; Windows serialization fixture; runtime activation not inferred)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
