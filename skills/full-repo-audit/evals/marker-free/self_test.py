#!/usr/bin/env python3
"""Self-test the marker-free evaluation schemas and scorer."""

from __future__ import annotations

import argparse
import json
import tempfile
from pathlib import Path

import score


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def expect_eval_error(function, message: str) -> None:
    try:
        function()
    except score.EvalError:
        return
    raise AssertionError(message)


def perfect_response(case: dict, oracle: dict) -> dict:
    gap = oracle["expected_findings"][0]
    control = oracle["precision_controls"][0]
    return {
        "schema_version": score.SCHEMA_VERSION,
        "case_id": case["id"],
        "findings": [
            {
                "class": case["class"],
                "path": gap["path"],
                "symbol": gap["symbols"][0],
                "summary": "The required observable behavior is incomplete.",
                "evidence": "The named source path does not carry the required operation to its outcome.",
            }
        ],
        "precision_controls": [
            {
                "path": control["path"],
                "symbol": control["symbols"][0],
                "reason": "The repository requirements explicitly define this behavior as intentional.",
            }
        ],
    }


def write_response(path: Path, response: dict) -> None:
    path.write_text(json.dumps(response, indent=2) + "\n", encoding="utf-8")


def run_quick_checks(suite: dict, oracles: dict[str, dict]) -> None:
    first = suite["cases"][0]
    repo = (score.ROOT / first["repo"]).resolve(strict=True)
    response = perfect_response(first, oracles[first["id"]])
    validated = score.validate_response(response, case=first, repo=repo)
    result = score.score_response(validated, case=first, oracle=oracles[first["id"]])
    check(result["score"] == 100 and result["passed"], "perfect response must score 100")

    malformed = dict(response)
    malformed["answer"] = "leaked"
    expect_eval_error(
        lambda: score.validate_response(malformed, case=first, repo=repo),
        "closed response schema must reject extra fields",
    )


def run_full_checks(suite: dict, oracles: dict[str, dict]) -> None:
    cases = score.case_map(suite)
    first = suite["cases"][0]
    repo = (score.ROOT / first["repo"]).resolve(strict=True)
    oracle = oracles[first["id"]]
    perfect = perfect_response(first, oracle)

    missed = dict(perfect)
    missed["findings"] = []
    missed_result = score.score_response(
        score.validate_response(missed, case=first, repo=repo),
        case=first,
        oracle=oracle,
    )
    check(missed_result["score"] == 40, "missing a gap must lose all finding-recall points")

    no_control = dict(perfect)
    no_control["precision_controls"] = []
    no_control_result = score.score_response(
        score.validate_response(no_control, case=first, repo=repo),
        case=first,
        oracle=oracle,
    )
    check(no_control_result["score"] == 80, "missing the precision control must lose control points")

    control = oracle["precision_controls"][0]
    false_positive = json.loads(json.dumps(perfect))
    false_positive["precision_controls"] = []
    false_positive["findings"].append(
        {
            "class": first["class"],
            "path": control["path"],
            "symbol": control["symbols"][0],
            "summary": "This intentional construct was incorrectly classified as incomplete.",
            "evidence": "The response cites a real source anchor but contradicts the requirements.",
        }
    )
    false_positive_result = score.score_response(
        score.validate_response(false_positive, case=first, repo=repo),
        case=first,
        oracle=oracle,
    )
    check(false_positive_result["score"] == 60, "a control false positive must lose both precision components")
    check(
        len(false_positive_result["precision_controls_misclassified_as_findings"]) == 1,
        "scorer must identify a precision control reported as a finding",
    )

    traversal = json.loads(json.dumps(perfect))
    traversal["findings"][0]["path"] = "../candidates.py"
    expect_eval_error(
        lambda: score.validate_response(traversal, case=first, repo=repo),
        "response paths must not traverse outside the agent input",
    )

    duplicate = json.loads(json.dumps(perfect))
    duplicate["findings"].append(dict(duplicate["findings"][0]))
    expect_eval_error(
        lambda: score.validate_response(duplicate, case=first, repo=repo),
        "duplicate finding anchors must be rejected",
    )

    with tempfile.TemporaryDirectory(prefix="marker-free-eval-self-test-") as temporary:
        response_dir = Path(temporary)
        for case_id, case in cases.items():
            write_response(response_dir / f"{case_id}.json", perfect_response(case, oracles[case_id]))
        aggregate = score.score_response_directory(response_dir)
        check(aggregate["score"] == 100 and aggregate["passed"], "perfect six-case response set must pass")
        (response_dir / f"{first['id']}.json").unlink()
        expect_eval_error(
            lambda: score.score_response_directory(response_dir),
            "incomplete response sets must be rejected instead of partially scored",
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--quick", action="store_true", help="run the schema/scorer smoke subset")
    args = parser.parse_args()
    suite, oracles = score.load_and_validate_suite()
    run_quick_checks(suite, oracles)
    if not args.quick:
        run_full_checks(suite, oracles)
    print("marker-free eval self-test ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
