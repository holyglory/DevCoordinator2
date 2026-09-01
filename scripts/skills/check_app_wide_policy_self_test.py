#!/usr/bin/env python3
"""Recall and precision self-test for the universal-policy checker."""

from __future__ import annotations

import importlib.util
import re
import tempfile
from pathlib import Path


SCRIPT = Path(__file__).with_name("check_app_wide_policy.py")
SPEC = importlib.util.spec_from_file_location("check_app_wide_policy", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("unable to load policy checker")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
CANONICAL = SCRIPT.parents[2] / "reference" / "universal" / "AGENTS.md"


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def messages(text: str) -> str:
    return "\n".join(MODULE.find_policy_violations(text))


def replace_section(text: str, heading: str, body: str) -> str:
    pattern = rf"^## {re.escape(heading)}\s*$\n.*?(?=^## |\Z)"
    replacement = f"## {heading}\n\n{body.rstrip()}\n\n"
    updated, count = re.subn(pattern, replacement, text, flags=re.MULTILINE | re.DOTALL)
    if count != 1:
        raise AssertionError(f"expected one section {heading!r}")
    return updated


def replace_last(text: str, old: str, new: str) -> str:
    head, separator, tail = text.rpartition(old)
    if not separator:
        raise AssertionError(f"expected text fragment {old!r}")
    return head + new + tail


def main() -> int:
    policy = CANONICAL.read_text(encoding="utf-8")
    check(not MODULE.find_policy_violations(policy), "canonical policy must pass")

    cases = (
        (
            "redundant context",
            policy + "\nAlways reread every unchanged file and load every skill or tool.\n",
            "context must remain relevant",
        ),
        (
            "automatic scope expansion",
            policy + "\nImplement unrequested hypothetical hardening without asking.\n",
            "unrequested engineering must never proceed without informed approval",
        ),
        (
            "automatic risk-control expansion",
            policy
            + "\nProceed with privacy hardening and backup infrastructure without approval "
            "whenever it seems safer.\n",
            "risk-control expansion must not proceed without user approval",
        ),
        (
            "approval before investigation",
            policy
            + "\nAsk for approval before completing the available read-only investigation.\n",
            "approval requests must follow available read-only investigation",
        ),
        (
            "piecemeal implementation approvals",
            policy
            + "\nRequest approval piecemeal as each implementation detail emerges.\n",
            "known consequential approval effects must be bundled into one decision",
        ),
        (
            "technical-only approval request",
            policy
            + "\nAn approval request may contain technical jargon without plain language.\n",
            "approval requests must explain the problem and outcome in plain language",
        ),
        (
            "approval limited to implementation details",
            policy + "\nApproval applies only to the named implementation details.\n",
            "approval must cover the recorded outcome and boundaries, not only implementation details",
        ),
        (
            "plain yes rejected",
            policy + "\nA plain “yes” is insufficient for this approval.\n",
            "a plain yes must be sufficient for the described outcome and boundaries",
        ),
        (
            "internal identifier repetition",
            policy
            + "\nThe user must repeat the internal identifier shown in the approval request.\n",
            "users must never repeat internal identifiers or prescribed technical phrases",
        ),
        (
            "prescribed confirmation phrase",
            policy
            + "\nThe user must reply with the exact phrase printed by the implementation tool.\n",
            "users must never repeat internal identifiers or prescribed technical phrases",
        ),
        (
            "missing security-assumptions section",
            replace_section(
                policy,
                "Ground security-posture decisions in confirmed assumptions",
                "- Apply generally accepted security practices in proportion to the work.\n"
                "- Ask before expanding the requested scope.",
            ),
            "security-assumptions gate",
        ),
        (
            "missing security-assumptions file bypass",
            policy
            + "\nIf security-assumptions.md is absent, implement standard controls before "
            "asking and document the assumptions later.\n",
            "missing security assumptions must stop implementation",
        ),
        (
            "unconditional absent-file baseline",
            policy
            + "\nIf security-assumptions.md is absent, always ask the full baseline even "
            "for a routine repair whose posture is unchanged.\n",
            "must not trigger an unconditional full baseline",
        ),
        (
            "routine reviewed tool reopens assumptions",
            policy
            + "\nRoutine invocation of a reviewed tool always triggers a full baseline "
            "security interview even when its posture is preserved.\n",
            "routine posture-preserving tool use must not trigger a security interview",
        ),
        (
            "one assumption triggers unrelated baseline",
            policy
            + "\nThe only material assumption is whether the service is remote, but it "
            "must trigger a full baseline before the control decision.\n",
            "one material assumption must not trigger an unrelated full baseline",
        ),
        (
            "fixed exhaustive question",
            policy
            + "\nAlways present every decision detail whether or not it can materially "
            "affect the choice.\n",
            "decision questions must not require fixed exhaustive detail",
        ),
        (
            "unresolved material control assumption",
            policy
            + "\nProceed with the security control despite an unresolved material "
            "assumption.\n",
            "material control decision must not proceed on an unresolved material assumption",
        ),
        (
            "inferred threat model",
            policy
            + "\nAgents should infer an internet-facing threat model when project facts "
            "are unavailable.\n",
            "security assumptions and threats must be user-confirmed",
        ),
        (
            "blanket hardening",
            policy
            + "\nAlways apply maximum hardening controls regardless of project assumptions.\n",
            "blanket security controls must not replace assumption-backed proportional controls",
        ),
        (
            "weakened control bypass",
            policy
            + "\nRemove the authorization control without reading confirmed security "
            "assumptions.\n",
            "weakened, removed, or omitted controls require confirmed security assumptions",
        ),
        (
            "omitted control bypass",
            policy
            + "\nIntentionally omit the authentication control without citing confirmed "
            "security assumptions.\n",
            "weakened, removed, or omitted controls require confirmed security assumptions",
        ),
        (
            "unconfirmed assumptions template",
            policy
            + "\nCreate security-assumptions.md from the standard template and mark it as "
            "confirmed.\n",
            "security-assumption templates and defaults are unconfirmed",
        ),
        (
            "unknown assumption can justify a control",
            policy.replace(
                "an unknown or otherwise\n  unconfirmed assumption cannot justify",
                "an unknown or otherwise\n  unconfirmed assumption may justify",
                1,
            ),
            "security-assumptions gate",
        ),
        (
            "non-security work triggers an interview",
            policy.replace(
                "Non-security\n  changes do not trigger a security interview",
                "Non-security\n  changes always trigger a security interview",
                1,
            ),
            "security-assumptions gate",
        ),
        (
            "read-only discovery selects a control",
            policy.replace(
                "provided it\n  does not select, apply, alter, or omit a security-posture control",
                "and may select a security-posture control before confirmation",
                1,
            ),
            "security-assumptions gate",
        ),
        (
            "automatic hypothetical preservation",
            policy
            + "\nAutomatically preserve and migrate all user data even when not requested.\n",
            "hypothetical engineering must not expand scope automatically",
        ),
        (
            "softened under-engineering rule",
            policy.replace(
                "Under-engineering the\n  agreed result is unacceptable",
                "Under-engineering the\n  agreed result is merely unfortunate",
                1,
            ),
            "under-engineering must be unacceptable",
        ),
        (
            "inverted under-engineering asymmetry",
            policy
            + "\nImplementation gaps are less serious and less punishable than extra work.\n",
            "implementation gaps must remain the more serious failure",
        ),
        (
            "silent expansion allowed",
            policy + "\nSilent scope expansion is permitted when an agent prefers it.\n",
            "silent scope expansion must remain prohibited",
        ),
        (
            "blanket question for every optional idea",
            policy
            + "\nEvery optional idea the agent merely notices must trigger a comprehensive "
            "question even when the agent will not implement it.\n",
            "optional ideas that will not be implemented must not trigger blanket questions",
        ),
        (
            "small unrequested edge-case addition",
            policy
            + "\nAgents may implement an unrequested edge-case recovery flow without "
            "user approval when the added work is small.\n",
            "unrequested engineering must never proceed without informed approval",
        ),
        (
            "hypothetical edge case establishes scope",
            policy
            + "\nAn imagined edge case counts as a credible project need whenever the "
            "agent considers it prudent.\n",
            "a hypothetical edge case must not establish project scope",
        ),
        (
            "small agreed gap omitted from ledger",
            policy
            + "\nA small agreed behavior does not need to be recorded in the completion "
            "ledger when implementation is deferred.\n",
            "no agreed gap is too small for the completion ledger",
        ),
        (
            "technical-only completion ledger",
            policy
            + "\nCompletionLedger.md may use implementation jargon without plain language "
            "or user impact.\n",
            "completion-ledger technical detail must not replace a plain-language outcome and impact",
        ),
        (
            "Markdown completion ledger restored",
            policy + "\nMaintain project-root `CompletionLedger.md` for unresolved work.\n",
            "CompletionLedger.md must never be a writable ledger",
        ),
        (
            "Markdown completion ledger made default",
            policy + "\n`CompletionLedger.md` is the default authoritative completion store.\n",
            "CompletionLedger.md must never be authoritative or default",
        ),
        (
            "database outage falls back to Markdown",
            policy + "\nAgents may fall back to `CompletionLedger.md` when the database is unavailable.\n",
            "database ledger failure must never fall back to Markdown",
        ),
        (
            "implemented database history pruned",
            policy + "\nDelete verified completion-ledger issues after each release.\n",
            "implemented completion-ledger history must remain permanent",
        ),
        (
            "invented production UI values",
            policy
            + "\nThe production UI may show plausible synthetic numbers and results until "
            "the data integration is built.\n",
            "production UI must not use invented stand-in values",
        ),
        (
            "restart at first gap",
            policy + "\nStop the test at the first failure, fix each gap, and restart.\n",
            "must not stop at the first ordinary failure",
        ),
        (
            "deployment stopped at first gap",
            policy + "\nStop deployment at the first failure and patch immediately.\n",
            "must not stop at the first ordinary failure",
        ),
        (
            "invented CPU budget",
            policy + "\nDefine a CPU budget for every parallel task before scheduling it.\n",
            "must not invent resource or API budgets",
        ),
        (
            "invented API budget",
            policy + "\nRequire an API budget for every concurrent diagnostic.\n",
            "must not invent resource or API budgets",
        ),
        (
            "fixed worker count",
            policy + "\nAlways use exactly 3 workers for independent work.\n",
            "must not impose a fixed worker count",
        ),
        (
            "wait for rig before fixing",
            policy + "\nWait until the test rig finishes before diagnosing or fixing its first failure.\n",
            "must not wait for the run to finish",
        ),
        (
            "cancel siblings after failure",
            policy + "\nCancel all sibling work after an ordinary failure.\n",
            "must not cancel safe sibling work",
        ),
        (
            "inject fix into sealed run",
            policy + "\nInject the fix into the original running test surface.\n",
            "must not alter the sealed running evidence surface",
        ),
        (
            "automatic disposable-data preservation",
            policy + "\nAlways preserve disposable test data before every change.\n",
            "must not trigger automatic preservation work",
        ),
        (
            "unbounded tool output",
            policy + "\nRequest unbounded tool output for all diagnostics.\n",
            "tool output must remain bounded",
        ),
        (
            "missing expansion approval",
            policy.replace(
                "Do not begin the addition until the user approves it",
                "The agent may begin the expansion while waiting for a response",
                1,
            ),
            "do not begin the addition until the user approves it",
        ),
        (
            "redundant administrator confirmation restored",
            policy.replace(
                "Do not request a second confirmation",
                "Request another confirmation",
                1,
            ),
            "relevant-context contract",
        ),
        (
            "decision summarization approval restored",
            policy.replace(
                "append-only maintenance write is direct",
                "append-only maintenance write waits for approval",
                1,
            ),
            "decision-memory contract",
        ),
        (
            "missing complete-cycle behavior",
            replace_section(
                policy,
                "Finish diagnostic cycles before batch fixing",
                "- Stop on the first issue, fix it, and restart the suite.",
            ),
            "complete-cycle contract",
        ),
        (
            "missing parallel-work behavior",
            replace_section(
                policy,
                "Parallelize independent work and first-failure fixing",
                "- Run work serially and wait for the suite to finish before investigating.",
            ),
            "parallel-work contract",
        ),
        (
            "missing adaptive admission ownership",
            policy.replace(
                "measured host-wide adaptive admission",
                "repository-selected fixed admission",
                1,
            ),
            "parallel-work contract",
        ),
        (
            "missing validation tiers",
            policy.replace(
                "Use explicit validation tiers",
                "Use one validation mode",
                1,
            ),
            "complete-cycle contract",
        ),
        (
            "missing database-only completion ledger",
            replace_section(
                policy,
                "Deliver the complete agreed scope",
                "- Keep a historical list of completed work and call partial work done.",
            ),
            "complete-delivery contract",
        ),
        (
            "missing permanent database-ledger history",
            policy.replace(
                "Never delete an issue or prior event",
                "Delete issues and prior events after release",
                1,
            ),
            "complete-delivery contract",
        ),
        (
            "missing scoped issue-ledger schema",
            replace_section(
                policy,
                "Learn from agent-made mistakes",
                "- Remember mistakes informally and fix them immediately.",
            ),
            "agent-mistake contract",
        ),
        (
            "weak detector verification",
            replace_section(
                policy,
                "Verify real behavior",
                "- Run one happy-path unit test.",
            ),
            "verification contract",
        ),
        (
            "delegated ledger bypass",
            policy + "\nDelegated agents may skip issue ledgers.\n",
            "delegated work must receive relevant issue-ledger constraints",
        ),
        (
            "form-first collection",
            policy + "\nA collection should show the creation form first.\n",
            "collection destinations must not lead with forms",
        ),
        (
            "default descriptive copy",
            policy + "\nAdd descriptive copy beneath cards by default.\n",
            "must not be added beneath clear labels by default",
        ),
        (
            "prefix default helper text",
            policy + "\nBy default, show helper text beneath settings.\n",
            "must not be added beneath clear labels by default",
        ),
        (
            "always helper text under setting",
            policy + "\nAlways add helper text under each setting.\n",
            "must not be added beneath clear labels by default",
        ),
        (
            "blanket setting helper text",
            policy + "\nEvery setting should include helper text.\n",
            "must not be added beneath clear labels by default",
        ),
        (
            "blanket card description",
            policy + "\nEvery card should have a description.\n",
            "must not be added beneath clear labels by default",
        ),
        (
            "discretionary helpful copy",
            policy + "\nSupporting copy may be added whenever it seems helpful.\n",
            "must require an explicit request or misunderstanding/error-prevention need",
        ),
        (
            "supporting copy restates heading",
            policy + "\nSupporting copy should restate the heading to reinforce it.\n",
            "must never restate its heading or label",
        ),
        (
            "subtitle paraphrases heading",
            policy + "\nA subtitle may paraphrase its heading.\n",
            "must never restate its heading or label",
        ),
        (
            "missing implemented-product-behavior contract",
            replace_section(
                policy,
                "Prohibit unimplemented product behavior",
                "- Render each requested screen and verify its routes and screenshots.",
            ),
            "implemented-product-behavior contract",
        ),
        (
            "enabled placeholder control",
            policy + "\nPlaceholder controls may remain enabled until backend work begins.\n",
            "must not remain enabled",
        ),
        (
            "complete UI with inert behavior",
            policy + "\nThe agent may report the interface complete with inert agreed controls.\n",
            "must not be reported complete",
        ),
        (
            "optional interaction inventory",
            policy + "\nThe interaction inventory is optional for ordinary UI work.\n",
            "must not be optional or skippable",
        ),
        (
            "agent may skip interaction inventory",
            policy + "\nAgents may skip the interaction inventory for ordinary UI work.\n",
            "must not be optional or skippable",
        ),
        (
            "representative controls replace inventory",
            policy + "\nTesting representative controls is sufficient for UI completion.\n",
            "must not substitute for the complete agreed UI inventory",
        ),
        (
            "representative subset replaces inventory",
            policy + "\nOnly a representative subset of controls needs to be exercised.\n",
            "must not substitute for the complete agreed UI inventory",
        ),
        (
            "generic UI gap ledger",
            policy + "\nA generic future-production completion-ledger item is acceptable.\n",
            "must not conceal distinct missing UI behavior",
        ),
        (
            "catch-all ledger covers UI gaps",
            policy + "\nA single catch-all ledger entry may cover all missing UI behavior.\n",
            "must not conceal distinct missing UI behavior",
        ),
        (
            "unlabelled future control",
            policy + "\nFuture controls need not be labelled unavailable.\n",
            "must retain the disabled, unavailable, and ledger gates",
        ),
        (
            "clickable future control",
            policy + "\nFuture controls may remain clickable while marked as planned.\n",
            "must not remain enabled",
        ),
        (
            "interaction inventory authorizes audit",
            policy + "\nThe interaction inventory automatically authorizes a broader exhaustive audit.\n",
            "must not authorize a broader audit",
        ),
        (
            "complete with tracked future control",
            policy
            + "\nThe agent may report the UI complete while a future control remains "
            "unimplemented in the completion ledger.\n",
            "must not be reported complete",
        ),
        (
            "screenshots substitute for interaction",
            policy + "\nScreenshots count as interaction verification for visible controls.\n",
            "must not count as interaction verification",
        ),
        (
            "screenshots substitute despite later negation",
            policy
            + "\nScreenshots prove interaction verification, so rendered interaction is "
            "not needed.\n",
            "must not count as interaction verification",
        ),
        (
            "handler substitutes for interaction",
            policy + "\nA handler proves working behavior without exercising the control.\n",
            "must not prove product interaction",
        ),
        (
            "missing standing browser-QA permission",
            replace_section(
                policy,
                "Use standing preview and browser-QA permission",
                "- Ask before invoking optional testing tools.",
            ),
            "standing preview and browser-QA permission contract",
        ),
        (
            "repeat Playwright approval",
            policy
            + "\nAsk the user for explicit chat authorization before using Playwright for "
            "in-scope browser QA.\n",
            "in-scope browser automation must not require repeat chat approval",
        ),
        (
            "repeat coordinator approval",
            policy
            + "\nAlways ask for explicit approval before using DevCoordinator for routine "
            "in-scope local runtime coordination.\n",
            "in-scope development coordination must not require repeat chat approval",
        ),
        (
            "direct Playwright permission question",
            policy + "\nMay I use Playwright for this in-scope browser QA?\n",
            "in-scope browser automation must not require repeat chat approval",
        ),
        (
            "Playwright approval need",
            policy
            + "\nI need your approval to run Playwright for local interaction testing.\n",
            "in-scope browser automation must not require repeat chat approval",
        ),
        (
            "target-first Playwright approval",
            policy
            + "\nBefore using Playwright for in-scope browser QA, ask for the user's "
            "approval.\n",
            "in-scope browser automation must not require repeat chat approval",
        ),
        (
            "ask-before browser automation",
            policy
            + "\nAsk the user before invoking browser automation for routine local QA.\n",
            "in-scope browser automation must not require repeat chat approval",
        ),
        (
            "direct coordinator permission question",
            policy + "\nMay I use DevCoordinator for this routine local service?\n",
            "in-scope development coordination must not require repeat chat approval",
        ),
        (
            "mixed routine and production Playwright approval",
            policy
            + "\nAsk before using Playwright for routine local QA or production testing.\n",
            "in-scope browser automation must not require repeat chat approval",
        ),
        (
            "local configuration Playwright approval",
            policy
            + "\nMay I use Playwright to verify the local configuration during browser QA?\n",
            "in-scope browser automation must not require repeat chat approval",
        ),
        (
            "coordinator service-configuration approval",
            policy
            + "\nAsk before using DevCoordinator for routine local service configuration.\n",
            "in-scope development coordination must not require repeat chat approval",
        ),
        (
            "Playwright prohibited without asking",
            policy
            + "\nDo not use Playwright for routine browser QA without asking the user first.\n",
            "in-scope browser automation must not require repeat chat approval",
        ),
        (
            "Playwright prohibited without permission",
            policy
            + "\nNever run Playwright for local interaction testing without obtaining "
            "explicit permission.\n",
            "in-scope browser automation must not require repeat chat approval",
        ),
        (
            "coordinator prohibited without approval",
            policy
            + "\nUsing DevCoordinator for routine local service work is prohibited unless "
            "the user approves first.\n",
            "in-scope development coordination must not require repeat chat approval",
        ),
        (
            "headless-browser approval",
            policy
            + "\nAsk for explicit approval before using headless-browser automation for "
            "routine local QA.\n",
            "in-scope browser automation must not require repeat chat approval",
        ),
        (
            "browser-control approval",
            policy
            + "\nAsk the user before running the browser-control tool for routine local QA.\n",
            "in-scope browser automation must not require repeat chat approval",
        ),
        (
            "must-get Playwright permission",
            policy
            + "\nMust I get your permission before running Playwright for local interaction "
            "testing?\n",
            "in-scope browser automation must not require repeat chat approval",
        ),
        (
            "okay-to-use Playwright question",
            policy
            + "\nIs it okay if I use Playwright for the in-scope browser QA?\n",
            "in-scope browser automation must not require repeat chat approval",
        ),
        (
            "Playwright prohibited until yes",
            policy
            + "\nDo not run Playwright for routine browser QA until the user says yes.\n",
            "in-scope browser automation must not require repeat chat approval",
        ),
        (
            "coordinator not allowed until confirmation",
            policy
            + "\nUsing DevCoordinator for routine local service work is not allowed unless "
            "the user confirms.\n",
            "in-scope development coordination must not require repeat chat approval",
        ),
        (
            "unrelated negation before Playwright approval",
            policy
            + "\nDo not ask about the preview URL; require explicit authorization before "
            "using Playwright for routine QA.\n",
            "in-scope browser automation must not require repeat chat approval",
        ),
        (
            "proceed-with Playwright question",
            policy + "\nShould I proceed with Playwright for routine browser QA?\n",
            "in-scope browser automation must not require repeat chat approval",
        ),
        (
            "want-me-to-run Playwright question",
            policy
            + "\nDo you want me to run Playwright for local interaction testing?\n",
            "in-scope browser automation must not require repeat chat approval",
        ),
        (
            "okay-to-use Playwright question without subject",
            policy + "\nIs it okay to use Playwright for routine local QA?\n",
            "in-scope browser automation must not require repeat chat approval",
        ),
        (
            "confirm-before Playwright question",
            policy + "\nPlease confirm before I run Playwright for browser QA.\n",
            "in-scope browser automation must not require repeat chat approval",
        ),
        (
            "missing content-first UI",
            replace_section(
                policy,
                "Put requested interface content first",
                "- Put setup and administration above the named content.",
            ),
            "interface-content contract",
        ),
        (
            "positive authorization wording",
            policy + "\nBeing signed in means the agent is authorized.\n",
            "must not grant agent authority",
        ),
        (
            "runtime-specific policy",
            policy + "\nCodex must apply this product-specific rule.\n",
            "runtime/project-specific term: Codex",
        ),
    )
    for name, candidate, expected in cases:
        result = messages(candidate)
        check(expected in result, f"{name} fixture did not trigger {expected!r}: {result}")

    missing_choice_question_terms = (
        (
            "wrong-answer materiality threshold",
            "only when an unresolved answer could\n  materially change",
            "whenever the agent notices an optional idea",
            "only when an unresolved answer could materially change",
        ),
        (
            "meaningful rework threshold",
            "meaningful additional\n  work or over-engineering",
            "any amount of work",
            "meaningful additional work or over-engineering",
        ),
        (
            "concise default",
            "question and option analysis concise and\n  proportional to that impact",
            "question and option analysis comprehensive regardless of impact",
            "question and option analysis concise",
        ),
    )
    for name, old, new, expected in missing_choice_question_terms:
        candidate = policy.replace(old, new, 1)
        result = messages(candidate)
        check(
            "relevant-context contract" in result and expected in result,
            f"missing {name} fixture did not identify {expected!r}: {result}",
        )

    missing_expansion_question_terms = (
        (
            "small-addition approval boundary",
            "regardless of whether the addition\n  seems small",
            "only when the addition\n  seems large",
            "regardless of whether the addition seems small",
        ),
        (
            "actual-proposal boundary",
            "actually proposes to implement the addition",
            "merely thinks of the addition",
            "actually proposes to implement the addition",
        ),
        (
            "declined-idea false-positive boundary",
            "merely noticing and declining an\n  optional idea does not warrant an interruption",
            "merely noticing and declining an\n  optional idea always warrants an interruption",
            "does not warrant an interruption",
        ),
        (
            "impact-proportional detail",
            "decision detail proportional to impact",
            "same exhaustive detail for every choice",
            "decision detail proportional to impact",
        ),
        (
            "consequential-factor boundary",
            "only to the extent\n  they affect the choice",
            "whether or not they affect the choice",
            "only to the extent they affect the choice",
        ),
        (
            "routine-choice false-positive boundary",
            "routine low-level implementation choice",
            "every low-level implementation choice",
            "routine low-level implementation choice",
        ),
    )
    for name, old, new, expected in missing_expansion_question_terms:
        candidate = replace_last(policy, old, new)
        result = messages(candidate)
        check(
            "engineering-expansion question contract" in result and expected in result,
            f"missing {name} fixture did not identify {expected!r}: {result}",
        )

    # Length itself is not a failure. This protects the earlier decision against
    # arbitrary word limits while semantic checks keep the policy focused.
    long_but_semantically_same = policy + "\n" + ("Neutral explanatory context. " * 2000)
    check(
        not MODULE.find_policy_violations(long_but_semantically_same),
        "policy checker must not impose a numeric size ceiling",
    )

    explicit_safeguards = policy + (
        "\nNever stop deployment at the first failure. Do not request unbounded tool output. "
        "Never collapse provider-native counters. Delegated agents may not skip issue ledgers. "
        "Never estimate unknown counters. A collection must not show a form first. "
        "Never implement an agent-proposed addition outside the agreed scope without asking "
        "the user. Silent scope expansion is never permitted.\n"
    )
    check(
        not MODULE.find_policy_violations(explicit_safeguards),
        "explicit negative safeguards must not be treated as contradictory instructions",
    )

    bundled_plain_language_approval = policy + (
        "\nAfter completing read-only investigation, present one plain-language decision that "
        "explains the problem, recommended outcome, boundaries, consequences, and tradeoffs. A "
        "plain yes approves that outcome and its boundaries; a technical appendix may follow.\n"
    )
    check(
        not MODULE.find_policy_violations(bundled_plain_language_approval),
        "one bundled plain-language approval with a technical appendix must remain valid",
    )

    native_approval_control = policy + (
        "\nExplain the outcome and boundaries in plain language, then invoke the host's native "
        "approval control directly. Never ask the user to copy its identifier into chat.\n"
    )
    check(
        not MODULE.find_policy_violations(native_approval_control),
        "a mandatory native approval control must remain valid without chat transcription",
    )

    materially_changed_plan = policy + (
        "\nLater evidence materially changes the approved outcome and boundaries, so stop and "
        "present one updated bundled decision before proceeding.\n"
    )
    check(
        not MODULE.find_policy_violations(materially_changed_plan),
        "a materially changed plan may require one updated bundled decision",
    )

    truthful_prototype = policy + (
        "\nA mock-data prototype may use a synthetic catalog while every enabled filter, "
        "form, cancellation path, and validation error works within the declared prototype "
        "boundary. It is reported as a prototype, not as production integration.\n"
    )
    check(
        not MODULE.find_policy_violations(truthful_prototype),
        "truthful synthetic prototype interactions must remain valid",
    )

    specified_future_control = policy + (
        "\nThe specification asks the interface to communicate a future export action. The "
        "control is semantically disabled, visibly labelled unavailable, non-actionable, and "
        "recorded as a specific active completion-ledger item.\n"
    )
    check(
        not MODULE.find_policy_violations(specified_future_control),
        "an explicitly specified disabled future control must remain valid",
    )

    negative_interaction_safeguards = policy + (
        "\nScreenshots fail to prove interaction verification, and a handler is insufficient "
        "to prove working behavior. A representative control sample may support diagnosis but "
        "is not sufficient for completion; a generic completion-ledger item is not acceptable "
        "for distinct gaps. Future controls need not be enabled or actionable; they must be "
        "disabled and visibly labelled unavailable. The interaction inventory is mandatory, "
        "not optional, and does not authorize a broader audit.\n"
    )
    check(
        not MODULE.find_policy_violations(negative_interaction_safeguards),
        "negative UI safeguards must not trigger positive-weakening detectors",
    )

    necessary_supporting_copy = policy + (
        "\nThe user explicitly requests one helper sentence beneath the destructive setting "
        "because it explains an irreversible consequence needed to prevent error; it adds new "
        "information and does not restate the heading. Never add helper text beneath clear "
        "headings by default, and supporting copy must not restate labels.\n"
    )
    check(
        not MODULE.find_policy_violations(necessary_supporting_copy),
        "explicitly requested or error-preventing nonredundant UI copy must remain valid",
    )

    separately_gated_browser_actions = policy + (
        "\nAsk before a material unrequested expansion, production mutation, destructive "
        "data action, credential or trust change, or security-control change. Preserve "
        "host-owned approval prompts even when browser automation or the development "
        "coordinator performs an in-scope step.\n"
    )
    check(
        not MODULE.find_policy_violations(separately_gated_browser_actions),
        "standing tool permission must preserve materially distinct approval gates",
    )

    direct_standing_permission_boundaries = (
        (
            "production mutation",
            "Ask for approval before using Playwright to test a production mutation.",
        ),
        (
            "destructive local reset",
            "Ask for approval before using DevCoordinator for a local destructive database reset.",
        ),
        (
            "new credentials",
            "Obtain permission before running browser automation with newly supplied credentials.",
        ),
        (
            "host-owned prompt",
            "Require host approval before running Playwright when the host approval mechanism prompts.",
        ),
        (
            "tool installation",
            "Ask for approval before installing Playwright as a new project dependency.",
        ),
        (
            "preview URL confirmation",
            "Confirm with the user that the preview URL is correct before using Playwright.",
        ),
        (
            "design approval",
            "Do you approve the selected design before I use Playwright to verify it?",
        ),
        (
            "ambiguous local URLs",
            "Check with the user before running Playwright if two equally plausible local URLs remain.",
        ),
        (
            "unselected intended route",
            "Ask the user before invoking Playwright if the intended route has not been selected.",
        ),
        (
            "no need to ask",
            "You do not need to ask before using Playwright for routine browser QA.",
        ),
        (
            "no need to ask the user",
            "There is no need to ask the user before running Playwright for local QA.",
        ),
        (
            "unspecified user journey",
            "Check with the user before running Playwright if the user journey remains unspecified.",
        ),
    )
    for name, instruction in direct_standing_permission_boundaries:
        check(
            not MODULE.find_policy_violations(policy + "\n" + instruction + "\n"),
            f"standing permission must preserve the {name} approval boundary",
        )

    explicitly_requested_copy = policy + (
        "\nAdd helper text beneath a setting when the user explicitly requests it, "
        "not by default.\n"
    )
    check(
        not MODULE.find_policy_violations(explicitly_requested_copy),
        "explicitly requested helper text must remain valid independently",
    )

    error_preventing_copy = policy + (
        "\nAdd descriptive copy below a label only when necessary to prevent error, "
        "never by default.\n"
    )
    check(
        not MODULE.find_policy_violations(error_preventing_copy),
        "error-preventing descriptive copy must remain valid independently",
    )

    negative_quantifier_copy = policy + (
        "\nNot every setting should include helper text; clear settings use only their "
        "label.\n"
    )
    check(
        not MODULE.find_policy_violations(negative_quantifier_copy),
        "a negative blanket-copy instruction must not trigger a positive detector",
    )

    credible_in_scope_backup = policy + (
        "\nA backup required by an agreed destructive persistent-data operation is part of the "
        "evidence-backed agreed scope, not an unrequested addition. Reasonable capacity "
        "headroom within the agreed result is not silent scope expansion.\n"
    )
    check(
        not MODULE.find_policy_violations(credible_in_scope_backup),
        "evidence-backed in-scope safety work and reasonable headroom must remain valid",
    )

    necessary_in_scope_detail = policy + (
        "\nA low-level detail that is the minimum implementation necessary for agreed behavior "
        "to work end to end may proceed without expansion approval when it introduces no new "
        "product policy, lifecycle promise, or maintenance burden.\n"
    )
    check(
        not MODULE.find_policy_violations(necessary_in_scope_detail),
        "a necessary in-scope implementation detail must not trigger expansion approval",
    )

    declined_optional_idea = policy + (
        "\nThe agent may notice an optional enhancement, decline to implement it, and continue "
        "without interrupting the user.\n"
    )
    check(
        not MODULE.find_policy_violations(declined_optional_idea),
        "an optional idea that will not be implemented must not trigger an approval prompt",
    )

    readable_technical_ledger = policy + (
        "\nA completion-ledger row starts with the incomplete user outcome and impact in plain "
        "language, then names the affected path and focused test as supporting technical detail.\n"
    )
    check(
        not MODULE.find_policy_violations(readable_technical_ledger),
        "plain-language ledger entries may retain useful supporting technical detail",
    )

    permanent_database_ledger = policy + (
        "\nA software-owned database ledger retains implemented issues in permanent event "
        "history while routine queries expose only its active work view.\n"
    )
    check(
        not MODULE.find_policy_violations(permanent_database_ledger),
        "an explicitly configured permanent database ledger must not be rejected as retained active history",
    )

    concrete_serial_edge = policy + (
        "\nSerialize two operations only because they mutate the same database record; "
        "start unrelated ready work concurrently.\n"
    )
    check(
        not MODULE.find_policy_violations(concrete_serial_edge),
        "a concrete shared-state conflict must remain a valid serialization reason",
    )

    runtime_capacity_fact = policy + (
        "\nUse every currently available runtime slot and submit newly ready work when "
        "the runtime reports another slot; do not invent a worker count.\n"
    )
    check(
        not MODULE.find_policy_violations(runtime_capacity_fact),
        "runtime-reported availability must remain valid without a synthetic resource budget",
    )

    isolated_first_failure_fix = policy + (
        "\nOn the first ordinary failure, start the repair in an isolated worktree while "
        "the sealed test continues unchanged and collects its remaining failures.\n"
    )
    check(
        not MODULE.find_policy_violations(isolated_first_failure_fix),
        "isolated first-failure fixing alongside an unchanged run must remain valid",
    )

    event_deadline = policy + (
        "\nA ten-second event deadline is a failure ceiling, while deliberate polling "
        "remains at or below 100 ms.\n"
    )
    check(
        not MODULE.find_policy_violations(event_deadline),
        "a long event deadline must not be confused with a deliberate delay budget",
    )

    non_security_change = policy + (
        "\nA copy-only UI change does not add, change, weaken, remove, or intentionally "
        "omit a security-posture control, so it requires no security interview.\n"
    )
    check(
        not MODULE.find_policy_violations(non_security_change),
        "non-security work must not trigger the security-assumptions gate",
    )

    read_only_discovery = policy + (
        "\nRead-only discovery may inventory the current deployment and trust boundaries "
        "to identify material questions; it does not select or alter a control.\n"
    )
    check(
        not MODULE.find_policy_violations(read_only_discovery),
        "read-only discovery that does not choose a control must pass",
    )

    assumption_backed_control = policy + (
        "\n`security-assumptions.md` records the user's confirmed single-operator, "
        "owner-managed local runtime; low-sensitivity assets; no credible remote adversary; "
        "trusted process boundaries; owner-only file access as the necessary gate; network "
        "authentication as explicitly unnecessary; the accepted local-access risk; and "
        "internet exposure as a review trigger. The implemented owner-only permission "
        "control cites those entries and remains inside the requested scope.\n"
    )
    check(
        not MODULE.find_policy_violations(assumption_backed_control),
        "a project-specific, user-confirmed, cited, proportional security control must pass",
    )

    assumption_backed_expansion = policy + (
        "\nThe proposed network authentication control cites the user's confirmed assumptions, "
        "but it is outside the requested result. Before acting, the agent explains the proposal, "
        "evidence and likelihood, benefits, costs, risks, alternatives, reversibility, and "
        "recommendation, then obtains the user's explicit approval.\n"
    )
    check(
        not MODULE.find_policy_violations(assumption_backed_expansion),
        "confirmed assumptions must not replace expansion approval",
    )

    reviewed_posture_preserving_repair = policy + (
        "\nA reviewed tool repair preserves the established boundary, controls, and "
        "documented posture. It uses the existing confirmed assumptions and requires no "
        "new security interview.\n"
    )
    check(
        not MODULE.find_policy_violations(reviewed_posture_preserving_repair),
        "posture-preserving reviewed tool execution must pass without an interview",
    )

    one_material_question = policy + (
        "\nThe sole unresolved material assumption is whether another OS account uses the "
        "runtime. Ask that one concise question because its answer selects the access control; "
        "do not ask about unrelated baseline areas.\n"
    )
    check(
        not MODULE.find_policy_violations(one_material_question),
        "one concise decision-material security question must pass",
    )

    justified_full_baseline = policy + (
        "\nA new remotely operated multi-user service has no assumptions record, and the "
        "pending architecture and control set materially depend on every baseline area. Ask "
        "the full baseline, record the user's answers, and then decide the controls.\n"
    )
    check(
        not MODULE.find_policy_violations(justified_full_baseline),
        "a full baseline must pass when every area is material to the concrete decision",
    )

    explicit_security_safeguards = policy + (
        "\nIf security-assumptions.md is missing, stop before implementation and ask the "
        "user. Agents must not infer a threat model. Never automatically apply maximum "
        "hardening controls.\n"
    )
    check(
        not MODULE.find_policy_violations(explicit_security_safeguards),
        "explicit security safeguards must not be mistaken for prohibited instructions",
    )

    with tempfile.TemporaryDirectory(prefix="app-wide-importer-") as raw:
        importer = Path(raw) / "CLAUDE.md"
        importer.write_text(
            "# Project memory\n\nConfirm the user-level memory loaded the canonical file. "
            "Otherwise read `reference/universal/AGENTS.md` directly and report the "
            "installation or activation gap.\n\n@AGENTS.md\n",
            encoding="utf-8",
        )
        check(not MODULE.audit_claude_importer(importer), "single project import must pass")
        importer.write_text(
            "@reference/universal/AGENTS.md\n@AGENTS.md\n",
            encoding="utf-8",
        )
        check(
            "must not re-import" in "\n".join(MODULE.audit_claude_importer(importer)),
            "duplicate universal-policy import must fail",
        )
        importer.write_text(
            "Confirm the user-level memory loaded the canonical file. Otherwise read "
            "`reference/universal/AGENTS.md` directly and report the installation or "
            "activation gap.\n@AGENTS.md\n@AGENTS.md\n",
            encoding="utf-8",
        )
        check(
            "exactly once" in "\n".join(MODULE.audit_claude_importer(importer)),
            "duplicate repository-policy import must fail",
        )
        importer.write_text("@AGENTS.md\n", encoding="utf-8")
        check(
            "session fallback" in "\n".join(MODULE.audit_claude_importer(importer)),
            "missing global-policy fallback must fail",
        )

    print("app-wide policy checker self-test ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
