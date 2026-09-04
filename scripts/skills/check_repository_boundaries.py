#!/usr/bin/env python3
"""Verify the unified six-skill repository and reject retired live dependencies."""

from __future__ import annotations

import argparse
import json
import re
from dataclasses import asdict, dataclass
from pathlib import Path


CANONICAL_SKILLS = {
    "dev-coordinator",
    "formal-web-ui-verification",
    "full-repo-audit",
    "full-repo-test-coverage-audit",
    "ui-implementation-audit",
    "user-journey-docs-audit",
}
CANONICAL_POLICY = Path("reference/universal/AGENTS.md")
RETIRED_PATHS = (
    Path("reference/codex-app-wide"),
    Path("skills/codex-dev-coordinator"),
)
SELF_FILES = {
    "scripts/skills/check_repository_boundaries.py",
    "scripts/skills/check_repository_boundaries_self_test.py",
}
HISTORICAL_FILES = {
    "DecisionHistory.md",
    "docs/holy-skills-snapshot.md",
}
ACTIVE_ROOT_FILES = {
    ".gitmodules",
    "AGENTS.md",
    "CLAUDE.md",
    "README.md",
    "security-assumptions.md",
}
ACTIVE_DIRECTORIES = {
    ".github",
    "deploy",
    "edge",
    "reference",
    "scripts",
    "skills",
    "src",
}
FORBIDDEN_PATTERNS = (
    ("retired-checkout", re.compile(r"/home/holyskills(?:[/\s'\"`]|$)", re.IGNORECASE)),
    (
        "retired-remote",
        re.compile(
            r"(?:github\.com[/:])?[A-Za-z0-9_.-]+/holyskills(?:\.git)?\b",
            re.IGNORECASE,
        ),
    ),
    ("retired-policy-path", re.compile(r"reference/codex-app-wide", re.IGNORECASE)),
    ("retired-skill-name", re.compile(r"(?:skills/|\$)codex-dev-coordinator\b", re.IGNORECASE)),
    ("retired-release-source", re.compile(r"/opt/devcoordinator2/(?:current|releases)(?:[/\s'\"`]|$)")),
)


@dataclass(frozen=True)
class Finding:
    rule: str
    path: str
    line: int | None
    detail: str


def _active_text_files(repository: Path) -> list[Path]:
    result: list[Path] = []
    for path in repository.rglob("*"):
        if not path.is_file() or path.is_symlink():
            continue
        relative = path.relative_to(repository)
        name = relative.as_posix()
        if name in SELF_FILES or name in HISTORICAL_FILES:
            continue
        if any(part in {".git", "__pycache__", "node_modules"} for part in relative.parts):
            continue
        if name not in ACTIVE_ROOT_FILES and relative.parts[0] not in ACTIVE_DIRECTORIES:
            continue
        data = path.read_bytes()
        if b"\0" in data:
            continue
        try:
            data.decode("utf-8")
        except UnicodeDecodeError:
            continue
        result.append(relative)
    return sorted(result)


def audit_repository(repository: Path) -> dict[str, object]:
    repository = repository.resolve()
    findings: list[Finding] = []
    skills_root = repository / "skills"
    actual = (
        {
            path.name
            for path in skills_root.iterdir()
            if path.is_dir() and not path.is_symlink() and (path / "SKILL.md").is_file()
        }
        if skills_root.is_dir()
        else set()
    )
    if actual != CANONICAL_SKILLS:
        findings.append(
            Finding(
                "canonical-skill-set",
                "skills",
                None,
                f"expected {sorted(CANONICAL_SKILLS)}, found {sorted(actual)}",
            )
        )

    policy = repository / CANONICAL_POLICY
    if not policy.is_file() or policy.is_symlink():
        findings.append(
            Finding(
                "canonical-policy",
                CANONICAL_POLICY.as_posix(),
                None,
                "regular policy file is missing",
            )
        )
    for retired in RETIRED_PATHS:
        path = repository / retired
        if path.exists() or path.is_symlink():
            findings.append(
                Finding("retired-path-present", retired.as_posix(), None, "retired path exists")
            )
    if (repository / ".gitmodules").exists():
        findings.append(
            Finding(
                "submodule-present",
                ".gitmodules",
                None,
                "submodules are not part of the unified source",
            )
        )

    for relative in _active_text_files(repository):
        text = (repository / relative).read_text(encoding="utf-8")
        for line_number, line in enumerate(text.splitlines(), start=1):
            for rule, pattern in FORBIDDEN_PATTERNS:
                if pattern.search(line):
                    findings.append(
                        Finding(
                            rule,
                            relative.as_posix(),
                            line_number,
                            "active source references retired ownership",
                        )
                    )

    return {
        "ok": not findings,
        "canonical_skills": sorted(CANONICAL_SKILLS),
        "canonical_policy": CANONICAL_POLICY.as_posix(),
        "finding_count": len(findings),
        "findings": [asdict(finding) for finding in findings],
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", default=".")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    report = audit_repository(Path(args.repo))
    if args.json:
        print(json.dumps(report, indent=2, sort_keys=True))
    elif report["ok"]:
        print("repository boundary check ok (one live checkout; six canonical skills)")
    else:
        for finding in report["findings"]:
            location = finding["path"]
            if finding["line"] is not None:
                location += f":{finding['line']}"
            print(f"{location}: {finding['rule']}: {finding['detail']}")
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
