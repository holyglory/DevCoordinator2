#!/usr/bin/env python3
"""Validate and score the marker-free manual-agent evaluation suite."""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path, PurePosixPath
from typing import Any


ROOT = Path(__file__).resolve().parent
SUITE_PATH = ROOT / "suite.json"
RESPONSE_SCHEMA_PATH = ROOT / "response.schema.json"
SCHEMA_VERSION = 1
CASE_CLASSES = (
    "ignored-input-config",
    "partial-plumbing-no-dependency-call",
    "false-persistence-success",
    "missing-registration-lifecycle",
    "production-fixture-mock",
    "shallow-outcome-tests",
)
CASE_ID_RE = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")
ANSWER_MARKER_PATTERNS = (
    re.compile(r"\bTODO\b", re.IGNORECASE),
    re.compile(r"\bNotImplemented(?:Error)?\b", re.IGNORECASE),
    re.compile(r"\bstub\b", re.IGNORECASE),
)


class EvalError(ValueError):
    """Raised for deterministic suite, response, or scoring failures."""


def load_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise EvalError(f"missing JSON file: {path}") from exc
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise EvalError(f"invalid JSON file {path}: {exc}") from exc


def require_object(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise EvalError(f"{label} must be a JSON object")
    return value


def require_exact_keys(value: dict[str, Any], required: set[str], label: str) -> None:
    actual = set(value)
    missing = sorted(required - actual)
    extra = sorted(actual - required)
    if missing or extra:
        details: list[str] = []
        if missing:
            details.append(f"missing={missing}")
        if extra:
            details.append(f"extra={extra}")
        raise EvalError(f"{label} has invalid fields: {', '.join(details)}")


def require_text(value: Any, label: str, *, minimum: int = 1) -> str:
    if not isinstance(value, str) or len(value.strip()) < minimum:
        raise EvalError(f"{label} must be a string with at least {minimum} non-whitespace characters")
    return value.strip()


def validate_relative_path(value: Any, label: str) -> str:
    path = require_text(value, label)
    if "\\" in path:
        raise EvalError(f"{label} must use POSIX separators")
    pure = PurePosixPath(path)
    if pure.is_absolute() or path != pure.as_posix() or any(part in {"", ".", ".."} for part in pure.parts):
        raise EvalError(f"{label} must be a normalized repository-relative path")
    return path


def require_repo_anchor(repo: Path, relative_path: str, symbol: str, label: str) -> None:
    source_path = repo / relative_path
    try:
        resolved = source_path.resolve(strict=True)
    except (OSError, RuntimeError) as exc:
        raise EvalError(f"{label} references missing source path {relative_path!r}") from exc
    try:
        resolved.relative_to(repo.resolve(strict=True))
    except ValueError as exc:
        raise EvalError(f"{label} escapes the agent-input repository") from exc
    if not resolved.is_file():
        raise EvalError(f"{label} source path is not a regular file: {relative_path!r}")
    try:
        source = resolved.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as exc:
        raise EvalError(f"{label} source path is not readable UTF-8: {relative_path!r}") from exc
    leaf_symbol = symbol.rsplit(".", 1)[-1]
    if symbol not in source and leaf_symbol not in source:
        raise EvalError(f"{label} symbol {symbol!r} is absent from {relative_path!r}")


def validate_response_schema_document() -> None:
    schema = require_object(load_json(RESPONSE_SCHEMA_PATH), "response.schema.json")
    if schema.get("type") != "object" or schema.get("additionalProperties") is not False:
        raise EvalError("response.schema.json must define a closed object schema")
    required = schema.get("required")
    if required != ["schema_version", "case_id", "findings", "precision_controls"]:
        raise EvalError("response.schema.json root required fields drifted from the scorer contract")
    properties = require_object(schema.get("properties"), "response.schema.json properties")
    if properties.get("schema_version", {}).get("const") != SCHEMA_VERSION:
        raise EvalError("response.schema.json schema_version drifted from the scorer contract")
    enum = properties.get("findings", {}).get("items", {}).get("properties", {}).get("class", {}).get("enum")
    if enum != list(CASE_CLASSES):
        raise EvalError("response.schema.json finding classes drifted from the scorer contract")


def validate_oracle(oracle: Any, *, case: dict[str, Any], repo: Path) -> dict[str, Any]:
    label = f"oracle for {case['id']}"
    oracle = require_object(oracle, label)
    require_exact_keys(
        oracle,
        {"schema_version", "case_id", "class", "expected_findings", "precision_controls"},
        label,
    )
    if oracle["schema_version"] != SCHEMA_VERSION:
        raise EvalError(f"{label} schema_version must equal {SCHEMA_VERSION}")
    if oracle["case_id"] != case["id"] or oracle["class"] != case["class"]:
        raise EvalError(f"{label} identity does not match suite.json")
    for field in ("expected_findings", "precision_controls"):
        entries = oracle[field]
        if not isinstance(entries, list) or len(entries) != 1:
            raise EvalError(f"{label} {field} must contain exactly one entry")
        entry = require_object(entries[0], f"{label} {field}[0]")
        require_exact_keys(entry, {"path", "symbols", "why"}, f"{label} {field}[0]")
        relative_path = validate_relative_path(entry["path"], f"{label} {field}[0].path")
        symbols = entry["symbols"]
        if not isinstance(symbols, list) or not symbols:
            raise EvalError(f"{label} {field}[0].symbols must be a non-empty array")
        clean_symbols = [require_text(symbol, f"{label} {field}[0].symbols", minimum=1) for symbol in symbols]
        if len(clean_symbols) != len(set(clean_symbols)):
            raise EvalError(f"{label} {field}[0].symbols contains duplicates")
        require_text(entry["why"], f"{label} {field}[0].why", minimum=12)
        anchor_errors: list[str] = []
        for symbol in clean_symbols:
            try:
                require_repo_anchor(repo, relative_path, symbol, f"{label} {field}[0]")
            except EvalError as exc:
                anchor_errors.append(str(exc))
        if len(anchor_errors) == len(clean_symbols):
            raise EvalError(anchor_errors[0])
    gap = oracle["expected_findings"][0]
    control = oracle["precision_controls"][0]
    if gap["path"] == control["path"] and set(gap["symbols"]) & set(control["symbols"]):
        raise EvalError(f"{label} gap and precision control anchors must be disjoint")
    return oracle


def validate_agent_input(repo: Path, case_id: str) -> None:
    if not repo.is_dir() or repo.is_symlink():
        raise EvalError(f"{case_id} repo must be a real directory")
    readme = repo / "README.md"
    try:
        readme_text = readme.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as exc:
        raise EvalError(f"{case_id} agent input must contain a readable README.md") from exc
    if "## Requirements" not in readme_text or readme_text.count("\n-") < 2:
        raise EvalError(f"{case_id} README.md must contain explicit repo-owned requirements")
    files = sorted(path for path in repo.rglob("*") if path.is_file())
    if len(files) < 2:
        raise EvalError(f"{case_id} repo must contain requirements and source")
    for path in files:
        if path.is_symlink():
            raise EvalError(f"{case_id} agent input must not contain symlinks: {path.relative_to(repo)}")
        if path.name == "oracle.json":
            raise EvalError(f"{case_id} oracle must not be inside the agent input")
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeError) as exc:
            raise EvalError(f"{case_id} agent input must be UTF-8 text: {path.relative_to(repo)}") from exc
        for pattern in ANSWER_MARKER_PATTERNS:
            match = pattern.search(text)
            if match:
                raise EvalError(
                    f"{case_id} agent input contains prohibited answer marker {match.group(0)!r} "
                    f"in {path.relative_to(repo)}"
                )


