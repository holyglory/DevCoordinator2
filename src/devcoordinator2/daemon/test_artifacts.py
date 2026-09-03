"""Hash-bound retained artifact trees for governed checks.

The executor snapshots declared directory trees into one private check leaf.
This service validates the immutable manifest, verifies every retained file,
and discloses only bounded exact chunks to trusted local callers or Console
administrators.  No source or storage path is returned.
"""

from __future__ import annotations

import base64
import hashlib
import json
import os
import re
import stat
import threading
from pathlib import Path, PurePosixPath
from typing import Any

from devcoordinator2 import ids
from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.gitinfo import GitResolveError, resolve_worktree
from devcoordinator2.daemon.registry import Registry
from devcoordinator2.daemon.test_evidence import (
    _identity,
    _Leaf,
    _open_dir,
    _open_evidence_dir,
    _open_relative,
    _open_run_dir,
    _read_regular,
)
from devcoordinator2.protocol import ProtocolError

MANIFEST_NAME = "retained-artifacts.json"
MANIFEST_KIND = "devcoordinator2-retained-artifact-trees"
MANIFEST_SCHEMA = 1
MAX_MANIFEST_BYTES = 4 * 1024 * 1024
MAX_ARTIFACTS = 8
MAX_FILES = 4096
MAX_ARTIFACT_BYTES = 1024 * 1024 * 1024
MAX_TOTAL_BYTES = 2 * 1024 * 1024 * 1024
MAX_CHUNK_BYTES = 180 * 1024
MAX_PAGE = 100

_RUN_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
_CHECK_RE = re.compile(r"[a-z0-9][a-z0-9-]{0,63}$")
_TEST_RE = re.compile(r"[a-z0-9][a-z0-9-]{0,31}$")
_SHA_RE = re.compile(r"[0-9a-f]{64}$")


def _safe_name(value: Any, pattern: re.Pattern[str], label: str) -> str:
    if not isinstance(value, str) or not pattern.fullmatch(value):
        raise ProtocolError("args_invalid", f"'{label}' is invalid")
    return value


def _plain_int(value: Any, label: str, minimum: int, maximum: int) -> int:
    if not isinstance(value, int) or isinstance(value, bool) \
            or not minimum <= value <= maximum:
        raise ProtocolError(
            "args_invalid", f"'{label}' must be an integer in {minimum}..{maximum}")
    return value


def _relative_file(value: Any) -> str:
    if not isinstance(value, str) or not value or len(value.encode("utf-8")) > 512 \
            or "\\" in value or any(ord(character) < 32 or ord(character) == 127
                                     for character in value):
        raise ValueError("retained artifact file path is invalid")
    parsed = PurePosixPath(value)
    if parsed.is_absolute() or parsed.as_posix() != value \
            or any(part in ("", ".", "..") for part in parsed.parts):
        raise ValueError("retained artifact file path is invalid")
    return value


def _tree_digest(entries: list[dict[str, Any]]) -> str:
    digest = hashlib.sha256(b"devcoordinator2-retained-artifact-tree-v1\0")
    for entry in entries:
        digest.update(entry["path"].encode("utf-8"))
        digest.update(b"\0")
        digest.update(str(entry["size"]).encode("ascii"))
        digest.update(b"\0")
        digest.update(entry["sha256"].encode("ascii"))
        digest.update(b"\0")
    return digest.hexdigest()


