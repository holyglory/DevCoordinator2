"""Retained formal-UI journey evidence and screenshot-anchored feedback.

Evidence is caller-owned cold data beneath one governed run log leaf.  The
root daemon opens every path component with ``O_NOFOLLOW``, validates the
privacy-safe manifest, and discloses image bytes only in bounded chunks.
Annotations are immutable overlays linked to ordinary Plan ``user_feedback``
tasks; the captured PNG is never modified.
"""

from __future__ import annotations

import base64
import hashlib
import json
import os
import re
import stat
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

from devcoordinator2 import ids
from devcoordinator2.daemon import plan_state
from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.deploy_state import now_iso
from devcoordinator2.daemon.gitinfo import GitResolveError, resolve_worktree
from devcoordinator2.daemon.registry import Registry
from devcoordinator2.protocol import ProtocolError

MANIFEST_NAME = "journey-evidence.json"
MANIFEST_KIND = "formal-web-ui-journey-evidence"
MANIFEST_SCHEMA = 1
MAX_MANIFEST_BYTES = 2 * 1024 * 1024
MAX_IMAGE_BYTES = 16 * 1024 * 1024
MAX_IMAGE_CHUNK_BYTES = 180 * 1024
MAX_MANIFESTS = 256
MAX_CELLS = 512
MAX_COMMENTS = 512
MAX_MARKS = 64
MAX_POINTS = 256

_RUN_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
_CHECK_RE = re.compile(r"[a-z0-9][a-z0-9-]{0,63}$")
_CASE_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
_CELL_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,255}$")
_MARK_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9_-]{0,63}$")
_SHA_RE = re.compile(r"[0-9a-f]{64}$")
_COLORS = frozenset(
    {
        "#4c8dff",
        "#f59e0b",
        "#ef4444",
        "#22c55e",
        "#a855f7",
        "#f8fafc",
    }
)
_PHASES = frozenset({"check", "discovery", "case"})


@dataclass(frozen=True)
class _Leaf:
    check: str
    phase: str
    case: str | None


@dataclass(frozen=True)
class _Image:
    image_id: str
    leaf: _Leaf
    relative_path: str
    mime: str
    size: int
    sha256: str
    width: int
    height: int
    kind: str
    cell: dict[str, Any]


def _actor(caller) -> str:
    return caller.identity or f"uid:{caller.uid}"


def _display_actor(value: str) -> str:
    return value if "@" in value else "Local administrator"


def _open_dir(parent_fd: int | None, name: str | Path) -> int:
    try:
        return os.open(
            name,
            os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
            dir_fd=parent_fd,
        )
    except OSError as exc:
        raise FileNotFoundError(str(name)) from exc


def _open_run_dir(worktree: Path, run_id: str) -> int:
    fd = _open_dir(None, worktree)
    try:
        for name in (".devcoordinator", "test", "logs", "runs", run_id):
            child = _open_dir(fd, name)
            os.close(fd)
            fd = child
        return fd
    except BaseException:
        os.close(fd)
        raise


def _open_leaf_dir(run_fd: int, leaf: _Leaf) -> int:
    fd = os.dup(run_fd)
    try:
        for name in ("checks", leaf.check):
            child = _open_dir(fd, name)
            os.close(fd)
            fd = child
        if leaf.phase == "case":
            for name in ("cases", leaf.case or ""):
                child = _open_dir(fd, name)
                os.close(fd)
                fd = child
        else:
            child = _open_dir(fd, leaf.phase)
            os.close(fd)
            fd = child
        return fd
    except BaseException:
        os.close(fd)
        raise


def _open_evidence_dir(run_fd: int, leaf: _Leaf) -> int:
    fd = _open_leaf_dir(run_fd, leaf)
    try:
        child = _open_dir(fd, "evidence")
        os.close(fd)
        return child
    except BaseException:
        os.close(fd)
        raise


def _read_regular(dir_fd: int, name: str, maximum: int) -> tuple[bytes, os.stat_result]:
    fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW, dir_fd=dir_fd)
    try:
        before = os.fstat(fd)
        if not stat.S_ISREG(before.st_mode) or before.st_size > maximum:
            raise ValueError("file is not a bounded regular file")
        data = bytearray()
        while len(data) <= maximum:
            block = os.read(fd, min(1024 * 1024, maximum + 1 - len(data)))
            if not block:
                break
            data.extend(block)
        after = os.fstat(fd)
        if _identity(before) != _identity(after) or len(data) != before.st_size:
            raise ValueError("file changed while it was read")
        return bytes(data), before
    finally:
        os.close(fd)


def _identity(details: os.stat_result) -> tuple[int, int, int, int, int]:
    return (
        details.st_dev,
        details.st_ino,
        details.st_size,
        details.st_mtime_ns,
        details.st_ctime_ns,
    )


def _png_dimensions(header: bytes) -> tuple[int, int] | None:
    if len(header) < 24 or not header.startswith(b"\x89PNG\r\n\x1a\n"):
        return None
    return int.from_bytes(header[16:20], "big"), int.from_bytes(header[20:24], "big")


def _safe_name(value: Any, regex: re.Pattern[str], label: str) -> str:
    if value is None:
        raise ProtocolError("args_invalid", f"'{label}' is required")
    if not isinstance(value, str) or not regex.fullmatch(value):
        raise ProtocolError("args_invalid", f"'{label}' is invalid")
    return value


def _bounded_text(value: Any, label: str, maximum: int, *, nullable=False) -> str | None:
    if value is None and nullable:
        return None
    if not isinstance(value, str) or not value or len(value.encode("utf-8")) > maximum:
        raise ValueError(f"{label} is not bounded text")
    return value


def _bounded_optional_text(value: Any, maximum: int) -> str | None:
    if value is None:
        return None
    if not isinstance(value, str) or len(value.encode("utf-8")) > maximum:
        raise ValueError("optional text is invalid")
    return value


