#!/usr/bin/env python3
"""Self-tests for the formal web UI verifier."""

from __future__ import annotations

import json
import hashlib
import os
import shutil
import ssl
import struct
import subprocess
import sys
import tempfile
import threading
import traceback
import urllib.request
import zlib
from http.server import BaseHTTPRequestHandler, SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from socketserver import TCPServer


ROOT = Path(__file__).resolve().parents[1]
VERIFY = ROOT / "scripts" / "formal_web_ui_verify.mjs"
REVIEW = ROOT / "scripts" / "formal_web_ui_review.py"
TIMEOUT_SECONDS = int(os.environ.get("FORMAL_WEB_UI_SELF_TEST_TIMEOUT", "45"))
KEEP_TEMP = os.environ.get("FORMAL_WEB_UI_SELF_TEST_KEEP_TEMP", "").lower() in {"1", "true", "yes", "on"}


def write(path: Path, body: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body, encoding="utf-8")


def page(body: str, css: str = "") -> str:
    return f"""<!doctype html>
<html>
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>formal fixture</title>
  <style>
    body {{ margin: 0; font: 16px system-ui, sans-serif; background: #fff; color: #111; }}
    main {{ padding: 20px; }}
    {css}
  </style>
</head>
<body>
  <main data-ui-continuation-anchor>
    {body}
  </main>
</body>
</html>
"""


class QuietHandler(SimpleHTTPRequestHandler):
    def log_message(self, format: str, *args: object) -> None:
        return


class FastBindThreadingHTTPServer(ThreadingHTTPServer):
    # macOS CI runners black-hole reverse DNS, so HTTPServer.server_bind's
    # socket.getfqdn() call stalls fixtures ~30s between bind() and listen().
    # The FQDN is unused here — bind like a plain TCPServer.
    def server_bind(self) -> None:
        TCPServer.server_bind(self)
        host, port = self.server_address[:2]
        self.server_name = str(host)
        self.server_port = int(port)


class Server:
    def __init__(self, root: Path) -> None:
        self.root = root
        self.previous = Path.cwd()
        os.chdir(root)
        self.httpd = FastBindThreadingHTTPServer(("127.0.0.1", 0), QuietHandler)
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)
        self.thread.start()

    @property
    def base_url(self) -> str:
        port = self.httpd.server_address[1]
        return f"http://127.0.0.1:{port}"

    def close(self) -> None:
        self.httpd.shutdown()
        self.thread.join(timeout=5)
        os.chdir(self.previous)


class _FixedPageHandler(BaseHTTPRequestHandler):
    """Serves a body chosen by pick_body(); used for cookie/TLS fixtures."""

    def log_message(self, format: str, *args: object) -> None:
        return

    def pick_body(self) -> str:
        raise NotImplementedError

    def do_GET(self) -> None:
        data = self.pick_body().encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


class CookieGateHandler(_FixedPageHandler):
    """Without sess=ok the page has invisible text (a must-catch critical)."""

    def pick_body(self) -> str:
        if "sess=ok" in (self.headers.get("Cookie") or ""):
            return page("<h1>Dashboard</h1><p>Session accepted.</p><button>Save changes</button>")
        return page("<p class='bad'>Invisible message</p>", ".bad { color: #fff; background: #fff; }")


class CleanPageHandler(_FixedPageHandler):
    def pick_body(self) -> str:
        return page("<h1>Secure</h1><p>Served over self-signed TLS.</p><button>Save changes</button>")


class RedirectToSignInHandler(BaseHTTPRequestHandler):
    def log_message(self, format: str, *args: object) -> None:
        return

    def do_GET(self) -> None:
        if self.path == "/dashboard":
            self.send_response(302)
            self.send_header("Location", "/sign-in")
            self.end_headers()
            return
        data = page("<h1>Sign in</h1><form><input placeholder='Email address'></form>").encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


class SourceBindingHandler(BaseHTTPRequestHandler):
    revision = "deployed-revision"
    request_count = 0
    lock = threading.Lock()

    def log_message(self, format: str, *args: object) -> None:
        return

    def do_GET(self) -> None:
        with self.lock:
            type(self).request_count += 1
        data = page("<h1>Bound deployment</h1><p>Current page content.</p>").encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("X-UI-Source-Revision", self.revision)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


class AuthReuseHandler(BaseHTTPRequestHandler):
    auth_calls = 0
    observations: list[str] = []
    lock = threading.Lock()

    def log_message(self, format: str, *args: object) -> None:
        return

    def send_html(self, body: str, status: int = 200, *, cookie: str | None = None) -> None:
        data = body.encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("X-UI-Source-Revision", "auth-fixture-revision")
        if cookie:
            self.send_header("Set-Cookie", cookie)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self) -> None:
        route, _, query = self.path.partition("?")
        if route == "/login":
            self.send_html(page(
                "<h1>Sign in once</h1><input id='password' type='password'><button id='login'>Sign in</button><div id='ready' hidden>Ready</div>"
                "<script>"
                "localStorage.setItem('auth-seed','ready');"
                "document.querySelector('#login').onclick=()=>fetch('/authenticate').then(()=>{document.querySelector('#ready').hidden=false});"
                "</script>"
            ))
            return
        if route == "/authenticate":
            with self.lock:
                type(self).auth_calls += 1
            self.send_html("ok", cookie="auth=ok; Path=/; SameSite=Lax")
            return
        if route == "/observe":
            value = query.removeprefix("value=") or "missing"
            with self.lock:
                type(self).observations.append(value)
            self.send_html("observed")
            return
        if route == "/protected":
            if "auth=ok" not in (self.headers.get("Cookie") or ""):
                self.send_html(page("<h1>Unauthorized</h1>"), status=401)
                return
            self.send_html(page(
                "<h1>Protected dashboard</h1><div id='protected'>Authenticated</div><div id='ready' hidden>Ready</div>"
                "<script>"
                "const before=localStorage.getItem('cellMutation')||'clean';"
                "fetch('/observe?value='+encodeURIComponent(before)).then(()=>{document.querySelector('#ready').hidden=false});"
                "localStorage.setItem('cellMutation','dirty');"
                "</script>"
            ))
            return
        self.send_html(page("<h1>Not found</h1>"), status=404)


class ConcurrencyHandler(BaseHTTPRequestHandler):
    condition = threading.Condition()
    arrivals = 0
    active = 0
    max_active = 0

    def log_message(self, format: str, *args: object) -> None:
        return

    def do_GET(self) -> None:
        with self.condition:
            type(self).arrivals += 1
            type(self).active += 1
            type(self).max_active = max(type(self).max_active, type(self).active)
            self.condition.notify_all()
            self.condition.wait_for(lambda: type(self).arrivals >= 3, timeout=2)
        data = page("<h1>Parallel cell</h1><p>Independent evidence.</p>").encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
        with self.condition:
            type(self).active -= 1
            self.condition.notify_all()


class RenderPerformanceHandler(BaseHTTPRequestHandler):
    def log_message(self, format: str, *args: object) -> None:
        return

    def do_GET(self) -> None:
        route = self.path.partition("?")[0]
        if route == "/lcp.png":
            width = 256
            height = 256
            raw = b"".join(
                b"\x00" + bytes(
                    component
                    for x in range(width)
                    for component in (x, y, (x + y) % 256, 255)
                )
                for y in range(height)
            )

            def chunk(kind: bytes, payload: bytes) -> bytes:
                return struct.pack(">I", len(payload)) + kind + payload + struct.pack(">I", zlib.crc32(kind + payload))

            data = (
                b"\x89PNG\r\n\x1a\n"
                + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
                + chunk(b"IDAT", zlib.compress(raw))
                + chunk(b"IEND", b"")
            )
            self.send_response(200)
            self.send_header("Content-Type", "image/png")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return
        if route == "/slow-ttfb":
            threading.Event().wait(0.03)
            body = page("<h1>Slow response</h1><p>The document paints quickly after its delayed first byte.</p>")
        elif route == "/slow-lcp":
            body = page(
                "<p>Initial content</p><img id='late-lcp' width='1000' height='420' alt='Late largest image'>"
                "<script>setTimeout(()=>{"
                "const image=document.querySelector('#late-lcp');"
                "image.addEventListener('load',()=>requestAnimationFrame(()=>requestAnimationFrame(()=>{"
                "image.dataset.lcpReady='true';"
                "})),{once:true});"
                "image.src='/lcp.png';"
                "},1200)</script>",
            )
        elif route == "/no-lcp":
            body = page("<input aria-label='Name' style='width:320px;height:52px'>")
        else:
            body = page("<h1>Fast performance</h1><p>Immediate local content.</p>")
        data = body.encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


class DynamicServer:
    def __init__(self, handler: type[BaseHTTPRequestHandler]) -> None:
        self.httpd = FastBindThreadingHTTPServer(("127.0.0.1", 0), handler)
        self.scheme = "http"
        self.thread: threading.Thread | None = None

    def start(self) -> "DynamicServer":
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)
        self.thread.start()
        return self

    @property
    def base_url(self) -> str:
        return f"{self.scheme}://127.0.0.1:{self.httpd.server_address[1]}"

    def close(self) -> None:
        self.httpd.shutdown()
        if self.thread:
            self.thread.join(timeout=5)


def make_tls_server(tmp: Path) -> DynamicServer:
    cert = tmp / "self-test-tls.crt"
    key = tmp / "self-test-tls.key"
    subprocess.run(
        [
            "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
            "-keyout", str(key), "-out", str(cert), "-days", "2",
            "-subj", "/CN=127.0.0.1", "-addext", "subjectAltName=IP:127.0.0.1",
        ],
        check=True,
        capture_output=True,
        timeout=TIMEOUT_SECONDS,
    )
    server = DynamicServer(CleanPageHandler)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(str(cert), str(key))
    server.httpd.socket = context.wrap_socket(server.httpd.socket, server_side=True)
    server.scheme = "https"
    return server.start()


def node_binary() -> str:
    return os.environ.get("FORMAL_WEB_UI_NODE") or shutil.which("node") or "node"


def playwright_module_dir() -> Path:
    """Resolve the verifier dependency independently of the audited cwd."""

    candidates: list[Path] = []
    explicit = os.environ.get("FORMAL_WEB_UI_PLAYWRIGHT_NODE_MODULES")
    if explicit:
        candidates.append(Path(explicit).expanduser())
    # Installed skills are direct links to <repo>/skills/<skill>. Resolve the
    # repository's locked dependency from the canonical script path.
    candidates.append(ROOT.parents[1] / "ci" / "playwright" / "node_modules")
    for item in os.environ.get("NODE_PATH", "").split(os.pathsep):
        if item.strip():
            candidates.append(Path(item.strip()).expanduser())
    candidates.append(
        Path.home()
        / ".cache"
        / "codex-runtimes"
        / "codex-primary-runtime"
        / "dependencies"
        / "node"
        / "node_modules"
    )
    checked: list[str] = []
    for candidate in candidates:
        resolved = candidate.resolve(strict=False)
        if str(resolved) in checked:
            continue
        checked.append(str(resolved))
        if (resolved / "playwright" / "package.json").is_file():
            return resolved
    raise AssertionError(
        "Playwright is unavailable for the formal UI self-test. Install the "
        f"locked dependency with `npm ci --ignore-scripts --prefix {ROOT.parents[1] / 'ci' / 'playwright'}` "
        "or set FORMAL_WEB_UI_PLAYWRIGHT_NODE_MODULES to a node_modules directory containing Playwright. "
        f"Checked: {checked}"
    )


def verifier_command(*args: str) -> list[str]:
    return [
        node_binary(),
        str(VERIFY),
        "--playwright-module-dir",
        str(playwright_module_dir()),
        *args,
    ]


def verifier_env() -> dict[str, str]:
    env = os.environ.copy()
    # The command carries an explicit module directory. Leaving NODE_PATH in
    # the child would hide regressions back to cwd/environment-only discovery.
    env.pop("NODE_PATH", None)
    # A governed self-test is a wrapper that launches many independent verifier
    # fixtures; nested verifier runs must not all claim the wrapper leaf's one
    # production evidence bundle. The dedicated governed-evidence case below
    # adds exact identities back for the single run that proves integration.
    for name in (
        "DEVCOORDINATOR_EVIDENCE_DIR",
        "DEVCOORDINATOR_RUN_ID",
        "DEVCOORDINATOR_CHECK_NAME",
    ):
        env.pop(name, None)
    return env


def receipt_artifact_paths(receipt: dict) -> tuple[Path, Path]:
    artifacts = receipt.get("artifacts")
    if not isinstance(artifacts, dict):
        raise AssertionError(f"Receipt must name report artifacts: {receipt}")
    if isinstance(artifacts.get("directory"), str):
        directory = Path(artifacts["directory"])
        return directory / artifacts.get("json", ""), directory / artifacts.get("markdown", "")
    if isinstance(artifacts.get("json"), str) and isinstance(artifacts.get("markdown"), str):
        return Path(artifacts["json"]), Path(artifacts["markdown"])
    raise AssertionError(f"Receipt artifact paths are incomplete: {receipt}")


def parse_bounded_receipt(stdout: str, *, expect: int) -> dict:
    value = stdout.strip()
    if not value or "\n" in value or len(value.encode("utf-8")) > 2048:
        raise AssertionError("Default stdout must be exactly one bounded receipt line")
    if "# Formal Web UI Verification" in value or "## Findings" in value:
        raise AssertionError("Default stdout leaked the Markdown report body")
    try:
        receipt = json.loads(value)
    except json.JSONDecodeError as exc:
        raise AssertionError(f"Default stdout must be JSON, got: {value[:400]}") from exc
    if receipt.get("exitCode") != expect:
        raise AssertionError(f"Receipt exitCode mismatch: {receipt}")
    receipt_artifact_paths(receipt)
    return receipt


def assert_complete_artifacts(json_out: Path, markdown_out: Path, *, expect: int) -> dict:
    if not json_out.is_file() or not markdown_out.is_file():
        raise AssertionError(f"Verifier did not write both report artifacts: {json_out}, {markdown_out}")
    report = json.loads(json_out.read_text(encoding="utf-8"))
    markdown = markdown_out.read_text(encoding="utf-8")
    expected_heading = (
        "# Formal Web UI Verification Setup Failure"
        if expect == 2
        else "# Formal Web UI Verification Report"
    )
    if expected_heading not in markdown:
        raise AssertionError(f"Markdown artifact is not the complete expected report: {markdown_out}")
    if report.get("runId") is None or (expect == 2 and report.get("status") != "setup-failure"):
        raise AssertionError(f"JSON artifact is not the complete expected report: {json_out}")
    if expect == 2:
        if (
            not report.get("error", {}).get("message")
            or not report.get("startedAt")
            or not report.get("endedAt")
            or len(report.get("evidence", {}).get("verifier", {}).get("sha256", "")) != 64
            or "## Diagnostic" not in markdown
        ):
            raise AssertionError(f"Setup-failure artifacts omitted diagnostic evidence: {json_out}")
    elif (
        report.get("schemaVersion") != 2
        or not isinstance(report.get("pages"), list)
        or not isinstance(report.get("findings"), list)
        or not isinstance(report.get("coverage"), dict)
        or not report.get("startedAt")
        or not report.get("endedAt")
        or len(report.get("evidence", {}).get("verifier", {}).get("sha256", "")) != 64
        or len(report.get("evidence", {}).get("config", {}).get("sha256", "")) != 64
        or report.get("plan", {}).get("widthCoverage") != "sampled-only"
        or not isinstance(report.get("review"), dict)
        or len(report.get("review", {}).get("queueSha256", "")) != 64
        or len(report.get("coverage", {}).get("cells", [])) != len(report.get("pages", []))
        or any(
            "requestedPath" not in page_report
            or "finalPath" not in page_report
            or not isinstance(page_report.get("sourceBinding"), dict)
            or not page_report.get("startedAt")
            or not page_report.get("endedAt")
            for page_report in report.get("pages", [])
        )
        or "## Target Coverage" not in markdown
        or "## Changed Visual Review" not in markdown
        or "## Findings" not in markdown
    ):
        raise AssertionError(f"Verification artifacts omitted full report evidence: {json_out}")
    if expect != 2:
        progress_path = Path(report.get("execution", {}).get("progressPath", ""))
        if not progress_path.is_file():
            raise AssertionError(f"Bounded progress artifact is missing: {progress_path}")
        progress_rows = [json.loads(line) for line in progress_path.read_text(encoding="utf-8").splitlines()]
        if (
            not progress_rows
            or progress_rows[0].get("kind") != "run-start"
            or progress_rows[-1].get("kind") != "run-complete"
        ):
            raise AssertionError(f"Progress artifact does not bind the complete run: {progress_path}")
        queue_path = Path(report["review"]["queuePath"])
        if not queue_path.is_file() or hashlib.sha256(queue_path.read_bytes()).hexdigest() != report["review"]["queueSha256"]:
            raise AssertionError(f"Review queue is missing or not hash-bound: {queue_path}")
        queue = json.loads(queue_path.read_text(encoding="utf-8"))
        if queue.get("kind") != "formal-web-ui-review-queue" or queue.get("runId") != report.get("runId"):
            raise AssertionError(f"Review queue does not belong to the report: {queue_path}")
        journey = report.get("evidence", {}).get("journey", {})
        journey_path = Path(str(journey.get("path", "")))
        if (
            not journey_path.is_file()
            or hashlib.sha256(journey_path.read_bytes()).hexdigest() != journey.get("sha256")
        ):
            raise AssertionError(f"Journey evidence is missing or not hash-bound: {journey_path}")
        journey_payload = json.loads(journey_path.read_text(encoding="utf-8"))
        if (
            journey_payload.get("kind") != "formal-web-ui-journey-evidence"
            or journey_payload.get("runId") != report.get("runId")
            or len(journey_payload.get("cells", [])) != len(report.get("pages", []))
            or journey.get("cellCount") != len(report.get("pages", []))
        ):
            raise AssertionError(f"Journey evidence does not bind the complete run: {journey_path}")
        journey_root = journey_path.parent.resolve()
        for cell in journey_payload.get("cells", []):
            for screenshot in (cell.get("screenshots") or {}).values():
                if screenshot is None:
                    continue
                screenshot_path = (journey_root / screenshot.get("path", "")).resolve()
                if (
                    not screenshot_path.is_relative_to(journey_root)
                    or not screenshot_path.is_file()
                    or hashlib.sha256(screenshot_path.read_bytes()).hexdigest()
                    != screenshot.get("sha256")
                ):
                    raise AssertionError(f"Journey screenshot is not confined and hash-bound: {screenshot}")
        for page_report in report.get("pages", []):
            if page_report.get("outcome") != "checked":
                continue
            screenshots = page_report.get("screenshots", {})
            for key in ("viewport", "fullPage"):
                evidence = screenshots.get(key)
                if not isinstance(evidence, dict):
                    raise AssertionError(f"Checked page omitted {key} screenshot evidence")
                screenshot_path = Path(evidence.get("path", ""))
                if (
                    not screenshot_path.is_file()
                    or evidence.get("mime") != "image/png"
                    or len(evidence.get("sha256", "")) != 64
                    or hashlib.sha256(screenshot_path.read_bytes()).hexdigest() != evidence.get("sha256")
                ):
                    raise AssertionError(f"Invalid {key} screenshot evidence: {evidence}")
    return report