def load_and_validate_suite() -> tuple[dict[str, Any], dict[str, dict[str, Any]]]:
    validate_response_schema_document()
    suite = require_object(load_json(SUITE_PATH), "suite.json")
    require_exact_keys(suite, {"schema_version", "cases"}, "suite.json")
    if suite["schema_version"] != SCHEMA_VERSION:
        raise EvalError(f"suite.json schema_version must equal {SCHEMA_VERSION}")
    cases = suite["cases"]
    if not isinstance(cases, list) or len(cases) != len(CASE_CLASSES):
        raise EvalError(f"suite.json must contain exactly {len(CASE_CLASSES)} cases")
    case_ids: list[str] = []
    classes: list[str] = []
    oracles: dict[str, dict[str, Any]] = {}
    for index, raw_case in enumerate(cases):
        case = require_object(raw_case, f"suite.json cases[{index}]")
        require_exact_keys(case, {"id", "class", "repo", "oracle"}, f"suite.json cases[{index}]")
        case_id = require_text(case["id"], f"suite.json cases[{index}].id")
        if not CASE_ID_RE.fullmatch(case_id):
            raise EvalError(f"invalid case id: {case_id!r}")
        case_class = require_text(case["class"], f"suite.json cases[{index}].class")
        repo_relative = validate_relative_path(case["repo"], f"suite.json cases[{index}].repo")
        oracle_relative = validate_relative_path(case["oracle"], f"suite.json cases[{index}].oracle")
        repo_path = ROOT / repo_relative
        oracle_source_path = ROOT / oracle_relative
        if repo_path.is_symlink():
            raise EvalError(f"{case_id} repo path must not be a symlink")
        if oracle_source_path.is_symlink():
            raise EvalError(f"{case_id} oracle path must not be a symlink")
        repo = repo_path.resolve(strict=True)
        oracle_path = oracle_source_path.resolve(strict=True)
        if not oracle_path.is_file():
            raise EvalError(f"{case_id} oracle must be a real regular file")
        if repo in oracle_path.parents:
            raise EvalError(f"{case_id} oracle must be outside the agent input repository")
        validate_agent_input(repo, case_id)
        oracles[case_id] = validate_oracle(load_json(oracle_path), case=case, repo=repo)
        case_ids.append(case_id)
        classes.append(case_class)
    if len(case_ids) != len(set(case_ids)):
        raise EvalError("suite.json case ids must be unique")
    if tuple(case_ids) != CASE_CLASSES or tuple(classes) != CASE_CLASSES:
        raise EvalError("suite.json must contain the six canonical classes in canonical order")
    case_directories = sorted(path.name for path in (ROOT / "cases").iterdir() if path.is_dir())
    if case_directories != sorted(case_ids):
        raise EvalError("case directories and suite.json case ids do not match exactly")
    return suite, oracles


