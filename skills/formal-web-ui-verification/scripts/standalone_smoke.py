#!/usr/bin/env python3
"""Focused copied-package browser proof for formal-web-ui-verification."""

from __future__ import annotations

import shutil
import tempfile
from pathlib import Path

import self_test as harness


def main() -> int:
    temporary = Path(tempfile.mkdtemp(prefix="fwv-standalone-smoke-"))
    server = None
    try:
        site = temporary / "site"
        harness.write(
            site / "clean.html",
            harness.page("<h1>Standalone clean page</h1><button>Save changes</button>"),
        )
        harness.write(
            site / "finding.html",
            harness.page(
                "<h1>Standalone finding</h1><button class='bad'>Account settings</button>",
                ".bad { width: 34px; overflow: hidden; white-space: nowrap; }",
            ),
        )
        server = harness.Server(site)
        clean = harness.run_verify(
            f"{server.base_url}/clean.html",
            temporary / "clean",
            expect=0,
        )
        harness.assert_no_critical(clean)
        finding = harness.run_verify(
            f"{server.base_url}/finding.html",
            temporary / "finding",
            expect=1,
        )
        harness.assert_critical_rule(finding, "clipped-x")
        print("standalone smoke ok")
        return 0
    finally:
        if server:
            server.close()
        shutil.rmtree(temporary, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