def _positive_int(value: Any, label: str, maximum: int) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or not 1 <= value <= maximum:
        raise ValueError(f"{label} is not a positive bounded integer")
    return value


def _nonnegative_int(value: Any, label: str, maximum: int) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or not 0 <= value <= maximum:
        raise ProtocolError("args_invalid", f"'{label}' is invalid")
    return value


def _relative_parts(value: Any) -> tuple[str, ...]:
    if (
        not isinstance(value, str)
        or not value
        or len(value.encode()) > 256
        or "\\" in value
        or "\0" in value
    ):
        raise ValueError("screenshot path is invalid")
    parsed = PurePosixPath(value)
    if (
        parsed.is_absolute()
        or any(part in ("", ".", "..") for part in parsed.parts)
        or parsed.as_posix() != value
    ):
        raise ValueError("screenshot path is not normalized")
    return parsed.parts


def _open_relative(evidence_fd: int, relative: str) -> int:
    parts = _relative_parts(relative)
    fd = os.dup(evidence_fd)
    try:
        for part in parts[:-1]:
            child = _open_dir(fd, part)
            os.close(fd)
            fd = child
        file_fd = os.open(parts[-1], os.O_RDONLY | os.O_NOFOLLOW, dir_fd=fd)
        os.close(fd)
        return file_fd
    except BaseException:
        os.close(fd)
        raise


def _manifest_leaves(run_fd: int) -> list[_Leaf]:
    try:
        checks_fd = _open_dir(run_fd, "checks")
    except FileNotFoundError:
        return []
    leaves: list[_Leaf] = []
    try:
        checks = sorted(name for name in os.listdir(checks_fd) if _CHECK_RE.fullmatch(name))
        for check in checks[:64]:
            try:
                check_fd = _open_dir(checks_fd, check)
            except FileNotFoundError:
                continue
            try:
                for phase in ("check", "discovery"):
                    try:
                        evidence_fd = _open_evidence_dir(run_fd, _Leaf(check, phase, None))
                    except FileNotFoundError:
                        continue
                    else:
                        os.close(evidence_fd)
                        leaves.append(_Leaf(check, phase, None))
                try:
                    cases_fd = _open_dir(check_fd, "cases")
                except FileNotFoundError:
                    continue
                try:
                    cases = sorted(
                        name for name in os.listdir(cases_fd) if _CASE_RE.fullmatch(name)
                    )
                    for case in cases[:MAX_MANIFESTS]:
                        leaf = _Leaf(check, "case", case)
                        try:
                            evidence_fd = _open_evidence_dir(run_fd, leaf)
                        except FileNotFoundError:
                            continue
                        else:
                            os.close(evidence_fd)
                            leaves.append(leaf)
                            if len(leaves) >= MAX_MANIFESTS:
                                return leaves
                finally:
                    os.close(cases_fd)
            finally:
                os.close(check_fd)
    finally:
        os.close(checks_fd)
    return leaves[:MAX_MANIFESTS]


def _screenshot(
    evidence_fd: int,
    leaf: _Leaf,
    manifest_sha: str,
    run_id: str,
    key: str,
    value: Any,
    cell: dict[str, Any],
) -> tuple[dict[str, Any] | None, _Image | None]:
    if value is None:
        return None, None
    if not isinstance(value, dict) or set(value) != {
        "kind",
        "path",
        "mime",
        "size",
        "sha256",
        "width",
        "height",
        "capturedAt",
    }:
        raise ValueError("screenshot descriptor has the wrong shape")
    expected_kind = "viewport" if key == "viewport" else "full-page"
    if (
        value.get("kind") != expected_kind
        or value.get("mime") != "image/png"
        or not _SHA_RE.fullmatch(str(value.get("sha256", "")))
    ):
        raise ValueError("screenshot descriptor identity is invalid")
    size = _positive_int(value.get("size"), "screenshot size", MAX_IMAGE_BYTES)
    width = _positive_int(value.get("width"), "screenshot width", 32_768)
    height = _positive_int(value.get("height"), "screenshot height", 262_144)
    captured_at = _bounded_optional_text(value.get("capturedAt"), 64)
    relative = value.get("path")
    _relative_parts(relative)
    try:
        image_fd = _open_relative(evidence_fd, relative)
        details = os.fstat(image_fd)
        header = os.read(image_fd, 24)
        os.close(image_fd)
        if (
            not stat.S_ISREG(details.st_mode)
            or details.st_size != size
            or _png_dimensions(header) != (width, height)
        ):
            raise ValueError("screenshot file does not match its descriptor")
    except (FileNotFoundError, OSError, ValueError):
        return {"status": "unavailable", "kind": expected_kind}, None
    digest_input = "\0".join(
        (
            run_id,
            leaf.check,
            leaf.phase,
            leaf.case or "",
            manifest_sha,
            relative,
            value["sha256"],
        )
    ).encode()
    image_id = hashlib.sha256(digest_input).hexdigest()
    public = {
        "status": "available",
        "image_id": image_id,
        "kind": expected_kind,
        "mime": "image/png",
        "size": size,
        "sha256": value["sha256"],
        "width": width,
        "height": height,
        "captured_at": captured_at,
    }
    return public, _Image(
        image_id=image_id,
        leaf=leaf,
        relative_path=relative,
        mime="image/png",
        size=size,
        sha256=value["sha256"],
        width=width,
        height=height,
        kind=expected_kind,
        cell=cell,
    )