def validate_response(response: Any, *, case: dict[str, Any], repo: Path) -> dict[str, Any]:
    label = f"response for {case['id']}"
    response = require_object(response, label)
    require_exact_keys(response, {"schema_version", "case_id", "findings", "precision_controls"}, label)
    if response["schema_version"] != SCHEMA_VERSION:
        raise EvalError(f"{label} schema_version must equal {SCHEMA_VERSION}")
    if response["case_id"] != case["id"]:
        raise EvalError(f"{label} case_id does not match the selected case")
    findings = response["findings"]
    controls = response["precision_controls"]
    if not isinstance(findings, list):
        raise EvalError(f"{label} findings must be an array")
    if not isinstance(controls, list):
        raise EvalError(f"{label} precision_controls must be an array")
    finding_keys: set[tuple[str, str, str]] = set()
    finding_anchors: set[tuple[str, str]] = set()
    for index, raw_finding in enumerate(findings):
        item_label = f"{label} findings[{index}]"
        finding = require_object(raw_finding, item_label)
        require_exact_keys(finding, {"class", "path", "symbol", "summary", "evidence"}, item_label)
        finding_class = require_text(finding["class"], f"{item_label}.class")
        if finding_class not in CASE_CLASSES:
            raise EvalError(f"{item_label}.class is not a supported marker-free class")
        path = validate_relative_path(finding["path"], f"{item_label}.path")
        symbol = require_text(finding["symbol"], f"{item_label}.symbol")
        require_text(finding["summary"], f"{item_label}.summary", minimum=12)
        require_text(finding["evidence"], f"{item_label}.evidence", minimum=12)
        require_repo_anchor(repo, path, symbol, item_label)
        key = (finding_class, path, symbol)
        if key in finding_keys:
            raise EvalError(f"{label} contains a duplicate finding anchor: {key}")
        finding_keys.add(key)
        finding_anchors.add((path, symbol))
    control_keys: set[tuple[str, str]] = set()
    for index, raw_control in enumerate(controls):
        item_label = f"{label} precision_controls[{index}]"
        control = require_object(raw_control, item_label)
        require_exact_keys(control, {"path", "symbol", "reason"}, item_label)
        path = validate_relative_path(control["path"], f"{item_label}.path")
        symbol = require_text(control["symbol"], f"{item_label}.symbol")
        require_text(control["reason"], f"{item_label}.reason", minimum=12)
        require_repo_anchor(repo, path, symbol, item_label)
        key = (path, symbol)
        if key in control_keys:
            raise EvalError(f"{label} contains a duplicate precision-control anchor: {key}")
        if key in finding_anchors:
            raise EvalError(f"{label} cannot classify the same anchor as a finding and precision control: {key}")
        control_keys.add(key)
    return response


def entry_matches(item: dict[str, Any], expected: dict[str, Any], *, include_class: str | None = None) -> bool:
    if include_class is not None and item["class"] != include_class:
        return False
    return item["path"] == expected["path"] and item["symbol"] in expected["symbols"]


