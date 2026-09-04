#!/usr/bin/env python3
"""Reject pull-request execution on repository self-hosted runners."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path
from typing import Dict, List, Optional, Tuple


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_WORKFLOW = ROOT / ".github" / "workflows" / "validate.yml"
JOB_HEADER = re.compile(r"^  ([A-Za-z0-9_-]+):\s*$")
TOP_LEVEL = re.compile(r"^[A-Za-z0-9_-]+:\s*(?:#.*)?$")
HOSTED_RUNNER = re.compile(
    r"^(?:ubuntu-(?:latest|[0-9]+(?:\.[0-9]+)?)|"
    r"windows-(?:latest|[0-9]+)|macos-(?:latest|[0-9]+))$"
)


class SecurityError(ValueError):
    """Raised when a workflow can expose a self-hosted runner to PR code."""


def _without_comments(text: str) -> str:
    """Remove YAML comment tails from this repository's simple workflow shape."""

    return "\n".join(line.split("#", 1)[0].rstrip() for line in text.splitlines())


def _has_pull_request_trigger(text: str) -> bool:
    lines = _without_comments(text).splitlines()
    for index, line in enumerate(lines):
        if re.match(r"^on:\s*", line):
            inline = line.split(":", 1)[1]
            if re.search(r"\bpull_request(?:_target)?\b", inline):
                return True
            for nested in lines[index + 1 :]:
                if nested and not nested.startswith(" "):
                    break
                if re.match(r"^  pull_request(?:_target)?:\s*", nested):
                    return True
            return False
    return False


def _job_blocks(text: str) -> Dict[str, List[str]]:
    lines = _without_comments(text).splitlines()
    jobs_index = next((index for index, line in enumerate(lines) if line == "jobs:"), None)
    if jobs_index is None:
        raise SecurityError("workflow has no top-level jobs section")

    jobs: Dict[str, List[str]] = {}
    current_name = None
    for line in lines[jobs_index + 1 :]:
        if line and not line.startswith(" ") and TOP_LEVEL.match(line):
            break
        header = JOB_HEADER.match(line)
        if header:
            current_name = header.group(1)
            jobs[current_name] = []
            continue
        if current_name is not None:
            jobs[current_name].append(line)
    if not jobs:
        raise SecurityError("workflow jobs section is empty or malformed")
    return jobs


def _matrix_axis_values(lines: List[str], axis: str) -> Optional[List[str]]:
    pattern = re.compile(r"^        {}:\s*\[([^]]*)\]\s*$".format(re.escape(axis)))
    for line in lines:
        match = pattern.match(line)
        if match:
            values = [item.strip().strip("'\"") for item in match.group(1).split(",")]
            return [item for item in values if item]
    return None


def _runner_value_may_be_self_hosted(value: str) -> bool:
    value = value.strip().strip("'\"")
    return not bool(HOSTED_RUNNER.fullmatch(value))


def _job_runs_self_hosted(lines: List[str]) -> bool:
    """Conservatively classify unknown runner expressions as self-hosted-capable."""

    for index, line in enumerate(lines):
        match = re.match(r"^    runs-on:\s*(.*)$", line)
        if not match:
            continue
        inline = match.group(1).strip()
        if inline:
            matrix = re.fullmatch(r"\$\{\{\s*matrix\.([A-Za-z0-9_-]+)\s*\}\}", inline)
            if matrix:
                values = _matrix_axis_values(lines, matrix.group(1))
                return values is None or any(
                    _runner_value_may_be_self_hosted(value) for value in values
                )
            if inline.startswith("[") and inline.endswith("]"):
                values = [item.strip() for item in inline[1:-1].split(",") if item.strip()]
                return len(values) != 1 or _runner_value_may_be_self_hosted(values[0])
            return _runner_value_may_be_self_hosted(inline)
        values: List[str] = []
        for nested in lines[index + 1 :]:
            if nested and len(nested) - len(nested.lstrip(" ")) <= 4:
                break
            match = re.match(r"^\s+-\s*(.+?)\s*$", nested)
            if match:
                values.append(match.group(1))
        return len(values) != 1 or _runner_value_may_be_self_hosted(values[0])
    return False


def _job_condition(lines: List[str]) -> str:
    for line in lines:
        match = re.match(r"^    if:\s*(.+)$", line)
        if match:
            return match.group(1).strip()
    return ""


def _trusted_job_condition(condition: str) -> bool:
    """Accept only an exact push/workflow_dispatch event allowlist.

    One repository-variable equality gate may precede the event allowlist. Any
    additional disjunction or function call is rejected instead of being
    interpreted as harmless.
    """

    match = re.fullmatch(r"\$\{\{(.*)\}\}", re.sub(r"\s+", "", condition))
    if not match:
        return False
    body = match.group(1)
    body = body.replace('"push"', "'push'").replace(
        '"workflow_dispatch"', "'workflow_dispatch'"
    )
    event_expressions = (
        "github.event_name=='push'||github.event_name=='workflow_dispatch'",
        "github.event_name=='workflow_dispatch'||github.event_name=='push'",
    )
    if body in event_expressions or body in tuple(
        "({})".format(expression) for expression in event_expressions
    ):
        return True
    gate = r"vars\.[A-Za-z_][A-Za-z0-9_.]*=='[A-Za-z0-9_.-]+'&&"
    return any(
        re.fullmatch(gate + r"\(" + re.escape(expression) + r"\)", body)
        for expression in event_expressions
    )


def find_violations(text: str) -> Tuple[List[str], int]:
    jobs = _job_blocks(text)
    self_hosted = [name for name, lines in jobs.items() if _job_runs_self_hosted(lines)]
    if not _has_pull_request_trigger(text):
        return [], len(self_hosted)

    violations: List[str] = []
    for name in self_hosted:
        condition = _job_condition(jobs[name])
        if not _trusted_job_condition(condition):
            violations.append(
                "self-hosted job {!r} must have a job-level event allowlist for only "
                "push and workflow_dispatch while pull_request is enabled".format(name)
            )
    return violations, len(self_hosted)


def check_workflow(path: Path) -> Tuple[List[str], int, bool]:
    if path.is_symlink() or not path.is_file():
        raise SecurityError("workflow must be a regular non-symlinked file: {}".format(path))
    text = path.read_text(encoding="utf-8")
    violations, count = find_violations(text)
    return violations, count, _has_pull_request_trigger(text)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--workflow", type=Path, default=DEFAULT_WORKFLOW)
    args = parser.parse_args()
    try:
        violations, count, pull_request = check_workflow(args.workflow)
    except (OSError, UnicodeError, SecurityError) as exc:
        print("ci security check error: {}".format(exc), file=sys.stderr)
        return 2
    if violations:
        for violation in violations:
            print("ci security violation: {}".format(violation), file=sys.stderr)
        return 1
    print(
        "ci security check ok ({} self-hosted job(s); pull_request={})".format(
            count, str(pull_request).lower()
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