def _sanitize_manifest(
    evidence_fd: int,
    payload: Any,
    raw: bytes,
    leaf: _Leaf,
    governed_run_id: str,
) -> tuple[dict[str, Any], dict[str, _Image]]:
    if not isinstance(payload, dict) or set(payload) != {
        "schemaVersion",
        "kind",
        "runId",
        "governedRunId",
        "governedCheck",
        "generatedAt",
        "browser",
        "coverage",
        "cells",
    }:
        raise ValueError("journey evidence manifest has the wrong shape")
    if payload.get("schemaVersion") != MANIFEST_SCHEMA or payload.get("kind") != MANIFEST_KIND:
        raise ValueError("journey evidence manifest has an unsupported schema")
    if (
        payload.get("governedRunId") != governed_run_id
        or payload.get("governedCheck") != leaf.check
    ):
        raise ValueError("journey evidence manifest does not match its governed run")
    formal_run_id = _bounded_text(payload.get("runId"), "formal run id", 128)
    generated_at = _bounded_text(payload.get("generatedAt"), "generated time", 64)
    browser = _bounded_text(payload.get("browser"), "browser", 256)
    coverage = payload.get("coverage")
    if not isinstance(coverage, dict) or set(coverage) != {
        "checkedPages",
        "plannedPages",
        "failed",
        "readinessEligible",
    }:
        raise ValueError("journey evidence coverage is invalid")
    checked = _nonnegative_int_for_manifest(coverage.get("checkedPages"), MAX_CELLS)
    planned = _nonnegative_int_for_manifest(coverage.get("plannedPages"), MAX_CELLS)
    if (
        checked > planned
        or not isinstance(coverage.get("failed"), bool)
        or not isinstance(coverage.get("readinessEligible"), bool)
    ):
        raise ValueError("journey evidence coverage values are invalid")
    cells = payload.get("cells")
    if not isinstance(cells, list) or len(cells) > MAX_CELLS:
        raise ValueError("journey evidence cells are invalid")
    manifest_sha = hashlib.sha256(raw).hexdigest()
    public_cells: list[dict[str, Any]] = []
    images: dict[str, _Image] = {}
    seen_cells: set[str] = set()
    for item in cells:
        public_cell = _sanitize_cell(item)
        cell_id = public_cell["cell_id"]
        if cell_id in seen_cells:
            raise ValueError("journey evidence repeats a cell")
        seen_cells.add(cell_id)
        source_screenshots = item.get("screenshots")
        if not isinstance(source_screenshots, dict) or set(source_screenshots) != {
            "viewport",
            "fullPage",
        }:
            raise ValueError("journey evidence screenshots are invalid")
        public_screenshots: dict[str, Any] = {}
        for source_key, public_key in (("viewport", "viewport"), ("fullPage", "full_page")):
            descriptor, image = _screenshot(
                evidence_fd,
                leaf,
                manifest_sha,
                governed_run_id,
                source_key,
                source_screenshots[source_key],
                public_cell,
            )
            public_screenshots[public_key] = descriptor
            if image is not None:
                images[image.image_id] = image
        public_cell["screenshots"] = public_screenshots
        public_cells.append(public_cell)
    return {
        "formal_run_id": formal_run_id,
        "generated_at": generated_at,
        "browser": browser,
        "check": leaf.check,
        "phase": leaf.phase,
        "case": leaf.case,
        "coverage": {
            "checked_pages": checked,
            "planned_pages": planned,
            "failed": coverage["failed"],
            "readiness_eligible": coverage["readinessEligible"],
        },
        "cells": public_cells,
    }, images


def _nonnegative_int_for_manifest(value: Any, maximum: int) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or not 0 <= value <= maximum:
        raise ValueError("manifest integer is invalid")
    return value


def _sanitize_cell(item: Any) -> dict[str, Any]:
    expected = {
        "cellId",
        "reviewCellKey",
        "planIndex",
        "targetName",
        "primaryJourney",
        "stateName",
        "requestedPath",
        "finalPath",
        "viewport",
        "startedAt",
        "endedAt",
        "durationMs",
        "outcome",
        "httpStatus",
        "sourceBindingStatus",
        "review",
        "actions",
        "findings",
        "screenshots",
    }
    if not isinstance(item, dict) or set(item) != expected:
        raise ValueError("journey evidence cell has the wrong shape")
    cell_id = _bounded_text(item.get("cellId"), "cell id", 256)
    if not _CELL_RE.fullmatch(cell_id):
        raise ValueError("journey evidence cell id is invalid")
    review_key = _bounded_optional_text(item.get("reviewCellKey"), 128)
    if review_key is not None and not _SHA_RE.fullmatch(review_key):
        raise ValueError("journey evidence review key is invalid")
    plan_index = item.get("planIndex")
    if plan_index is not None:
        plan_index = _nonnegative_int_for_manifest(plan_index, MAX_CELLS)
    target_name = _bounded_text(item.get("targetName"), "target name", 512)
    primary = _bounded_optional_text(item.get("primaryJourney"), 128)
    state_name = _bounded_text(item.get("stateName"), "state name", 128)
    requested_path = _bounded_optional_text(item.get("requestedPath"), 2048)
    final_path = _bounded_optional_text(item.get("finalPath"), 2048)
    viewport = item.get("viewport")
    if not isinstance(viewport, dict):
        raise ValueError("journey evidence viewport is invalid")
    public_viewport = {
        "name": _bounded_text(viewport.get("name"), "viewport name", 128),
        "width": _positive_int(viewport.get("width"), "viewport width", 32_768),
        "height": _positive_int(viewport.get("height"), "viewport height", 32_768),
    }
    if viewport.get("device") is not None:
        public_viewport["device"] = _bounded_text(viewport.get("device"), "device", 128)
    for key in viewport:
        if key not in {"name", "width", "height", "device", "sampling"}:
            raise ValueError("journey evidence viewport has unknown fields")
    duration = item.get("durationMs")
    if duration is not None:
        duration = _nonnegative_int_for_manifest(duration, 86_400_000)
    outcome = _bounded_text(item.get("outcome"), "outcome", 64)
    http_status = item.get("httpStatus")
    if http_status is not None and (
        not isinstance(http_status, int)
        or isinstance(http_status, bool)
        or not 100 <= http_status <= 599
    ):
        raise ValueError("journey evidence HTTP status is invalid")
    actions = item.get("actions")
    if not isinstance(actions, list) or len(actions) > 128:
        raise ValueError("journey evidence actions are invalid")
    public_actions = []
    for action in actions:
        if not isinstance(action, dict) or set(action) != {
            "index",
            "action",
            "outcome",
            "durationMs",
        }:
            raise ValueError("journey action has the wrong shape")
        public_actions.append(
            {
                "index": _nonnegative_int_for_manifest(action.get("index"), 1024),
                "action": _bounded_text(action.get("action"), "action", 32),
                "outcome": _bounded_text(action.get("outcome"), "action outcome", 64),
                "duration_ms": _nonnegative_int_for_manifest(
                    action.get("durationMs"), 86_400_000
                ),
            }
        )
    findings = item.get("findings")
    if not isinstance(findings, list) or len(findings) > 64:
        raise ValueError("journey findings are invalid")
    public_findings = []
    for finding in findings:
        if (
            not isinstance(finding, dict)
            or set(finding) != {"severity", "rule"}
            or finding.get("severity") not in {"info", "warning", "critical"}
        ):
            raise ValueError("journey finding has the wrong shape")
        public_findings.append(
            {
                "severity": finding["severity"],
                "rule": _bounded_text(finding.get("rule"), "finding rule", 128),
            }
        )
    review = item.get("review")
    public_review = None
    if review is not None:
        if not isinstance(review, dict) or set(review) != {"status", "decision"}:
            raise ValueError("journey review status is invalid")
        public_review = {
            "status": _bounded_text(review.get("status"), "review status", 64),
            "decision": _bounded_optional_text(review.get("decision"), 32),
        }
    return {
        "cell_id": cell_id,
        "review_cell_key": review_key,
        "plan_index": plan_index,
        "target_name": target_name,
        "primary_journey": primary,
        "state_name": state_name,
        "requested_path": requested_path,
        "final_path": final_path,
        "viewport": public_viewport,
        "started_at": _bounded_optional_text(item.get("startedAt"), 64),
        "ended_at": _bounded_optional_text(item.get("endedAt"), 64),
        "duration_ms": duration,
        "outcome": outcome,
        "http_status": http_status,
        "source_binding_status": _bounded_text(
            item.get("sourceBindingStatus"), "source binding status", 64
        ),
        "review": public_review,
        "actions": public_actions,
        "findings": public_findings,
    }