def score_response(response: dict[str, Any], *, case: dict[str, Any], oracle: dict[str, Any]) -> dict[str, Any]:
    expected_findings = oracle["expected_findings"]
    expected_controls = oracle["precision_controls"]
    consumed_finding_indices: set[int] = set()
    matched_findings: list[dict[str, Any]] = []
    for expected in expected_findings:
        match_index = next(
            (
                index
                for index, item in enumerate(response["findings"])
                if index not in consumed_finding_indices
                and entry_matches(item, expected, include_class=case["class"])
            ),
            None,
        )
        if match_index is not None:
            consumed_finding_indices.add(match_index)
            matched_findings.append(expected)
    unmatched_findings = [
        item
        for index, item in enumerate(response["findings"])
        if index not in consumed_finding_indices
    ]
    consumed_control_indices: set[int] = set()
    matched_controls: list[dict[str, Any]] = []
    for expected in expected_controls:
        match_index = next(
            (
                index
                for index, item in enumerate(response["precision_controls"])
                if index not in consumed_control_indices and entry_matches(item, expected)
            ),
            None,
        )
        if match_index is not None:
            consumed_control_indices.add(match_index)
            matched_controls.append(expected)
    unexpected_controls = [
        item
        for index, item in enumerate(response["precision_controls"])
        if index not in consumed_control_indices
    ]
    controls_as_findings = [
        item
        for item in response["findings"]
        if any(entry_matches(item, expected) for expected in expected_controls)
    ]
    recall_points = round(60 * len(matched_findings) / len(expected_findings))
    control_points = round(20 * len(matched_controls) / len(expected_controls))
    precision_points = 20 if not unmatched_findings else 0
    total = recall_points + control_points + precision_points

    def public_anchor(entry: dict[str, Any]) -> dict[str, Any]:
        return {"path": entry["path"], "symbols": entry["symbols"]}

    return {
        "case_id": case["id"],
        "class": case["class"],
        "score": total,
        "passed": total == 100,
        "points": {
            "finding_recall": recall_points,
            "precision_control": control_points,
            "unmatched-finding_precision": precision_points,
        },
        "matched_findings": [public_anchor(item) for item in matched_findings],
        "missed_findings": [
            public_anchor(item) for item in expected_findings if item not in matched_findings
        ],
        "matched_precision_controls": [public_anchor(item) for item in matched_controls],
        "missed_precision_controls": [
            public_anchor(item) for item in expected_controls if item not in matched_controls
        ],
        "unmatched_findings": unmatched_findings,
        "unexpected_precision_controls": unexpected_controls,
        "precision_controls_misclassified_as_findings": controls_as_findings,
    }


def case_map(suite: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {case["id"]: case for case in suite["cases"]}


def response_for_case(path: Path, *, case: dict[str, Any]) -> dict[str, Any]:
    repo = (ROOT / case["repo"]).resolve(strict=True)
    return validate_response(load_json(path), case=case, repo=repo)


def score_response_directory(responses: Path) -> dict[str, Any]:
    suite, oracles = load_and_validate_suite()
    if not responses.is_dir():
        raise EvalError(f"responses path is not a directory: {responses}")
    cases = case_map(suite)
    expected_names = {f"{case_id}.json" for case_id in cases}
    actual_names = {path.name for path in responses.iterdir() if path.is_file() and path.suffix == ".json"}
    missing = sorted(expected_names - actual_names)
    extra = sorted(actual_names - expected_names)
    if missing or extra:
        raise EvalError(f"response set mismatch: missing={missing}, extra={extra}")
    results: list[dict[str, Any]] = []
    for case_id, case in cases.items():
        response = response_for_case(responses / f"{case_id}.json", case=case)
        results.append(score_response(response, case=case, oracle=oracles[case_id]))
    average = round(sum(result["score"] for result in results) / len(results), 2)
    return {
        "schema_version": SCHEMA_VERSION,
        "case_count": len(results),
        "score": average,
        "passed": all(result["passed"] for result in results),
        "cases": results,
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("validate-suite", help="validate all repositories, hidden oracles, and schemas")
    validate = subparsers.add_parser("validate-response", help="validate one manual-agent response")
    validate.add_argument("--case", required=True, choices=CASE_CLASSES)
    validate.add_argument("--response", required=True, type=Path)
    score = subparsers.add_parser("score", help="validate and score one response JSON per case")
    score.add_argument("--responses", required=True, type=Path)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        if args.command == "validate-suite":
            suite, _ = load_and_validate_suite()
            result: dict[str, Any] = {
                "valid": True,
                "schema_version": SCHEMA_VERSION,
                "case_count": len(suite["cases"]),
                "cases": [case["id"] for case in suite["cases"]],
            }
        elif args.command == "validate-response":
            suite, _ = load_and_validate_suite()
            case = case_map(suite)[args.case]
            response_for_case(args.response, case=case)
            result = {"valid": True, "case_id": args.case}
        else:
            result = score_response_directory(args.responses)
    except EvalError as exc:
        print(f"marker-free eval error: {exc}", file=sys.stderr)
        return 2
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
