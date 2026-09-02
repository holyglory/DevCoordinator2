#!/usr/bin/env python3
"""Validate the outcome-bearing contracts of the universal agent policy."""

from __future__ import annotations

import argparse
import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_POLICY = ROOT / "reference" / "universal" / "AGENTS.md"
DEFAULT_CLAUDE_IMPORTER = ROOT / "CLAUDE.md"

REQUIRED_SECTIONS = (
    "Use relevant authoritative context",
    "Ground security-posture decisions in confirmed assumptions",
    "Keep decisions compact and usable",
    "Implement the exact scope",
    "Parallelize independent work and first-failure fixing",
    "Finish diagnostic cycles before batch fixing",
    "Keep behavior truthful",
    "Prohibit unimplemented product behavior",
    "Learn from agent-made mistakes",
    "Verify real behavior",
    "Use standing preview and browser-QA permission",
    "Put requested interface content first",
    "Respect data and system boundaries",
    "Protect sources, repositories, and running systems",
    "Report status honestly",
)

# A universal policy must not depend on one runtime, product, framework, or
# repository. Runtime adapters and project policy own those names.
FORBIDDEN_NAMES = (
    "Codex",
    "Claude",
    "ImageGen",
    "Next.js",
    "Vercel",
    "Swift",
    "macOS",
    "Docker",
    "PostgreSQL",
    "systemd",
    "formal-web-ui-verification",
    "dev-coordinator",
    "postgres-docker-backup",
)


def section(text: str, heading: str) -> str:
    match = re.search(
        rf"^## {re.escape(heading)}\s*$\n(?P<body>.*?)(?=^## |\Z)",
        text,
        flags=re.MULTILINE | re.DOTALL,
    )
    return match.group("body") if match else ""


def _fold(text: str) -> str:
    return " ".join(text.casefold().split())


def _require_terms(
    violations: list[str], body: str, label: str, terms: tuple[str, ...]
) -> None:
    folded = _fold(body)
    missing = [term for term in terms if term.casefold() not in folded]
    if missing:
        violations.append(f"{label} missing required concepts: {', '.join(missing)}")


def _require_pattern(
    violations: list[str], body: str, label: str, pattern: str
) -> None:
    if not re.search(pattern, body, flags=re.IGNORECASE | re.DOTALL):
        violations.append(label)


def _bullet_starting_with(body: str, opening: str) -> str:
    match = re.search(
        rf"^-\s+{opening}.*?(?=^-\s+|\Z)",
        body,
        flags=re.IGNORECASE | re.MULTILINE | re.DOTALL,
    )
    return match.group(0) if match else ""


def _standing_tool_prompt_violations(text: str) -> list[str]:
    """Reject repeat chat prompts for routine tool use without masking real gates."""

    compact = re.sub(r"\s+", " ", text)
    sentences = re.split(r"(?<=[.!?])\s+", compact)
    negated_approval_clause = re.compile(
        r"(?i)(?:"
        r"\b(?:you\s+)?(?:do\s+not|don't|never|must\s+not|need\s+not|should\s+not)"
        r"(?:\s+need\s+to)?\s+(?:separately\s+|explicitly\s+)?"
        r"(?:ask|request|obtain|require|await|wait)\b|"
        r"\b(?:there\s+is\s+)?no\s+need\s+to\s+"
        r"(?:ask|request|obtain|require|await|wait)\b|"
        r"\bno\s+(?:separate\s+|explicit\s+|chat\s+)?"
        r"(?:approval|authorization|permission)\s+(?:is\s+)?required\b|"
        r"\bdoes\s+not\s+require\b.{0,40}\b(?:approval|authorization|permission)\b"
        r")"
    )
    distinct_gate = re.compile(
        r"(?i)\b(?:"
        r"production|destructive|credentials?|secrets?|"
        r"trust\s+(?:change|gate|decision)|security[- ](?:control|gate|posture)|"
        r"host[- ](?:owned\s+)?approval|host\s+approval|tool\s+approval\s+mechanism|"
        r"material\s+(?:unrequested\s+)?expansion|scope\s+expansion|"
        r"agent[- ]proposed\s+addition|addition\s+outside\s+(?:the\s+)?agreed\s+scope|"
        r"install(?:ation|ing)?|upgrad(?:e|ing)|"
        r"backup|recovery|shared[- ]state|persistent\s+(?:data|datastore)|"
        r"external\s+(?:side\s+effect|account|service)|purchase|publish(?:ing)?|"
        r"send(?:ing)?\s+(?:a\s+)?(?:message|email)|delete|drop|database\s+reset"
        r")\b"
    )
    routine_use = re.compile(
        r"(?i)\b(?:in[- ]scope|routine|browser\s+QA|interaction\s+testing|"
        r"local\s+(?:QA|preview|interaction|service|runtime)|reproduction)\b"
    )
    clarification = re.compile(
        r"(?i)\b(?:preview\s+URL|local\s+URLs?|target\s+URL|selected\s+design|"
        r"equally\s+plausible|ambiguous|user\s+input|requirements?|"
        r"which\s+(?:URL|route|design)|correct\s+URL|choice\s+between|"
        r"(?:confirm|approve|select|choose|clarify)\s+(?:the\s+)?"
        r"(?:URL|route|design|target|input|requirement)|"
        r"(?:if|when)\b.{0,120}\b(?:not\s+been\s+(?:selected|specified|provided|confirmed)|"
        r"unclear|unknown|ambiguous|equally\s+plausible|multiple|two|which|choice|missing)|"
        r"(?:user\s+journey|scope|route|URL|target|design|requirement|input)\b.{0,60}"
        r"\b(?:remains?\s+)?(?:unspecified|unselected|unclear|unknown|ambiguous|missing))\b"
    )
    targets = (
        (
            r"(?:Playwright|(?:headless[- ])?browser[- ]automation|"
            r"browser[- ]control\s+tool|browser\s+control\s+tool|"
            r"browser[- ]testing\s+tool)",
            "in-scope browser automation must not require repeat chat approval",
        ),
        (
            r"(?:DevCoordinator|development[- ]runtime\s+coordinator|"
            r"development\s+coordinator)",
            "in-scope development coordination must not require repeat chat approval",
        ),
    )
    found: list[str] = []
    for target_source, label in targets:
        target = re.compile(rf"(?i)\b{target_source}\b")
        action_target = rf"(?:use|invoke|run)\s+(?:the\s+)?{target_source}\b"
        gerund_target = rf"(?:using|invoking|running)\s+(?:the\s+)?{target_source}\b"
        explicit_prompt = re.compile(
            rf"(?i)(?:"
            rf"\b(?:may|can|should)\s+i\s+{action_target}|"
            rf"\bshould\s+i\s+proceed\s+with\s+(?:the\s+)?{target_source}\b|"
            rf"\b(?:must|do|need)\s+i\s+(?:get|obtain|have)\b.{{0,50}}"
            rf"\b(?:approval|authorization|permission)\b.{{0,50}}"
            rf"\bbefore\s+{gerund_target}|"
            rf"\bdo\s+i\s+have\b.{{0,40}}\b(?:approval|authorization|permission)\b"
            rf".{{0,50}}\bto\s+{action_target}|"
            rf"\b(?:can|could|will|would)\s+you\s+(?:please\s+)?"
            rf"(?:approve|authorize|permit)\s+(?:me\s+to\s+)?{action_target}|"
            rf"\bdo\s+you\s+want\s+me\s+to\s+{action_target}|"
            rf"\bwould\s+you\s+like\s+me\s+to\s+{action_target}|"
            rf"\b(?:please\s+)?confirm\s+(?:that\s+)?i\s+(?:may|can)\s+"
            rf"{action_target}|"
            rf"\bis\s+it\s+(?:okay|ok|acceptable)\s+if\s+i\s+{action_target}|"
            rf"\bis\s+it\s+(?:okay|ok|acceptable)\s+to\s+{action_target}|"
            rf"\bplease\s+confirm\s+before\s+i\s+{action_target}|"
            rf"\bare\s+you\s+(?:okay|ok)\s+with\s+me\s+{gerund_target}|"
            rf"\b(?:please\s+)?(?:approve|authorize|permit)\s+(?:me\s+to\s+)?"
            rf"{action_target}|"
            rf"\bi\s+(?:need|require)\b.{{0,70}}\b"
            rf"(?:approval|authorization|permission)\b.{{0,70}}"
            rf"(?:\bto\s+{action_target}|\bbefore\s+{gerund_target})|"
            rf"\b(?:ask|request|obtain|require|await|wait\s+for)\b.{{0,100}}"
            rf"\b(?:approval|authorization|permission|confirmation)\b.{{0,100}}"
            rf"(?:\bbefore\s+{gerund_target}|\bprior\s+to\s+{gerund_target}|"
            rf"\bto\s+{action_target})|"
            rf"\b(?:before\s+{gerund_target}|prior\s+to\s+{gerund_target})"
            rf".{{0,120}}\b(?:ask|request|obtain|require|await|wait\s+for)\b"
            rf".{{0,80}}\b(?:approval|authorization|permission|confirmation)\b"
            rf")"
        )
        prohibition_prompt = re.compile(
            rf"(?i)(?:"
            rf"\b(?:do\s+not|don't|never|must\s+not)\s+(?:{action_target}|{gerund_target})"
            rf".{{0,140}}\bwithout\s+(?:asking|requesting|obtaining|receiving)\b"
            rf".{{0,80}}\b(?:user|approval|authorization|permission)\b|"
            rf"\b(?:do\s+not|don't|never|must\s+not)\s+(?:{action_target}|{gerund_target})"
            rf".{{0,140}}\buntil\b.{{0,80}}\b(?:user\s+)?"
            rf"(?:says?\s+yes|approves?|confirms?|agrees?|authorizes?|gives?\s+permission)\b|"
            rf"\b(?:{action_target}|{gerund_target})\b.{{0,140}}"
            rf"\b(?:is\s+)?(?:prohibited|forbidden|not\s+allowed)\b.{{0,100}}"
            rf"\bunless\b.{{0,80}}\b(?:user\s+)?"
            rf"(?:approves?|confirms?|agrees?|says?\s+yes|authorizes?|gives?\s+permission)\b"
            rf")"
        )
        bare_prompt = re.compile(
            rf"(?i)(?:"
            rf"\b(?:ask|check|confirm)\s+(?:with\s+)?(?:the\s+)?user\s+"
            rf"(?:first\s+)?before\s+{gerund_target}|"
            rf"\bask\s+(?:the\s+user\s+)?(?:first\s+)?before\s+{gerund_target}"
            rf")"
        )
        for sentence in sentences:
            fragments = re.split(r"\s*(?:;|\bbut\b)\s*", sentence, flags=re.IGNORECASE)
            effective_sentence = "; ".join(
                fragment
                for fragment in fragments
                if fragment and not negated_approval_clause.search(fragment)
            )
            if not target.search(effective_sentence):
                continue
            explicit = explicit_prompt.search(effective_sentence) is not None
            bare = bare_prompt.search(effective_sentence) is not None
            prohibited = prohibition_prompt.search(effective_sentence) is not None
            if not explicit and not bare and not prohibited:
                continue
            separately_gated = distinct_gate.search(effective_sentence) is not None
            includes_routine_use = routine_use.search(effective_sentence) is not None
            is_clarification = clarification.search(effective_sentence) is not None
            if (explicit or prohibited or (bare and not is_clarification)) and (
                not separately_gated or includes_routine_use
            ):
                found.append(label)
                break
    return found