def _validate_manifest(raw: bytes, value: Any, run_id: str, check: str) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != {
            "schema", "kind", "run_id", "test", "check", "requested_tier",
            "readiness_eligible", "proof", "source_sha256", "config_sha256",
            "artifacts"}:
        raise ValueError("retained artifact manifest fields differ")
    if value["schema"] != MANIFEST_SCHEMA or value["kind"] != MANIFEST_KIND \
            or value["run_id"] != run_id or value["check"] != check:
        raise ValueError("retained artifact manifest identity differs")
    if not isinstance(value["test"], str) or not _TEST_RE.fullmatch(value["test"]) \
            or value["requested_tier"] not in ("development", "pre-merge", "release") \
            or not isinstance(value["readiness_eligible"], bool) \
            or value["proof"] not in ("complete", "selected", "retry") \
            or not isinstance(value["source_sha256"], str) \
            or not _SHA_RE.fullmatch(value["source_sha256"]) \
            or not isinstance(value["config_sha256"], str) \
            or not _SHA_RE.fullmatch(value["config_sha256"]):
        raise ValueError("retained artifact run binding is invalid")
    artifacts = value["artifacts"]
    if not isinstance(artifacts, list) or not 1 <= len(artifacts) <= MAX_ARTIFACTS:
        raise ValueError("retained artifact manifest count is invalid")
    names: set[str] = set()
    sanitized = []
    artifact_total = 0
    for artifact in artifacts:
        if not isinstance(artifact, dict) or set(artifact) != {
                "name", "size", "files", "sha256", "entries"}:
            raise ValueError("retained artifact descriptor fields differ")
        name = artifact["name"]
        if not isinstance(name, str) or not _CHECK_RE.fullmatch(name) or name in names:
            raise ValueError("retained artifact name is invalid or repeated")
        size = artifact["size"]
        files = artifact["files"]
        sha256 = artifact["sha256"]
        entries = artifact["entries"]
        if not isinstance(size, int) or isinstance(size, bool) \
                or not 0 <= size <= MAX_ARTIFACT_BYTES \
                or not isinstance(files, int) or isinstance(files, bool) \
                or not 1 <= files <= MAX_FILES \
                or not isinstance(sha256, str) or not _SHA_RE.fullmatch(sha256) \
                or not isinstance(entries, list) or len(entries) != files:
            raise ValueError("retained artifact summary is invalid")
        clean_entries = []
        seen_paths: set[str] = set()
        total = 0
        for entry in entries:
            if not isinstance(entry, dict) or set(entry) != {"path", "size", "sha256"}:
                raise ValueError("retained artifact file descriptor fields differ")
            path = _relative_file(entry["path"])
            file_size = entry["size"]
            file_sha = entry["sha256"]
            if path in seen_paths \
                    or not isinstance(file_size, int) or isinstance(file_size, bool) \
                    or not 0 <= file_size <= MAX_ARTIFACT_BYTES \
                    or not isinstance(file_sha, str) or not _SHA_RE.fullmatch(file_sha):
                raise ValueError("retained artifact file descriptor is invalid")
            total += file_size
            if total > MAX_ARTIFACT_BYTES:
                raise ValueError("retained artifact size exceeds its bound")
            seen_paths.add(path)
            clean_entries.append({"path": path, "size": file_size, "sha256": file_sha})
        if clean_entries != sorted(clean_entries, key=lambda item: item["path"]) \
                or total != size or _tree_digest(clean_entries) != sha256:
            raise ValueError("retained artifact tree digest differs")
        artifact_total += size
        if artifact_total > MAX_TOTAL_BYTES:
            raise ValueError("retained artifacts exceed their combined bound")
        names.add(name)
        sanitized.append({
            "name": name,
            "size": size,
            "files": files,
            "sha256": sha256,
            "entries": clean_entries,
        })
    return {
        "manifest_sha256": hashlib.sha256(raw).hexdigest(),
        "test": value["test"],
        "requested_tier": value["requested_tier"],
        "readiness_eligible": value["readiness_eligible"],
        "proof": value["proof"],
        "source_sha256": value["source_sha256"],
        "config_sha256": value["config_sha256"],
        "artifacts": sanitized,
    }


def _validate_run(raw: bytes, value: Any, run_id: str, test: str) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != {
            "schema", "run_id", "test", "started_at_epoch_ms",
            "finished_at_epoch_ms", "status", "complete"}:
        raise ValueError("retained run metadata fields differ")
    if value["schema"] != 2 or value["run_id"] != run_id or value["test"] != test \
            or value["status"] not in ("running", "passed", "failed") \
            or not isinstance(value["complete"], bool) \
            or not isinstance(value["started_at_epoch_ms"], int) \
            or isinstance(value["started_at_epoch_ms"], bool) \
            or value["started_at_epoch_ms"] < 0:
        raise ValueError("retained run metadata identity is invalid")
    finished = value["finished_at_epoch_ms"]
    if finished is not None and (
            not isinstance(finished, int) or isinstance(finished, bool)
            or finished < value["started_at_epoch_ms"]):
        raise ValueError("retained run completion time is invalid")
    if value["complete"] != (finished is not None) \
            or (value["status"] == "running") == value["complete"]:
        raise ValueError("retained run completion state is inconsistent")
    return {
        "run_status": value["status"],
        "run_complete": value["complete"],
        "run_finished_at_epoch_ms": finished,
        "run_metadata_sha256": hashlib.sha256(raw).hexdigest(),
    }