def run_verifier_command(
    cmd: list[str],
    json_out: Path,
    markdown_out: Path,
    *,
    expect: int | tuple[int, ...],
    env: dict[str, str] | None = None,
) -> dict:
    result = subprocess.run(
        cmd,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=TIMEOUT_SECONDS,
        env=env or verifier_env(),
    )
    allowed_exits = (expect,) if isinstance(expect, int) else expect
    if result.returncode not in allowed_exits:
        print(result.stdout)
        print(result.stderr, file=sys.stderr)
        raise AssertionError(
            f"Expected exit in {sorted(allowed_exits)}, got {result.returncode}: {' '.join(cmd)}"
        )
    if result.stderr.strip():
        raise AssertionError(f"Default invocation emitted non-receipt stderr: {result.stderr[:400]}")
    actual_exit = result.returncode
    receipt = parse_bounded_receipt(result.stdout, expect=actual_exit)
    receipt_json, receipt_markdown = receipt_artifact_paths(receipt)
    if receipt_json.resolve() != json_out.resolve() or receipt_markdown.resolve() != markdown_out.resolve():
        raise AssertionError(f"Receipt must point to the exact report artifacts: {receipt}")
    report = assert_complete_artifacts(json_out, markdown_out, expect=actual_exit)
    if len(allowed_exits) > 1:
        expected_exit = 1 if any(
            finding.get("severity") == "critical"
            for finding in report.get("findings", [])
        ) else 0
        if actual_exit != expected_exit:
            raise AssertionError(
                f"Verifier exit {actual_exit} disagrees with its structured critical findings"
            )
    return report


def default_target_contract() -> dict:
    return {
        "journeys": [
            {
                "id": "fixture-primary",
                "name": "Exercise the fixture",
                "frequencyPercent": 100,
                "risk": "normal",
                "rationale": "Self-test target",
            }
        ],
        "primaryJourney": "fixture-primary",
        "regions": [
            {
                "selector": "main",
                "role": "primary-content",
                "journey": "fixture-primary",
                "name": "Fixture content",
            }
        ],
        "theme": "light",
        "reviewInputs": [
            {"path": "SKILL.md", "kind": "ui-code"}
        ],
    }


def with_target_contract(config: dict) -> dict:
    prepared = json.loads(json.dumps(config))
    prepared.setdefault("repoRoot", str(ROOT.resolve()))
    prepared.setdefault(
        "performance",
        {"ttfbMs": 10000, "lcpMs": 10000, "ttfbLocalOnly": False},
    )
    defaults = default_target_contract()
    target_defaults = prepared.setdefault("targetDefaults", {})
    if isinstance(target_defaults, dict):
        for key, value in defaults.items():
            target_defaults.setdefault(key, value)
    targets = prepared.get("targets")
    if isinstance(targets, list):
        normalized_targets: list[dict | str] = []
        for target in targets:
            if not isinstance(target, dict):
                normalized_targets.append(target)
                continue
            for key, value in defaults.items():
                target.setdefault(key, value)
            for state in target.get("states", []):
                if not isinstance(state, dict) or "continuation" in state:
                    continue
                actions = state.get("actions", [])
                if any(
                    isinstance(action, dict)
                    and action.get("action") in {"click", "press", "check", "uncheck", "selectOption"}
                    for action in actions
                ):
                    state["continuation"] = {
                        "kind": "in-page",
                        "anchor": "main",
                        "focusWithin": "main",
                    }
            normalized_targets.append(target)
        prepared["targets"] = normalized_targets
    return prepared


def run_verify(url: str, out: Path, *, expect: int, extra: list[str] | None = None) -> dict:
    return run_verify_config(
        {
            "targets": [{"url": url}],
            "viewports": [{"name": "mobile", "width": 390, "height": 844}],
        },
        out,
        expect=expect,
        extra=extra,
    )


def run_verify_config(
    config: dict,
    out: Path,
    *,
    expect: int | tuple[int, ...],
    extra: list[str] | None = None,
    apply_contract_defaults: bool = True,
) -> dict:
    out.mkdir(parents=True, exist_ok=True)
    json_out = out / "report.json"
    md_out = out / "report.md"
    config_path = out / "formal-web-ui.json"
    effective = with_target_contract(config) if apply_contract_defaults else config
    config_path.write_text(json.dumps(effective, ensure_ascii=False, indent=2), encoding="utf-8")
    cmd = verifier_command(
        "--config",
        str(config_path),
        "--json-out",
        str(json_out),
        "--markdown-out",
        str(md_out),
        "--fail-on",
        "critical",
    )
    if extra:
        cmd.extend(extra)
    return run_verifier_command(cmd, json_out, md_out, expect=expect)


def run_review(args: list[str], *, expect: int) -> dict:
    result = subprocess.run(
        [sys.executable, str(REVIEW), *args],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=TIMEOUT_SECONDS,
    )
    if result.returncode != expect or result.stderr.strip():
        raise AssertionError(
            f"Expected review exit {expect}, got {result.returncode}: {result.stdout} {result.stderr}"
        )
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError as exc:
        raise AssertionError(f"Review helper must emit one JSON receipt: {result.stdout}") from exc


def finding_rules(report: dict) -> set[str]:
    return {item["rule"] for item in report.get("findings", [])}


def assert_rules(report: dict, *rules: str) -> None:
    present = finding_rules(report)
    missing = set(rules) - present
    if missing:
        raise AssertionError(f"Missing expected rules {sorted(missing)}; present={sorted(present)}")


def assert_no_critical(report: dict) -> None:
    critical = [item for item in report.get("findings", []) if item.get("severity") == "critical"]
    if critical:
        raise AssertionError(f"Unexpected critical findings: {critical}")


def assert_no_rule(report: dict, rule: str) -> None:
    matches = [item for item in report.get("findings", []) if item.get("rule") == rule]
    if matches:
        raise AssertionError(f"Unexpected '{rule}' findings: {matches}")


def assert_critical_rule(report: dict, rule: str) -> None:
    matches = [
        item
        for item in report.get("findings", [])
        if item.get("rule") == rule and item.get("severity") == "critical"
    ]
    if not matches:
        present = sorted({(item.get("severity"), item.get("rule")) for item in report.get("findings", [])})
        raise AssertionError(f"Expected critical '{rule}' finding; present={present}")


def assert_warning_rule(report: dict, rule: str) -> None:
    matches = [
        item
        for item in report.get("findings", [])
        if item.get("rule") == rule and item.get("severity") == "warning"
    ]
    if not matches:
        present = sorted({(item.get("severity"), item.get("rule")) for item in report.get("findings", [])})
        raise AssertionError(f"Expected warning '{rule}' finding; present={present}")


def page_metrics(report: dict) -> list[dict]:
    return [p.get("metrics", {}) for p in report.get("pages", []) if not p.get("skipped")]


def visible_scrollbars(report: dict) -> list[dict]:
    return [
        item
        for metrics in page_metrics(report)
        for item in metrics.get("visibleScrollbars", [])
    ]


def require_scrollbar(report: dict, selector_suffix: str, axis: str, depth: int) -> dict:
    matches = [
        item
        for item in visible_scrollbars(report)
        if selector_suffix in item.get("selector", "") and item.get("axis") == axis
    ]
    if not matches:
        raise AssertionError(
            f"Expected {selector_suffix} {axis}-axis scrollbar, got {visible_scrollbars(report)}"
        )
    scrollbar = matches[0]
    if scrollbar.get("sameAxisDepth") != depth or len(scrollbar.get("scrollChain", [])) != depth:
        raise AssertionError(f"Expected same-axis depth {depth}, got {scrollbar}")
    if selector_suffix not in scrollbar.get("scrollChain", [])[-1]:
        raise AssertionError(f"Scroll chain must end at its reported scrollbar: {scrollbar}")
    return scrollbar