def _coord(value: Any, label: str) -> float:
    if not isinstance(value, int | float) or isinstance(value, bool):
        raise ProtocolError("args_invalid", f"'{label}' must be a normalized number")
    result = float(value)
    if not 0 <= result <= 1:
        raise ProtocolError("args_invalid", f"'{label}' must be between 0 and 1")
    return round(result, 6)


def _validate_marks(value: Any) -> list[dict[str, Any]]:
    if not isinstance(value, list) or not value or len(value) > MAX_MARKS:
        raise ProtocolError("args_invalid", f"'marks' must contain 1..{MAX_MARKS} annotations")
    normalized: list[dict[str, Any]] = []
    seen: set[str] = set()
    for index, mark in enumerate(value):
        if not isinstance(mark, dict):
            raise ProtocolError("args_invalid", f"marks[{index}] must be an object")
        mark_id = mark.get("id")
        kind = mark.get("type")
        color = str(mark.get("color", "")).lower()
        if not isinstance(mark_id, str) or not _MARK_RE.fullmatch(mark_id) or mark_id in seen:
            raise ProtocolError("args_invalid", f"marks[{index}].id is invalid")
        if kind not in {"pin", "rectangle", "arrow", "freehand", "highlight", "text"}:
            raise ProtocolError("args_invalid", f"marks[{index}].type is invalid")
        if color not in _COLORS:
            raise ProtocolError("args_invalid", f"marks[{index}].color is invalid")
        seen.add(mark_id)
        common = {"id": mark_id, "type": kind, "color": color}
        if kind in {"pin", "text"}:
            allowed = {"id", "type", "color", "x", "y"} | (
                {"text"} if kind == "text" else set()
            )
            if set(mark) != allowed:
                raise ProtocolError("args_invalid", f"marks[{index}] has unknown fields")
            common.update(x=_coord(mark.get("x"), "x"), y=_coord(mark.get("y"), "y"))
            if kind == "text":
                common["text"] = _comment_text(mark.get("text"), "text", maximum=120)
        elif kind == "rectangle":
            if set(mark) != {"id", "type", "color", "x", "y", "width", "height"}:
                raise ProtocolError("args_invalid", f"marks[{index}] has unknown fields")
            x, y = _coord(mark.get("x"), "x"), _coord(mark.get("y"), "y")
            width, height = (
                _coord(mark.get("width"), "width"),
                _coord(mark.get("height"), "height"),
            )
            if width <= 0 or height <= 0 or x + width > 1.000001 or y + height > 1.000001:
                raise ProtocolError(
                    "args_invalid", "rectangle must remain inside the screenshot"
                )
            common.update(x=x, y=y, width=width, height=height)
        elif kind == "arrow":
            if set(mark) != {"id", "type", "color", "x1", "y1", "x2", "y2"}:
                raise ProtocolError("args_invalid", f"marks[{index}] has unknown fields")
            common.update({key: _coord(mark.get(key), key) for key in ("x1", "y1", "x2", "y2")})
        else:
            if set(mark) != {"id", "type", "color", "points"}:
                raise ProtocolError("args_invalid", f"marks[{index}] has unknown fields")
            points = mark.get("points")
            if not isinstance(points, list) or not 2 <= len(points) <= MAX_POINTS:
                raise ProtocolError("args_invalid", f"marks[{index}].points is invalid")
            common["points"] = [
                {
                    "x": _coord(point.get("x") if isinstance(point, dict) else None, "x"),
                    "y": _coord(point.get("y") if isinstance(point, dict) else None, "y"),
                }
                for point in points
            ]
        normalized.append(common)
    encoded = json.dumps(normalized, separators=(",", ":"), sort_keys=True)
    if len(encoded.encode()) > 64 * 1024:
        raise ProtocolError("args_invalid", "annotation geometry is too large")
    return normalized