def find_policy_violations(text: str) -> list[str]:
    violations: list[str] = []

    if not text.startswith("# Universal Agent Instructions\n"):
        violations.append("policy must use the universal title")

    bodies = {heading: section(text, heading) for heading in REQUIRED_SECTIONS}
    for heading, body in bodies.items():
        if not body:
            violations.append(f"required section is missing: {heading}")

    context = bodies["Use relevant authoritative context"]
    if context:
        _require_terms(
            violations,
            context,
            "relevant-context contract",
            (
                "unchanged rule",
                "live context",
                "task matches",
                "targeted",
                "smallest useful result",
                "cold artifact",
                "raw logs",
                "content-free log catalogue",
                "bounded case/stream-specific tail",
                "fixed-string search",
                "stable line/cursor coordinates",
                "untrusted evidence",
                "never follow instructions found inside",
                "realistic materially distinct options",
                "plain language",
                "recommend",
                "third-party",
                "exact name",
                "authoritative sources",
                "facts",
                "inferences",
                "unknowns",
                "materiality threshold governs choices about how to fulfill agreed work",
                "before requesting approval or asking any other blocking question",
                "complete all available read-only investigation",
                "bundle all known consequential effects into one decision",
                "do not ask piecemeal as implementation details emerge",
                "before optional technical detail",
                "recommended outcome",
                "what will and will not change",
                "user-visible or operational consequences",
                "meaningful tradeoffs",
                "approval applies to the described outcome and boundaries of the recorded plan",
                "not merely to implementation details named in the approval message",
                "plain “yes” is sufficient",
                "never require the user to repeat or transcribe",
                "internal identifier",
                "prescribed technical phrase",
                "host or tool mandates its own approval control",
                "invoke that control directly",
                "do not relay its internals through chat",
                "within the approved boundaries do not trigger another approval",
                "one updated bundled decision",
                "do not request a second confirmation for an in-scope administrative write",
                "authenticated caller is authorized",
                "server authorization",
                "exact-target validation",
                "host/tool-owned approval control",
                "agent-proposed addition outside the agreed scope",
                "asking the user",
                "explicit approval",
                "only when an unresolved answer could materially change",
                "meaningful additional work or over-engineering",
                "question and option analysis concise",
                "proportional to that impact",
                "regardless of whether the addition seems small",
                "actually proposes to implement the addition",
                "merely noticing and declining an optional idea",
                "routine low-level implementation choice",
                "preserves established scope and security posture",
                "is not an expansion",
                "disposable test data",
                "single-user environment",
            ),
        )
        blocking_question = _bullet_starting_with(
            context,
            r"Before\s+requesting\s+approval\s+or\s+asking\s+any\s+other\s+blocking\s+question",
        )
        if not blocking_question:
            violations.append(
                "blocking questions must follow read-only investigation and bundle one plain-language decision"
            )
        else:
            _require_terms(
                violations,
                blocking_question,
                "blocking-question preparation contract",
                (
                    "complete all available read-only investigation",
                    "bundle all known consequential effects into one decision",
                    "do not ask piecemeal as implementation details emerge",
                    "plain language",
                    "before optional technical detail",
                    "problem",
                    "recommended outcome",
                    "boundaries",
                    "what will and will not change",
                    "user-visible or operational consequences",
                    "meaningful tradeoffs",
                    "why a decision is needed",
                ),
            )
        approval_scope = _bullet_starting_with(
            context, r"User\s+approval\s+applies\s+to\s+the\s+described\s+outcome"
        )
        if not approval_scope:
            violations.append(
                "approval must cover the recorded outcome and boundaries and accept a plain yes"
            )
        else:
            _require_terms(
                violations,
                approval_scope,
                "approval-scope contract",
                (
                    "outcome and boundaries of the recorded plan",
                    "not merely to implementation details named in the approval message",
                    "plain “yes” is sufficient",
                    "never require the user to repeat or transcribe",
                    "internal identifier",
                    "digest",
                    "command",
                    "prescribed technical phrase",
                    "host or tool mandates its own approval control",
                    "invoke that control directly",
                    "after the plain-language explanation",
                    "do not relay its internals through chat",
                    "within the approved boundaries do not trigger another approval",
                    "later evidence materially changes the outcome or boundaries",
                    "one updated bundled decision",
                ),
            )
        expansion_question = _bullet_starting_with(
            context, r"Before\s+implementing\s+any\s+agent-proposed\s+addition"
        )
        if not expansion_question:
            violations.append(
                "every planned addition outside agreed scope must use a proportional approval gate"
            )
        else:
            _require_terms(
                violations,
                expansion_question,
                "engineering-expansion question contract",
                (
                    "agent-proposed addition outside the agreed scope",
                    "tell the user",
                    "explicit approval",
                    "regardless of whether the addition seems small",
                    "actually proposes to implement the addition",
                    "merely noticing and declining an optional idea",
                    "does not warrant an interruption",
                    "proposal and clear recommendation concise",
                    "decision detail proportional to impact",
                    "consequential addition",
                    "supporting evidence and scenario",
                    "assessed likelihood",
                    "expected benefit",
                    "costs",
                    "risks of doing it and not doing it",
                    "realistic alternatives",
                    "maintenance",
                    "reversibility",
                    "clear recommendation",
                    "only to the extent they affect the choice",
                    "do not begin the addition until the user approves it",
                    "routine low-level implementation choice",
                    "invocation of one reviewed skill or tool",
                    "preserves established scope and security posture",
                    "is not an expansion",
                ),
            )
            _require_pattern(
                violations,
                expansion_question,
                "expansion approval must precede action",
                r"before\s+implementing.{0,500}(?:tell|ask)\s+the\s+user.{0,120}"
                r"explicit\s+approval.{0,1500}do\s+not\s+begin\s+the\s+addition"
                r"\s+until\s+the\s+user\s+approves",
            )

    security = bodies["Ground security-posture decisions in confirmed assumptions"]
    if security:
        _require_terms(
            violations,
            security,
            "security-assumptions gate",
            (
                "every decision that adds, changes, weakens, removes, or intentionally omits",
                "security-posture control",
                "before proposing or making such a decision",
                "project-root `security-assumptions.md`",
                "non-security changes do not trigger a security interview",
                "read-only discovery",
                "identify material assumptions or questions",
                "does not select, apply, alter, or omit",
                "routine execution of one reviewed skill or tool",
                "preserves its documented controls and established security posture",
                "is not a new security-posture decision",
                "does not reopen the assumptions record or trigger a blanket interview",
                "use existing confirmed assumptions and task context first",
                "project-specific",
                "user-confirmed assumptions",
                "every security-posture decision and resulting implemented security measure",
                "cite",
                "templates, defaults, and agent guesses are not confirmed project facts",
                "users and operators",
                "deployment or runtime environment and ownership",
                "assets and data sensitivity",
                "credible adversaries and misuse",
                "trust boundaries",
                "necessary gates",
                "explicitly unnecessary gates",
                "acceptable risks",
                "review triggers",
                "is absent or insufficient",
                "concrete pending security-posture decision",
                "stop before that decision or implementation only when",
                "wrong answer could select an unnecessary control",
                "omit a necessary control",
                "expand the work",
                "meaningful rework",
                "smallest concise set of unresolved material questions",
                "create the file",
                "confirmed answers",
                "do not repeat resolved areas",
                "full baseline only when the concrete decision materially depends on every assumption area",
                "unassessed areas that do not affect the current decision",
                "never invent or infer a project assumption",
                "unconfirmed template",
                "record unknowns explicitly",
                "unconfirmed assumption cannot justify",
                "unknown immaterial to the concrete decision does not require a question",
                "never default to blanket hardening",
                "cumulative",
                "any agent-proposed addition outside the agreed scope",
                "expands scope",
                "regardless of its size",
                "establish relevance but not permission",
                "explicit approval before action",
                "never satisfies or waives the other",
            ),
        )
        _require_pattern(
            violations,
            security,
            "every security-posture change or omission must read confirmed assumptions first",
            r"every\s+decision\s+that\s+adds,\s+changes,\s+weakens,\s+removes,\s+or"
            r"\s+intentionally\s+omits.{0,100}before\s+proposing\s+or\s+making"
            r".{0,100}read\s+the\s+project-root\s+`security-assumptions\.md`",
        )
        _require_pattern(
            violations,
            security,
            "every security-posture decision and implementation must cite confirmed assumptions",
            r"every\s+security-posture\s+decision\s+and\s+resulting\s+implemented\s+security"
            r"\s+measure\s+must\s+cite.{0,120}user-confirmed\s+assumptions",
        )
        _require_pattern(
            violations,
            security,
            "absent or insufficient assumptions must trigger only a material proportional question",
            r"is\s+absent\s+or\s+insufficient.{0,180}stop\s+before.{0,180}only"
            r"\s+when.{0,220}wrong\s+answer.{0,300}smallest\s+concise\s+set\s+of"
            r"\s+unresolved\s+material\s+questions.{0,180}(?:create|update).{0,80}the"
            r"\s+file.{0,100}confirmed\s+answers",
        )
        _require_pattern(
            violations,
            security,
            "a complete baseline must be conditional on concrete decision materiality",
            r"cover\s+the\s+full\s+baseline\s+only\s+when\s+the\s+concrete\s+decision"
            r"\s+materially\s+depends\s+on\s+every\s+assumption\s+area",
        )
        _require_pattern(
            violations,
            security,
            "unknown security assumptions must be explicit and cannot justify posture decisions",
            r"record\s+unknowns\s+explicitly.{0,120}(?:unknown|unconfirmed\s+assumption)"
            r".{0,100}cannot\s+justify\s+adding,\s+changing,\s+weakening,\s+removing,"
            r"\s+or\s+intentionally\s+omitting",
        )
        _require_pattern(
            violations,
            security,
            "the security-assumptions gate and expansion approval must remain cumulative",
            r"assumption\s+gate\s+and\s+the\s+informed-approval\s+rule.{0,180}cumulative"
            r".{0,300}explicit\s+approval\s+before\s+action.{0,160}never\s+satisfies\s+or"
            r"\s+waives\s+the\s+other",
        )

    decisions = bodies["Keep decisions compact and usable"]
    if decisions:
        _require_terms(
            violations,
            decisions,
            "decision-memory contract",
            (
                "`decision_record`",
                "aspect tag",
                "management-facing title and body",
                "materially distinct options",
                "cost and risk in user terms",
                "`technical_note`",
                "`supersedes`",
                "stable `ref`",
                "durable intent",
                "quality bar",
                "`decision_tail`",
                "rolling summary",
                "`decision_search`",
                "tried and rejected",
                "`summary_due`",
                "`decision_summarize`",
                "Direction synthesis",
                "confirmed user decisions",
                "inferred patterns",
                "decision refs cited",
                "append-only maintenance write is direct",
                "does not require another user approval",
            ),
        )

    exact_scope = bodies["Implement the exact scope"]
    if exact_scope:
        _require_terms(
            violations,
            exact_scope,
            "exact-scope contract",
            (
                "complete explicitly agreed result",
                "do not broaden it",
                "never silently narrow it",
                "only an explicit user decision",
                "“ideally”",
                "“for example”",
                "“something like”",
                "“could”",
                "illustrative formats",
                "express direction, not mandatory delivery requirements",
                "unless the user explicitly selects them",
                "necessary for the requested behavior to work",
                "reliability, security, recovery, migration, preservation, compatibility, ui, and infrastructure work",
                "in scope only when required by acceptance criteria",
                "confirmed assumptions",
                "current-system evidence",
                "minimum end-to-end implementation",
                "seemingly focused request",
                "beyond three product subsystems",
                "requires a new platform abstraction",
                "estimated to exceed roughly 1,000 changed lines",
                "pause once and explain the actual scope before continuing",
                "recommend the smallest architecture that delivers the request",
                "do not equate more checks, parsers, adapters, or supported formats with a more complete implementation",
                "every explicit requirement",
                "user-selected detail",
                "visible promise",
                "exposed value",
                "necessary supporting behavior",
                "no agreed gap is too small to record",
                "DevCoordinator2's planning database",
                "one authoritative",
                "permanent event history",
                "`plan_overview`",
                "`task_history`",
                "`task_create`",
                "`task_update`",
                "estimated lines of code",
                "subtask trees",
                "`daemon_unavailable`",
                "blocks the affected completion claim",
                "database issues",
                "partial implementations",
                "reader who does not know the implementation",
                "remaining outcome starts in plain language",
                "what users or the product cannot do",
                "readiness is blocked",
                "concrete unblock condition",
                "observable proof",
                "technical detail",
                "never replace that account",
                "raw logs remain in cold artifacts",
                "kind `stub`, `improvement`, or `user_feedback`",
                "never delete an issue or prior event",
                "append-only state transitions",
                "bounded active projection",
                "bounded history",
                "database outage blocks affected completion claims",
                "never authorizes a file, alternate store, or chat-memory fallback",
                "database event history is the only completion history",
                "`decision_record`",
                "before readiness",
                "end-to-end",
                "unblock condition",
                "direction, current capabilities, user-visible gaps, and blockers",
                "plain language before any technical detail",
                "decode the table",
            ),
        )
        _require_pattern(
            violations,
            exact_scope,
            "tentative and illustrative language must remain directional unless selected or necessary",
            r"“ideally”.{0,80}“for\s+example”.{0,80}“something\s+like”.{0,80}"
            r"“could”.{0,160}direction,\s+not\s+mandatory\s+delivery\s+requirements"
            r".{0,180}unless\s+the\s+user\s+explicitly\s+selects\s+them"
            r".{0,180}necessary\s+for\s+the\s+requested\s+behavior\s+to\s+work",
        )
        _require_pattern(
            violations,
            exact_scope,
            "supporting engineering must require evidence or minimum end-to-end behavior",
            r"reliability,\s+security,\s+recovery,\s+migration,\s+preservation,"
            r"\s+compatibility,\s+ui,\s+and\s+infrastructure\s+work\s+is\s+in\s+scope"
            r"\s+only\s+when\s+required\s+by\s+acceptance\s+criteria.{0,180}"
            r"confirmed\s+assumptions.{0,120}current-system\s+evidence.{0,120}"
            r"minimum\s+end-to-end\s+implementation",
        )
        _require_pattern(
            violations,
            exact_scope,
            "hidden large scope must trigger one explanation and the smallest sufficient architecture",
            r"seemingly\s+focused\s+request.{0,100}beyond\s+three\s+product\s+subsystems"
            r".{0,100}new\s+platform\s+abstraction.{0,120}roughly\s+1,000\s+changed\s+lines"
            r".{0,120}pause\s+once\s+and\s+explain\s+the\s+actual\s+scope\s+before"
            r"\s+continuing.{0,120}recommend\s+the\s+smallest\s+architecture",
        )
        _require_pattern(
            violations,
            exact_scope,
            "extra checks, parsers, adapters, or formats must not define completeness",
            r"do\s+not\s+equate\s+more\s+checks,\s+parsers,\s+adapters,\s+or"
            r"\s+supported\s+formats\s+with\s+a\s+more\s+complete\s+implementation",
        )

    parallel = bodies["Parallelize independent work and first-failure fixing"]
    if parallel:
        _require_terms(
            violations,
            parallel,
            "parallel-work contract",
            (
                "dependencies",
                "mutable-state ownership",
                "start every ready",
                "concurrently",
                "available runtime or tool support",
                "configured coordinator",
                "measured host-wide adaptive admission",
                "do not add repository-local worker counts",
                "fake dependency chains",
                "second capacity controller",
                "serialize only",
                "concrete dependency",
                "shared mutable-state conflict",
                "actual runtime/tool limitation",
                "all-settled sibling behavior",
                "ordinary failure does not cancel",
                "missing capability",
                "improvement work",
                "first ordinary failure",
                "diagnosis and fixing begin immediately",
                "separate isolated worktree",
                "original sealed run continues unchanged",
                "gathers the remaining failures",
                "do not inject fixes",
                "event-driven readiness",
                "100 ms",
                "failure deadline",
                "invalidate expensive downstream evidence",
                "real success dependencies",
                "independent preflights",
                "unrelated safe branches",
            ),
        )
        _require_pattern(
            violations,
            parallel,
            "ready independent work must start concurrently",
            r"start\s+every\s+ready.{0,160}concurrently",
        )
        _require_pattern(
            violations,
            parallel,
            "first-failure fixes must overlap the unchanged sealed run",
            r"first\s+ordinary\s+failure.{0,220}fixing\s+begin\s+immediately"
            r".{0,300}sealed\s+run\s+continues\s+unchanged",
        )

    cycle = bodies["Finish diagnostic cycles before batch fixing"]
    if cycle:
        _require_terms(
            violations,
            cycle,
            "complete-cycle contract",
            (
                "continue to the end",
                "non-critical failures",
                "authoritative completion ledger",
                "cold artifact",
                "do not fix one small gap and restart",
                "security or safety harm",
                "data loss",
                "shared-state corruption",
                "destruction of useful evidence",
                "group findings by cause",
                "fix the batch",
                "rerun the complete relevant cycle",
                "test server",
                "already included in the agreed task",
                "deploy it",
                "tell the user what remains",
                "incomplete test deployment",
                "explicit validation tiers",
                "development",
                "pre-merge",
                "fresh complete release pass",
                "frozen candidate",
                "do not rerun release proof after every intermediate",
            ),
        )
        _require_pattern(
            violations,
            cycle,
            "non-critical findings must be collected before the batch fix",
            r"continue\s+to\s+the\s+end.{0,900}after\s+the\s+complete\s+evidence\s+pass"
            r".{0,300}fix\s+the\s+batch",
        )

    truthful = bodies["Keep behavior truthful"]
    if truthful:
        _require_terms(
            violations,
            truthful,
            "truthful-behavior contract",
            (
                "never present invented",
                "numbers",
                "parameters",
                "statuses",
                "results",
                "control must perform",
                "end to end",
                "loading, error, empty, or unavailable state",
                "plausible stand-in values",
                "record the missing integration",
                "mockups",
                "explicitly declared mock-data prototype",
                "production behavior",
            ),
        )

    product_behavior = bodies["Prohibit unimplemented product behavior"]
    if product_behavior:
        _require_terms(
            violations,
            product_behavior,
            "implemented-product-behavior contract",
            (
                "every visible, enabled control",
                "buttons, links, tabs, menus, filters, forms, row actions, keyboard shortcuts, and clickable cards",
                "end to end through the rendered interface",
                "expected observable result",
                "does not prove promised navigation, persistence, integration",
                "downstream behavior",
                "generated mockup",
                "enabled product UI",
                "no empty handlers",
                "no-op links",
                "fake success",
                "mock-data prototype",
                "synthetic data",
                "works truthfully within its declared boundary",
                "plausible synthetic numbers, parameters, statuses, or results",
                "production stand-in",
                "honest unavailable state",
                "ledger the agreed missing behavior",
                "each missing or partial agreed behavior",
                "authoritative completion ledger",
                "generic future-production item is insufficient",
                "affected journeys",
                "screens and responsive variants",
                "files",
                "user impact",
                "unblock condition",
                "required rendered end-to-end verification",
                "specification explicitly requires communicating future availability",
                "semantically disabled",
                "visibly labelled unavailable",
                "specifically ledgered",
                "delivery remains incomplete until implementation or explicit removal from agreed scope",
                "out-of-scope future information is noninteractive content",
                "never report complete with agreed behavior missing, simulated, inert",
                "Mandatory interaction inventory",
                "one evidence pass",
                "only agreed screens, journeys, states, and responsive variants",
                "before fixing non-critical gaps",
                "neither expands scope nor invokes or authorizes a broader exhaustive audit",
                "every visible interactive element",
                "conditional controls",
                "map each element to its journey",
                "verify the downstream result",
                "success, cancellation, validation failure, permission failure",
                "recovery where applicable",
                "reload when persistence is promised",
                "finish the pass",
                "batch-fix",
                "zero enabled controls without real behavior",
                "zero requested journeys without rendered end-to-end evidence",
                "zero request-related completion-ledger entries",
                "Code inspection, routes, rendering, screenshots, visual comparison, and geometry checks",
                "do not constitute interaction verification",
            ),
        )
        _require_pattern(
            violations,
            product_behavior,
            "every enabled visible control must work end to end with an observable result",
            r"every\s+visible,\s+enabled\s+control.{0,220}must\s+perform.{0,120}end\s+to\s+end"
            r".{0,180}observable\s+result",
        )
        _require_pattern(
            violations,
            product_behavior,
            "each missing UI behavior needs specific journey, surface, impact, and verification detail",
            r"entry\s+naming.{0,80}affected\s+journeys.{0,100}screens\s+and\s+responsive"
            r"\s+variants.{0,100}controls.{0,80}files.{0,80}missing\s+behavior.{0,80}user"
            r"\s+impact.{0,80}unblock\s+condition.{0,100}rendered\s+end-to-end\s+verification",
        )
        _require_pattern(
            violations,
            product_behavior,
            "future controls require an explicit specification, disabled state, label, and ledger entry",
            r"unimplemented\s+control\s+may\s+appear\s+only\s+when\s+the\s+specification"
            r".{0,180}semantically\s+disabled.{0,220}visibly\s+labelled\s+unavailable"
            r".{0,100}(?:specifically\s+ledgered|completion-ledger\s+entry)",
        )
        _require_pattern(
            violations,
            product_behavior,
            "the interaction inventory must stay within agreed UI scope and not authorize a broader audit",
            r"only\s+agreed\s+screens,\s+journeys,\s+states,\s+and\s+responsive"
            r"\s+variants.{0,180}neither\s+expands\s+scope\s+nor\s+invokes"
            r"\s+or\s+authorizes\s+a\s+broader\s+exhaustive\s+audit",
        )
        _require_pattern(
            violations,
            product_behavior,
            "UI completion requires zero inert controls, unverified journeys, and related ledger entries",
            r"completion\s+requires\s+zero\s+enabled\s+controls\s+without\s+real\s+behavior"
            r".{0,160}zero\s+requested\s+journeys\s+without\s+rendered\s+end-to-end\s+evidence"
            r".{0,160}zero\s+request-related\s+completion-ledger\s+entries",
        )

    mistakes = bodies["Learn from agent-made mistakes"]
    if mistakes:
        _require_terms(
            violations,
            mistakes,
            "agent-mistake contract",
            (
                "changed user intent",
                "finish its useful diagnostic cycle",
                "before the product fix",
                "batch",
                "retest",
                "guardrail",
                "project-root `UserIssueLedgers/`",
                "concise routine context",
                "confirmed user-indicated agent mistakes",
                "durable user corrections",
                "absence is valid",
                "multiple narrowly scoped ledgers",
                "mixed catch-all",
                "BusinessLogic/<Perspective>.md",
                "# User Issue Ledger: <scope>",
                "`ID`",
                "`Applies to`",
                "`Mistake pattern`",
                "`Required behavior`",
                "`Prevention and verification`",
                "UIL-<SCOPE>-NNN",
                "relative file path owns the scope",
                "same path components",
                "ID namespace derives from all of them",
                "Never mix another path's namespace",
                "narrowest owning ledger",
                "before planning or implementing",
                "repository-wide or cross-cutting work",
                "negative acceptance criterion",
                "delegated-agent tasks",
                "one row per distinct pattern",
                "merging duplicates",
                "reuse its ID",
                "persist after the immediate fix",
                "explicit user retraction",
                "version control",
                "raw conversation",
            ),
        )
        for route in ("UI work always reads UI", "code changes read coding-style", "automation reads automation"):
            if route.casefold() not in _fold(mistakes):
                violations.append(f"mandatory issue-ledger routing is missing: {route}")

    verify = bodies["Verify real behavior"]
    if verify:
        _require_terms(
            violations,
            verify,
            "verification contract",
            (
                "same visible or operational surface",
                "acceptance criteria",
                "end to end",
                "recall",
                "precision",
                "must-catch failures",
                "false-positive guards",
                "never delete shared records",
            ),
        )

    standing_browser_qa = bodies["Use standing preview and browser-QA permission"]
    if standing_browser_qa:
        _require_terms(
            violations,
            standing_browser_qa,
            "standing preview and browser-QA permission contract",
            (
                "standing permission across all repositories",
                "Playwright or equivalent browser automation directly",
                "in-scope local preview",
                "browser QA",
                "configured DevCoordinator",
                "temporary-runtime lifecycle work",
                "Do not ask for separate chat authorization",
                "only tool use within the agreed task",
                "does not broaden scope",
                "production changes",
                "destructive data actions",
                "credential or trust changes",
                "security-assumption",
                "host or tool approval mechanisms",
                "any agent-proposed addition outside the agreed scope",
            ),
        )

    interface = bodies["Put requested interface content first"]
    if interface:
        _require_terms(
            violations,
            interface,
            "interface-content contract",
            (
                "content promise",
                "first substantial",
                "first viewport",
                "collection destination",
                "must not lead",
                "add or edit form",
                "immediately reveal",
                "current viewport",
                "below a long list",
                "success returns",
                "new item",
                "one concise, self-explanatory heading or label",
                "subtitles",
                "helper text",
                "descriptive copy",
                "headings, labels, cards, or settings",
                "by default",
                "user explicitly requests it",
                "necessary to prevent misunderstanding or error",
                "never use it to restate the heading or label",
                "narrow constraints",
                "loading, empty, error, populated, and long-content states",
                "functional defect",
                "visual exploration only for new directions or redesigns",
            ),
        )
        _require_pattern(
            violations,
            interface,
            "UI descriptions must default to one self-explanatory label and never restate it",
            r"prefer\s+one\s+concise,\s+self-explanatory\s+heading\s+or\s+label"
            r".{0,220}do\s+not\s+add\s+subtitles.{0,220}by\s+default"
            r".{0,260}only\s+when\s+the\s+user\s+explicitly\s+requests\s+it\s+or"
            r".{0,180}necessary\s+to\s+prevent\s+misunderstanding\s+or\s+error"
            r".{0,160}never\s+use\s+it\s+to\s+restate\s+the\s+heading\s+or\s+label",
        )

    boundaries = bodies["Respect data and system boundaries"]
    if boundaries:
        _require_terms(
            violations,
            boundaries,
            "data-boundary contract",
            ("domain meaning", "ownership", "lifecycle", "does not imply shared ownership"),
        )

    protection = bodies["Protect sources, repositories, and running systems"]
    if protection:
        _require_terms(
            violations,
            protection,
            "source-and-system protection contract",
            (
                "canonical sources",
                "current remote",
                "Remote-unavailable means unknown",
                "valuable dirty work",
                "shared resource",
                "data loss",
                "recoverable backup",
                "disposable and isolated",
                "unambiguous mutation targets",
            ),
        )

    reporting = bodies["Report status honestly"]
    if reporting:
        _require_terms(
            violations,
            reporting,
            "honest-status contract",
            (
                "outcomes and evidence",
                "facts",
                "inferences",
                "assumptions",
                "never ready",
                "when a completion ledger exists",
                "what works now",
                "what remains incomplete for users",
                "what blocks it",
                "what result comes next",
                "technical identifiers",
                "must not be the account",
            ),
        )

    for name in FORBIDDEN_NAMES:
        if re.search(rf"\b{re.escape(name)}\b", text):
            violations.append(f"universal policy must not name runtime/project-specific term: {name}")

    positive_authorization = re.search(
        r"(?i)(?:authenticated|signed[ -]?in).{0,80}"
        r"(?:agent\s+is\s+authorized|authorizes\s+(?:the\s+)?agent|grants\s+(?:the\s+)?agent\s+permission)",
        text,
    )
    if positive_authorization:
        violations.append("operational runtime state must not grant agent authority")

    contradictory_instructions = (
        (r"(?i)\balways\s+reread\b|\bload\s+every\s+(?:skill|tool)\b", "context must remain relevant and non-redundant"),
        (r"(?i)(?<!never )(?<!do not )(?<!must not )\b(?:ask|request|seek|obtain)\b.{0,80}\bapproval\b.{0,100}\bbefore\b.{0,80}\b(?:read[- ]only\s+)?(?:investigation|discovery)\b", "approval requests must follow available read-only investigation"),
        (r"(?i)(?<!never )(?<!do not )(?<!must not )\b(?:ask|request|seek|obtain)\b.{0,80}\bapproval\b.{0,140}\b(?:piecemeal|one\s+at\s+a\s+time|as\s+each\s+(?:implementation|technical)\s+detail\s+(?:appears|emerges|is\s+found))\b", "known consequential approval effects must be bundled into one decision"),
        (r"(?i)\bapproval\s+(?:request|question|message)\b.{0,100}\b(?:may|can|should)\b.{0,100}\b(?:technical\s+jargon|implementation\s+details?|internal\s+identifiers?)\b.{0,120}\b(?:without|instead\s+of)\b.{0,80}\bplain\s+language\b", "approval requests must explain the problem and outcome in plain language"),
        (r"(?i)\bapproval\b.{0,60}\b(?:applies|covers|authorizes)\b.{0,60}\bonly\b.{0,100}\b(?:named\s+)?implementation\s+details?\b", "approval must cover the recorded outcome and boundaries, not only implementation details"),
        (r"(?i)\ba\s+plain\s+[\"“”']?yes\b[\"“”']?.{0,60}\b(?:is\s+insufficient|is\s+not\s+enough|must\s+not\s+be\s+accepted)\b", "a plain yes must be sufficient for the described outcome and boundaries"),
        (r"(?i)\b(?:the\s+)?user\s+(?:must|should|needs?\s+to|has\s+to)\s+(?:repeat|copy|paste|type|transcribe|reply\s+with)\b.{0,140}\b(?:internal\s+identifier|digest|uuid|command|confirmation\s+(?:phrase|string)|technical\s+incantation|exact\s+phrase)\b", "users must never repeat internal identifiers or prescribed technical phrases"),
        (r"(?i)(?<!never )(?<!do not )(?<!must not )\bstop\s+(?:the\s+)?(?:test|suite|debug|audit|rehearsal|deployment|cycle|pass)\s+at\s+(?:the\s+)?first\b", "diagnostic cycles must not stop at the first ordinary failure"),
        (r"(?i)(?<!never )(?<!do not )(?<!must not )\bfix\s+each\s+(?:error|failure|gap).{0,100}\brestart\b", "diagnostic findings must be batch-fixed after the evidence pass"),
        (r"(?i)(?<!do not )(?<!never )\b(?:set|define|require|allocate)\b.{0,100}\b(?:cpu|memory|api|cost)\s+(?:limit|budget)s?\b", "parallel scheduling must not invent resource or API budgets"),
        (r"(?i)\b(?:always\s+use|use\s+exactly|require\s+exactly|limit\s+(?:the\s+)?(?:pool|run)\s+to)\s+\d+\s+(?:workers|agents)\b", "parallel scheduling must not impose a fixed worker count"),
        (r"(?i)\bwait\s+until\b.{0,120}\b(?:test|suite|rig|run|audit)\b.{0,80}\b(?:finishes|completes|ends)\b.{0,120}\bbefore\b.{0,80}\b(?:diagnos|fix|repair)", "first-failure diagnosis and fixing must not wait for the run to finish"),
        (r"(?i)\b(?:cancel|stop|abort)\b.{0,100}\b(?:siblings?|other\s+(?:safe|independent)\s+work)\b.{0,80}\b(?:after|when|on)\b.{0,60}\bfailure\b", "an ordinary failure must not cancel safe sibling work"),
        (r"(?i)(?<!do not )(?<!never )\b(?:apply|inject|merge)\b.{0,80}\bfix(?:es)?\b.{0,100}\b(?:original|sealed|running)\b.{0,80}\b(?:test|suite|rig|run|surface)\b", "concurrent fixes must not alter the sealed running evidence surface"),
        (r"(?i)(?<!never )(?<!do not )(?<!must not )\b(?:implement|add|begin|proceed\s+with)\b.{0,100}\b(?:unrequested|hypothetical|agent-proposed|outside\s+(?:the\s+)?agreed\s+scope)\b.{0,140}\bwithout\s+(?:asking\b|asking\s+(?:the\s+)?user\b|(?:obtaining\s+)?(?:(?:the\s+)?user(?:['’]s)?\s+)?(?:approval|permission)\b)", "unrequested engineering must never proceed without informed approval"),
        (r"(?i)\b(?:under-engineering|implementation\s+gaps?)\s+(?:is|are)\s+acceptable\b", "explicitly agreed work must remain mandatory"),
        (r"(?i)(?:\b(?:under-engineering|implementation\s+gaps?).{0,100}\b(?:more\s+serious|more\s+punishable)\b.{0,100}\b(?:reasonable\s+)?over-engineering\b|\b(?:reasonable\s+)?over-engineering\b.{0,100}\b(?:more\s+serious|more\s+punishable)\b.{0,100}\b(?:under-engineering|implementation\s+gaps?)\b)", "scope policy must not rank under-engineering against over-engineering"),
        (r"(?i)\bsilent\s+scope\s+expansion\s+is\s+(?:acceptable|allowed|authorized|permitted)\b", "silent scope expansion must remain prohibited"),
        (r"(?i)\b(?:ideally|for\s+example|something\s+like|could|illustrative\s+formats?)\b.{0,140}\b(?:is|are|becomes?|counts?\s+as)\b.{0,100}\bmandatory\s+(?:delivery\s+)?requirements?\b", "tentative or illustrative language must not become mandatory by default"),
        (r"(?i)\b(?:reliability|security|recovery|migration|preservation|compatibility|ui|infrastructure)\s+work\s+(?:is|are)\s+(?:always|automatically)\s+in\s+scope\b", "supporting engineering must not enter scope without a confirmed requirement or necessity"),
        (r"(?i)\b(?:focused|seemingly\s+focused)\s+request\b.{0,180}\b(?:more\s+than|beyond)\s+three\s+(?:product\s+)?subsystems\b.{0,180}\bcontinue\s+without\s+(?:a\s+)?(?:pause|scope\s+explanation)\b", "hidden large scope must not continue without one scope explanation"),
        (r"(?i)\bmore\s+(?:checks|parsers|adapters|supported\s+formats)\b.{0,140}\b(?:always\s+)?(?:make|makes|mean|means|prove|proves)\b.{0,100}\bmore\s+complete\b", "extra machinery must not be treated as proof of completeness"),
        (r"(?i)(?<!never )(?<!do not )(?<!must not )\b(?:implement|perform|begin|proceed\s+with|automatically\s+add).{0,100}(?:security|privacy|backup|migration|preservation|data[- ]safety|hardening|infrastructure).{0,120}\bwithout\s+(?:asking|approval)\b", "risk-control expansion must not proceed without user approval"),
        (r"(?i)\b(?:automatically|always)\s+(?:add|implement|perform|create|preserve|migrate|back\s*up|harden).{0,160}(?:even\s+(?:if|when).{0,80}(?:unrequested|not\s+(?:requested|required|needed))|without\s+(?:evidence|a\s+credible\s+(?:project\s+)?need))", "hypothetical engineering must not expand scope automatically"),
        (r"(?i)\b(?:every|any)\s+(?:possible|potential|optional|merely\s+noticed)\s+(?:idea|expansion|improvement)\b.{0,180}\b(?:must|always|requires?|triggers?)\b.{0,100}\b(?:question|interview|approval)\b", "optional ideas that will not be implemented must not trigger blanket questions"),
        (r"(?i)\b(?:possible|imagined|hypothetical)\s+(?:edge\s+case|failure\s+scenario)\b.{0,100}\b(?:is(?!\s+not\b)|counts?\s+as|establishes?)\b.{0,60}\b(?:credible\s+)?project\s+need\b", "a hypothetical edge case must not establish project scope"),
        (r"(?i)\b(?:small|minor|tiny)\s+(?:agreed|requested|in[- ]scope)\s+(?:gap|detail|behavior)\b.{0,120}\b(?:need\s+not|does\s+not\s+need\s+to|may\s+skip|can\s+skip)\b.{0,100}\b(?:CompletionLedger|completion[- ]ledger|ledger|recorded|tracked)\b", "no agreed gap is too small for the completion ledger"),
        (r"(?i)\bCompletionLedger(?:\.md)?\b.{0,180}\b(?:may|can|should)\b.{0,100}\b(?:technical|implementation)\s+(?:detail|jargon|identifier)s?\b.{0,120}\b(?:instead\s+of|without)\b.{0,80}\b(?:plain\s+language|user\s+(?:or\s+product\s+)?impact)\b", "completion-ledger technical detail must not replace a plain-language outcome and impact"),
        (r"(?i)\bproduction\s+UI\b.{0,120}\b(?:may|can|should)\b.{0,80}\b(?:plausible|synthetic|made-up|placeholder)\b.{0,60}\b(?:numbers|parameters|statuses|results|values)\b", "production UI must not use invented stand-in values"),
        (r"(?i)\b(?:always|must)\s+(?:ask|include|present)\b.{0,140}\b(?:every|all)\b.{0,80}\b(?:question|factor|detail|baseline\s+area)s?\b.{0,140}\b(?:regardless\s+of|whether\s+or\s+not)\b.{0,100}\b(?:material|affect|change)\b", "decision questions must not require fixed exhaustive detail"),
        (r"(?i)\b(?:if|when)\s+`?security-assumptions\.md`?\s+is\s+(?:absent|missing)\b.{0,160}\b(?:always|must|requires?)\b.{0,100}\b(?:full|complete)\s+baseline\b", "an absent assumptions file must not trigger an unconditional full baseline"),
        (r"(?i)\b(?:only|sole)\s+(?:unresolved\s+)?(?:material\s+)?assumption\b.{0,180}\b(?:must|always|requires?)\b.{0,100}\b(?:full|complete)\s+baseline\b", "one material assumption must not trigger an unrelated full baseline"),
        (r"(?i)\broutine\s+(?:invocation|use|execution)\b.{0,140}\b(?:reviewed\s+)?(?:skill|tool)\b.{0,140}\b(?:must|always|requires?|triggers?)\b.{0,100}\b(?:full\s+baseline|blanket\s+interview|security\s+interview)\b", "routine posture-preserving tool use must not trigger a security interview"),
        (r"(?i)\b(?:implement|apply|begin|continue|proceed\s+with)\b.{0,140}\b(?:security(?:-posture)?\s+)?control\b.{0,160}\b(?:despite|with)\b.{0,80}\bunresolved\s+material\s+assumption\b", "a material control decision must not proceed on an unresolved material assumption"),
        (r"(?i)(?<!never )(?<!do not )(?<!must not )\b(?:if|when)\s+`?security-assumptions\.md`?\s+is\s+(?:absent|missing|unavailable|incomplete|insufficient).{0,180}\b(?:implement|apply|begin|continue|proceed\s+with)\b.{0,180}\b(?:before\s+(?:asking|confirmation)|document|record|update).{0,80}\b(?:later|afterward|after\s+implementation)\b", "missing security assumptions must stop implementation before controls are applied"),
        (r"(?i)\b(?:agent|agents|we|you)\s+(?:may|can|should|must|will)\s+(?:infer|assume|presume)\b.{0,140}\b(?:security\s+assumptions?|threat\s+model|credible\s+adversar(?:y|ies)|misuse|trust\s+boundaries?|maximum\s+threat|multi[- ]tenant|internet[- ]facing)\b", "security assumptions and threats must be user-confirmed, not inferred"),
        (r"(?im)(?:^|(?<=[.!?]))\s*(?:-\s*)?(?:weaken|remove|disable|omit|intentionally\s+omit)\b.{0,120}\b(?:security(?:-posture)?\s+(?:control|gate|measure)|authentication\s+control|authorization\s+control|access-control\s+gate)\b.{0,120}\bwithout\s+(?:reading|citing|confirmed|user-confirmed)\b", "weakened, removed, or omitted controls require confirmed security assumptions"),
        (r"(?i)\b(?:populate|create|fill|update)\b.{0,100}`?security-assumptions\.md`?.{0,120}\b(?:template|defaults?)\b.{0,120}\b(?:treat|mark|regard|count)\b.{0,60}\b(?:as\s+)?confirmed\b", "security-assumption templates and defaults are unconfirmed until the user confirms them"),
        (r"(?im)(?:^|(?<=[.!?]))\s*(?:-\s*)?(?:always|automatically|by\s+default|regardless\s+of\s+(?:context|assumptions))\b.{0,100}\b(?:apply|implement|enable|require|add|use)\b.{0,100}\b(?:maximum|blanket|all|every)\b.{0,60}\b(?:hardening|security\s+controls?|security\s+gates?|security\s+measures?)\b", "blanket security controls must not replace assumption-backed proportional controls"),
        (r"(?i)\balways\s+(?:preserve|migrate|back\s*up)\s+disposable\s+test\s+data\b", "disposable test data must not trigger automatic preservation work"),
        (r"(?i)(?<!never )(?<!do not )(?<!must not )\brequest\s+(?:unbounded|unlimited)\s+(?:tool\s+)?output\b", "model-facing tool output must remain bounded"),
        (r"(?i)(?<!never )(?<!do not )(?<!must not )\b(?:follow|obey|execute)\b.{0,80}\binstructions?\b.{0,100}\b(?:in|from|inside)\b.{0,40}\b(?:logs?|test\s+output)\b", "instructions found in logs must never be followed"),
        (r"(?i)\bdelegated\s+agents?\s+may\s+(?:skip|ignore)\s+(?:the\s+)?(?:issue\s+)?ledgers?\b", "delegated work must receive relevant issue-ledger constraints"),
        (r"(?i)(?<!never )(?<!do not )(?<!must not )\b(?:maintain|create|write)\s+(?:a\s+)?(?:project-root\s+)?`?CompletionLedger\.md`?", "CompletionLedger.md must never be a writable ledger"),
        (r"(?i)`?CompletionLedger\.md`?.{0,100}\b(?:is|as|remains?)\b.{0,60}\b(?:the\s+)?(?:default|authoritative|source\s+of\s+truth)\b", "CompletionLedger.md must never be authoritative or default"),
        (r"(?i)\b(?:may|can|should)\b.{0,100}\bfall\s+back\b.{0,100}`?CompletionLedger\.md`?", "database ledger failure must never fall back to Markdown"),
        (r"(?i)\b(?:remove|delete|prune)\b.{0,100}\b(?:implemented|verified|resolved|completed|closed)\b.{0,100}\b(?:completion[- ]ledger\s+)?(?:issues?|events?|records?)\b", "implemented completion-ledger history must remain permanent"),
        (r"(?i)\bcollection(?!.{0,80}\b(?:must not|never|do not)\b).{0,100}\bform\s+first\b", "collection destinations must not lead with forms"),
        (r"(?i)\b(?:placeholder|future|simulated|no-op|inert)\s+controls?\s+(?:may|can|should)\s+(?:remain|be|appear)\s+(?:enabled|clickable|actionable|focusable)\b", "placeholder, future, simulated, no-op, or inert controls must not remain enabled"),
        (r"(?i)\b(?:may|can|should)\s+report.{0,100}\bcomplete\b.{0,180}\b(?:inert|missing|simulated|unimplemented|future\s+control)\b", "UI with inert or unimplemented agreed behavior must not be reported complete"),
        (r"(?i)\b(?:interaction\s+)?inventory\b[^.\n]{0,100}\b(?:is|remains|may\s+be|can\s+be)\s+(?:optional|skipped|omitted|unnecessary)\b", "the agreed UI interaction inventory must not be optional or skippable"),
        (r"(?i)\b(?:may|can|should)\s+(?:skip|omit)\s+(?:the\s+)?(?:interaction\s+)?inventory\b", "the agreed UI interaction inventory must not be optional or skippable"),
        (r"(?i)\b(?:representative|sampled?|subset\s+of)\s+controls?\b[^.\n]{0,120}\b(?:is|are|provides?|gives?|counts?\s+as)\s+(?:sufficient|enough|complete)\b", "representative controls must not substitute for the complete agreed UI inventory"),
        (r"(?i)\bonly\s+(?:a\s+)?(?:representative\s+)?(?:sample|subset)\s+of\s+controls?\s+(?:needs?|must|should)\s+(?:to\s+)?be\s+(?:exercised|tested|verified)\b", "representative controls must not substitute for the complete agreed UI inventory"),
        (r"(?i)\b(?:generic|catch-all|future-production)\s+(?:(?:completion[- ]ledger|ledger)\s+)?(?:item|entry)\b[^.\n]{0,100}\b(?:is|are|may\s+be|can\s+be)\s+(?:sufficient|enough|acceptable)\b", "generic completion-ledger entries must not conceal distinct missing UI behavior"),
        (r"(?i)\b(?:single|one)\s+(?:generic|catch-all|future-production)\s+(?:(?:completion[- ]ledger|ledger)\s+)?(?:item|entry)\s+(?:may|can|should)\s+(?:cover|combine|hide|represent)\s+(?:all|multiple)\s+missing\s+(?:UI\s+)?behaviors?\b", "generic completion-ledger entries must not conceal distinct missing UI behavior"),
        (r"(?i)\bfuture\s+controls?\b[^.;\n]{0,100}\b(?:need\s+not|do\s+not\s+need\s+to)\s+(?:be\s+|have\s+(?:an?\s+)?)?(?:disabled|labelled|labeled|unavailable|ledgered|tracked|disabled\s+state|unavailable\s+label|completion[- ]ledger\s+entry|ledger\s+entry)\b", "future controls must retain the disabled, unavailable, and ledger gates"),
        (r"(?i)\bfuture\s+controls?\b[^.;\n]{0,100}\bmay\s+(?:omit|skip)\s+(?:the\s+)?(?:disabled\s+state|unavailable\s+label|completion[- ]ledger\s+entry|ledger\s+entry)\b", "future controls must retain the disabled, unavailable, and ledger gates"),
        (r"(?i)\binteraction\s+inventory\b[^.\n]{0,140}\b(?:automatically|by\s+itself)\s+(?:invokes?|authorizes?|requires?)\b[^.\n]{0,100}\b(?:broader|repository-wide|exhaustive)\s+audit\b", "the scoped interaction inventory must not authorize a broader audit"),
        (r"(?i)\b(?:route\s+coverage|screenshots?|visual\s+comparison|geometry\s+checks?)\b[^.;\n]{0,120}(?<!not )(?<!never )(?<!cannot )(?<!can't )(?<!fail to )(?<!fails to )(?<!failed to )(?<!insufficient to )(?<!inadequate to )\b(?:constitutes?|proves?|counts?\s+as|(?:is|are)\s+sufficient)\b[^.;\n]{0,80}\binteraction\s+verification\b", "static or visual evidence alone must not count as interaction verification"),
        (r"(?i)\b(?:code\s+inspection|component\s+rendering|handler|route)\b[^.;\n]{0,120}(?<!not )(?<!never )(?<!cannot )(?<!can't )(?<!fail to )(?<!fails to )(?<!failed to )(?<!insufficient to )(?<!inadequate to )\b(?:constitutes?|proves?|counts?\s+as|(?:is|are)\s+sufficient)\b[^.;\n]{0,80}\b(?:interaction\s+verification|working\s+behavior)\b", "implementation structure alone must not prove product interaction"),
        (r"(?i)(?<!not )(?<!never )\b(?:by\s+default|always)\s*,?\s*(?:add|include|show)\b[^.\n]{0,100}\b(?:subtitles?|helper\s+text|descriptive\s+copy|descriptions?)\b[^.\n]{0,120}\b(?:beneath|below|under)\b[^.\n]{0,60}\b(?:(?:each|every|all)\s+)?(?:headings?|labels?|cards?|settings?)\b", "UI supporting copy must not be added beneath clear labels by default"),
        (r"(?i)(?<!not )(?<!never )\b(?:add|include|show)\b[^.\n]{0,100}\b(?:subtitles?|helper\s+text|descriptive\s+copy|descriptions?)\b[^.\n]{0,120}\b(?:beneath|below|under)\b[^.\n]{0,60}\b(?:(?:each|every|all)\s+)?(?:headings?|labels?|cards?|settings?)\b[^.\n]{0,120}(?<!not )(?<!never )\b(?:by\s+default|always|for\s+every|to\s+all)\b", "UI supporting copy must not be added beneath clear labels by default"),
        (r"(?i)(?<!not )(?<!never )\b(?:every|all|each)\s+(?:headings?|labels?|cards?|settings?)\b[^.\n]{0,100}\b(?:should|must|needs?\s+to)\s+(?:include|have|show)\b[^.\n]{0,80}\b(?:subtitles?|helper\s+text|descriptive\s+copy|descriptions?)\b", "UI supporting copy must not be added beneath clear labels by default"),
        (r"(?i)\b(?:supporting\s+copy|subtitles?|helper\s+text|descriptive\s+copy|descriptions?)\b[^.\n]{0,100}\b(?:may|can|should)\s+be\s+(?:added|included|shown)\b[^.\n]{0,100}\b(?:whenever|any\s+time|wherever)\b[^.\n]{0,80}\b(?:helpful|useful|nice|appropriate)\b", "supporting UI copy must require an explicit request or misunderstanding/error-prevention need"),
        (r"(?i)\b(?:supporting\s+copy|subtitles?|helper\s+text|descriptive\s+copy|descriptions?)\b[^.\n]{0,100}\b(?:may|can|should|must)\s+(?:repeat|restate|duplicate|paraphrase|echo)\b[^.\n]{0,80}\b(?:(?:the|its|their)\s+)?(?:headings?|labels?)\b", "supporting UI copy must never restate its heading or label"),
    )
    for pattern, label in contradictory_instructions:
        if re.search(pattern, text, flags=re.DOTALL):
            violations.append(label)

    violations.extend(_standing_tool_prompt_violations(text))

    return violations