class TestArtifactService:
    def __init__(self, db: Database, registry: Registry):
        self._db = db
        self._registry = registry
        self._verified: set[tuple[Any, ...]] = set()
        self._verified_lock = threading.Lock()

    def catalog(self, path: Path, args: dict[str, Any], caller) -> dict[str, Any]:
        worktree, repository_id, worktree_id = self._resolve(path, caller)
        run_id = _safe_name(args.get("run_id"), _RUN_RE, "run_id")
        check = _safe_name(args.get("check"), _CHECK_RE, "check")
        artifact_name = args.get("artifact")
        if artifact_name is not None:
            artifact_name = _safe_name(artifact_name, _CHECK_RE, "artifact")
        offset = _plain_int(args.get("offset", 0), "offset", 0, MAX_FILES)
        limit = _plain_int(args.get("limit", MAX_PAGE), "limit", 1, MAX_PAGE)
        expected_manifest = args.get("manifest_sha256")
        if expected_manifest is not None:
            expected_manifest = _safe_name(
                expected_manifest, _SHA_RE, "manifest_sha256")
        manifest = self._manifest(worktree, run_id, check)
        if expected_manifest is not None \
                and manifest["manifest_sha256"] != expected_manifest:
            raise ProtocolError(
                "test_artifact_tampered", "The retained artifact manifest changed.")
        artifacts = manifest["artifacts"]
        selected = None
        if artifact_name is not None:
            selected = next(
                (artifact for artifact in artifacts if artifact["name"] == artifact_name), None)
            if selected is None:
                raise ProtocolError(
                    "test_artifact_not_found", "The selected retained artifact is unavailable.")
            self._verify_tree(worktree, run_id, check, selected)
        entries = selected["entries"] if selected is not None else []
        if offset > len(entries):
            raise ProtocolError("args_invalid", "'offset' exceeds the artifact file count")
        page = entries[offset:offset + limit]
        next_offset = offset + len(page)
        summaries = [
            {key: artifact[key] for key in ("name", "size", "files", "sha256")}
            for artifact in artifacts
        ]
        return {
            "repository_id": repository_id,
            "worktree_id": worktree_id,
            "run_id": run_id,
            "check": check,
            "manifest_sha256": manifest["manifest_sha256"],
            "test": manifest["test"],
            "requested_tier": manifest["requested_tier"],
            "readiness_eligible": manifest["readiness_eligible"],
            "proof": manifest["proof"],
            "source_sha256": manifest["source_sha256"],
            "config_sha256": manifest["config_sha256"],
            "run_status": manifest["run_status"],
            "run_complete": manifest["run_complete"],
            "run_finished_at_epoch_ms": manifest["run_finished_at_epoch_ms"],
            "run_metadata_sha256": manifest["run_metadata_sha256"],
            "artifacts": summaries,
            "artifact": (
                {key: selected[key] for key in ("name", "size", "files", "sha256")}
                if selected is not None else None),
            "entries": page,
            "next_offset": next_offset if next_offset < len(entries) else None,
        }

    def file(self, path: Path, args: dict[str, Any], caller) -> dict[str, Any]:
        worktree, _repository_id, _worktree_id = self._resolve(path, caller)
        run_id = _safe_name(args.get("run_id"), _RUN_RE, "run_id")
        check = _safe_name(args.get("check"), _CHECK_RE, "check")
        artifact_name = _safe_name(args.get("artifact"), _CHECK_RE, "artifact")
        relative = args.get("file")
        try:
            relative = _relative_file(relative)
        except ValueError as exc:
            raise ProtocolError("args_invalid", "'file' is invalid") from exc
        offset = _plain_int(args.get("offset", 0), "offset", 0, MAX_ARTIFACT_BYTES)
        maximum = _plain_int(
            args.get("max_bytes", MAX_CHUNK_BYTES),
            "max_bytes", 1, MAX_CHUNK_BYTES)
        manifest_sha = _safe_name(
            args.get("manifest_sha256"), _SHA_RE, "manifest_sha256")
        manifest = self._manifest(worktree, run_id, check)
        if manifest["manifest_sha256"] != manifest_sha:
            raise ProtocolError(
                "test_artifact_tampered", "The retained artifact manifest changed.")
        artifact = next(
            (item for item in manifest["artifacts"] if item["name"] == artifact_name), None)
        if artifact is None:
            raise ProtocolError(
                "test_artifact_not_found", "The selected retained artifact is unavailable.")
        entry = next((item for item in artifact["entries"] if item["path"] == relative), None)
        if entry is None:
            raise ProtocolError(
                "test_artifact_not_found",
                "The selected retained artifact file is unavailable.")
        if offset > entry["size"]:
            raise ProtocolError("args_invalid", "'offset' is beyond the retained file")
        file_fd, before = self._verified_file(
            worktree, run_id, check, artifact_name, entry)
        try:
            os.lseek(file_fd, offset, os.SEEK_SET)
            block = os.read(file_fd, min(maximum, entry["size"] - offset))
            after = os.fstat(file_fd)
            if _identity(before) != _identity(after):
                raise ProtocolError(
                    "test_artifact_tampered", "The retained artifact changed while read.")
        finally:
            os.close(file_fd)
        next_offset = offset + len(block)
        return {
            "run_id": run_id,
            "check": check,
            "artifact": artifact_name,
            "file": relative,
            "sha256": entry["sha256"],
            "total_bytes": entry["size"],
            "offset": offset,
            "bytes": len(block),
            "base64": base64.b64encode(block).decode("ascii"),
            "next_offset": next_offset if next_offset < entry["size"] else None,
        }

    def _manifest(self, worktree: Path, run_id: str, check: str) -> dict[str, Any]:
        try:
            run_fd = _open_run_dir(worktree, run_id)
        except FileNotFoundError as exc:
            raise ProtocolError(
                "test_artifact_expired",
                "The selected retained artifact is unavailable or has expired.") from exc
        try:
            run_raw, _run_details = _read_regular(run_fd, "run.json", 64 * 1024)
            try:
                evidence_fd = _open_evidence_dir(run_fd, _Leaf(check, "check", None))
            except FileNotFoundError as exc:
                raise ProtocolError(
                    "test_artifact_not_found",
                    "The selected check has no retained artifacts.") from exc
            try:
                raw, _details = _read_regular(
                    evidence_fd, MANIFEST_NAME, MAX_MANIFEST_BYTES)
            finally:
                os.close(evidence_fd)
        except ProtocolError:
            raise
        except (FileNotFoundError, OSError, ValueError) as exc:
            raise ProtocolError(
                "test_artifact_not_found",
                "The selected check has no retained artifacts.") from exc
        finally:
            os.close(run_fd)
        try:
            manifest = _validate_manifest(raw, json.loads(raw), run_id, check)
            manifest.update(_validate_run(
                run_raw, json.loads(run_raw), run_id, manifest["test"]))
            return manifest
        except (UnicodeDecodeError, json.JSONDecodeError, ValueError) as exc:
            raise ProtocolError(
                "test_artifact_tampered", "The retained artifact manifest is invalid.") from exc

    def _verify_tree(
            self, worktree: Path, run_id: str, check: str,
            artifact: dict[str, Any]) -> None:
        for entry in artifact["entries"]:
            fd, _details = self._verified_file(
                worktree, run_id, check, artifact["name"], entry)
            os.close(fd)

    def _verified_file(
            self, worktree: Path, run_id: str, check: str, artifact: str,
            entry: dict[str, Any]) -> tuple[int, os.stat_result]:
        run_fd = None
        evidence_fd = None
        retained_fd = None
        artifact_fd = None
        try:
            run_fd = _open_run_dir(worktree, run_id)
            evidence_fd = _open_evidence_dir(run_fd, _Leaf(check, "check", None))
            retained_fd = _open_dir(evidence_fd, "retained")
            artifact_fd = _open_dir(retained_fd, artifact)
            file_fd = _open_relative(artifact_fd, entry["path"])
        except (FileNotFoundError, OSError, ValueError) as exc:
            raise ProtocolError(
                "test_artifact_tampered", "A retained artifact file is unavailable.") from exc
        finally:
            for fd in (artifact_fd, retained_fd, evidence_fd, run_fd):
                if fd is not None:
                    os.close(fd)
        try:
            before = os.fstat(file_fd)
            if not stat.S_ISREG(before.st_mode) or before.st_size != entry["size"]:
                raise ProtocolError(
                    "test_artifact_tampered", "A retained artifact file no longer matches.")
            key = (
                str(worktree), run_id, check, artifact, entry["path"],
                entry["sha256"], _identity(before),
            )
            with self._verified_lock:
                cached = key in self._verified
            if not cached:
                digest = hashlib.sha256()
                while True:
                    block = os.read(file_fd, 1024 * 1024)
                    if not block:
                        break
                    digest.update(block)
                after = os.fstat(file_fd)
                if _identity(before) != _identity(after) \
                        or digest.hexdigest() != entry["sha256"]:
                    raise ProtocolError(
                        "test_artifact_tampered", "A retained artifact file no longer matches.")
                with self._verified_lock:
                    if len(self._verified) >= MAX_FILES * MAX_ARTIFACTS:
                        self._verified.clear()
                    self._verified.add(key)
            os.lseek(file_fd, 0, os.SEEK_SET)
            return file_fd, before
        except BaseException:
            os.close(file_fd)
            raise

    def _resolve(self, path: Path, caller) -> tuple[Path, str, str]:
        if caller.identity is not None:
            requested = os.path.normpath(str(path))
            rows = self._db.query(
                "SELECT worktree_path,worktree_id,repository_id FROM worktrees"
                " WHERE worktree_path=?", (requested,))
            if not rows:
                raise ProtocolError(
                    "repository_not_found", "No registered worktree matches this request.")
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
                (worktree_id, repository_id)):
            raise ProtocolError("repository_not_found", "The worktree is not registered.")
        return info.worktree_root, repository_id, worktree_id