def _comment_text(value: Any, label: str = "body", *, maximum=2000) -> str:
    if not isinstance(value, str) or not 3 <= len(value.strip()) <= maximum:
        raise ProtocolError(
            "args_invalid", f"'{label}' must be plain text of 3..{maximum} characters"
        )
    return value.strip()


def _task_title(body: str) -> str:
    first = " ".join((body.splitlines() or [body])[0].split())
    if len(first) > 108:
        first = first[:107].rsplit(" ", 1)[0] + "…"
    title = f"Review: {first}"
    return title if len(title) <= 120 else title[:119] + "…"


def _task_outcome(body: str) -> str:
    return body if len(body) >= 10 else f"Change requested: {body}"


class TestEvidenceService:
    def __init__(self, db: Database, registry: Registry):
        self._db = db
        self._registry = registry

    def get(self, path: Path, run_id: Any, caller) -> dict[str, Any]:
        worktree, repository_id, worktree_id = self._resolve(path, caller)
        run = _safe_name(run_id, _RUN_RE, "run_id")
        bundles, images, issues = self._load(worktree, run)
        return {
            "repository_id": repository_id,
            "worktree_id": worktree_id,
            "run_id": run,
            "status": "available" if bundles else "unavailable",
            "bundles": bundles,
            "feedback": feedback_for_run(
                self._db, repository_id, worktree_id, run, _actor(caller)
            ),
            "issues": issues,
            "issues_truncated": len(issues) >= 64,
            "image_count": len(images),
        }

    def image(self, path: Path, args: dict[str, Any], caller) -> dict[str, Any]:
        worktree, _repository_id, _worktree_id = self._resolve(path, caller)
        run_id = _safe_name(args.get("run_id"), _RUN_RE, "run_id")
        image_id = _safe_name(args.get("image_id"), _SHA_RE, "image_id")
        offset = _nonnegative_int(args.get("offset", 0), "offset", MAX_IMAGE_BYTES)
        maximum = _nonnegative_int(
            args.get("max_bytes", MAX_IMAGE_CHUNK_BYTES),
            "max_bytes",
            MAX_IMAGE_CHUNK_BYTES,
        )
        if maximum < 1:
            raise ProtocolError("args_invalid", "'max_bytes' must be positive")
        _bundles, images, _issues = self._load(worktree, run_id)
        image = images.get(image_id)
        if image is None:
            raise ProtocolError(
                "test_evidence_not_found", "The selected screenshot is unavailable."
            )
        if offset > image.size:
            raise ProtocolError("args_invalid", "'offset' is beyond the screenshot")
        file_fd, before = self._verified_image_file(worktree, run_id, image)
        try:
            os.lseek(file_fd, offset, os.SEEK_SET)
            block = os.read(file_fd, min(maximum, image.size - offset))
            after = os.fstat(file_fd)
            if _identity(before) != _identity(after):
                raise ProtocolError(
                    "test_evidence_tampered", "The screenshot changed while it was read."
                )
        finally:
            os.close(file_fd)
        next_offset = offset + len(block)
        return {
            "image_id": image.image_id,
            "mime": image.mime,
            "sha256": image.sha256,
            "total_bytes": image.size,
            "offset": offset,
            "bytes": len(block),
            "base64": base64.b64encode(block).decode("ascii"),
            "next_offset": next_offset if next_offset < image.size else None,
        }

    def _verified_image_file(
        self, worktree: Path, run_id: str, image: _Image
    ) -> tuple[int, os.stat_result]:
        run_fd = self._run_fd(worktree, run_id)
        try:
            evidence_fd = _open_evidence_dir(run_fd, image.leaf)
            try:
                file_fd = _open_relative(evidence_fd, image.relative_path)
            finally:
                os.close(evidence_fd)
        finally:
            os.close(run_fd)
        try:
            before = os.fstat(file_fd)
            if not stat.S_ISREG(before.st_mode) or before.st_size != image.size:
                raise ProtocolError(
                    "test_evidence_tampered", "The screenshot no longer matches its evidence."
                )
            os.lseek(file_fd, 0, os.SEEK_SET)
            header = os.read(file_fd, 24)
            if _png_dimensions(header) != (image.width, image.height):
                raise ProtocolError(
                    "test_evidence_tampered", "The screenshot no longer matches its evidence."
                )
            digest = hashlib.sha256(header)
            while True:
                digest_block = os.read(file_fd, 1024 * 1024)
                if not digest_block:
                    break
                digest.update(digest_block)
            if digest.hexdigest() != image.sha256:
                raise ProtocolError(
                    "test_evidence_tampered", "The screenshot no longer matches its evidence."
                )
            after = os.fstat(file_fd)
            if _identity(before) != _identity(after):
                raise ProtocolError(
                    "test_evidence_tampered", "The screenshot changed while it was read."
                )
            os.lseek(file_fd, 0, os.SEEK_SET)
            return file_fd, before
        except BaseException:
            os.close(file_fd)
            raise

    def create_feedback(self, path: Path, args: dict[str, Any], caller) -> dict[str, Any]:
        worktree, repository_id, worktree_id = self._resolve(path, caller)
        run_id = _safe_name(args.get("run_id"), _RUN_RE, "run_id")
        image_id = _safe_name(args.get("image_id"), _SHA_RE, "image_id")
        body = _comment_text(args.get("body"))
        marks = _validate_marks(args.get("marks"))
        _bundles, images, _issues = self._load(worktree, run_id)
        image = images.get(image_id)
        if image is None:
            raise ProtocolError(
                "test_evidence_not_found", "The selected screenshot is unavailable."
            )
        verified_fd, _details = self._verified_image_file(worktree, run_id, image)
        os.close(verified_fd)
        actor = _actor(caller)
        now = now_iso()
        task_id = ids.task_id()
        feedback_id = ids.feedback_id()
        comment_id = ids.comment_id()
        cell = image.cell
        title = _task_title(body)
        outcome = _task_outcome(body)
        journey = cell.get("primary_journey") or "this journey"
        impact = (
            f"The tested {cell['state_name']} screen in {journey} "
            "does not yet match the owner's expectation."
        )
        verification = (
            "Address the marked visual feedback, rerun the same UI journey, and publish "
            "a new retained screenshot that the owner can inspect."
        )
        technical_note = (
            f"Visual feedback {feedback_id}; governed run {run_id}; check {image.leaf.check}; "
            f"formal cell {cell['cell_id']}; image {image.image_id}; "
            f"screenshot {image.sha256}. "
            "Refs DC2-2026-09-02-VISUAL-JOURNEY-EVIDENCE and "
            "DC2-2026-09-02-SCREENSHOT-FEEDBACK."
        )
        with self._db.transaction() as conn:
            seq = plan_state.next_seq(conn, "tasks", repository_id)
            conn.execute(
                "INSERT INTO tasks(task_id,repository_id,parent_task_id,release_id,seq,"
                " position,title,outcome,impact,unblock_condition,verification,technical_note,"
                " kind,status,estimated_loc,created_at,created_by,updated_at)"
                " VALUES(?,?,NULL,NULL,?,0,?,?,?,?,?,?,'user_feedback','planned',NULL,?,?,?)",
                (
                    task_id,
                    repository_id,
                    seq,
                    title,
                    outcome,
                    impact,
                    verification,
                    verification,
                    technical_note,
                    now,
                    actor,
                    now,
                ),
            )
            position = plan_state.place_task(conn, repository_id, None, None, task_id, None)
            plan_state.append_event(
                conn,
                repository_id,
                "task",
                task_id,
                "created",
                None,
                "user_feedback",
                actor,
                "Created from a marked visual test screenshot.",
            )
            conn.execute(
                "INSERT INTO visual_feedback(feedback_id,task_id,repository_id,worktree_id,"
                " run_id,check_name,phase,case_id,formal_run_id,cell_id,review_cell_key,"
                " screenshot_kind,screenshot_sha256,image_id,geometry_json,root_comment_id,"
                " created_at,created_by,updated_at)"
                " VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                (
                    feedback_id,
                    task_id,
                    repository_id,
                    worktree_id,
                    run_id,
                    image.leaf.check,
                    image.leaf.phase,
                    image.leaf.case,
                    cell.get("formal_run_id"),
                    cell["cell_id"],
                    cell.get("review_cell_key"),
                    image.kind,
                    image.sha256,
                    image.image_id,
                    json.dumps(marks, separators=(",", ":"), sort_keys=True),
                    comment_id,
                    now,
                    actor,
                    now,
                ),
            )
            conn.execute(
                "INSERT INTO visual_feedback_comments("
                " comment_id,feedback_id,seq,body,created_at,"
                " created_by,updated_at) VALUES(?,?,1,?,?,?,?)",
                (comment_id, feedback_id, body, now, actor, now),
            )
            _append_feedback_event(
                conn, feedback_id, "created", comment_id, None, body, actor, now
            )
        return {
            "task_id": task_id,
            "feedback_id": feedback_id,
            "position": position,
            "feedback": feedback_for_id(self._db, feedback_id, actor),
        }

    def reply(self, path: Path, args: dict[str, Any], caller) -> dict[str, Any]:
        repository_id, worktree_id, run_id, feedback = self._feedback_target(path, args, caller)
        del repository_id, worktree_id, run_id
        if feedback["deleted_at"] is not None:
            raise ProtocolError("args_invalid", "The visual feedback was deleted.")
        body = _comment_text(args.get("body"))
        actor, now, comment_id = _actor(caller), now_iso(), ids.comment_id()
        with self._db.transaction() as conn:
            seq = conn.execute(
                "SELECT COALESCE(MAX(seq),0)+1 FROM visual_feedback_comments"
                " WHERE feedback_id=?",
                (feedback["feedback_id"],),
            ).fetchone()[0]
            conn.execute(
                "INSERT INTO visual_feedback_comments("
                " comment_id,feedback_id,seq,body,created_at,"
                " created_by,updated_at) VALUES(?,?,?,?,?,?,?)",
                (comment_id, feedback["feedback_id"], seq, body, now, actor, now),
            )
            conn.execute(
                "UPDATE visual_feedback SET updated_at=? WHERE feedback_id=?",
                (now, feedback["feedback_id"]),
            )
            _append_feedback_event(
                conn, feedback["feedback_id"], "replied", comment_id, None, body, actor, now
            )
            plan_state.append_event(
                conn,
                feedback["repository_id"],
                "task",
                feedback["task_id"],
                "visual_feedback_reply",
                None,
                comment_id,
                actor,
                body[:500],
            )
        return {"feedback": feedback_for_id(self._db, feedback["feedback_id"], actor)}

    def edit(self, path: Path, args: dict[str, Any], caller) -> dict[str, Any]:
        _repository_id, _worktree_id, _run_id, feedback = self._feedback_target(
            path, args, caller
        )
        comment_id = _safe_name(args.get("comment_id"), _MARK_RE, "comment_id")
        body = _comment_text(args.get("body"))
        rows = self._db.query(
            "SELECT * FROM visual_feedback_comments WHERE comment_id=? AND feedback_id=?",
            (comment_id, feedback["feedback_id"]),
        )
        if not rows:
            raise ProtocolError("args_invalid", "The comment does not belong to this feedback.")
        comment = dict(rows[0])
        actor = _actor(caller)
        if comment["created_by"] != actor or comment["deleted_at"] is not None:
            raise ProtocolError("permission_denied", "Only the comment author can edit it.")
        now = now_iso()
        with self._db.transaction() as conn:
            conn.execute(
                "UPDATE visual_feedback_comments SET body=?,updated_at=? WHERE comment_id=?",
                (body, now, comment_id),
            )
            conn.execute(
                "UPDATE visual_feedback SET updated_at=? WHERE feedback_id=?",
                (now, feedback["feedback_id"]),
            )
            _append_feedback_event(
                conn,
                feedback["feedback_id"],
                "comment_edited",
                comment_id,
                comment["body"],
                body,
                actor,
                now,
            )
            if comment_id == feedback["root_comment_id"]:
                title, outcome = _task_title(body), _task_outcome(body)
                plan_state.set_fields(
                    conn, "tasks", "task_id", feedback["task_id"], title=title, outcome=outcome
                )
                plan_state.append_event(
                    conn,
                    feedback["repository_id"],
                    "task",
                    feedback["task_id"],
                    "edited",
                    None,
                    "outcome,title",
                    actor,
                    "Updated from the linked screenshot discussion.",
                )
        return {"feedback": feedback_for_id(self._db, feedback["feedback_id"], actor)}

    def set_state(self, path: Path, args: dict[str, Any], caller) -> dict[str, Any]:
        _repository_id, _worktree_id, _run_id, feedback = self._feedback_target(
            path, args, caller
        )
        state = args.get("state")
        if state not in {"open", "resolved"}:
            raise ProtocolError("args_invalid", "'state' must be open or resolved")
        if feedback["deleted_at"] is not None:
            raise ProtocolError("args_invalid", "The visual feedback was deleted.")
        task = plan_state.get_task(self._db, feedback["task_id"])
        target = "done" if state == "resolved" else "planned"
        actor, now = _actor(caller), now_iso()
        if task["status"] != target:
            with self._db.transaction() as conn:
                plan_state.set_fields(conn, "tasks", "task_id", task["task_id"], status=target)
                plan_state.append_event(
                    conn,
                    feedback["repository_id"],
                    "task",
                    task["task_id"],
                    "status",
                    task["status"],
                    target,
                    actor,
                    "Updated from the linked screenshot discussion.",
                )
                conn.execute(
                    "UPDATE visual_feedback SET updated_at=? WHERE feedback_id=?",
                    (now, feedback["feedback_id"]),
                )
                _append_feedback_event(
                    conn,
                    feedback["feedback_id"],
                    "resolved" if state == "resolved" else "reopened",
                    None,
                    None,
                    state,
                    actor,
                    now,
                )
        return {"feedback": feedback_for_id(self._db, feedback["feedback_id"], actor)}

    def delete(self, path: Path, args: dict[str, Any], caller) -> dict[str, Any]:
        _repository_id, _worktree_id, _run_id, feedback = self._feedback_target(
            path, args, caller
        )
        actor = _actor(caller)
        if feedback["created_by"] != actor:
            raise ProtocolError(
                "permission_denied", "Only the annotation author can delete it."
            )
        if feedback["deleted_at"] is None:
            task = plan_state.get_task(self._db, feedback["task_id"])
            now = now_iso()
            with self._db.transaction() as conn:
                conn.execute(
                    "UPDATE visual_feedback SET deleted_at=?,deleted_by=?,updated_at=?"
                    " WHERE feedback_id=?",
                    (now, actor, now, feedback["feedback_id"]),
                )
                if task["status"] != "dropped":
                    plan_state.set_fields(
                        conn, "tasks", "task_id", task["task_id"], status="dropped"
                    )
                    plan_state.append_event(
                        conn,
                        feedback["repository_id"],
                        "task",
                        task["task_id"],
                        "status",
                        task["status"],
                        "dropped",
                        actor,
                        "The linked screenshot annotation was explicitly deleted.",
                    )
                _append_feedback_event(
                    conn, feedback["feedback_id"], "deleted", None, None, "deleted", actor, now
                )
        return {"feedback": feedback_for_id(self._db, feedback["feedback_id"], actor)}

    def _feedback_target(self, path: Path, args: dict[str, Any], caller):
        _worktree, repository_id, worktree_id = self._resolve(path, caller)
        run_id = _safe_name(args.get("run_id"), _RUN_RE, "run_id")
        feedback_id = _safe_name(args.get("feedback_id"), _MARK_RE, "feedback_id")
        rows = self._db.query(
            "SELECT * FROM visual_feedback WHERE feedback_id=? AND repository_id=?"
            " AND worktree_id=? AND run_id=?",
            (feedback_id, repository_id, worktree_id, run_id),
        )
        if not rows:
            raise ProtocolError(
                "args_invalid", "The feedback does not belong to this test run."
            )
        return repository_id, worktree_id, run_id, dict(rows[0])

    def _resolve(self, path: Path, caller) -> tuple[Path, str, str]:
        if caller.identity is not None:
            requested = os.path.normpath(str(path))
            rows = self._db.query(
                "SELECT worktree_path,worktree_id,repository_id FROM worktrees"
                " WHERE worktree_path=?",
                (requested,),
            )
            if not rows:
                raise ProtocolError(
                    "repository_not_found", "No registered worktree matches this request."
                )
            return (
                Path(rows[0]["worktree_path"]),
                rows[0]["repository_id"],
                rows[0]["worktree_id"],
            )
        try:
            info = resolve_worktree(path, run_as=(caller.uid, caller.gid))
        except GitResolveError as exc:
            raise ProtocolError("repository_not_found", str(exc)) from exc
        repository_id = ids.repository_id(info.repository_root)
        worktree_id = ids.worktree_id(info.worktree_root)
        if not self._db.query(
            "SELECT 1 FROM worktrees WHERE worktree_id=? AND repository_id=?",
            (worktree_id, repository_id),
        ):
            raise ProtocolError("repository_not_found", "The worktree is not registered.")
        return info.worktree_root, repository_id, worktree_id

    def _run_fd(self, worktree: Path, run_id: str) -> int:
        try:
            return _open_run_dir(worktree, run_id)
        except FileNotFoundError as exc:
            raise ProtocolError(
                "test_evidence_expired",
                "The selected visual evidence is unavailable or has expired.",
            ) from exc

    def _load(self, worktree: Path, run_id: str):
        run_fd = self._run_fd(worktree, run_id)
        bundles: list[dict[str, Any]] = []
        images: dict[str, _Image] = {}
        issues: list[dict[str, str]] = []
        try:
            leaves = _manifest_leaves(run_fd)
            for leaf in leaves:
                try:
                    evidence_fd = _open_evidence_dir(run_fd, leaf)
                    try:
                        raw, _details = _read_regular(
                            evidence_fd, MANIFEST_NAME, MAX_MANIFEST_BYTES
                        )
                        payload = json.loads(raw)
                        bundle, bundle_images = _sanitize_manifest(
                            evidence_fd, payload, raw, leaf, run_id
                        )
                    finally:
                        os.close(evidence_fd)
                except (
                    FileNotFoundError,
                    OSError,
                    UnicodeDecodeError,
                    json.JSONDecodeError,
                    ValueError,
                ):
                    if len(issues) < 64:
                        issues.append(
                            {
                                "check": leaf.check,
                                "phase": leaf.phase,
                                "case": leaf.case or "",
                                "code": "invalid_evidence",
                            }
                        )
                    continue
                for cell in bundle["cells"]:
                    cell["formal_run_id"] = bundle["formal_run_id"]
                bundles.append(bundle)
                images.update(bundle_images)
        finally:
            os.close(run_fd)
        bundles.sort(key=lambda item: (item["check"], item["phase"], item["case"] or ""))
        return bundles, images, issues