def audit_policy(path: Path) -> list[str]:
    try:
        return find_policy_violations(path.read_text(encoding="utf-8"))
    except OSError as exc:
        return [f"could not read policy: {exc}"]


def audit_claude_importer(path: Path) -> list[str]:
    """Keep project memory from importing the global policy a second time."""

    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        return [f"could not read Claude project importer: {exc}"]
    import_lines = [line.strip() for line in text.splitlines() if line.lstrip().startswith("@")]
    violations: list[str] = []
    if import_lines.count("@AGENTS.md") != 1:
        violations.append("Claude project memory must import root AGENTS.md exactly once")
    if any("reference/universal/AGENTS.md" in line for line in import_lines):
        violations.append("Claude project memory must not re-import the user-level universal policy")
    if "# Universal Agent Instructions" in text or "# Repo Agent Instructions" in text:
        violations.append("Claude project memory must not copy either authoritative policy")
    folded = _fold(text)
    if (
        "confirm the user-level memory loaded" not in folded
        or "read `reference/universal/agents.md` directly" not in folded
        or "installation or activation gap" not in folded
    ):
        violations.append("Claude project memory must provide a session fallback for missing global policy")
    return violations


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--policy", type=Path, default=DEFAULT_POLICY)
    args = parser.parse_args()

    violations = audit_policy(args.policy)
    try:
        is_canonical = args.policy.resolve() == DEFAULT_POLICY.resolve()
    except OSError:
        is_canonical = False
    if is_canonical:
        violations.extend(audit_claude_importer(DEFAULT_CLAUDE_IMPORTER))
    if violations:
        for violation in violations:
            print(f"policy violation: {violation}")
        return 1
    print(f"app-wide policy check ok ({args.policy})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