def main() -> int:
    tmp = Path(tempfile.mkdtemp(prefix="fwv-")).resolve()
    server = None
    try:
        skill_contract = (ROOT / "SKILL.md").read_text(encoding="utf-8")
        if (
            "--human-readable-stdout" not in skill_contract
            or "--receipt-only" not in skill_contract
            or "deprecated" not in skill_contract.lower()
            or "human-only" not in skill_contract
            or "default" not in skill_contract
            or "bounded" not in skill_contract
            or "control-text-clipped" not in skill_contract
            or "data-ui-verify-min-content-inset" not in skill_contract
            or "breakpointProfile" not in skill_contract
            or "maxPageCount" not in skill_contract
            or "sampled-only" not in skill_contract
            or "sourceBinding" not in skill_contract
            or "journey_review_contract.md" not in skill_contract
            or "review-queue.json" not in skill_contract
            or "journey-evidence.json" not in skill_contract
            or "formal_web_ui_review.py" not in skill_contract
            or "secondary-workflow-precedes-primary" not in skill_contract
            or "insufficient-text-contrast" not in skill_contract
            or "declared-theme-contradiction" not in skill_contract
        ):
            raise AssertionError("Skill contract omits required output, detector, breakpoint, or evidence behavior")
        journey_reference = ROOT / "references" / "journey_review_contract.md"
        if not journey_reference.is_file() or not all(
            token in journey_reference.read_text(encoding="utf-8")
            for token in ("primaryJourney", "continuation", "reviewInputs", "manual-review.json", "Screenshot SHA-256")
        ):
            raise AssertionError("Journey/theme/changed-review reference is missing or incomplete")
        fixtures = tmp / "site"
        write(fixtures / "clean.html", page("<h1>Dashboard</h1><p>Everything fits.</p><button>Save changes</button>"))
        write(
            fixtures / "source-binding-meta.html",
            page("<meta name='ui-source-revision' content='meta-deployed-revision'><h1>Meta-bound deployment</h1>"),
        )
        write(
            fixtures / "clipped-button.html",
            page("<button class='bad'>Save changes now</button>", ".bad { width: 42px; overflow: hidden; white-space: nowrap; }"),
        )
        write(
            fixtures / "clipped-text.html",
            page("<p class='bad'>This paragraph is intentionally too tall for the container.</p>", ".bad { width: 180px; height: 12px; overflow: hidden; }"),
        )
        write(
            fixtures / "occluded.html",
            page("<button class='target'>Checkout</button><div class='cover'></div>", ".target { margin-top: 40px; } .cover { position: fixed; inset: 50px auto auto 20px; width: 140px; height: 50px; background: rgba(0,0,0,.85); z-index: 20; }"),
        )
        write(
            fixtures / "outside-area.html",
            page("<section data-ui-verify-area='card' class='card'><button class='bad'>Open</button></section>", ".card { position: relative; width: 120px; height: 80px; border: 1px solid #000; overflow: visible; } .bad { position: absolute; left: 150px; top: 20px; }"),
        )
        write(fixtures / "broken-image.html", page("<h1>Media</h1><img src='/missing-image.png' alt='Missing asset' width='120' height='80'>"))
        write(fixtures / "invisible-text.html", page("<p class='bad'>Invisible message</p>", ".bad { color: #fff; background: #fff; }"))
        write(
            fixtures / "lab-color.html",
            page("<button class='lab'>Save</button>", ".lab { color: white; background: lab(1.76974 1.32743 -9.28855); }"),
        )
        write(
            fixtures / "oklab-color.html",
            page("<span class='badge'>0:08</span>", ".badge { color: white; background: oklab(0.128998 -0.0038857 -0.0418156 / 0.8); }"),
        )
        write(
            fixtures / "allowed-ellipsis.html",
            page("<span class='file' data-ui-allow-truncation='filename ellipsis'>very-long-file-name-for-a-report.pdf</span>", ".file { display: block; width: 80px; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }"),
        )
        write(
            fixtures / "ignored.html",
            page("<button data-ui-verify-ignore='fixture intentionally broken' class='bad'>Save changes now</button>", ".bad { width: 42px; overflow: hidden; white-space: nowrap; }"),
        )
        write(
            fixtures / "chart-overflow.html",
            page("<div class='chart'><svg width='1200' height='160'><rect width='1200' height='160' fill='#ddd'/></svg></div>", ".chart { width: 300px; height: 160px; overflow: hidden; }"),
        )
        write(
            fixtures / "scrollbars.html",
            page("<div class='scrollbox'><p>One</p><p>Two</p><p>Three</p><p>Four</p></div>", ".scrollbox { width: 180px; height: 48px; overflow-y: scroll; overflow-x: hidden; border: 1px solid #aaa; }"),
        )
        write(
            fixtures / "horizontal-scrollbar.html",
            page(
                "<h1>Wide comparison</h1><div class='x-scroll'><div class='wide'>Account status and billing details</div></div>",
                ".x-scroll { width: 240px; overflow-x: auto; border: 1px solid #aaa; } .wide { width: 640px; padding: 12px; white-space: nowrap; }",
            ),
        )
        write(
            fixtures / "forced-horizontal-scrollbar.html",
            page(
                "<h1>Forced scrollbar</h1><div class='forced-x'>Content already fits</div>",
                ".forced-x { width: 240px; overflow-x: scroll; padding: 12px; border: 1px solid #aaa; }",
            ),
        )
        write(
            fixtures / "nested-horizontal-scrollbars.html",
            page(
                "<h1>Nested comparisons</h1><div class='outer-x'><div class='inner-x'><div class='wide'>A very wide comparison table</div></div></div>",
                ".outer-x { width: 280px; overflow-x: auto; border: 1px solid #777; } .inner-x { width: 520px; overflow-x: auto; } .wide { width: 900px; padding: 12px; white-space: nowrap; }",
            ),
        )
        write(
            fixtures / "double-vertical-scrollbars.html",
            page(
                "<h1>Nested activity</h1><div class='outer-v'><div class='inner-v'><div class='inner-content' aria-hidden='true'></div></div><div class='outer-fill' aria-hidden='true'></div></div>",
                ".outer-v { width: 280px; height: 280px; overflow-y: auto; border: 1px solid #777; } .inner-v { height: 150px; overflow-y: auto; } .inner-content { height: 520px; } .outer-fill { height: 320px; }",
            ),
        )
        write(
            fixtures / "triple-vertical-scrollbars.html",
            page(
                "<h1>Deeply nested activity</h1><div class='outer-v'><div class='middle-v'><div class='inner-v'><div class='inner-content' aria-hidden='true'></div></div><div class='middle-fill' aria-hidden='true'></div></div><div class='outer-fill' aria-hidden='true'></div></div>",
                ".outer-v { width: 280px; height: 320px; overflow-y: auto; border: 1px solid #777; } .middle-v { height: 240px; overflow-y: auto; } .inner-v { height: 140px; overflow-y: auto; } .inner-content { height: 500px; } .middle-fill { height: 250px; } .outer-fill { height: 250px; }",
            ),
        )
        write(
            fixtures / "document-plus-vertical-panel.html",
            page(
                "<h1>Long activity page</h1><div class='panel-v'><div class='panel-content' aria-hidden='true'></div></div><div class='page-fill' aria-hidden='true'></div>",
                ".panel-v { width: 280px; height: 160px; overflow-y: auto; border: 1px solid #777; } .panel-content { height: 480px; } .page-fill { height: 900px; }",
            ),
        )
        write(
            fixtures / "mixed-axis-scrollbars.html",
            page(
                "<h1>Mixed axes</h1><div class='outer-y'><div class='inner-x'><div class='wide'>Wide comparison content</div></div><div class='outer-fill' aria-hidden='true'></div></div>",
                ".outer-y { width: 280px; height: 260px; overflow-y: auto; border: 1px solid #777; } .inner-x { width: 220px; overflow-x: auto; } .wide { width: 600px; padding: 12px; white-space: nowrap; } .outer-fill { height: 320px; }",
            ),
        )
        write(
            fixtures / "nonoverflowing-auto.html",
            page(
                "<h1>Fitting panel</h1><div class='fits-auto'>Content fits without scrolling</div>",
                ".fits-auto { width: 240px; height: 80px; overflow: auto; padding: 12px; border: 1px solid #aaa; }",
            ),
        )
        # Fix 2: white text on a gradient must NOT be a critical invisible-text finding,
        # and must be recorded under metrics.unmeasurableContrast.
        write(
            fixtures / "gradient-contrast.html",
            page(
                "<p class='hero'>Readable on a gradient</p>",
                ".hero { color: #fff; background: linear-gradient(90deg, #0a3d62, #3c6382); padding: 24px; }",
            ),
        )
        # Reachable open-shadow and iframe content must be inspected. Both
        # contexts contain a realistic clipped control that the top document
        # itself does not contain.
        write(
            fixtures / "iframe-child.html",
            page(
                "<button class='bad'>Framed action</button><div class='frame-x'><div class='frame-wide'>Framed comparison</div></div>",
                ".bad { width: 34px; overflow: hidden; white-space: nowrap; } .frame-x { width: 120px; overflow-x: auto; } .frame-wide { width: 320px; white-space: nowrap; }",
            ),
        )
        write(
            fixtures / "shadow-iframe.html",
            page(
                "<h1>Component library</h1>"
                "<div id='host'></div>"
                "<iframe src='/iframe-child.html' width='200' height='80' title='child'></iframe>"
                "<script>"
                "const host = document.getElementById('host');"
                "const root = host.attachShadow({mode: 'open'});"
                "root.innerHTML = '<style>.bad{width:34px;overflow:hidden;white-space:nowrap}</style>' +"
                " '<button class=\"bad\">Shadow action</button>';"
                "</script>",
            ),
        )
        # A narrow viewport alone is not a mobile-device emulation. This page
        # exposes a realistic touch/UA-only responsive defect so the descriptor
        # path proves it sets device semantics as well as width and height.
        write(
            fixtures / "device-responsive.html",
            page(
                "<h1>Responsive account</h1><div id='slot'></div>"
                "<script>"
                "if (navigator.maxTouchPoints > 0 && /iPhone/.test(navigator.userAgent)) {"
                " document.getElementById('slot').innerHTML = '<button class=\"bad\">Touch account actions</button>';"
                "}"
                "</script>",
                ".bad { width: 38px; overflow: hidden; white-space: nowrap; }",
            ),
        )
        # Hidden/transient states are measured only after explicit, auditable
        # actions. The base state is clean; one opened state is broken and one
        # opened state is an intentional clean disclosure.
        write(
            fixtures / "interaction-states.html",
            page(
                "<button id='open-bad' onclick=\"document.getElementById('bad-panel').hidden=false\">Open actions</button>"
                "<button id='open-good' onclick=\"document.getElementById('good-panel').hidden=false\">Open help</button>"
                "<section id='bad-panel' hidden><button class='bad'>Destructive account action</button></section>"
                "<section id='good-panel' hidden><p>Keyboard-accessible help content.</p></section>",
                ".bad { width: 40px; overflow: hidden; white-space: nowrap; }",
            ),
        )
        write(
            fixtures / "conditional-ownership.html",
            page(
                "<h1>Conditional controls</h1>"
                "<button id='show-special' onclick=\"document.getElementById('special').hidden=false\">Open specialist journey</button>"
                "<button id='special' hidden onclick=\"document.getElementById('special-form').hidden=false;document.getElementById('special-name').focus()\">Special action</button>"
                "<form id='special-form' data-ui-continuation-anchor hidden><label>Special name <input id='special-name'></label></form>",
            ),
        )
        write(
            fixtures / "event-readiness.html",
            page(
                "<h1>Event readiness</h1><button id='start'>Start</button><button id='slow'>Slow</button><button id='fail'>Fail</button><div id='ready' hidden>Ready</div><div id='error' hidden>Error</div>"
                "<script>document.querySelector('#start').onclick=()=>{fetch('/readback.json');requestAnimationFrame(()=>requestAnimationFrame(()=>{document.querySelector('#ready').hidden=false}))};document.querySelector('#slow').onclick=()=>setTimeout(()=>{document.querySelector('#ready').hidden=false},180);document.querySelector('#fail').onclick=()=>{document.querySelector('#error').hidden=false}</script>",
            ),
        )
        write(fixtures / "readback.json", '{"ready":true}\n')
        # Fix 1: a tall lazy-load page whose below-the-fold control is only created when
        # it scrolls into view. With scrolling on it is found (and its clip is critical);
        # with --no-scroll it is never created.
        write(
            fixtures / "lazy-scroll.html",
            page(
                "<div class='spacer'>Scroll down for lazy content.</div>"
                "<div id='sentinel'></div>"
                "<div id='lazy-slot'></div>"
                "<script>"
                "const io = new IntersectionObserver((entries) => {"
                "  for (const entry of entries) {"
                "    if (entry.isIntersecting) {"
                "      const slot = document.getElementById('lazy-slot');"
                "      if (!slot.dataset.filled) {"
                "        slot.dataset.filled = '1';"
                "        slot.innerHTML = '<button class=\"lazybad\">Lazy loaded action button</button>';"
                "      }"
                "    }"
                "  }"
                "});"
                "io.observe(document.getElementById('sentinel'));"
                "</script>",
                ".spacer { height: 2400px; } "
                "#sentinel { height: 1px; } "
                ".lazybad { width: 40px; overflow: hidden; white-space: nowrap; }",
            ),
        )
        # Realistic-breakage regressions: each of these fixtures reproduces a defect
        # class the verifier previously missed entirely (agent-report 2026-07-03:
        # "doesn't report problems in most of the cases").
        write(
            fixtures / "div-clipped-ancestor.html",
            page(
                "<div class='card'><div class='title'>Monthly report</div>"
                "<div class='body'>This body text is long enough to need four lines of vertical space "
                "inside the card so the fixed card height visibly cuts the text mid-line.</div></div>",
                ".card { width: 220px; height: 58px; overflow: hidden; border: 1px solid #ccc; padding: 8px; }",
            ),
        )
        write(
            fixtures / "abs-button-cut.html",
            page(
                "<div class='panel'><button class='cta'>Confirm order</button></div>",
                ".panel { position: relative; width: 240px; height: 90px; overflow: hidden; border: 1px solid #ccc; }"
                " .cta { position: absolute; left: 170px; top: 30px; width: 140px; }",
            ),
        )
        write(
            fixtures / "negative-top-cut.html",
            page(
                "<div class='hero'><h2 class='title'>Quarterly results overview</h2></div>",
                ".hero { width: 320px; height: 80px; overflow: hidden; border: 1px solid #ccc; } .title { margin-top: -14px; }",
            ),
        )
        write(
            fixtures / "partial-overlap.html",
            page(
                "<div class='row'><span class='price'>$1,299.00</span><span class='badge'>SALE</span></div>",
                ".row { position: relative; width: 300px; height: 44px; }"
                " .price { position: absolute; left: 0; top: 8px; display: block; width: 120px; height: 20px; }"
                " .badge { position: absolute; left: 40px; top: 4px; display: block; width: 100px; height: 28px; background: #d33; color: #fff; }",
            ),
        )
        write(
            fixtures / "broken-image-collapsed.html",
            page("<h1>Product</h1><img src='/missing-photo.png' alt=''><p>Great product.</p>"),
        )
        write(
            fixtures / "roadmap-invisible.html",
            page(
                "<div class='roadmap-section'><p class='ghost'>Phase 2: launch billing</p></div>",
                ".ghost { color: #fff; background: #fff; }",
            ),
        )
        write(
            fixtures / "div-invisible-text.html",
            page("<div class='note'>Your subscription expired</div>", ".note { color: #fff; }"),
        )
        write(
            fixtures / "offcanvas-left-cut.html",
            page(
                "<button class='back'>Back to dashboard</button>",
                ".back { position: absolute; left: -60px; top: 40px; width: 180px; }",
            ),
        )
        write(
            fixtures / "fixed-toolbar-cut.html",
            page(
                "<p>Content</p><div class='toolbar'><button>Accept</button><button>Reject</button></div>",
                ".toolbar { position: fixed; left: 0; right: 0; bottom: -30px; height: 52px; background: #eee; }"
                " .toolbar button { height: 44px; }",
            ),
        )
        write(
            fixtures / "nowrap-spill-cut.html",
            page(
                "<div class='cell'><div class='val'>4111 1111 1111 1111 (Visa)</div></div>",
                ".cell { width: 120px; overflow: hidden; border: 1px solid #ccc; } .val { white-space: nowrap; }",
            ),
        )
        # Composite clean modern page: sticky header, ellipsis card titles inside
        # overflow-hidden cards, line-clamp, scrollable table, FAB, sr-only link.
        # Guards against false criticals from rule interplay (a card h3 with its own
        # ellipsis inside an overflow-hidden card once produced a false
        # clipped-by-ancestor via the scrollWidth spill extension).
        write(
            fixtures / "modern-clean.html",
            page(
                "<header class='app'><span class='logo'>Acme</span><span class='chip'>Berlin · synced</span>"
                "<nav><a href='#a'>Home</a><a href='#b'>Reports</a></nav></header>"
                "<div class='card'><h3 class='title'>Quarterly-financial-forecast-and-planning-notes.xlsx</h3>"
                "<div class='desc'>Latest revision includes the updated growth assumptions for the EMEA region and "
                "the revised hiring plan for the platform team.</div>"
                "<div class='row'><span class='chip'>Updated 2h ago</span><button>Open</button></div></div>"
                "<div class='scroll-table'><table><tr><th>Project</th><th>Owner</th><th>Status</th><th>Notes</th></tr>"
                "<tr><td>Atlas</td><td>J. Rivera</td><td>Active</td><td>Waiting on vendor quote</td></tr></table></div>"
                "<button class='fab' aria-label='New item'>+</button>"
                "<a class='sr-only' href='#main'>Skip to content</a>",
                "header.app { position: sticky; top: 0; display: flex; gap: 10px; align-items: center; background: #fff; padding: 10px; }"
                " .chip { font-size: 12px; background: #eef1f5; border-radius: 999px; padding: 4px 10px; color: #444; }"
                " .card { width: 260px; background: #fff; border: 1px solid #e3e6ea; border-radius: 10px; padding: 12px; overflow: hidden; }"
                " .card h3.title { margin: 0 0 6px; font-size: 16px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }"
                " .card .desc { color: #555; font-size: 14px; display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 2; overflow: hidden; }"
                " .card .row { display: flex; gap: 8px; margin-top: 10px; }"
                " .scroll-table { overflow-x: auto; margin-top: 16px; } table { border-collapse: collapse; min-width: 900px; }"
                " td, th { padding: 10px 12px; white-space: nowrap; text-align: left; }"
                " .fab { position: fixed; right: 16px; bottom: 16px; width: 56px; height: 56px; border-radius: 50%; background: #2a5bd7; color: #fff; border: 0; }"
                " .sr-only { position: absolute; width: 1px; height: 1px; margin: -1px; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; }",
            ),
        )
        # False-positive guards: common intentional patterns must not become criticals.
        write(
            fixtures / "truncate-inner-span.html",
            page(
                "<div class='cell'><span>very-long-file-name-that-should-ellipsize.pdf</span></div>",
                ".cell { width: 120px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }",
            ),
        )
        write(
            fixtures / "line-clamp.html",
            page(
                "<div class='clamp'>Long description text that wraps over many lines and is intentionally "
                "clamped to two lines with a standard line-clamp pattern used across modern apps.</div>",
                ".clamp { width: 220px; display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 2; overflow: hidden; }",
            ),
        )
        write(
            fixtures / "carousel-peek.html",
            page(
                "<div class='swiper'><div class='track'>"
                "<div class='slide'>First slide content</div>"
                "<div class='slide'>Second slide peeks out</div>"
                "<div class='slide'>Third slide fully hidden</div>"
                "</div></div>",
                ".swiper { width: 300px; overflow: hidden; } .track { display: flex; gap: 8px; }"
                " .slide { flex: 0 0 220px; height: 80px; background: #eef; padding: 8px; }",
            ),
        )
        write(
            fixtures / "app-shell-scroll.html",
            page(
                "<div class='shell'><div class='spacer'>Top of the app shell.</div>"
                "<button class='deep'>Deep action</button></div>",
                "html, body { height: 100%; overflow: hidden; } main { padding: 0; height: 100%; }"
                " .shell { height: 100%; overflow-y: auto; } .spacer { height: 1900px; }",
            ),
        )
        write(
            fixtures / "skip-link.html",
            page(
                "<a class='skip' href='#main'>Skip to content</a><h1 id='main'>Welcome</h1>",
                ".skip { position: absolute; left: -9999px; top: 0; }",
            ),
        )
        write(
            fixtures / "fab-over-text.html",
            page(
                "<div class='msg'>A long status message pinned near the bottom of the screen.</div>"
                "<button class='fab' aria-label='Compose'>+</button>",
                ".msg { position: fixed; bottom: 10px; left: 10px; width: 300px; height: 40px; }"
                " .fab { position: fixed; bottom: 10px; left: 250px; width: 70px; height: 70px; border-radius: 50%; background: #06c; color: #fff; }",
            ),
        )
        write(
            fixtures / "accordion-closed.html",
            page(
                "<div class='acc'><p>Hidden panel content that is intentionally collapsed.</p></div><p>Visible content.</p>",
                ".acc { max-height: 0; overflow: hidden; }",
            ),
        )
        write(
            fixtures / "form-controls.html",
            page(
                "<input class='f' value='a very long value that exceeds the field width for sure'>"
                "<select><option>Pick one option here</option></select>",
                ".f { width: 120px; }",
            ),
        )
        write(
            fixtures / "native-placeholder-clipped.html",
            page(
                "<label for='query'>Search</label>"
                "<input id='query' class='narrow' placeholder='PRIVATE_PLACEHOLDER_MUST_NOT_LEAK'>",
                ".narrow { width: 92px; padding: 5px 10px; font: 16px Arial, sans-serif; }",
            ),
        )
        write(
            fixtures / "native-select-clipped.html",
            page(
                "<label for='choice'>Choice</label>"
                "<select id='choice' class='narrow'><option selected>PRIVATE_SELECTED_LABEL_MUST_NOT_LEAK</option></select>",
                ".narrow { width: 112px; padding: 5px 10px; font: 16px Arial, sans-serif; }",
            ),
        )
        write(
            fixtures / "native-typed-scrollable.html",
            page(
                "<label for='typed'>Reference</label>"
                "<input id='typed' class='narrow' value='PRIVATE_TYPED_VALUE_MUST_NOT_LEAK'>",
                ".narrow { width: 92px; padding: 5px 10px; font: 16px Arial, sans-serif; }",
            ),
        )
        write(
            fixtures / "native-placeholder-allowed.html",
            page(
                "<input class='narrow' data-ui-allow-truncation='compact search prompt' "
                "placeholder='PRIVATE_ALLOWED_PLACEHOLDER_MUST_NOT_LEAK'>",
                ".narrow { width: 92px; padding: 5px 10px; font: 16px Arial, sans-serif; }",
            ),
        )
        write(
            fixtures / "content-inset-bad.html",
            page(
                "<section class='card' data-ui-verify-min-content-inset='12'><span>Items</span></section>",
                ".card { width: 240px; border: 1px solid #aaa; }",
            ),
        )
        write(
            fixtures / "content-inset-clean.html",
            page(
                "<section class='card'><span>Items</span><button>Add item</button></section>",
                ".card { width: 240px; padding: 16px; border: 1px solid #aaa; } .card button { display: block; margin-top: 8px; }",
            ),
        )
        write(
            fixtures / "content-inset-intentional-edges.html",
            page(
                "<fieldset><legend>Delivery</legend><label>Street <input></label></fieldset>"
                "<div class='attached'><input aria-label='Quantity'><button>Apply</button></div>",
                ".attached { display: flex; margin-top: 16px; } .attached input, .attached button { margin: 0; }",
            ),
        )
        write(
            fixtures / "breakpoint-edge.html",
            page(
                "<button class='bp'>Responsive navigation actions</button>",
                "@media (max-width: 767px) { .bp { width: 38px; overflow: hidden; white-space: nowrap; } }",
            ),
        )
        # Scroll-container reachability: a wide table row scrolled out of its own
        # overflow-x container sits (in document coordinates) under a neighboring
        # panel. That is reachable content, not an occlusion — hit-testing must
        # scroll the container into view instead of blaming the neighbor.
        write(
            fixtures / "scroll-panel-neighbor.html",
            page(
                "<div class='cols'>"
                "<div class='left'><div class='hscroll'><div class='wide'>"
                "<span class='cell'>OpenFOAM long run condition name</span>"
                "<span class='cell state'>pending</span>"
                "</div></div></div>"
                "<div class='right'><p>Neighbor panel content sits here.</p></div>"
                "</div>",
                ".cols { display: flex; gap: 8px; } .left { width: 160px; }"
                " .hscroll { overflow-x: auto; } .wide { display: flex; gap: 12px; min-width: 480px; }"
                " .cell { white-space: nowrap; flex: 0 0 auto; }"
                " .right { width: 170px; background: #f4f6f8; padding: 8px; }",
            ),
        )
        # Closed <details>: Chromium keeps layout boxes for the hidden content
        # (content-visibility), so a later sibling legitimately occupies the same
        # document coordinates. The hidden fields must not be reported occluded.
        write(
            fixtures / "details-closed-neighbor.html",
            page(
                "<details><summary>advanced settings</summary>"
                "<label>Span <input value='1.0'></label>"
                "<select><option>derived value</option></select>"
                "</details>"
                "<div class='after'>Preview panel that renders where the closed details content would be.</div>",
                ".after { background: #f4f6f8; padding: 10px; }",
            ),
        )
        # Framework dev overlays (Next.js badge portal) are dev-server artifacts,
        # not page content: they must not count as occluders.
        write(
            fixtures / "dev-overlay-badge.html",
            page(
                "<div class='msg'>Status text pinned near the bottom left corner.</div>"
                "<nextjs-portal class='badge'></nextjs-portal>",
                ".msg { position: fixed; bottom: 12px; left: 12px; width: 260px; height: 36px; }"
                " nextjs-portal.badge { position: fixed; bottom: 8px; left: 8px; width: 270px; height: 44px;"
                " display: block; background: #000; border-radius: 22px; }",
            ),
        )
        # Recall guard for the same code path: an opaque overlay covering content
        # that IS visible inside the scroll container must still be caught.
        write(
            fixtures / "overlay-in-scroll.html",
            page(
                "<div class='hscroll'><div class='wide'>"
                "<span class='cell'>Visible target text</span>"
                "</div><div class='cover'></div></div>",
                ".hscroll { position: relative; width: 300px; overflow-x: auto; }"
                " .wide { min-width: 600px; } .cell { white-space: nowrap; }"
                " .cover { position: absolute; left: 0; top: 0; width: 220px; height: 44px; background: #222; }",
            ),
        )
        write(
            fixtures / "accounts-form-first.html",
            page(
                """
<section id="add-account" class="add-account">
  <h2>Add account</h2>
  <label>Name <input></label>
  <button>Save account</button>
</section>
<section id="account-list" class="account-list">
  <h1>Accounts</h1>
  <article>Ada</article><article>Grace</article><article>Linus</article>
</section>
""",
                ".add-account { min-height: 320px; padding: 24px; background: #eef2ff; }"
                " .account-list { min-height: 420px; padding: 24px; }",
            ),
        )
        write(
            fixtures / "accounts-offscreen-add.html",
            page(
                """
<h1>Accounts</h1>
<button id="open-add">Add account</button>
<section id="account-list" class="long-list">
  <p>Ada</p><p>Grace</p><p>Linus</p><p>Margaret</p><p>Barbara</p>
</section>
<form id="add-form" hidden>
  <h2>Add account</h2>
  <label>Name <input id="account-name"></label>
</form>
<script>
document.querySelector('#open-add').addEventListener('click', () => {
  document.querySelector('#add-form').hidden = false;
});
</script>
""",
                ".long-list { min-height: 1450px; } #add-form { min-height: 260px; padding: 20px; background: #eef2ff; }",
            ),
        )
        write(
            fixtures / "accounts-modal-add.html",
            page(
                """
<h1>Accounts</h1>
<button id="open-add">Add account</button>
<section id="account-list" class="long-list"><p>Ada</p><p>Grace</p><p>Linus</p></section>
<div id="add-dialog" role="dialog" aria-modal="true" hidden>
  <h2>Add account</h2>
  <label>Name <input id="account-name"></label>
  <button>Save</button>
</div>
<script>
document.querySelector('#open-add').addEventListener('click', () => {
  const dialog = document.querySelector('#add-dialog');
  dialog.hidden = false;
  document.querySelector('#account-name').focus();
});
</script>
""",
                ".long-list { min-height: 1400px; } #add-dialog { position: fixed; inset: 18% 12%; z-index: 20; padding: 24px; background: white; border: 2px solid #334155; }",
            ),
        )
        write(
            fixtures / "accounts-toolbar.html",
            page(
                """
<div id="account-tools"><label>Search <input></label><button>Filter</button></div>
<section id="account-list"><h1>Accounts</h1><article>Ada</article><article>Grace</article></section>
""",
                "#account-tools { min-height: 44px; display: flex; gap: 8px; } #account-list { min-height: 420px; }",
            ),
        )
        write(
            fixtures / "accounts-alert.html",
            page(
                """
<div id="blocking-alert" role="alert">Account data is unavailable until the connection is restored.</div>
<section id="account-list"><h1>Accounts</h1><p>Unavailable</p></section>
""",
                "#blocking-alert { padding: 18px; background: #fee2e2; color: #7f1d1d; } #account-list { min-height: 360px; }",
            ),
        )
        write(
            fixtures / "create-account.html",
            page(
                """
<form id="create-form">
  <h1>Add account</h1>
  <label>Name <input id="account-name" autofocus></label>
  <button>Save account</button>
</form>
""",
                "#create-form { min-height: 420px; padding: 24px; }",
            ),
        )
        write(
            fixtures / "accounts-navigation.html",
            page(
                "<h1>Accounts</h1><a id='go-create' href='/create-account.html'>Add account</a><section id='account-list'><p>Ada</p></section>",
                "#account-list { min-height: 420px; }",
            ),
        )
        write(
            fixtures / "theme-dark-with-bright-surface.html",
            page(
                "<section id='primary' class='bright'><h1>Dark workspace</h1><p>A bright sheet dominates this dark target.</p></section>",
                "body { background: #0b1020; color: #e5e7eb; } .bright { min-height: 760px; background: #fff; color: #111; padding: 24px; }",
            ),
        )
        write(
            fixtures / "theme-light-with-dark-surface.html",
            page(
                "<section id='primary' class='dark'><h1>Light workspace</h1><p>A dark sheet dominates this light target.</p></section>",
                ".dark { min-height: 760px; background: #111827; color: #fff; padding: 24px; }",
            ),
        )
        write(
            fixtures / "theme-mixed.html",
            page(
                "<section id='primary' class='mixed'><div class='light'><h1>Mixed canvas</h1></div><div class='dark'>Dark comparison surface</div></section>",
                ".mixed { display: grid; grid-template-columns: 1fr 1fr; min-height: 760px; } .light { background: white; color: #111; padding: 24px; } .dark { background: #111827; color: white; padding: 24px; }",
            ),
        )
        write(
            fixtures / "contrast-aa-fail.html",
            page("<section id='primary'><h1>Accounts</h1><p class='low'>Normal text below AA contrast.</p></section>", ".low { color: #888; background: #fff; } #primary { min-height: 360px; }"),
        )
        write(
            fixtures / "contrast-large-pass.html",
            page("<section id='primary'><h1>Accounts</h1><p class='large'>Large text passes its 3:1 threshold.</p></section>", ".large { color: #777; background: #fff; font-size: 24px; } #primary { min-height: 360px; }"),
        )
        write(
            fixtures / "contrast-allowed.html",
            page("<section id='primary'><h1>Accounts</h1><p data-ui-allow-contrast='inactive decorative watermark' class='low'>Watermark</p></section>", ".low { color: #aaa; background: #fff; } #primary { min-height: 360px; }"),
        )
        write(
            fixtures / "palette-competing.html",
            page(
                "<section id='primary' class='palette'><div class='red'></div><div class='green'></div><div class='blue'></div><div class='purple'></div><h1>Palette lab</h1></section>",
                ".palette { position: relative; display: grid; grid-template-columns: 1fr 1fr; min-height: 760px; }"
                " .palette > div { min-height: 380px; } .red { background: #ef233c; } .green { background: #00a878; }"
                " .blue { background: #0057ff; } .purple { background: #b517ff; }"
                " .palette h1 { position: absolute; top: 12px; left: 12px; margin: 0; padding: 8px; color: white; background: #111; }",
            ),
        )
        write(
            fixtures / "dynamic-review.html",
            page(
                "<section id='primary'><h1>Dynamic review fixture</h1><p id='dynamic'></p></section><script>document.querySelector('#dynamic').textContent = String(Date.now());</script>",
                "#primary { min-height: 420px; }",
            ),
        )
        server = Server(fixtures)
        performance_server = DynamicServer(RenderPerformanceHandler).start()
        try:
            with urllib.request.urlopen(f"{performance_server.base_url}/fast", timeout=2) as warm_response:
                warm_response.read()

            def performance_config(route: str, **target_overrides: object) -> dict:
                return {
                    "repoRoot": str(ROOT.resolve()),
                    "targets": [{
                        **default_target_contract(),
                        "url": f"{performance_server.base_url}{route}",
                        **target_overrides,
                    }],
                    "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
                    "maxPageCount": 1,
                }

            fast_performance = run_verify_config(
                performance_config("/fast"),
                tmp / "performance-fast-defaults",
                expect=(0, 1),
                apply_contract_defaults=False,
            )
            fast_metrics = fast_performance.get("pages", [{}])[0].get("metrics", {}).get("performance", {})
            if (
                fast_metrics.get("ttfb", {}).get("thresholdMs") != 10
                or fast_metrics.get("lcp", {}).get("thresholdMs") != 800
                or fast_metrics.get("ttfb", {}).get("comparison") != "<"
                or fast_metrics.get("lcp", {}).get("comparison") != "<"
            ):
                raise AssertionError(f"Default rendered-performance thresholds were not applied: {fast_metrics}")
            expected_performance_rules: set[str] = set()
            for metric, threshold in (("ttfb", 10), ("lcp", 800)):
                evidence = fast_metrics.get(metric, {})
                value = evidence.get("valueMs")
                if isinstance(value, bool) or not isinstance(value, (int, float)):
                    raise AssertionError(f"Default {metric.upper()} fixture was not measured: {fast_metrics}")
                expected_status = "pass" if value < threshold else "fail"
                if evidence.get("status") != expected_status:
                    raise AssertionError(
                        f"Default {metric.upper()} classification disagreed with its measurement: {fast_metrics}"
                    )
                if expected_status == "fail":
                    expected_performance_rules.add(f"{metric}-above-threshold")
            actual_critical_rules = {
                finding.get("rule")
                for finding in fast_performance.get("findings", [])
                if finding.get("severity") == "critical"
            }
            if actual_critical_rules != expected_performance_rules:
                raise AssertionError(
                    "Default performance findings disagreed with the observed metrics: "
                    f"expected={sorted(expected_performance_rules)} actual={sorted(actual_critical_rules)}"
                )
            performance_markdown = (tmp / "performance-fast-defaults" / "report.md").read_text(encoding="utf-8")
            if "## Rendered Performance" not in performance_markdown or "< 10 ms" not in performance_markdown or "< 800 ms" not in performance_markdown:
                raise AssertionError("Markdown report omitted formal TTFB/LCP thresholds")

            slow_ttfb = run_verify_config(
                performance_config(
                    "/slow-ttfb",
                    performance={"ttfbMs": 10, "lcpMs": 10000},
                ),
                tmp / "performance-slow-ttfb",
                expect=1,
                apply_contract_defaults=False,
            )
            assert_critical_rule(slow_ttfb, "ttfb-above-threshold")
            slow_ttfb_metrics = slow_ttfb.get("pages", [{}])[0].get("metrics", {}).get("performance", {})
            if slow_ttfb_metrics.get("ttfb", {}).get("valueMs", 0) < 10:
                raise AssertionError(f"Slow-TTFB fixture did not exceed the default: {slow_ttfb_metrics}")

            slow_lcp = run_verify_config(
                performance_config(
                    "/slow-lcp",
                    waitFor={
                        "selector": "#late-lcp[data-lcp-ready='true']",
                        "responseUrl": "**/lcp.png",
                        "timeoutMs": 5000,
                    },
                    performance={"ttfbMs": 10000, "lcpMs": 800},
                ),
                tmp / "performance-slow-lcp",
                expect=1,
                apply_contract_defaults=False,
            )
            assert_critical_rule(slow_lcp, "lcp-above-threshold")
            slow_lcp_metrics = slow_lcp.get("pages", [{}])[0].get("metrics", {}).get("performance", {})
            if slow_lcp_metrics.get("lcp", {}).get("valueMs", 0) < 800:
                raise AssertionError(f"Slow-LCP fixture did not exceed the default: {slow_lcp_metrics}")

            prescribed = run_verify_config(
                {
                    "repoRoot": str(ROOT.resolve()),
                    "targets": [
                        {
                            **default_target_contract(),
                            "url": f"{performance_server.base_url}/slow-ttfb",
                            "performance": {"ttfbMs": 10000, "lcpMs": 10000},
                        },
                        {
                            **default_target_contract(),
                            "url": f"{performance_server.base_url}/slow-lcp",
                            "waitFor": {
                                "selector": "#late-lcp[data-lcp-ready='true']",
                                "responseUrl": "**/lcp.png",
                                "timeoutMs": 5000,
                            },
                            "performance": {"ttfbMs": 10000, "lcpMs": 10000},
                        },
                    ],
                    "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
                    "maxPageCount": 2,
                },
                tmp / "performance-prescribed-thresholds",
                expect=0,
                apply_contract_defaults=False,
            )
            assert_no_critical(prescribed)
            if any(
                page_report.get("metrics", {}).get("performance", {}).get("ttfb", {}).get("thresholdMs") != 10000
                or page_report.get("metrics", {}).get("performance", {}).get("lcp", {}).get("thresholdMs") != 10000
                for page_report in prescribed.get("pages", [])
            ):
                raise AssertionError("Per-target performance thresholds did not override defaults")

            unavailable_lcp = run_verify_config(
                performance_config("/no-lcp"),
                tmp / "performance-lcp-unavailable",
                expect=0,
                apply_contract_defaults=False,
            )
            unavailable_findings = [
                finding
                for finding in unavailable_lcp.get("findings", [])
                if finding.get("rule") == "performance-metric-unavailable"
                and finding.get("evidence", {}).get("metric") == "LCP"
            ]
            if not unavailable_findings:
                raise AssertionError("Unavailable LCP was silently treated as a pass")

            private_performance = {
                "metrics": slow_lcp_metrics,
                "findings": [
                    finding.get("evidence", {})
                    for finding in slow_lcp.get("findings", [])
                    if finding.get("rule") in {"lcp-above-threshold", "ttfb-above-threshold", "performance-metric-unavailable"}
                ],
            }
            serialized_performance = json.dumps(private_performance)
            if "late-lcp" in serialized_performance or "Late largest image" in serialized_performance or performance_server.base_url in serialized_performance:
                raise AssertionError("Performance evidence leaked an element selector, text, or URL")

            local_scope_probe = subprocess.run(
                [
                    node_binary(),
                    "--input-type=module",
                    "--eval",
                    f"import {{ isLocalServerUrl, performanceThresholdStatus }} from {json.dumps(VERIFY.as_uri())};"
                    "if (!isLocalServerUrl('http://127.0.0.1:3000/') || !isLocalServerUrl('http://localhost:3000/') || isLocalServerUrl('https://example.test/') || performanceThresholdStatus(10,10)!=='fail' || performanceThresholdStatus(9.99,10)!=='pass' || performanceThresholdStatus(null,10)!=='unavailable' || performanceThresholdStatus(20,10,false)!=='not-applicable') process.exit(9);",
                ],
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=TIMEOUT_SECONDS,
                env=verifier_env(),
            )
            if local_scope_probe.returncode != 0:
                raise AssertionError(f"Local-only TTFB scope probe failed: {local_scope_probe.stderr}")
        finally:
            performance_server.close()

        unsafe_progress = tmp / "unsafe-scheduler-progress.jsonl"
        scheduler_probe = f"""
import {{ executePlan }} from {json.dumps(VERIFY.as_uri())};
const cells = [0, 1, 2].map((planIndex) => ({{
  cellId: `cell-${{planIndex + 1}}`,
  planIndex,
  executionPriority: 10 - planIndex,
  target: {{
    name: `unsafe-${{planIndex + 1}}`,
    url: 'http://127.0.0.1/unsafe',
    stateName: 'base',
    execution: {{ parallelSafe: false, resourceLocks: [], priority: null }},
    reviewEvidence: null,
    intentFingerprint: null,
  }},
  viewport: {{ name: 'desktop', width: 1280, height: 800, contextOptions: {{}} }},
}}));
const runner = async (_browser, cell, _config, _label, _auth, _cache, executionIndex) => ({{
  page: {{
    cellId: cell.cellId,
    target: cell.target,
    viewport: cell.viewport,
    outcome: 'internal_cell_error',
    skipped: true,
    skipReason: 'browser authority lost',
    durationMs: 1,
    cache: {{ hit: false }},
    cleanup: {{ status: 'completed' }},
    execution: {{ planIndex: cell.planIndex, executionIndex }},
  }},
  unsafeStop: 'browser-authority-lost',
}});
const result = await executePlan(
  {{}},
  cells,
  {{ execution: {{ maxConcurrency: 2 }}, progressOut: {json.dumps(str(unsafe_progress))} }},
  'fixture-browser',
  new Map(),
  null,
  runner,
);
if (result.pages.length !== 3 || result.pages.slice(1).some((page) => page.outcome !== 'unsafe_stop_unexecuted')) process.exit(7);
if (result.executionCount !== 1 || result.unsafeStop !== 'browser-authority-lost') process.exit(8);
"""
        scheduler_result = subprocess.run(
            [node_binary(), "--input-type=module", "--eval", scheduler_probe],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=TIMEOUT_SECONDS,
            env=verifier_env(),
        )
        if scheduler_result.returncode != 0:
            raise AssertionError(
                f"Unsafe-stop scheduler probe failed: {scheduler_result.stdout} {scheduler_result.stderr}"
            )
        clean_contract_config = tmp / "clean-contract.json"
        clean_contract_config.write_text(
            json.dumps(
                with_target_contract(
                    {
                        "targets": [{"url": f"{server.base_url}/clean.html"}],
                        "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
                    }
                ),
                indent=2,
            ),
            encoding="utf-8",
        )

        # The normal no-output-path invocation must create both complete
        # artifacts outside the audited working tree and print only a receipt.
        audited_worktree = tmp / "audited-worktree"
        audited_worktree.mkdir()
        default_root = tmp / "auto-artifacts"
        default_root.mkdir()
        default_env = verifier_env()
        default_env["TMPDIR"] = str(default_root)
        automatic = subprocess.run(
            verifier_command(
                "--config",
                str(clean_contract_config),
            ),
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=TIMEOUT_SECONDS,
            env=default_env,
            cwd=audited_worktree,
        )
        if automatic.returncode != 0 or automatic.stderr.strip():
            diagnostic: object = None
            try:
                failure_receipt = parse_bounded_receipt(
                    automatic.stdout,
                    expect=automatic.returncode,
                )
                failure_json, _ = receipt_artifact_paths(failure_receipt)
                diagnostic = json.loads(failure_json.read_text(encoding="utf-8")).get("error")
            except Exception as error:  # noqa: BLE001 - retain the primary verifier failure
                diagnostic = f"unreadable setup-failure artifact: {error}"
            raise AssertionError(
                "Automatic artifact run failed: "
                f"returncode={automatic.returncode}; diagnostic={diagnostic!r}; "
                f"stderr={automatic.stderr!r}"
            )
        automatic_receipt = parse_bounded_receipt(automatic.stdout, expect=0)
        automatic_json, automatic_markdown = receipt_artifact_paths(automatic_receipt)
        assert_complete_artifacts(automatic_json, automatic_markdown, expect=0)
        if not automatic_json.resolve().is_relative_to(default_root.resolve()):
            raise AssertionError(f"Default artifacts were not allocated under the external temp root: {automatic_receipt}")
        if automatic_json.parent.name == default_root.name or not automatic_json.parent.name.startswith("formal-web-ui-verification-"):
            raise AssertionError(f"Default artifacts must use a unique per-run directory: {automatic_receipt}")
        if automatic_json.resolve().is_relative_to(audited_worktree.resolve()):
            raise AssertionError("Default artifacts must stay outside the audited repository")

        governed_dir = tmp / "governed-run" / "checks" / "formal-ui" / "check" / "evidence"
        governed_env = verifier_env()
        governed_env.update({
            "DEVCOORDINATOR_EVIDENCE_DIR": str(governed_dir),
            "DEVCOORDINATOR_RUN_ID": "t20260902T010203Z-abcdef",
            "DEVCOORDINATOR_CHECK_NAME": "formal-ui",
        })
        governed = subprocess.run(
            verifier_command("--config", str(clean_contract_config)),
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=TIMEOUT_SECONDS,
            env=governed_env,
            cwd=audited_worktree,
        )
        if governed.returncode != 0 or governed.stderr.strip():
            raise AssertionError(
                f"Governed evidence run failed: {governed.returncode}; {governed.stderr!r}")
        governed_receipt = parse_bounded_receipt(governed.stdout, expect=0)
        governed_json, governed_markdown = receipt_artifact_paths(governed_receipt)
        governed_report = assert_complete_artifacts(
            governed_json, governed_markdown, expect=0)
        governed_journey = json.loads(
            (governed_dir / "journey-evidence.json").read_text(encoding="utf-8"))
        if (
            governed_json.parent.resolve() != governed_dir.resolve()
            or governed_journey.get("governedRunId") != "t20260902T010203Z-abcdef"
            or governed_journey.get("governedCheck") != "formal-ui"
            or governed_report.get("evidence", {}).get("journey", {}).get("path")
            != str(governed_dir / "journey-evidence.json")
        ):
            raise AssertionError("Governed run evidence did not use and bind its exact leaf directory")

        custom_bundle_runs: list[str] = []
        for index in range(2):
            custom_dir = tmp / f"governed-custom-output-{index}"
            custom_json = custom_dir / "report.json"
            custom_markdown = custom_dir / "report.md"
            custom_screenshots = custom_dir / "screenshots"
            custom = subprocess.run(
                verifier_command(
                    "--config",
                    str(clean_contract_config),
                    "--json-out",
                    str(custom_json),
                    "--markdown-out",
                    str(custom_markdown),
                    "--screenshot-dir",
                    str(custom_screenshots),
                ),
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=TIMEOUT_SECONDS,
                env=governed_env,
                cwd=audited_worktree,
            )
            if custom.returncode != 0 or custom.stderr.strip():
                raise AssertionError(
                    f"Governed custom-output run failed: {custom.returncode}; {custom.stderr!r}")
            custom_receipt = parse_bounded_receipt(custom.stdout, expect=0)
            receipt_json, receipt_markdown = receipt_artifact_paths(custom_receipt)
            custom_report = assert_complete_artifacts(
                receipt_json, receipt_markdown, expect=0)
            if receipt_json.resolve() != custom_json.resolve():
                raise AssertionError("Custom report output was replaced by governed publication")
            custom_bundle_runs.append(custom_report["runId"])

        governed_bundles = sorted(
            path for path in (governed_dir / "formal-runs").iterdir()
            if path.is_dir()
        )
        if len(governed_bundles) != 2 or len(set(custom_bundle_runs)) != 2:
            raise AssertionError(
                "Governed custom-output runs did not publish unique immutable bundles")
        published_runs: set[str] = set()
        for bundle in governed_bundles:
            manifest_path = bundle / "journey-evidence.json"
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            if (
                len(bundle.name) != 64
                or any(character not in "0123456789abcdef" for character in bundle.name)
                or manifest.get("governedRunId") != "t20260902T010203Z-abcdef"
                or manifest.get("governedCheck") != "formal-ui"
            ):
                raise AssertionError("Governed custom bundle identity is invalid")
            published_runs.add(manifest.get("runId"))
            for cell in manifest.get("cells", []):
                for screenshot in cell.get("screenshots", {}).values():
                    if screenshot is None:
                        continue
                    screenshot_path = (bundle / screenshot["path"]).resolve()
                    if not screenshot_path.is_relative_to(bundle.resolve()) or not screenshot_path.is_file():
                        raise AssertionError("Governed custom screenshot escaped its immutable bundle")
        if published_runs != set(custom_bundle_runs):
            raise AssertionError("Governed custom bundles lost or replaced a formal run")

        # Commands published while receipt mode was opt-in remain safe: the
        # old CLI flag is accepted as a no-op, never as an output-mode switch.
        alias_dir = tmp / "deprecated-receipt-alias"
        alias_json = alias_dir / "report.json"
        alias_markdown = alias_dir / "report.md"
        alias_report = run_verifier_command(
            verifier_command(
                "--config",
                str(clean_contract_config),
                "--json-out",
                str(alias_json),
                "--markdown-out",
                str(alias_markdown),
                "--receipt-only",
            ),
            alias_json,
            alias_markdown,
            expect=0,
        )
        assert_no_critical(alias_report)

        # Both legacy config booleans are accepted as compatibility no-ops.
        # In particular, false cannot revive full Markdown stdout.
        for receipt_only in (True, False):
            compatibility_report = run_verify_config(
                {
                    "targets": [{"url": f"{server.base_url}/clean.html"}],
                    "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
                    "receiptOnly": receipt_only,
                },
                tmp / f"receipt-config-{str(receipt_only).lower()}",
                expect=0,
            )
            assert_no_critical(compatibility_report)

        # Full Markdown stdout is compatibility behavior for an attended human
        # terminal and must require the explicit human-only flag.
        human_dir = tmp / "human-readable-stdout"
        human_json = human_dir / "report.json"
        human_markdown = human_dir / "report.md"
        human = subprocess.run(
            verifier_command(
                "--config",
                str(clean_contract_config),
                "--json-out",
                str(human_json),
                "--markdown-out",
                str(human_markdown),
                "--human-readable-stdout",
            ),
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=TIMEOUT_SECONDS,
            env=verifier_env(),
        )
        if human.returncode != 0 or human.stderr.strip() or "# Formal Web UI Verification Report" not in human.stdout:
            raise AssertionError("Explicit human-readable stdout mode must print the full Markdown report")
        assert_complete_artifacts(human_json, human_markdown, expect=0)

        # Configuration/setup exit 2 remains artifact-first. Explicit output
        # paths are honored when they are valid and the receipt stays bounded.
        setup_dir = tmp / "setup-failure"
        setup_json = setup_dir / "report.json"
        setup_markdown = setup_dir / "report.md"
        setup_report = run_verifier_command(
            verifier_command(
                "--url",
                f"{server.base_url}/clean.html",
                "--json-out",
                str(setup_json),
                "--markdown-out",
                str(setup_markdown),
                "--fail-on",
                "not-a-severity",
            ),
            setup_json,
            setup_markdown,
            expect=2,
        )
        if "Invalid failOn severity" not in setup_report.get("error", {}).get("message", ""):
            raise AssertionError("Setup-failure JSON artifact must preserve the actionable configuration error")

        bad_receipt_config = run_verify_config(
            {
                "targets": [{"url": f"{server.base_url}/clean.html"}],
                "receiptOnly": "yes",
            },
            tmp / "receipt-config-invalid",
            expect=2,
        )
        if "receiptOnly must be a boolean" not in bad_receipt_config.get("error", {}).get("message", ""):
            raise AssertionError("Non-boolean receiptOnly config must fail through the artifact-first setup contract")

        # Even a parse failure with no caller paths gets a machine-readable
        # failure artifact in the software-owned external directory.
        no_path_failure = subprocess.run(
            verifier_command("--unknown-option"),
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=TIMEOUT_SECONDS,
            env=default_env,
        )
        if no_path_failure.returncode != 2 or no_path_failure.stderr.strip():
            raise AssertionError(f"No-path setup failure was not bounded: {no_path_failure.stderr}")
        no_path_receipt = parse_bounded_receipt(no_path_failure.stdout, expect=2)
        no_path_json, no_path_markdown = receipt_artifact_paths(no_path_receipt)
        assert_complete_artifacts(no_path_json, no_path_markdown, expect=2)

        # Coverage recall: an explicit target that cannot be checked must fail
        # the run instead of producing a successful zero-page report.
        dead_target = run_verify("http://127.0.0.1:9/", tmp / "dead-target", expect=3)
        if not dead_target.get("coverage", {}).get("failed"):
            raise AssertionError("Dead explicit target should record a coverage failure")

        not_found = run_verify(f"{server.base_url}/missing-route.html", tmp / "not-found", expect=3)
        if not not_found.get("coverage", {}).get("failed"):
            raise AssertionError("404 explicit target should record a coverage failure")

        write(fixtures / "plain.txt", "not html\n")
        non_html = run_verify(f"{server.base_url}/plain.txt", tmp / "non-html", expect=3)
        if not non_html.get("coverage", {}).get("failed"):
            raise AssertionError("Non-HTML explicit target should record a coverage failure")

        redirect_server = DynamicServer(RedirectToSignInHandler).start()
        try:
            redirected = run_verify(
                f"{redirect_server.base_url}/dashboard",
                tmp / "redirected-to-sign-in",
                expect=3,
            )
            redirected_page = redirected.get("pages", [{}])[0]
            if (
                redirected_page.get("outcome") != "route_mismatch"
                or redirected_page.get("requestedPath") != "/dashboard"
                or redirected_page.get("finalPath") != "/sign-in"
                or redirected.get("coverage", {}).get("checkedPages") != 0
            ):
                raise AssertionError(f"Sign-in redirect counted as checked coverage: {redirected_page}")
        finally:
            redirect_server.close()

        binding_server = DynamicServer(SourceBindingHandler).start()
        try:
            binding_config = {
                "targets": [{
                    "url": f"{binding_server.base_url}/bound",
                    "sourceBinding": {"expected": SourceBindingHandler.revision},
                }],
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
                "maxPageCount": 1,
            }
            binding_match = run_verify_config(
                binding_config,
                tmp / "source-binding-match",
                expect=0,
            )
            binding_page = binding_match.get("pages", [{}])[0]
            if binding_page.get("sourceBinding", {}).get("status") != "matched":
                raise AssertionError(f"Matching deployment/source binding was not recorded: {binding_page}")
            if binding_page.get("requestedPath") != "/bound" or binding_page.get("finalPath") != "/bound":
                raise AssertionError("Matching page omitted requested/final path evidence")

            binding_repeat = run_verify_config(
                binding_config,
                tmp / "source-binding-match-repeat",
                expect=0,
            )
            evidence = binding_match.get("evidence", {})
            verifier_hash = evidence.get("verifier", {}).get("sha256", "")
            config_hash = evidence.get("config", {}).get("sha256", "")
            expected_verifier_hash = hashlib.sha256(VERIFY.read_bytes()).hexdigest()
            if verifier_hash != expected_verifier_hash or len(config_hash) != 64:
                raise AssertionError(f"Verifier/config hashes are incomplete: {evidence}")
            if config_hash != binding_repeat.get("evidence", {}).get("config", {}).get("sha256"):
                raise AssertionError("Equivalent effective configs did not produce a stable hash")
            if not binding_match.get("startedAt") or not binding_match.get("endedAt"):
                raise AssertionError("Run start/end times are missing")
            if binding_match.get("durationMs", -1) < 0 or binding_page.get("durationMs", -1) < 0:
                raise AssertionError("Run/cell durations must be non-negative")
            if binding_match.get("plan", {}).get("widthCoverage") != "sampled-only":
                raise AssertionError("Plan omitted sampled-only width coverage")
            binding_markdown = (tmp / "source-binding-match" / "report.md").read_text(encoding="utf-8")
            if "## Evidence Identity" not in binding_markdown or "Requested path" not in binding_markdown:
                raise AssertionError("Markdown report omitted evidence identity or exact path cells")

            cache_root = tmp / "formal-cell-cache"
            cache_root.mkdir()
            cached_config = {
                **binding_config,
                "development": {
                    "cache": {
                        "directory": str(cache_root),
                        "dataRevision": "fixture-data-v1",
                    }
                },
            }
            before_cache_requests = SourceBindingHandler.request_count
            cache_first = run_verify_config(cached_config, tmp / "cache-first", expect=0)
            first_cache = cache_first.get("pages", [{}])[0].get("cache", {})
            if not first_cache.get("write", {}).get("written") or cache_first.get("coverage", {}).get("readinessEligible"):
                raise AssertionError(f"First development cache run did not write an ineligible exact entry: {first_cache}")
            after_first_requests = SourceBindingHandler.request_count
            cache_second = run_verify_config(cached_config, tmp / "cache-second", expect=0)
            second_cache = cache_second.get("pages", [{}])[0].get("cache", {})
            if not second_cache.get("hit") or SourceBindingHandler.request_count != after_first_requests:
                raise AssertionError(f"Exact cache hit still navigated or was not reused: {second_cache}")
            if after_first_requests <= before_cache_requests:
                raise AssertionError("First cache run did not execute the real browser cell")
            if not cache_second.get("pages", [{}])[0].get("screenshots", {}).get("fullPage", {}).get("path"):
                raise AssertionError("Cache hit did not restore full screenshot evidence")

            cache_key = second_cache.get("key")
            manifest = cache_root / "v1" / cache_key[:2] / cache_key / "manifest.json"
            manifest.write_text("{\"corrupt\":true}\n", encoding="utf-8")
            requests_before_corrupt = SourceBindingHandler.request_count
            cache_corrupt = run_verify_config(cached_config, tmp / "cache-corrupt", expect=0)
            corrupt_cache = cache_corrupt.get("pages", [{}])[0].get("cache", {})
            if corrupt_cache.get("hit") or SourceBindingHandler.request_count <= requests_before_corrupt:
                raise AssertionError(f"Corrupt cache entry was reused: {corrupt_cache}")
            if not str(corrupt_cache.get("reason", "")).startswith("rejected:"):
                raise AssertionError(f"Corrupt cache rejection was not explicit: {corrupt_cache}")

            changed_data = {
                **cached_config,
                "development": {
                    "cache": {
                        "directory": str(cache_root),
                        "dataRevision": "fixture-data-v2",
                    }
                },
            }
            changed_data_report = run_verify_config(changed_data, tmp / "cache-data-changed", expect=0)
            if changed_data_report.get("pages", [{}])[0].get("cache", {}).get("hit"):
                raise AssertionError("Changed fixture/data revision reused a stale cache cell")

            changed_viewport = {
                **cached_config,
                "viewports": [{"name": "different", "width": 1024, "height": 700}],
            }
            changed_viewport_report = run_verify_config(
                changed_viewport,
                tmp / "cache-viewport-changed",
                expect=0,
            )
            if changed_viewport_report.get("pages", [{}])[0].get("cache", {}).get("hit"):
                raise AssertionError("Changed viewport reused a stale cache cell")

            symlink_cache_root = tmp / "formal-cell-cache-symlink"
            symlink_cache_root.mkdir()
            symlink_config = {
                **binding_config,
                "development": {
                    "cache": {
                        "directory": str(symlink_cache_root),
                        "dataRevision": "fixture-data-v1",
                    }
                },
            }
            symlink_first = run_verify_config(symlink_config, tmp / "cache-symlink-first", expect=0)
            symlink_key = symlink_first.get("pages", [{}])[0].get("cache", {}).get("write", {}).get("key")
            symlink_manifest = symlink_cache_root / "v1" / symlink_key[:2] / symlink_key / "manifest.json"
            outside_manifest = tmp / "outside-cache-manifest.json"
            outside_manifest.write_text("{}\n", encoding="utf-8")
            symlink_manifest.unlink()
            symlink_manifest.symlink_to(outside_manifest)
            symlink_second = run_verify_config(symlink_config, tmp / "cache-symlink-second", expect=0)
            symlink_result = symlink_second.get("pages", [{}])[0].get("cache", {})
            if symlink_result.get("hit") or "symlink" not in symlink_result.get("reason", ""):
                raise AssertionError(f"Symlinked cache evidence was not rejected: {symlink_result}")

            stale_config = {
                **binding_config,
                "targets": [{
                    "url": f"{binding_server.base_url}/bound",
                    "sourceBinding": {"expected": "newer-source-revision"},
                }],
            }
            stale = run_verify_config(stale_config, tmp / "source-binding-stale", expect=3)
            stale_page = stale.get("pages", [{}])[0]
            if (
                stale_page.get("outcome") != "stale_deployment"
                or stale_page.get("sourceBinding", {}).get("status") != "mismatched"
                or stale.get("coverage", {}).get("checkedPages") != 0
            ):
                raise AssertionError(f"Stale deployment counted as checked coverage: {stale_page}")
        finally:
            binding_server.close()

        missing_binding = run_verify_config(
            {
                "targets": [{
                    "url": f"{server.base_url}/clean.html",
                    "sourceBinding": {"expected": "required-revision"},
                }],
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "source-binding-missing",
            expect=3,
        )
        if missing_binding.get("pages", [{}])[0].get("outcome") != "source_binding_missing":
            raise AssertionError("A missing required deployment binding did not fail coverage")

        meta_binding = run_verify_config(
            {
                "targets": [{
                    "url": f"{server.base_url}/source-binding-meta.html",
                    "sourceBinding": {"expected": "meta-deployed-revision"},
                }],
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "source-binding-meta",
            expect=0,
        )
        meta_evidence = meta_binding.get("pages", [{}])[0].get("sourceBinding", {})
        if meta_evidence.get("status") != "matched" or meta_evidence.get("observedFrom") != "meta:ui-source-revision":
            raise AssertionError(f"Meta deployment/source binding fallback was not exercised: {meta_evidence}")

        allowed_missing = run_verify_config(
            {
                "targets": [
                    {"url": f"{server.base_url}/clean.html"},
                    {"url": f"{server.base_url}/optional.html", "allowFailure": "optional route is absent in this fixture"},
                ],
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "allowed-missing",
            expect=0,
        )
        if allowed_missing.get("coverage", {}).get("failed") or len(allowed_missing.get("coverage", {}).get("tolerated", [])) != 1:
            raise AssertionError("A reasoned optional-target exemption should remain reported without failing a checked run")

        fake_coordinator = tmp / "fake-coordinator.py"
        write(
            fake_coordinator,
            "#!/usr/bin/env python3\n"
            "import json\n"
            "print(json.dumps({'protocol': 2, 'id': 'formal-ui-self-test', 'ok': True, 'data': {'deployments': [{"
            "'name': 'fixture', 'repository_name': 'fixture', "
            "'route_port': 9, 'state': 'running'}]}}))\n",
        )
        fake_coordinator.chmod(0o755)
        discovered_default = run_verify_config(
            {
                "fromCoordinator": True,
                "coordinatorCommand": str(fake_coordinator),
                "minCheckedPages": 0,
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "discovered-default",
            expect=3,
        )
        if not discovered_default.get("coverage", {}).get("failed"):
            raise AssertionError("Coordinator-discovered failures should fail unless explicitly tolerated")
        discovered_tolerated = run_verify_config(
            {
                "fromCoordinator": True,
                "coordinatorCommand": str(fake_coordinator),
                "allowDiscoveredTargetFailures": True,
                "minCheckedPages": 0,
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "discovered-tolerated",
            expect=0,
        )
        if discovered_tolerated.get("coverage", {}).get("failed") or not discovered_tolerated.get("coverage", {}).get("tolerated"):
            raise AssertionError("Explicitly tolerated discovered failures should stay visible without failing")

        clean = run_verify(f"{server.base_url}/clean.html", tmp / "clean", expect=0)
        assert_no_critical(clean)

        wait_for_config = run_verify_config(
            {
                "targets": [{"url": f"{server.base_url}/clean.html"}],
                "viewports": [{"name": "mobile", "width": 390, "height": 844}],
                "waitFor": {"selector": "main", "networkIdleMs": 500, "settleMs": 100},
                "rules": {"failOn": "critical"},
            },
            tmp / "wait-for-config",
            expect=0,
        )
        if not any(not page_report.get("skipped") for page_report in wait_for_config.get("pages", [])):
            raise AssertionError("Structured waitFor config skipped every page")
        assert_no_critical(wait_for_config)

        narrow_not_device = run_verify_config(
            {
                "targets": [{"url": f"{server.base_url}/device-responsive.html"}],
                "viewports": [{"name": "narrow-browser", "width": 390, "height": 844}],
            },
            tmp / "narrow-not-device",
            expect=0,
        )
        assert_no_rule(narrow_not_device, "clipped-x")
        mobile_descriptor = run_verify_config(
            {
                "targets": [{"url": f"{server.base_url}/device-responsive.html"}],
                "viewports": [{"name": "iphone", "device": "iPhone 13"}],
            },
            tmp / "mobile-device-descriptor",
            expect=1,
        )
        assert_critical_rule(mobile_descriptor, "clipped-x")

        interaction_state = run_verify_config(
            {
                "targets": [{
                    "url": f"{server.base_url}/interaction-states.html",
                    "states": [{
                        "name": "actions-open",
                        "actions": [{"action": "click", "selector": "#open-bad"}],
                        "waitFor": {"selector": "#bad-panel:not([hidden])", "settleMs": 25},
                    }],
                }],
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "interaction-state",
            expect=1,
        )
        pages_by_state = {page_report.get("target", {}).get("stateName"): page_report for page_report in interaction_state.get("pages", [])}
        if set(pages_by_state) != {"base", "actions-open"}:
            raise AssertionError(f"Expected independently checked base/open states, got {pages_by_state.keys()}")
        if any(item.get("rule") == "clipped-x" for item in pages_by_state["base"].get("findings", [])):
            raise AssertionError("Hidden base-state content must not be reported as a visible clip")
        if not any(item.get("rule") == "clipped-x" for item in pages_by_state["actions-open"].get("findings", [])):
            raise AssertionError("Opened transient state clip was not detected")
        if any("actions" in page_report.get("target", {}) for page_report in interaction_state.get("pages", [])):
            raise AssertionError("Interaction action payloads must not leak into public reports")

        same_url_distinct_states = run_verify_config(
            {
                "targets": [
                    {
                        "name": "base-only",
                        "url": f"{server.base_url}/interaction-states.html",
                    },
                    {
                        "name": "opened-only",
                        "url": f"{server.base_url}/interaction-states.html",
                        "includeBase": False,
                        "states": [{
                            "name": "actions-open",
                            "actions": [{"action": "click", "selector": "#open-bad"}],
                            "waitFor": {"selector": "#bad-panel:not([hidden])", "settleMs": 25},
                        }],
                    },
                ],
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "same-url-distinct-states",
            expect=1,
        )
        same_url_pages = same_url_distinct_states.get("pages", [])
        same_url_states = [page_report.get("target", {}).get("stateName") for page_report in same_url_pages]
        if len(same_url_pages) != 2 or set(same_url_states) != {"base", "actions-open"}:
            raise AssertionError(
                "Distinct target configurations for the same URL must preserve both base and interaction states; "
                f"got {same_url_states}"
            )

        clean_interaction_state = run_verify_config(
            {
                "targets": [{
                    "url": f"{server.base_url}/interaction-states.html",
                    "includeBase": False,
                    "states": [{
                        "name": "help-open",
                        "actions": [{"action": "click", "selector": "#open-good"}],
                    }],
                }],
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "clean-interaction-state",
            expect=0,
        )
        assert_no_critical(clean_interaction_state)

        failed_interaction_state = run_verify_config(
            {
                "targets": [{
                    "url": f"{server.base_url}/interaction-states.html",
                    "includeBase": False,
                    "states": [{
                        "name": "missing-trigger",
                        "actions": [{
                            "action": "fill",
                            "selector": "#does-not-exist",
                            "value": "SECRET_ACTION_VALUE_MUST_NOT_LEAK",
                            "timeoutMs": 100,
                        }],
                    }],
                }],
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "failed-interaction-state",
            expect=3,
        )
        if not failed_interaction_state.get("coverage", {}).get("failed"):
            raise AssertionError("A failed required interaction state must fail the coverage gate")
        if "SECRET_ACTION_VALUE_MUST_NOT_LEAK" in json.dumps(failed_interaction_state):
            raise AssertionError("Failed interaction report leaked an action value")
        failed_timings = failed_interaction_state.get("pages", [{}])[0].get("actionTimings", [])
        if not failed_timings or failed_timings[0].get("durationMs", 1000) >= 500:
            raise AssertionError(f"Unowned absent control incurred a locator timeout: {failed_timings}")

        ownership = run_verify_config(
            {
                "targets": [{
                    "url": f"{server.base_url}/conditional-ownership.html",
                    "includeBase": False,
                    "journeys": [
                        {"id": "general", "frequencyPercent": 95, "risk": "normal"},
                        {"id": "special", "frequencyPercent": 5, "risk": "normal"},
                    ],
                    "primaryJourney": "general",
                    "regions": [{"selector": "main", "role": "primary-content", "journey": "general"}],
                    "states": [
                        {
                            "name": "general-conditional",
                            "actions": [{
                                "action": "click",
                                "selector": "#special",
                                "ownerJourney": "special",
                                "ownerState": "specialized",
                            }],
                        },
                        {
                            "name": "specialized",
                            "primaryJourney": "special",
                            "priorityOverrideReason": "Specialized ownership fixture",
                            "regions": [{"selector": "main", "role": "primary-content", "journey": "special"}],
                            "actions": [
                                {"action": "click", "selector": "#show-special"},
                                {
                                    "action": "click",
                                    "selector": "#special",
                                    "ownerJourney": "special",
                                    "ownerState": "specialized",
                                },
                            ],
                            "continuation": {
                                "kind": "in-page",
                                "anchor": "#special-form",
                                "focusWithin": "#special-form",
                                "triggerActionIndex": 1,
                            },
                        },
                    ],
                }],
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "conditional-ownership",
            expect=0,
        )
        ownership_pages = {item.get("target", {}).get("stateName"): item for item in ownership.get("pages", [])}
        general_handoffs = ownership_pages.get("general-conditional", {}).get("handoffs", [])
        if len(general_handoffs) != 1 or general_handoffs[0].get("locatorWaitMs") != 0:
            raise AssertionError(f"Owned absence did not hand off immediately: {general_handoffs}")
        if ownership_pages.get("specialized", {}).get("outcome") != "checked":
            raise AssertionError(f"Specialized owner journey did not execute: {ownership_pages}")

        missing_owner = run_verify_config(
            {
                "targets": [{
                    "url": f"{server.base_url}/conditional-ownership.html",
                    "includeBase": False,
                    "states": [{
                        "name": "general-conditional",
                        "actions": [{
                            "action": "click",
                            "selector": "#special",
                            "ownerJourney": "fixture-primary",
                            "ownerState": "missing-owner-state",
                        }],
                    }],
                }],
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "conditional-owner-missing",
            expect=3,
        )
        if missing_owner.get("pages", [{}])[0].get("outcome") != "journey_contract_error":
            raise AssertionError("A missing specialized owner state did not fail the journey contract")

        event_readiness = run_verify_config(
            {
                "targets": [{
                    "url": f"{server.base_url}/event-readiness.html",
                    "includeBase": False,
                    "states": [{
                        "name": "event-ready",
                        "actions": [{"action": "click", "selector": "#start"}],
                        "waitFor": {
                            "selector": "#ready",
                            "errorSelector": "#error",
                            "responseUrl": "**/readback.json",
                            "renderFrames": 2,
                            "timeoutMs": 2000,
                        },
                    }],
                }],
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "event-readiness",
            expect=0,
        )
        wait_kinds = {
            item.get("kind")
            for item in event_readiness.get("pages", [{}])[0].get("waitEvidence", [])
        }
        if not {"response", "ready-or-error-dom", "render-frames"}.issubset(wait_kinds):
            raise AssertionError(f"Exact interaction readiness evidence is incomplete: {wait_kinds}")

        readiness_error = run_verify_config(
            {
                "targets": [{
                    "url": f"{server.base_url}/event-readiness.html",
                    "includeBase": False,
                    "states": [{
                        "name": "event-error",
                        "actions": [{"action": "click", "selector": "#fail"}],
                        "waitFor": {
                            "selector": "#ready",
                            "errorSelector": "#error",
                            "timeoutMs": 1000,
                        },
                    }],
                }],
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "event-readiness-error",
            expect=3,
        )
        if readiness_error.get("pages", [{}])[0].get("outcome") != "interaction_error":
            raise AssertionError("Ready-or-error DOM race did not surface the error state")

        missing_event = run_verify_config(
            {
                "targets": [{
                    "url": f"{server.base_url}/event-readiness.html",
                    "includeBase": False,
                    "states": [{
                        "name": "event-missing",
                        "actions": [{"action": "click", "selector": "#fail"}],
                        "waitFor": {"selector": "#never-ready", "timeoutMs": 150},
                    }],
                }],
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "event-readiness-missing",
            expect=3,
        )
        missing_duration = missing_event.get("pages", [{}])[0].get("durationMs", 0)
        if missing_duration < 100 or missing_duration > 1000:
            raise AssertionError(f"Missing event did not fail at its bounded outer deadline: {missing_duration} ms")

        slow_event = run_verify_config(
            {
                "targets": [{
                    "url": f"{server.base_url}/event-readiness.html",
                    "includeBase": False,
                    "states": [{
                        "name": "event-slow-valid",
                        "actions": [{"action": "click", "selector": "#slow"}],
                        "waitFor": {"selector": "#ready", "timeoutMs": 1000},
                    }],
                }],
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "event-readiness-slow",
            expect=0,
        )
        selector_waits = [
            item.get("durationMs", 0)
            for item in slow_event.get("pages", [{}])[0].get("waitEvidence", [])
            if item.get("kind") == "selector"
        ]
        if not selector_waits or selector_waits[0] < 100 or selector_waits[0] >= 900:
            raise AssertionError(f"Slow valid event did not return on arrival: {selector_waits}")

        readback = run_verify_config(
            {
                "targets": [{
                    "url": f"{server.base_url}/clean.html",
                    "waitFor": {
                        "readback": {
                            "url": f"{server.base_url}/readback.json",
                            "status": 200,
                            "jsonPath": "ready",
                            "equals": True,
                            "intervalMs": 25,
                        },
                        "timeoutMs": 1000,
                    },
                }],
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "server-readback",
            expect=0,
        )
        if "server-readback" not in {
            item.get("kind") for item in readback.get("pages", [{}])[0].get("waitEvidence", [])
        }:
            raise AssertionError("Server readback readiness was not recorded")

        excessive_delay = run_verify_config(
            {
                "targets": [{"url": f"{server.base_url}/clean.html"}],
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
                "waitFor": {"settleMs": 101},
            },
            tmp / "excessive-delay-rejected",
            expect=2,
        )
        if "must not exceed 100 ms" not in excessive_delay.get("error", {}).get("message", ""):
            raise AssertionError("A deliberate delay above 100 ms was accepted")
        invalid_performance = run_verify_config(
            {
                "targets": [{"url": f"{server.base_url}/clean.html"}],
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
                "performance": {"ttfbMs": 0, "lcpMs": 800},
            },
            tmp / "invalid-performance-threshold",
            expect=2,
        )
        if "performance.ttfbMs must be a positive number" not in invalid_performance.get("error", {}).get("message", ""):
            raise AssertionError("A non-positive rendered-performance threshold was accepted")
        verifier_source = VERIFY.read_text(encoding="utf-8")
        if "settleMs = 120" in verifier_source or "waitForTimeout(120" in verifier_source:
            raise AssertionError("The verifier retained a deliberate delay above 100 ms")

        clipped_button = run_verify(f"{server.base_url}/clipped-button.html", tmp / "clipped-button", expect=1)
        assert_rules(clipped_button, "clipped-x")

        clipped_text = run_verify(f"{server.base_url}/clipped-text.html", tmp / "clipped-text", expect=1)
        assert_rules(clipped_text, "clipped-y")

        occluded = run_verify(f"{server.base_url}/occluded.html", tmp / "occluded", expect=1)
        assert_rules(occluded, "occluded")

        outside_area = run_verify(f"{server.base_url}/outside-area.html", tmp / "outside-area", expect=1)
        assert_rules(outside_area, "outside-area")

        broken_image = run_verify(f"{server.base_url}/broken-image.html", tmp / "broken-image", expect=1)
        assert_rules(broken_image, "broken-image")

        invisible_text = run_verify(f"{server.base_url}/invisible-text.html", tmp / "invisible-text", expect=1)
        assert_rules(invisible_text, "invisible-text")

        lab_color = run_verify(f"{server.base_url}/lab-color.html", tmp / "lab-color", expect=0)
        assert_no_critical(lab_color)

        oklab_color = run_verify(f"{server.base_url}/oklab-color.html", tmp / "oklab-color", expect=0)
        assert_no_critical(oklab_color)

        allowed = run_verify(f"{server.base_url}/allowed-ellipsis.html", tmp / "allowed-ellipsis", expect=0)
        assert_no_critical(allowed)

        ignored = run_verify(f"{server.base_url}/ignored.html", tmp / "ignored", expect=0)
        assert_no_critical(ignored)

        chart = run_verify(f"{server.base_url}/chart-overflow.html", tmp / "chart-overflow", expect=0)
        assert_no_critical(chart)

        scrollbars = run_verify(f"{server.base_url}/scrollbars.html", tmp / "scrollbars", expect=0)
        require_scrollbar(scrollbars, ".scrollbox", "y", 1)
        assert_no_rule(scrollbars, "double-nested-vertical-scrollbars")
        markdown = (tmp / "scrollbars" / "report.md").read_text(encoding="utf-8")
        if (
            "## Visible Scrollbars" not in markdown
            or "Same-axis depth" not in markdown
            or "Scroll chain" not in markdown
            or ".scrollbox" not in markdown
        ):
            raise AssertionError("Markdown report did not include scroll-depth evidence")

        # Scroll topology must-catches: every horizontal path warns, same-axis
        # horizontal depth two blocks, vertical depth two warns, and vertical
        # depth three blocks.
        horizontal = run_verify(
            f"{server.base_url}/horizontal-scrollbar.html",
            tmp / "horizontal-scrollbar",
            expect=0,
        )
        assert_warning_rule(horizontal, "horizontal-scrollbar")
        assert_no_rule(horizontal, "nested-horizontal-scrollbars")
        require_scrollbar(horizontal, ".x-scroll", "x", 1)

        forced_horizontal = run_verify(
            f"{server.base_url}/forced-horizontal-scrollbar.html",
            tmp / "forced-horizontal-scrollbar",
            expect=0,
        )
        assert_warning_rule(forced_horizontal, "horizontal-scrollbar")
        forced_entry = require_scrollbar(forced_horizontal, ".forced-x", "x", 1)
        if forced_entry.get("scrollWidth", 0) > forced_entry.get("clientWidth", 0) + 2:
            raise AssertionError(f"Forced-scroll fixture unexpectedly had a content overflow range: {forced_entry}")

        nested_horizontal = run_verify(
            f"{server.base_url}/nested-horizontal-scrollbars.html",
            tmp / "nested-horizontal-scrollbars",
            expect=1,
        )
        assert_warning_rule(nested_horizontal, "horizontal-scrollbar")
        assert_critical_rule(nested_horizontal, "nested-horizontal-scrollbars")
        require_scrollbar(nested_horizontal, ".outer-x", "x", 1)
        require_scrollbar(nested_horizontal, ".inner-x", "x", 2)

        double_vertical = run_verify(
            f"{server.base_url}/double-vertical-scrollbars.html",
            tmp / "double-vertical-scrollbars",
            expect=0,
        )
        assert_warning_rule(double_vertical, "double-nested-vertical-scrollbars")
        assert_no_rule(double_vertical, "triple-nested-vertical-scrollbars")
        require_scrollbar(double_vertical, ".outer-v", "y", 1)
        require_scrollbar(double_vertical, ".inner-v", "y", 2)

        triple_vertical = run_verify(
            f"{server.base_url}/triple-vertical-scrollbars.html",
            tmp / "triple-vertical-scrollbars",
            expect=1,
        )
        assert_warning_rule(triple_vertical, "double-nested-vertical-scrollbars")
        assert_critical_rule(triple_vertical, "triple-nested-vertical-scrollbars")
        require_scrollbar(triple_vertical, ".middle-v", "y", 2)
        require_scrollbar(triple_vertical, ".inner-v", "y", 3)

        document_panel = run_verify(
            f"{server.base_url}/document-plus-vertical-panel.html",
            tmp / "document-plus-vertical-panel",
            expect=0,
        )
        assert_warning_rule(document_panel, "double-nested-vertical-scrollbars")
        panel_entry = require_scrollbar(document_panel, ".panel-v", "y", 2)
        if panel_entry.get("scrollChain", [None])[0] != "document.scrollingElement":
            raise AssertionError(f"Document scrollbar must count as the outer layer: {panel_entry}")

        # Precision guards: unlike axes do not nest, and overflow:auto without
        # an actual scroll range does not invent a scrollbar.
        mixed_axes = run_verify(
            f"{server.base_url}/mixed-axis-scrollbars.html",
            tmp / "mixed-axis-scrollbars",
            expect=0,
        )
        assert_warning_rule(mixed_axes, "horizontal-scrollbar")
        assert_no_rule(mixed_axes, "nested-horizontal-scrollbars")
        assert_no_rule(mixed_axes, "double-nested-vertical-scrollbars")
        assert_no_rule(mixed_axes, "triple-nested-vertical-scrollbars")
        require_scrollbar(mixed_axes, ".outer-y", "y", 1)
        require_scrollbar(mixed_axes, ".inner-x", "x", 1)

        nonoverflowing_auto = run_verify(
            f"{server.base_url}/nonoverflowing-auto.html",
            tmp / "nonoverflowing-auto",
            expect=0,
        )
        assert_no_rule(nonoverflowing_auto, "horizontal-scrollbar")
        if any(item.get("selector", "").endswith(".fits-auto") for item in visible_scrollbars(nonoverflowing_auto)):
            raise AssertionError("Non-overflowing auto container was incorrectly inventoried as a scrollbar")

        # Fix 2: gradient background must not yield an invisible-text critical and must be
        # recorded as unmeasurable contrast.
        gradient = run_verify(f"{server.base_url}/gradient-contrast.html", tmp / "gradient-contrast", expect=0)
        assert_no_critical(gradient)
        assert_no_rule(gradient, "invisible-text")
        unmeasurable = [
            entry
            for metrics in page_metrics(gradient)
            for entry in metrics.get("unmeasurableContrast", [])
        ]
        if not any("gradient" in (entry.get("reason") or "") for entry in unmeasurable):
            raise AssertionError(f"Expected gradient entry in unmeasurableContrast, got {unmeasurable}")

        # Reachable open-shadow and iframe content is part of coverage and both
        # clipped controls must be caught.
        shadow = run_verify(f"{server.base_url}/shadow-iframe.html", tmp / "shadow-iframe", expect=1)
        clipped_contexts = [item for item in shadow.get("findings", []) if item.get("rule") == "clipped-x"]
        if len(clipped_contexts) < 2:
            raise AssertionError(f"Expected clipped controls from shadow root and iframe, got {clipped_contexts}")
        framed_scrollbars = [
            item
            for item in visible_scrollbars(shadow)
            if ".frame-x" in item.get("selector", "")
        ]
        if (
            len(framed_scrollbars) != 1
            or framed_scrollbars[0].get("sameAxisDepth") != 1
            or not framed_scrollbars[0].get("scrollChain", [""])[0].startswith("[frame ")
        ):
            raise AssertionError(f"Iframe horizontal scrollbar lost its prefixed depth evidence: {framed_scrollbars}")
        framed_warnings = [
            item
            for item in shadow.get("findings", [])
            if item.get("rule") == "horizontal-scrollbar" and item.get("selector", "").startswith("[frame ")
        ]
        if len(framed_warnings) != 1:
            raise AssertionError(f"Iframe horizontal scrollbar did not produce one warning: {framed_warnings}")
        not_inspected_totals = [m.get("notInspected", {}) for m in page_metrics(shadow)]
        if any(ni.get("openShadowRoots", 0) for ni in not_inspected_totals):
            raise AssertionError(f"Reachable open shadow roots should be inspected, got {not_inspected_totals}")
        if any(ni.get("iframes", 0) for ni in not_inspected_totals):
            raise AssertionError(f"Reachable iframes should be inspected, got {not_inspected_totals}")

        # Fix 1: below-the-fold lazy content is only found when scrolling is on.
        lazy_scroll_on = run_verify(f"{server.base_url}/lazy-scroll.html", tmp / "lazy-scroll-on", expect=1)
        assert_rules(lazy_scroll_on, "clipped-x")
        scroll_metrics = [m.get("scroll", {}) for m in page_metrics(lazy_scroll_on)]
        if not any((sm.get("scrolledTo") or 0) > 844 for sm in scroll_metrics):
            raise AssertionError(f"Expected scroll pass to advance beyond one viewport, got {scroll_metrics}")

        lazy_scroll_off = run_verify(
            f"{server.base_url}/lazy-scroll.html", tmp / "lazy-scroll-off", expect=0, extra=["--no-scroll"]
        )
        assert_no_critical(lazy_scroll_off)
        assert_no_rule(lazy_scroll_off, "clipped-x")

        # Realistic-breakage regressions (previously all reported zero findings).
        div_clip = run_verify(f"{server.base_url}/div-clipped-ancestor.html", tmp / "div-clipped-ancestor", expect=1)
        assert_critical_rule(div_clip, "clipped-by-ancestor")

        abs_cut = run_verify(f"{server.base_url}/abs-button-cut.html", tmp / "abs-button-cut", expect=1)
        assert_critical_rule(abs_cut, "clipped-by-ancestor")

        neg_cut = run_verify(f"{server.base_url}/negative-top-cut.html", tmp / "negative-top-cut", expect=1)
        assert_critical_rule(neg_cut, "clipped-by-ancestor")

        overlap = run_verify(f"{server.base_url}/partial-overlap.html", tmp / "partial-overlap", expect=1)
        assert_critical_rule(overlap, "partially-occluded")

        collapsed_img = run_verify(f"{server.base_url}/broken-image-collapsed.html", tmp / "broken-image-collapsed", expect=1)
        assert_critical_rule(collapsed_img, "broken-image")

        roadmap = run_verify(f"{server.base_url}/roadmap-invisible.html", tmp / "roadmap-invisible", expect=1)
        assert_critical_rule(roadmap, "invisible-text")

        div_invisible = run_verify(f"{server.base_url}/div-invisible-text.html", tmp / "div-invisible-text", expect=1)
        assert_critical_rule(div_invisible, "invisible-text")

        offcanvas = run_verify(f"{server.base_url}/offcanvas-left-cut.html", tmp / "offcanvas-left-cut", expect=1)
        assert_critical_rule(offcanvas, "offcanvas-cut")

        fixed_cut = run_verify(f"{server.base_url}/fixed-toolbar-cut.html", tmp / "fixed-toolbar-cut", expect=1)
        assert_critical_rule(fixed_cut, "fixed-offscreen-cut")

        nowrap_spill = run_verify(f"{server.base_url}/nowrap-spill-cut.html", tmp / "nowrap-spill-cut", expect=1)
        assert_critical_rule(nowrap_spill, "clipped-by-ancestor")

        modern_clean = run_verify(f"{server.base_url}/modern-clean.html", tmp / "modern-clean", expect=0)
        assert_no_critical(modern_clean)

        # False-positive guards: intentional patterns stay below critical.
        truncate_span = run_verify(f"{server.base_url}/truncate-inner-span.html", tmp / "truncate-inner-span", expect=0)
        assert_no_critical(truncate_span)
        truncations = [
            entry
            for metrics in page_metrics(truncate_span)
            for entry in metrics.get("ellipsisTruncations", [])
        ]
        if not any("ellipsis" in (entry.get("kind") or "") for entry in truncations):
            raise AssertionError(f"Expected ellipsis entry in ellipsisTruncations, got {truncations}")

        clamp = run_verify(f"{server.base_url}/line-clamp.html", tmp / "line-clamp", expect=0)
        assert_no_critical(clamp)

        carousel = run_verify(f"{server.base_url}/carousel-peek.html", tmp / "carousel-peek", expect=0)
        assert_no_critical(carousel)

        app_shell = run_verify(f"{server.base_url}/app-shell-scroll.html", tmp / "app-shell-scroll", expect=0)
        assert_no_critical(app_shell)

        skip_link = run_verify(f"{server.base_url}/skip-link.html", tmp / "skip-link", expect=0)
        assert_no_critical(skip_link)
        assert_warning_rule(skip_link, "offcanvas-hidden")

        fab = run_verify(f"{server.base_url}/fab-over-text.html", tmp / "fab-over-text", expect=0)
        assert_no_critical(fab)

        accordion = run_verify(f"{server.base_url}/accordion-closed.html", tmp / "accordion-closed", expect=0)
        assert_no_critical(accordion)
        assert_warning_rule(accordion, "clipped-hidden")

        form = run_verify(f"{server.base_url}/form-controls.html", tmp / "form-controls", expect=0)
        assert_no_critical(form)

        # Native control text is not represented by DOM text geometry. Measure
        # placeholders and selected labels against the true inner content width,
        # but never persist the text being measured or a typed value.
        placeholder = run_verify(
            f"{server.base_url}/native-placeholder-clipped.html",
            tmp / "native-placeholder-clipped",
            expect=1,
        )
        assert_critical_rule(placeholder, "control-text-clipped")
        placeholder_blob = json.dumps(placeholder)
        if "PRIVATE_PLACEHOLDER_MUST_NOT_LEAK" in placeholder_blob:
            raise AssertionError("Clipped placeholder text leaked into the privacy-safe report")
        placeholder_findings = [
            item for item in placeholder.get("findings", []) if item.get("rule") == "control-text-clipped"
        ]
        if not placeholder_findings or any(item.get("textSnippet") for item in placeholder_findings):
            raise AssertionError("control-text-clipped findings must redact their text snippet")
        if not all(
            item.get("evidence", {}).get("measuredTextWidth", 0)
            > item.get("evidence", {}).get("availableInnerWidth", 0)
            for item in placeholder_findings
        ):
            raise AssertionError("Placeholder finding omitted real inner-width evidence")

        selected = run_verify(
            f"{server.base_url}/native-select-clipped.html",
            tmp / "native-select-clipped",
            expect=1,
        )
        assert_critical_rule(selected, "control-text-clipped")
        if "PRIVATE_SELECTED_LABEL_MUST_NOT_LEAK" in json.dumps(selected):
            raise AssertionError("Selected option label leaked into the privacy-safe report")
        selected_evidence = [
            item.get("evidence", {})
            for item in selected.get("findings", [])
            if item.get("rule") == "control-text-clipped"
        ]
        if not any(
            evidence.get("controlTextKind") == "selected-option"
            and evidence.get("nativeAffordanceWidth", 0) > 0
            for evidence in selected_evidence
        ):
            raise AssertionError("Selected option measurement did not account for the native select affordance")

        typed = run_verify(
            f"{server.base_url}/native-typed-scrollable.html",
            tmp / "native-typed-scrollable",
            expect=0,
        )
        assert_no_critical(typed)
        assert_no_rule(typed, "control-text-clipped")
        if "PRIVATE_TYPED_VALUE_MUST_NOT_LEAK" in json.dumps(typed):
            raise AssertionError("A typed, scrollable native control value leaked into the report")

        allowed_control = run_verify(
            f"{server.base_url}/native-placeholder-allowed.html",
            tmp / "native-placeholder-allowed",
            expect=0,
        )
        assert_no_critical(allowed_control)
        assert_no_rule(allowed_control, "control-text-clipped")
        assert_warning_rule(allowed_control, "allowed-truncation")
        if "PRIVATE_ALLOWED_PLACEHOLDER_MUST_NOT_LEAK" in json.dumps(allowed_control):
            raise AssertionError("Allowed placeholder text leaked into the report")

        inset_bad = run_verify(
            f"{server.base_url}/content-inset-bad.html",
            tmp / "content-inset-bad",
            expect=1,
        )
        assert_critical_rule(inset_bad, "content-inset-below-minimum")
        inset_clean = run_verify_config(
            {
                "targets": [{
                    "url": f"{server.base_url}/content-inset-clean.html",
                    "contentInsets": [{"selector": ".card", "min": 12, "name": "items card"}],
                }],
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "content-inset-clean",
            expect=0,
        )
        assert_no_critical(inset_clean)
        inset_entries = [
            entry
            for metrics in page_metrics(inset_clean)
            for entry in metrics.get("contentInsetMeasurements", [])
        ]
        if not any(entry.get("status") == "passed" and entry.get("requiredInset") == 12 for entry in inset_entries):
            raise AssertionError(f"Configured content inset was not measured as passing: {inset_entries}")
        intentional_edges = run_verify(
            f"{server.base_url}/content-inset-intentional-edges.html",
            tmp / "content-inset-intentional-edges",
            expect=0,
        )
        assert_no_critical(intentional_edges)
        assert_no_rule(intentional_edges, "content-inset-below-minimum")

        breakpoint_config = {
            "targets": [
                {
                    "name": "breakpoint edge",
                    "url": f"{server.base_url}/breakpoint-edge.html",
                    "breakpointProfile": {
                        "name": "navigation",
                        "breakpoints": [768],
                        "height": 800,
                    },
                },
                {"name": "plain target", "url": f"{server.base_url}/clean.html"},
            ],
            "viewports": [{"name": "configured-768", "width": 768, "height": 800}],
            "maxPageCount": 4,
        }
        breakpoint_report = run_verify_config(
            breakpoint_config,
            tmp / "breakpoint-profile",
            expect=1,
        )
        edge_pages = [
            item for item in breakpoint_report.get("pages", [])
            if item.get("target", {}).get("name") == "breakpoint edge"
        ]
        plain_pages = [
            item for item in breakpoint_report.get("pages", [])
            if item.get("target", {}).get("name") == "plain target"
        ]
        if sorted(page_item.get("viewport", {}).get("width") for page_item in edge_pages) != [767, 768, 769]:
            raise AssertionError(f"Breakpoint profile did not generate exact boundary widths: {edge_pages}")
        if len(plain_pages) != 1 or plain_pages[0].get("viewport", {}).get("width") != 768:
            raise AssertionError("Breakpoint profile leaked onto a target that did not declare it")
        if not any(
            page_item.get("viewport", {}).get("width") == 767
            and any(finding.get("rule") == "clipped-x" for finding in page_item.get("findings", []))
            for page_item in edge_pages
        ):
            raise AssertionError("The breakpoint-minus-one must-catch defect was not detected")
        exact_cells = breakpoint_report.get("coverage", {}).get("cells", [])
        if len(exact_cells) != 4 or any(not cell.get("cellId") for cell in exact_cells):
            raise AssertionError(f"Coverage omitted exact route/state/viewport cells: {exact_cells}")
        if breakpoint_report.get("coverage", {}).get("widthCoverageMode") != "sampled-only":
            raise AssertionError("Width coverage must be labelled sampled-only")
        exact_viewport = next(
            page_item.get("viewport", {})
            for page_item in edge_pages
            if page_item.get("viewport", {}).get("width") == 768
        )
        if set(exact_viewport.get("sampling", {}).get("sources", [])) != {"configured", "breakpoint-profile"}:
            raise AssertionError("Equivalent configured/breakpoint cells were not deterministically de-duplicated")

        over_budget = {**breakpoint_config, "maxPageCount": 3}
        budget_failure = run_verify_config(over_budget, tmp / "breakpoint-budget", expect=2)
        if "exceeding maxPageCount 3" not in budget_failure.get("error", {}).get("message", ""):
            raise AssertionError("Breakpoint expansion did not fail closed at the hard page-count budget")

        # --cookie: the gated fixture is broken for anonymous visitors and clean
        # with the session cookie, proving the cookie reaches the page (recall
        # both ways: the anon run must still catch the critical).
        cookie_server = DynamicServer(CookieGateHandler).start()
        try:
            cookie_anon = run_verify(f"{cookie_server.base_url}/gated.html", tmp / "cookie-anon", expect=1)
            assert_critical_rule(cookie_anon, "invisible-text")
            cookie_authed = run_verify(
                f"{cookie_server.base_url}/gated.html",
                tmp / "cookie-authed",
                expect=0,
                extra=["--cookie", "sess=ok"],
            )
            assert_no_critical(cookie_authed)
            if not page_metrics(cookie_authed):
                raise AssertionError("Cookie-authenticated page was skipped")

            # Config-object cookie form with domain/path scoping: must travel
            # through normalizeCookieList's object branch AND the addCookies
            # domain/path plumbing (not the url fallback) and still reach the
            # gated page — recall, not just parse acceptance.
            cookie_scoped = run_verify_config(
                {
                    "targets": [{"url": f"{cookie_server.base_url}/gated.html"}],
                    "cookies": [{"name": "sess", "value": "ok", "domain": "127.0.0.1", "path": "/"}],
                },
                tmp / "cookie-config-scoped",
                expect=0,
            )
            assert_no_critical(cookie_scoped)
            if not page_metrics(cookie_scoped):
                raise AssertionError("Config-scoped cookie page was skipped")

            # Malformed scoping fields must fail fast with the validator's
            # message in the cold setup artifact, not a downstream Playwright
            # error or a verbose process payload.
            bad_out = tmp / "cookie-config-bad"
            bad_out.mkdir(parents=True, exist_ok=True)
            bad_config = bad_out / "formal-web-ui.json"
            bad_config.write_text(
                json.dumps(
                    {
                        "targets": [{"url": f"{cookie_server.base_url}/gated.html"}],
                        "cookies": [{"name": "sess", "value": "ok", "domain": True}],
                    }
                ),
                encoding="utf-8",
            )
            bad_result = subprocess.run(
                verifier_command("--config", str(bad_config)),
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=TIMEOUT_SECONDS,
                env=verifier_env(),
            )
            if bad_result.returncode != 2 or bad_result.stderr.strip():
                raise AssertionError("Malformed cookie domain should fail with one bounded setup receipt")
            bad_receipt = parse_bounded_receipt(bad_result.stdout, expect=2)
            bad_json, bad_markdown = receipt_artifact_paths(bad_receipt)
            bad_report = assert_complete_artifacts(bad_json, bad_markdown, expect=2)
            if "domain must be a non-empty string" not in bad_report.get("error", {}).get("message", ""):
                raise AssertionError("Malformed cookie domain should preserve the validator message in the JSON artifact")
        finally:
            cookie_server.close()

        AuthReuseHandler.auth_calls = 0
        AuthReuseHandler.observations = []
        auth_server = DynamicServer(AuthReuseHandler).start()
        try:
            auth_cache_root = tmp / "auth-evidence-cache"
            auth_cache_root.mkdir()
            auth_reuse = run_verify_config(
                {
                    "authProfiles": [{
                        "name": "admin",
                        "url": f"{auth_server.base_url}/login",
                        "actions": [
                            {
                                "action": "fill",
                                "selector": "#password",
                                "value": "AUTH_SECRET_MUST_NOT_LEAK",
                            },
                            {"action": "click", "selector": "#login"},
                        ],
                        "waitFor": {
                            "responseUrl": "**/authenticate",
                            "selector": "#ready",
                            "timeoutMs": 2000,
                        },
                    }],
                    "targets": [{
                        "url": f"{auth_server.base_url}/protected",
                        "authProfile": "admin",
                        "sourceBinding": {"expected": "auth-fixture-revision"},
                        "waitFor": {"selector": "#ready", "timeoutMs": 2000},
                        "execution": {"parallelSafe": True},
                    }],
                    "execution": {"maxConcurrency": 2},
                    "development": {
                        "cache": {
                            "directory": str(auth_cache_root),
                            "dataRevision": "auth-fixture-data-v1",
                        }
                    },
                    "viewports": [
                        {"name": "mobile", "width": 390, "height": 844},
                        {"name": "desktop", "width": 1280, "height": 800},
                    ],
                },
                tmp / "auth-profile-reuse",
                expect=0,
            )
            if AuthReuseHandler.auth_calls != 1:
                raise AssertionError(f"Authentication bootstrap ran {AuthReuseHandler.auth_calls} times instead of once")
            if sorted(AuthReuseHandler.observations) != ["clean", "clean"]:
                raise AssertionError(f"Fresh contexts leaked local storage across cells: {AuthReuseHandler.observations}")
            auth_status = auth_reuse.get("authentication", [])
            if len(auth_status) != 1 or auth_status[0].get("status") != "ready":
                raise AssertionError(f"Authentication profile status is incomplete: {auth_status}")
            if "AUTH_SECRET_MUST_NOT_LEAK" in json.dumps(auth_reuse):
                raise AssertionError("Authentication action value leaked into formal artifacts")
            if any(page_report.get("outcome") != "checked" for page_report in auth_reuse.get("pages", [])):
                raise AssertionError("Authenticated cells did not complete")
            auth_manifests = list(auth_cache_root.rglob("manifest.json"))
            if len(auth_manifests) != 2:
                raise AssertionError("Authenticated successful cells did not create exact cache evidence")
            cached_auth_text = "\n".join(item.read_text(encoding="utf-8") for item in auth_manifests)
            if "AUTH_SECRET_MUST_NOT_LEAK" in cached_auth_text or '"cookies"' in cached_auth_text or '"storageState"' in cached_auth_text:
                raise AssertionError("Authentication or cookie state leaked into the development cache")

            auth_isolation = run_verify_config(
                {
                    "authProfiles": [
                        {
                            "name": "admin",
                            "url": f"{auth_server.base_url}/login",
                            "actions": [{"action": "click", "selector": "#login"}],
                            "waitFor": {"responseUrl": "**/authenticate", "selector": "#ready"},
                        },
                        {
                            "name": "broken-role",
                            "url": f"{auth_server.base_url}/login",
                            "actions": [{"action": "click", "selector": "#missing-login-control"}],
                        },
                    ],
                    "targets": [
                        {
                            "name": "good-role-target",
                            "url": f"{auth_server.base_url}/protected",
                            "authProfile": "admin",
                            "waitFor": {"selector": "#ready"},
                        },
                        {
                            "name": "bad-role-target",
                            "url": f"{auth_server.base_url}/protected",
                            "authProfile": "broken-role",
                        },
                    ],
                    "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
                },
                tmp / "auth-profile-isolation",
                expect=3,
            )
            auth_pages = {
                page_report.get("target", {}).get("name"): page_report
                for page_report in auth_isolation.get("pages", [])
            }
            if auth_pages.get("good-role-target", {}).get("outcome") != "checked":
                raise AssertionError("A failed auth profile blocked an unrelated ready profile")
            if auth_pages.get("bad-role-target", {}).get("outcome") != "auth_setup_error":
                raise AssertionError("Failed auth profile did not fail only its owned target")
        finally:
            auth_server.close()

        ConcurrencyHandler.arrivals = 0
        ConcurrencyHandler.active = 0
        ConcurrencyHandler.max_active = 0
        concurrency_server = DynamicServer(ConcurrencyHandler).start()
        try:
            parallel = run_verify_config(
                {
                    "targets": [
                        {
                            "name": f"parallel-{index}",
                            "url": f"{concurrency_server.base_url}/parallel-{index}",
                            "execution": {"parallelSafe": True},
                        }
                        for index in range(3)
                    ],
                    "execution": {"maxConcurrency": 3},
                    "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
                },
                tmp / "bounded-concurrency",
                expect=0,
            )
            if ConcurrencyHandler.max_active < 3:
                raise AssertionError(f"Independent cells did not overlap: max_active={ConcurrencyHandler.max_active}")
            execution_indices = sorted(
                page_report.get("execution", {}).get("executionIndex")
                for page_report in parallel.get("pages", [])
            )
            if execution_indices != [1, 2, 3]:
                raise AssertionError(f"Bounded scheduler execution indices are incomplete: {execution_indices}")
        finally:
            concurrency_server.close()

        locked = run_verify_config(
            {
                "targets": [
                    {
                        "name": "locked-a",
                        "url": f"{server.base_url}/clean.html",
                        "execution": {"parallelSafe": True, "resourceLocks": ["shared-account"]},
                    },
                    {
                        "name": "locked-b",
                        "url": f"{server.base_url}/clean.html",
                        "execution": {"parallelSafe": True, "resourceLocks": ["shared-account"]},
                    },
                ],
                "execution": {"maxConcurrency": 2},
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "resource-locks",
            expect=0,
        )
        locked_by_execution = sorted(
            locked.get("pages", []),
            key=lambda page_report: page_report.get("execution", {}).get("executionIndex", 0),
        )
        if locked_by_execution[1].get("startedAt", "") < locked_by_execution[0].get("endedAt", ""):
            raise AssertionError("Conflicting resource locks overlapped")

        priority = run_verify_config(
            {
                "targets": [
                    {
                        "name": "low-priority",
                        "url": f"{server.base_url}/clean.html",
                        "execution": {"priority": 1},
                    },
                    {
                        "name": "high-priority",
                        "url": f"{server.base_url}/clean.html",
                        "execution": {"priority": 100},
                    },
                ],
                "execution": {"maxConcurrency": 1},
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "priority-order",
            expect=0,
        )
        if [page_report.get("target", {}).get("name") for page_report in priority.get("pages", [])] != [
            "low-priority", "high-priority"
        ]:
            raise AssertionError("Final report order did not preserve the declared plan")
        priority_execution = {
            page_report.get("target", {}).get("name"): page_report.get("execution", {}).get("executionIndex")
            for page_report in priority.get("pages", [])
        }
        if priority_execution != {"low-priority": 2, "high-priority": 1}:
            raise AssertionError(f"High-value cell did not execute first: {priority_execution}")
        progress_rows = [
            json.loads(line)
            for line in (tmp / "priority-order" / "progress.jsonl").read_text(encoding="utf-8").splitlines()
        ]
        if progress_rows[1].get("targetName") != "high-priority":
            raise AssertionError("Bounded progress evidence did not expose the first high-priority result")

        locale_targets = []
        for locale_index in range(1, 8):
            target: dict = {
                "name": f"locale-{locale_index}",
                "url": f"{server.base_url}/clean.html",
            }
            if locale_index == 2:
                target.update({
                    "url": f"{server.base_url}/event-readiness.html",
                    "includeBase": False,
                    "states": [{
                        "name": "focus-check",
                        "actions": [
                            {"action": "click", "selector": "#start"},
                            {"action": "focus", "selector": "#missing-locale-focus"},
                        ],
                        "afterFailureWaitFor": {"selector": "#ready", "timeoutMs": 1000},
                    }],
                })
            locale_targets.append(target)
        complete_after_failure = run_verify_config(
            {
                "targets": locale_targets,
                "execution": {"maxConcurrency": 1},
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "complete-after-failure",
            expect=3,
        )
        locale_pages = complete_after_failure.get("pages", [])
        if len(locale_pages) != 7:
            raise AssertionError(f"Ordinary locale failure stopped the safe remainder: {len(locale_pages)} cells")
        if locale_pages[1].get("outcome") != "interaction_error":
            raise AssertionError("Injected locale-two focus failure was not recorded")
        after_failure = locale_pages[1].get("interactionFailure", {}).get("afterFailureObservation", {})
        if not after_failure.get("checked"):
            raise AssertionError(f"Useful downstream observation was not retained: {after_failure}")
        if any(page_report.get("outcome") != "checked" for page_report in locale_pages[2:]):
            raise AssertionError("Later locales did not continue after the ordinary failure")
        if any(page_report.get("cleanup", {}).get("status") != "completed" for page_report in locale_pages):
            raise AssertionError("Every executed locale must complete isolated-context cleanup")
        if len(complete_after_failure.get("coverage", {}).get("failures", [])) != 1:
            raise AssertionError("Ordinary failures were not returned as one combined failure list")

        unsafe_targets = json.loads(json.dumps(locale_targets))
        unsafe_targets[1]["execution"] = {
            "stopOnFailure": True,
            "stopReason": "Injected shared fixture corruption",
        }
        unsafe_stop = run_verify_config(
            {
                "targets": unsafe_targets,
                "execution": {"maxConcurrency": 1},
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "declared-unsafe-stop",
            expect=3,
        )
        unsafe_pages = unsafe_stop.get("pages", [])
        if len(unsafe_pages) != 7 or unsafe_stop.get("execution", {}).get("executedCount") != 2:
            raise AssertionError("Declared unsafe stop did not retain the full planned-cell account")
        if any(page_report.get("outcome") != "unsafe_stop_unexecuted" for page_report in unsafe_pages[2:]):
            raise AssertionError("Cells after a declared unsafe failure were not explicitly marked unexecuted")
        if "Injected shared fixture corruption" not in (unsafe_stop.get("execution", {}).get("unsafeStop") or ""):
            raise AssertionError("Unsafe-stop report omitted the declared shared-state reason")

        change_repo = tmp / "change-aware-repo"
        write(change_repo / "ui" / "a.css", ".a { color: #111; }\n")
        write(change_repo / "ui" / "b.css", ".b { color: #111; }\n")
        change_config = {
            "repoRoot": str(change_repo),
            "targets": [
                {
                    "name": "route-a",
                    "url": f"{server.base_url}/clean.html",
                    "reviewInputs": [{"path": "ui/a.css", "kind": "style"}],
                },
                {
                    "name": "route-b",
                    "url": f"{server.base_url}/clean.html",
                    "reviewInputs": [{"path": "ui/b.css", "kind": "style"}],
                },
            ],
            "development": {"changedPaths": ["ui/a.css"]},
            "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
        }
        changed_subset = run_verify_config(change_config, tmp / "changed-subset", expect=0)
        if [page_report.get("target", {}).get("name") for page_report in changed_subset.get("pages", [])] != ["route-a"]:
            raise AssertionError("Changed-input selection did not isolate the mapped target")
        selection = changed_subset.get("plan", {}).get("selection", {})
        if selection.get("fullPlanCount") != 2 or selection.get("selectedCount") != 1:
            raise AssertionError(f"Development selection counts are incomplete: {selection}")
        if changed_subset.get("coverage", {}).get("readinessEligible"):
            raise AssertionError("A changed-cell subset was presented as readiness evidence")

        unmapped_config = {
            **change_config,
            "development": {"changedPaths": ["unmapped/shared.css"]},
        }
        unmapped = run_verify_config(unmapped_config, tmp / "changed-unmapped", expect=0)
        unmapped_selection = unmapped.get("plan", {}).get("selection", {})
        if len(unmapped.get("pages", [])) != 2 or not unmapped_selection.get("fallbackToFull"):
            raise AssertionError(f"Unmapped change did not safely expand to the full plan: {unmapped_selection}")

        # --ignore-https-errors: a self-signed TLS target must be verifiable.
        tls_server = make_tls_server(tmp)
        try:
            tls_clean = run_verify(
                f"{tls_server.base_url}/clean.html",
                tmp / "tls-clean",
                expect=0,
                extra=["--ignore-https-errors"],
            )
            assert_no_critical(tls_clean)
            if not page_metrics(tls_clean):
                raise AssertionError("Self-signed TLS page was skipped")
        finally:
            tls_server.close()

        # Scroll-container reachability: content scrolled out of an inner
        # overflow-x container must not be reported as occluded by whatever
        # neighbors its document coordinates.
        scroll_neighbor = run_verify(
            f"{server.base_url}/scroll-panel-neighbor.html", tmp / "scroll-panel-neighbor", expect=0
        )
        assert_no_critical(scroll_neighbor)
        assert_no_rule(scroll_neighbor, "occluded")
        assert_no_rule(scroll_neighbor, "partially-occluded")

        # ...while an overlay really covering visible content inside a scroll
        # container must still be caught (recall guard for the same code path).
        overlay_scroll = run_verify(f"{server.base_url}/overlay-in-scroll.html", tmp / "overlay-in-scroll", expect=1)
        assert_critical_rule(overlay_scroll, "occluded")

        # Closed-details content keeps layout boxes but is not rendered: no
        # occlusion findings for it.
        details_closed = run_verify(
            f"{server.base_url}/details-closed-neighbor.html", tmp / "details-closed-neighbor", expect=0
        )
        assert_no_critical(details_closed)
        assert_no_rule(details_closed, "occluded")
        assert_no_rule(details_closed, "partially-occluded")

        # Framework dev overlays are ignored as occluders.
        dev_overlay = run_verify(f"{server.base_url}/dev-overlay-badge.html", tmp / "dev-overlay-badge", expect=0)
        assert_no_critical(dev_overlay)
        assert_no_rule(dev_overlay, "occluded")
        assert_no_rule(dev_overlay, "partially-occluded")

        # Journey contracts are mandatory for every target, including an
        # otherwise healthy bare URL.
        missing_contract = run_verify_config(
            {
                "targets": [{"url": f"{server.base_url}/clean.html"}],
                "viewports": [{"name": "mobile", "width": 390, "height": 844}],
            },
            tmp / "missing-journey-contract",
            expect=3,
            apply_contract_defaults=False,
        )
        if missing_contract.get("pages", [{}])[0].get("outcome") != "journey_contract_error":
            raise AssertionError("A target without journey/theme/review-input intent did not fail coverage")

        account_journeys = [
            {"id": "view-accounts", "frequencyPercent": 99, "risk": "normal", "rationale": "Normal destination use"},
            {"id": "add-account", "frequencyPercent": 1, "risk": "normal", "rationale": "Occasional creation"},
        ]
        account_review_inputs = [{"path": "SKILL.md", "kind": "ui-code"}]
        account_base = {
            "journeys": account_journeys,
            "primaryJourney": "view-accounts",
            "theme": "light",
            "reviewInputs": account_review_inputs,
        }
        form_first = run_verify_config(
            {
                "targets": [{
                    **account_base,
                    "url": f"{server.base_url}/accounts-form-first.html",
                    "regions": [
                        {"selector": "#add-account", "role": "workflow-surface", "journey": "add-account"},
                        {"selector": "#account-list", "role": "primary-content", "journey": "view-accounts"},
                    ],
                }],
                "viewports": [{"name": "mobile", "width": 390, "height": 844}],
            },
            tmp / "accounts-form-first",
            expect=1,
        )
        assert_critical_rule(form_first, "secondary-workflow-precedes-primary")

        compact_toolbar = run_verify_config(
            {
                "targets": [{
                    **account_base,
                    "url": f"{server.base_url}/accounts-toolbar.html",
                    "regions": [
                        {"selector": "#account-tools", "role": "supporting"},
                        {"selector": "#account-list", "role": "primary-content", "journey": "view-accounts"},
                    ],
                }],
                "viewports": [{"name": "mobile", "width": 390, "height": 844}],
            },
            tmp / "accounts-toolbar",
            expect=0,
        )
        assert_no_rule(compact_toolbar, "supporting-content-dominates-primary")

        blocking_alert = run_verify_config(
            {
                "targets": [{
                    **account_base,
                    "url": f"{server.base_url}/accounts-alert.html",
                    "regions": [
                        {"selector": "#blocking-alert", "role": "blocking-alert", "reason": "The collection cannot load"},
                        {"selector": "#account-list", "role": "primary-content", "journey": "view-accounts"},
                    ],
                }],
                "viewports": [{"name": "mobile", "width": 390, "height": 844}],
            },
            tmp / "accounts-blocking-alert",
            expect=0,
        )
        assert_no_critical(blocking_alert)

        dedicated_create = run_verify_config(
            {
                "targets": [{
                    "url": f"{server.base_url}/create-account.html",
                    "journeys": [{"id": "add-account", "frequencyPercent": 100, "risk": "normal"}],
                    "primaryJourney": "add-account",
                    "regions": [{"selector": "#create-form", "role": "primary-content", "journey": "add-account"}],
                    "theme": "light",
                    "reviewInputs": account_review_inputs,
                }],
                "viewports": [{"name": "mobile", "width": 390, "height": 844}],
            },
            tmp / "dedicated-create",
            expect=0,
        )
        assert_no_critical(dedicated_create)

        offscreen_add = run_verify_config(
            {
                "targets": [{
                    **account_base,
                    "url": f"{server.base_url}/accounts-offscreen-add.html",
                    "includeBase": False,
                    "regions": [{"selector": "#account-list", "role": "primary-content", "journey": "view-accounts"}],
                    "states": [{
                        "name": "add-open",
                        "actions": [{"action": "click", "selector": "#open-add"}],
                        "primaryJourney": "add-account",
                        "priorityOverrideReason": "The user explicitly activated account creation",
                        "regions": [{"selector": "#add-form", "role": "primary-content", "journey": "add-account"}],
                        "continuation": {"kind": "in-page", "anchor": "#add-form h2", "focusWithin": "#add-form"},
                    }],
                }],
                "viewports": [{"name": "mobile", "width": 390, "height": 844}],
            },
            tmp / "accounts-offscreen-add",
            expect=1,
        )
        assert_critical_rule(offscreen_add, "continuation-anchor-offscreen")
        assert_critical_rule(offscreen_add, "continuation-focus-missing")

        modal_add = run_verify_config(
            {
                "targets": [{
                    **account_base,
                    "url": f"{server.base_url}/accounts-modal-add.html",
                    "includeBase": False,
                    "regions": [{"selector": "#account-list", "role": "primary-content", "journey": "view-accounts"}],
                    "states": [{
                        "name": "add-dialog",
                        "actions": [{"action": "click", "selector": "#open-add"}],
                        "primaryJourney": "add-account",
                        "priorityOverrideReason": "The user explicitly activated account creation",
                        "regions": [{"selector": "#add-dialog", "role": "primary-content", "journey": "add-account"}],
                        "continuation": {"kind": "in-page", "anchor": "#add-dialog h2", "focusWithin": "#add-dialog"},
                    }],
                }],
                "viewports": [{"name": "mobile", "width": 390, "height": 844}],
            },
            tmp / "accounts-modal-add",
            expect=0,
        )
        assert_no_critical(modal_add)
        if not modal_add.get("pages", [{}])[0].get("continuation", {}).get("evidence", {}).get("focusSatisfied"):
            raise AssertionError("Visible modal continuation did not preserve focus evidence")

        broad_container_anchor = run_verify_config(
            {
                "targets": [{
                    **account_base,
                    "url": f"{server.base_url}/accounts-modal-add.html",
                    "includeBase": False,
                    "regions": [{"selector": "#account-list", "role": "primary-content", "journey": "view-accounts"}],
                    "states": [{
                        "name": "add-dialog-broad-anchor",
                        "actions": [{"action": "click", "selector": "#open-add"}],
                        "primaryJourney": "add-account",
                        "priorityOverrideReason": "The user explicitly activated account creation",
                        "regions": [{"selector": "#add-dialog", "role": "primary-content", "journey": "add-account"}],
                        "continuation": {"kind": "in-page", "anchor": "#add-dialog", "focusWithin": "#add-dialog"},
                    }],
                }],
                "viewports": [{"name": "mobile", "width": 390, "height": 844}],
            },
            tmp / "accounts-broad-container-anchor",
            expect=1,
        )
        assert_critical_rule(broad_container_anchor, "continuation-anchor-not-recognizable")

        navigation_add = run_verify_config(
            {
                "targets": [{
                    **account_base,
                    "url": f"{server.base_url}/accounts-navigation.html",
                    "includeBase": False,
                    "regions": [{"selector": "#account-list", "role": "primary-content", "journey": "view-accounts"}],
                    "states": [{
                        "name": "dedicated-create",
                        "actions": [{"action": "click", "selector": "#go-create"}],
                        "primaryJourney": "add-account",
                        "priorityOverrideReason": "The user explicitly activated account creation",
                        "regions": [{"selector": "#create-form", "role": "primary-content", "journey": "add-account"}],
                        "continuation": {"kind": "navigation", "anchor": "#create-form h1", "expectedPath": "/create-account.html"},
                    }],
                }],
                "viewports": [{"name": "mobile", "width": 390, "height": 844}],
            },
            tmp / "accounts-navigation",
            expect=0,
        )
        if navigation_add.get("pages", [{}])[0].get("finalPath") != "/create-account.html":
            raise AssertionError("Expected journey navigation was not accepted and recorded")

        dark_inversion = run_verify_config(
            {
                "targets": [{
                    "url": f"{server.base_url}/theme-dark-with-bright-surface.html",
                    "theme": "dark",
                    "regions": [{"selector": "#primary", "role": "primary-content", "journey": "fixture-primary"}],
                }],
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "dark-theme-inversion",
            expect=1,
        )
        assert_critical_rule(dark_inversion, "declared-theme-contradiction")
        light_inversion = run_verify_config(
            {
                "targets": [{
                    "url": f"{server.base_url}/theme-light-with-dark-surface.html",
                    "theme": "light",
                    "regions": [{"selector": "#primary", "role": "primary-content", "journey": "fixture-primary"}],
                }],
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "light-theme-inversion",
            expect=1,
        )
        assert_critical_rule(light_inversion, "declared-theme-contradiction")
        mixed_theme = run_verify_config(
            {
                "targets": [{
                    "url": f"{server.base_url}/theme-mixed.html",
                    "theme": "mixed",
                    "regions": [{"selector": "#primary", "role": "primary-content", "journey": "fixture-primary"}],
                }],
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "mixed-theme",
            expect=0,
        )
        assert_no_rule(mixed_theme, "declared-theme-contradiction")

        contrast_fail = run_verify(f"{server.base_url}/contrast-aa-fail.html", tmp / "contrast-aa-fail", expect=1)
        assert_critical_rule(contrast_fail, "insufficient-text-contrast")
        contrast_large = run_verify(f"{server.base_url}/contrast-large-pass.html", tmp / "contrast-large-pass", expect=0)
        assert_no_rule(contrast_large, "insufficient-text-contrast")
        contrast_allowed = run_verify(f"{server.base_url}/contrast-allowed.html", tmp / "contrast-allowed", expect=0)
        assert_warning_rule(contrast_allowed, "allowed-contrast")
        palette_risk = run_verify_config(
            {
                "targets": [{
                    "url": f"{server.base_url}/palette-competing.html",
                    "theme": "mixed",
                    "regions": [{"selector": "#primary", "role": "primary-content", "journey": "fixture-primary"}],
                }],
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "palette-competing",
            expect=0,
        )
        assert_warning_rule(palette_risk, "high-chroma-surface-risk")
        assert_warning_rule(palette_risk, "competing-accent-hues")

        # Review selection follows declared implementation inputs and intent,
        # never screenshot pixels or unrelated files.
        review_repo = (tmp / "review-repo").resolve()
        write(review_repo / "ui" / "screen.css", ".screen { padding: 16px; }\n")
        write(review_repo / "backend.txt", "unrelated backend v1\n")
        review_config = {
            "repoRoot": str(review_repo),
            "targets": [{
                "url": f"{server.base_url}/dynamic-review.html",
                "reviewInputs": [{"path": "ui/screen.css", "kind": "style"}],
            }],
            "viewports": [{"name": "mobile", "width": 390, "height": 844}],
        }
        review_first_dir = tmp / "review-first"
        review_first = run_verify_config(review_config, review_first_dir, expect=0)
        if review_first.get("review", {}).get("pendingCount") != 1:
            raise AssertionError("A newly covered cell must enter changed visual review")
        first_cell = review_first["review"]["cells"][0]
        decisions_path = review_first_dir / "decisions.json"
        decisions_path.write_text(
            json.dumps({"decisions": [{"reviewCellKey": first_cell["reviewCellKey"], "decision": "pass"}]}),
            encoding="utf-8",
        )
        prior_pass = review_first_dir / "manual-review.json"
        run_review(
            [
                "--report", str(review_first_dir / "report.json"),
                "--queue", str(review_first_dir / "review-queue.json"),
                "--decisions", str(decisions_path),
                "--out", str(prior_pass),
            ],
            expect=0,
        )

        review_second_dir = tmp / "review-second"
        review_second = run_verify_config(
            {**review_config, "reviewAgainst": str(prior_pass)},
            review_second_dir,
            expect=0,
        )
        if review_second.get("review", {}).get("pendingCount") != 0 or review_second.get("review", {}).get("carriedPassCount") != 1:
            raise AssertionError("Unchanged UI inputs and intent should carry the pass without reopening screenshots")
        first_hash = first_cell["screenshots"]["viewport"]["sha256"]
        second_hash = review_second["review"]["cells"][0]["screenshots"]["viewport"]["sha256"]
        if first_hash == second_hash:
            raise AssertionError("Dynamic fixture did not prove that pixel drift is ignored as a review trigger")

        write(review_repo / "backend.txt", "unrelated backend v2\n")
        unrelated = run_verify_config(
            {**review_config, "reviewAgainst": str(prior_pass)},
            tmp / "review-unrelated-change",
            expect=0,
        )
        if unrelated.get("review", {}).get("pendingCount") != 0:
            raise AssertionError("An unrelated file change incorrectly triggered manual UI review")

        write(review_repo / "ui" / "screen.css", ".screen { padding: 24px; }\n")
        mapped_change = run_verify_config(
            {**review_config, "reviewAgainst": str(prior_pass)},
            tmp / "review-mapped-change",
            expect=0,
        )
        if mapped_change.get("review", {}).get("pendingCount") != 1:
            raise AssertionError("A declared UI-input change did not trigger manual review")
        write(review_repo / "ui" / "screen.css", ".screen { padding: 16px; }\n")

        intent_change = run_verify_config(
            {
                **review_config,
                "reviewAgainst": str(prior_pass),
                "targets": [{
                    "url": f"{server.base_url}/dynamic-review.html",
                    "theme": "mixed",
                    "reviewInputs": [{"path": "ui/screen.css", "kind": "style"}],
                }],
            },
            tmp / "review-intent-change",
            expect=0,
        )
        if intent_change.get("review", {}).get("pendingCount") != 1:
            raise AssertionError("Changed journey/theme intent did not trigger manual review")

        new_viewport = run_verify_config(
            {
                **review_config,
                "reviewAgainst": str(prior_pass),
                "viewports": [
                    {"name": "mobile", "width": 390, "height": 844},
                    {"name": "desktop", "width": 1280, "height": 800},
                ],
            },
            tmp / "review-new-viewport",
            expect=0,
        )
        if new_viewport.get("review", {}).get("pendingCount") != 1 or new_viewport.get("review", {}).get("carriedPassCount") != 1:
            raise AssertionError("A new viewport should be reviewed without reopening the unchanged existing viewport")

        raw_prior = run_verify_config(
            {**review_config, "reviewAgainst": str(review_first_dir / "report.json")},
            tmp / "review-raw-prior-rejected",
            expect=2,
        )
        if "not a supported reviewed manifest" not in raw_prior.get("error", {}).get("message", ""):
            raise AssertionError("An unreviewed formal report was accepted as a manual-review baseline")

        removed_undisposed = run_verify_config(
            {
                **review_config,
                "reviewAgainst": str(prior_pass),
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "review-removed-undisposed",
            expect=3,
        )
        if not removed_undisposed.get("coverage", {}).get("reviewFailures"):
            raise AssertionError("A removed reviewed cell disappeared without an explicit disposition")
        removed_key = first_cell["reviewCellKey"]
        removed_disposed = run_verify_config(
            {
                **review_config,
                "reviewAgainst": str(prior_pass),
                "reviewRemovedCells": [{"reviewCellKey": removed_key, "reason": "The mobile viewport is no longer supported"}],
                "viewports": [{"name": "desktop", "width": 1280, "height": 800}],
            },
            tmp / "review-removed-disposed",
            expect=0,
        )
        if removed_disposed.get("coverage", {}).get("reviewFailures"):
            raise AssertionError("An explicitly dispositioned removed cell still failed review coverage")

        gap_decisions = review_first_dir / "gap-decisions.json"
        gap_decisions.write_text(
            json.dumps({"decisions": [{"reviewCellKey": first_cell["reviewCellKey"], "decision": "gap", "note": "Palette needs correction"}]}),
            encoding="utf-8",
        )
        prior_gap = review_first_dir / "manual-review-gap.json"
        run_review(
            [
                "--report", str(review_first_dir / "report.json"),
                "--queue", str(review_first_dir / "review-queue.json"),
                "--decisions", str(gap_decisions),
                "--out", str(prior_gap),
            ],
            expect=1,
        )
        carried_gap = run_verify_config(
            {**review_config, "reviewAgainst": str(prior_gap)},
            tmp / "review-carried-gap",
            expect=1,
        )
        if carried_gap.get("review", {}).get("pendingCount") != 0:
            raise AssertionError("An unchanged prior gap should remain blocking without reopening images")
        assert_critical_rule(carried_gap, "manual-review-gap-carried")

        first_screenshot = Path(first_cell["screenshots"]["viewport"]["path"])
        first_screenshot.write_bytes(first_screenshot.read_bytes() + b"tampered")
        tampered_review = run_review(
            [
                "--report", str(review_first_dir / "report.json"),
                "--queue", str(review_first_dir / "review-queue.json"),
                "--review", str(prior_pass),
            ],
            expect=2,
        )
        if "hash mismatch" not in tampered_review.get("error", ""):
            raise AssertionError("Screenshot replacement was not rejected as an integrity failure")

        print("self-test ok")
        return 0
    finally:
        if server:
            server.close()
        if KEEP_TEMP:
            print(f"Preserved self-test workspace: {tmp}", file=sys.stderr)
        else:
            shutil.rmtree(tmp, ignore_errors=True)


FAILURE_EXIT_BASE = 20
FAILURE_BUCKET_LINES = 20
FAILURE_MAX_BUCKET = 220


def failure_exit_code_for_line(line: int) -> int:
    relative_line = max(0, line - main.__code__.co_firstlineno)
    return FAILURE_EXIT_BASE + min(
        relative_line // FAILURE_BUCKET_LINES,
        FAILURE_MAX_BUCKET,
    )


def failure_exit_code(error: Exception) -> int:
    """Encode only the failing main-section bucket for safe CI diagnosis."""

    main_lines = [
        frame.lineno
        for frame in traceback.extract_tb(error.__traceback__)
        if frame.name == "main" and Path(frame.filename).resolve() == Path(__file__).resolve()
    ]
    line = main_lines[-1] if main_lines else main.__code__.co_firstlineno
    return failure_exit_code_for_line(line)


def validate_failure_classifier() -> None:
    """Guard the diagnostic range without executing or exposing a failure."""

    start = main.__code__.co_firstlineno
    end = failure_exit_code_for_line.__code__.co_firstlineno - 1
    if failure_exit_code_for_line(start) != FAILURE_EXIT_BASE:
        raise AssertionError("failure classifier base changed")
    if failure_exit_code_for_line(start + FAILURE_BUCKET_LINES) != FAILURE_EXIT_BASE + 1:
        raise AssertionError("failure classifier bucket changed")
    if end - start >= FAILURE_BUCKET_LINES * FAILURE_MAX_BUCKET:
        raise AssertionError("failure classifier range is exhausted")


def retain_failure_traceback(error: Exception) -> None:
    """Keep governed output content-free; direct developer runs retain stderr."""

    if not os.environ.get("DEVCOORDINATOR_CHECK_SCRATCH"):
        traceback.print_exception(error)


if __name__ == "__main__":
    try:
        validate_failure_classifier()
        result = main()
    except Exception as error:
        retain_failure_traceback(error)
        raise SystemExit(failure_exit_code(error)) from None
    raise SystemExit(result)
