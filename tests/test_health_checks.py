from devcoordinator2.daemon import deploy_engine, health_checks


def test_http_readiness_can_abort_on_terminal_binding():
    assert health_checks.http_ready(
        1, "/healthz", 30, lambda: "unit became terminal") == (
            False, "unit became terminal")


def test_tcp_readiness_can_abort_on_terminal_binding():
    assert health_checks.tcp_ready(
        "127.0.0.1", 1, 30, lambda: "container became terminal") == (
            False, "container became terminal")


def test_terminal_unit_probe_allows_restart_in_progress(monkeypatch):
    times = iter([0.0, 2.0])
    monkeypatch.setattr(deploy_engine.time, "monotonic", lambda: next(times))
    monkeypatch.setattr(
        deploy_engine.rt, "process_state",
        lambda _unit: {"state": "starting", "active_state": "activating",
                       "sub_state": "auto-restart", "result": "exit-code"})
    assert deploy_engine._terminal_binding_check("unit", "u")() is None


def test_terminal_unit_probe_reports_exhausted_failure(monkeypatch):
    times = iter([0.0, 2.0])
    monkeypatch.setattr(deploy_engine.time, "monotonic", lambda: next(times))
    monkeypatch.setattr(
        deploy_engine.rt, "process_state",
        lambda _unit: {"state": "failed", "active_state": "failed",
                       "sub_state": "failed", "result": "start-limit-hit"})
    reason = deploy_engine._terminal_binding_check("unit", "u")()
    assert reason == "unit became terminal: failed/failed (start-limit-hit)"
