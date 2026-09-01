from __future__ import annotations

import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "check_no_test_timer_waits", ROOT / "scripts/check_no_test_timer_waits.py")
assert SPEC and SPEC.loader
detector = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(detector)


def test_detector_catches_slow_python_sleep_and_allows_bounded_fallback(tmp_path):
    slow = tmp_path / "slow.py"
    slow.write_text("import time\ntime.sleep(0.5)\n")
    assert "not completion evidence" in detector.scan_python(
        slow, "src/check_runner.py")[0]
    fast = tmp_path / "fast.py"
    fast.write_text("import time\ntime.sleep(0.05)\n")
    assert detector.scan_python(
        fast, "src/devcoordinator2/daemon/tests_lifecycle.py") == []
    assert detector.scan_python(fast, "tests/integration/test_health.py") == []


def test_detector_catches_browser_timer_waits_without_matching_prose(tmp_path):
    source = tmp_path / "verify.mjs"
    source.write_text("await page.waitForTimeout(500); setTimeout(done, 20);\n")
    assert len(detector.scan_javascript(source, "console/verify.mjs")) == 2
    source.write_text("// timers are discussed here without invoking them\n")
    assert detector.scan_javascript(source, "console/verify.mjs") == []
