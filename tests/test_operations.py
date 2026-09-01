from __future__ import annotations

import pytest

from devcoordinator2.operations import OPERATIONS, policy_for


def test_decision_summarize_is_append_only_local_administration():
    policy = policy_for("decision.summarize")

    assert policy.scope == "repository"
    assert policy.role == "administrator"
    assert policy.effect == "append"
    assert policy.mcp_annotations() == {
        "readOnlyHint": False,
        "destructiveHint": False,
        "idempotentHint": False,
        "openWorldHint": False,
    }


def test_effect_hints_stay_truthful_for_read_destructive_and_external_operations():
    assert policy_for("decision.tail").mcp_annotations()["readOnlyHint"] is True
    assert policy_for("deployment.remove").mcp_annotations()["destructiveHint"] is True
    assert policy_for("telegram.subscribe").mcp_annotations()["openWorldHint"] is True
    assert policy_for("test.capacity.set").effect == "reversible"


def test_registry_contains_no_implicit_or_unknown_policy_values():
    assert len(OPERATIONS) == len(set(OPERATIONS))
    assert all(policy.scope in {"public", "self", "server", "repository", "deployment"}
               for policy in OPERATIONS.values())
    assert all(policy.role in {"anonymous", "self", "viewer", "operator", "administrator"}
               for policy in OPERATIONS.values())
    assert all(policy.effect in {"read", "append", "reversible", "destructive", "external"}
               for policy in OPERATIONS.values())

    with pytest.raises(KeyError, match="has no authority/effect policy"):
        policy_for("unregistered.command")
