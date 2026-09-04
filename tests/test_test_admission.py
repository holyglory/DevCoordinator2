from __future__ import annotations

import json
import threading

import pytest

from devcoordinator2.daemon.test_admission import (
    AdmissionError,
    begin_drain,
    end_drain,
    read_activity,
    wait_for_zero_activity,
)
from devcoordinator2.daemon.test_admission import TestAdmission as AdmissionController
from devcoordinator2.daemon.test_admission import TestsDraining as DrainingError


def test_drain_closes_admission_and_activity_is_exact(tmp_path):
    admission = AdmissionController(tmp_path)
    admission.reset()
    with admission.start_guard() as guard:
        guard.started("t1", "unit-1")
    assert read_activity(tmp_path)["active"] == [
        {"run_id": "t1", "unit": "unit-1"}]
    lease = begin_drain(tmp_path, "upgrade")
    with pytest.raises(DrainingError, match="upgrade"):
        with admission.start_guard():
            pass
    admission.finished("t1")
    assert read_activity(tmp_path)["active"] == []
    end_drain(lease)
    with admission.start_guard():
        pass


def test_wait_for_zero_activity_wakes_from_atomic_receipt_event(tmp_path):
    admission = AdmissionController(tmp_path)
    admission.reset()
    admission.started("t1", "unit-1")
    finished = threading.Event()

    def wait():
        wait_for_zero_activity(tmp_path)
        finished.set()

    thread = threading.Thread(target=wait)
    thread.start()
    admission.finished("t1")
    thread.join(10)
    assert finished.is_set()


def test_stale_drain_lease_is_recovered_on_next_start(tmp_path):
    admission = AdmissionController(tmp_path)
    admission.reset()
    (tmp_path / "test-drain.json").write_text(json.dumps({
        "schema": 1, "pid": 999_999_999, "process_start": "1",
        "nonce": "stale", "reason": "old upgrade",
    }))
    with admission.start_guard():
        pass
    assert not (tmp_path / "test-drain.json").exists()


def test_second_live_drain_is_refused(tmp_path):
    AdmissionController(tmp_path).reset()
    lease = begin_drain(tmp_path, "first")
    try:
        with pytest.raises(AdmissionError, match="another live"):
            begin_drain(tmp_path, "second")
    finally:
        end_drain(lease)