def _append_feedback_event(
    conn,
    feedback_id: str,
    event: str,
    comment_id: str | None,
    from_value: str | None,
    to_value: str | None,
    actor: str,
    at: str,
) -> None:
    conn.execute(
        "INSERT INTO visual_feedback_events(feedback_id,event,comment_id,from_value,"
        " to_value,actor,at) VALUES(?,?,?,?,?,?,?)",
        (feedback_id, event, comment_id, from_value, to_value, actor, at),
    )


def _public_feedback(db: Database, row: dict[str, Any], actor: str) -> dict[str, Any]:
    comments = []
    rows = db.query(
        "SELECT * FROM visual_feedback_comments WHERE feedback_id=? ORDER BY seq LIMIT ?",
        (row["feedback_id"], MAX_COMMENTS),
    )
    for comment in rows:
        deleted = comment["deleted_at"] is not None
        comments.append(
            {
                "comment_id": comment["comment_id"],
                "body": "Comment deleted" if deleted else comment["body"],
                "author": _display_actor(comment["created_by"]),
                "created_at": comment["created_at"],
                "updated_at": comment["updated_at"],
                "deleted": deleted,
                "can_edit": not deleted and comment["created_by"] == actor,
            }
        )
    deleted = row["deleted_at"] is not None
    task_status = row["task_status"]
    return {
        "feedback_id": row["feedback_id"],
        "task_id": row["task_id"],
        "task_status": task_status,
        "state": "deleted" if deleted else ("resolved" if task_status == "done" else "open"),
        "run_id": row["run_id"],
        "check": row["check_name"],
        "phase": row["phase"],
        "case": row["case_id"],
        "formal_run_id": row["formal_run_id"],
        "cell_id": row["cell_id"],
        "review_cell_key": row["review_cell_key"],
        "image_id": row["image_id"],
        "screenshot_kind": row["screenshot_kind"],
        "screenshot_sha256": row["screenshot_sha256"],
        "marks": json.loads(row["geometry_json"]),
        "author": _display_actor(row["created_by"]),
        "created_at": row["created_at"],
        "updated_at": row["updated_at"],
        "can_delete": not deleted and row["created_by"] == actor,
        "comments": comments,
        "comments_truncated": len(rows) >= MAX_COMMENTS,
    }


