#!/usr/bin/env python3
"""Owner-executed public edge switch between the legacy edge and
DevCoordinator2's edge (docs/cutover.md). Run as root.

  edge_switch.py --to devcoordinator2   stop the legacy edge units, put the
                                         DevCoordinator2 edge on 80/443 (TLS)
  edge_switch.py --to legacy             the exact reverse (rollback)

The legacy unit names are arguments (instance data). The script refuses to
proceed unless the DevCoordinator2 edge has TLS and OIDC credentials in
place, and it never deletes anything.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

ETC = Path("/etc/devcoordinator2")
REQUIRED = ("edge/tls.crt", "edge/tls.key", "edge/session.secret", "edge/oidc.client_id",
            "edge/oidc.client_secret")


def sc(*args: str, check: bool = True) -> subprocess.CompletedProcess:
    return subprocess.run(["systemctl", *args], check=check, capture_output=True, text=True)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--to", choices=("devcoordinator2", "legacy"), required=True)
    ap.add_argument("--legacy-units", required=True,
                    help="comma-separated legacy edge units (services and sockets)")
    ap.add_argument("--yes", action="store_true", help="actually perform the switch")
    ns = ap.parse_args()
    legacy = [u.strip() for u in ns.legacy_units.split(",") if u.strip()]
    env = ETC / "edge.env"
    text = env.read_text() if env.exists() else ""
    if ns.to == "devcoordinator2":
        missing = [r for r in REQUIRED if not (ETC / r).exists()]
        if missing:
            print(f"refusing: missing credentials {missing}", file=sys.stderr)
            return 2
        if "EDGE_HTTP_ONLY=1" in text:
            print("refusing: edge.env still in http-only canary mode; set EDGE_HTTP_ONLY=0,"
                  " EDGE_HTTP_PORT=80, EDGE_HTTPS_PORT=443 and reinstall the unit without"
                  " --canary", file=sys.stderr)
            return 2
    plan = ([("stop", u) for u in legacy] + [("start", "devcoordinator2-edge.service")]
            if ns.to == "devcoordinator2" else
            [("stop", "devcoordinator2-edge.service")] + [("start", u) for u in legacy])
    print("plan:", plan)
    if not ns.yes:
        print("dry run; add --yes to execute")
        return 0
    for action, unit in plan:
        sc(action, unit, check=False)
    for unit in [*legacy, "devcoordinator2-edge.service"]:
        state = sc("is-active", unit, check=False).stdout.strip()
        print(f"{unit}: {state}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
