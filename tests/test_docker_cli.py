import json
import subprocess

import pytest

from devcoordinator2.daemon import docker_cli


def _result(returncode=0, stdout="", stderr=""):
    return subprocess.CompletedProcess([], returncode, stdout, stderr)


def test_mutable_image_keeps_preloaded_path(monkeypatch):
    calls = []
    monkeypatch.setattr(docker_cli, "_run", lambda *args, **kwargs: calls.append(args))
    docker_cli.ensure_digest_image("postgres:16-alpine")
    assert calls == []


def test_existing_digest_is_inspected_without_pull(monkeypatch):
    image = "postgis/postgis@sha256:" + "a" * 64
    calls = []

    def run(argv, **_kwargs):
        calls.append(argv)
        return _result(stdout=json.dumps([image]))

    monkeypatch.setattr(docker_cli, "_run", run)
    docker_cli.ensure_digest_image(image)
    assert [call[0] for call in calls] == ["image"]


def test_missing_digest_is_pulled_then_verified(monkeypatch):
    image = "postgis/postgis@sha256:" + "b" * 64
    calls = []
    inspections = iter([_result(returncode=1),
                        _result(stdout=json.dumps([image]))])

    def run(argv, **_kwargs):
        calls.append(argv)
        if argv[0] == "image":
            return next(inspections)
        return _result()

    monkeypatch.setattr(docker_cli, "_run", run)
    docker_cli.ensure_digest_image(image)
    assert [call[0] for call in calls] == ["image", "pull", "image"]
    assert calls[1] == ["pull", "--quiet", image]


def test_pull_must_resolve_to_requested_digest(monkeypatch):
    image = "postgis/postgis@sha256:" + "c" * 64
    inspections = iter([_result(returncode=1),
                        _result(stdout=json.dumps([
                            "postgis/postgis@sha256:" + "d" * 64
                        ]))])

    def run(argv, **_kwargs):
        return next(inspections) if argv[0] == "image" else _result()

    monkeypatch.setattr(docker_cli, "_run", run)
    with pytest.raises(docker_cli.DockerError, match="requested sha256"):
        docker_cli.ensure_digest_image(image)