def feedback_for_run(
    db: Database, repository_id: str, worktree_id: str, run_id: str, actor: str
) -> list[dict[str, Any]]:
    rows = db.query(
        "SELECT vf.*,t.status AS task_status FROM visual_feedback vf"
        " JOIN tasks t ON t.task_id=vf.task_id"
        " WHERE vf.repository_id=? AND vf.worktree_id=? AND vf.run_id=?"
        " ORDER BY vf.created_at,vf.feedback_id LIMIT 512",
        (repository_id, worktree_id, run_id),
    )
    return [_public_feedback(db, dict(row), actor) for row in rows]


def feedback_for_id(db: Database, feedback_id: str, actor: str) -> dict[str, Any]:
    rows = db.query(
        "SELECT vf.*,t.status AS task_status FROM visual_feedback vf"
        " JOIN tasks t ON t.task_id=vf.task_id WHERE vf.feedback_id=?",
        (feedback_id,),
    )
    if not rows:
        raise ProtocolError("args_invalid", "Visual feedback was not found.")
    return _public_feedback(db, dict(rows[0]), actor)


def feedback_for_task(db: Database, task_id: str, actor: str) -> dict[str, Any] | None:
    rows = db.query(
        "SELECT vf.*,t.status AS task_status FROM visual_feedback vf"
        " JOIN tasks t ON t.task_id=vf.task_id WHERE vf.task_id=?",
        (task_id,),
    )
    return _public_feedback(db, dict(rows[0]), actor) if rows else None
