#!/usr/bin/env python3
"""Verify subagent file-coverage reports against a full-repo-audit manifest."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import struct
import sys
import tempfile
import xml.etree.ElementTree as ET
from collections import Counter, defaultdict
from pathlib import Path, PurePosixPath


SCRIPT_DIR = Path(__file__).resolve().parent
SKILL_DIR = SCRIPT_DIR.parent
REPO_ROOT = Path(__file__).resolve().parents[3]
VENDOR_ROOT = SCRIPT_DIR / "_vendor"
DEV_SKILL_DIR = (REPO_ROOT / "skills" / "full-repo-audit").resolve()
running_in_dev_repo = DEV_SKILL_DIR == SKILL_DIR.resolve() and (REPO_ROOT / "full_repo_harness" / "verify_common.py").is_file()
path_roots = [REPO_ROOT, VENDOR_ROOT] if running_in_dev_repo else [VENDOR_ROOT]
for root in reversed([item for item in path_roots if item.is_dir()]):
    root_text = str(root)
    if root_text not in sys.path:
        sys.path.insert(0, root_text)

from full_repo_harness import evidence as audit_evidence
from full_repo_harness import merge_findings
from full_repo_harness import verify_common as common


BATCH_ID_RE = re.compile(r"\bbatch_(\d{3,})\b", re.IGNORECASE)
REPORT_FILENAME_RE = re.compile(r"^batch_\d{3,}\.md$", re.IGNORECASE)
LEAD_RECONCILIATION_REPORT_NAME = "lead_reconciliation.md"
SHA256_RE = re.compile(r"^[0-9a-fA-F]{64}$")
SECTION_RE = re.compile(r"^##\s+(.+?)\s*$")
MARKDOWN_UNSAFE_PATH_CHARS = {"|", "`"}
REQUIRED_SECTIONS = (
    "run id",
    "batch id",
    "batch summary",
    "file coverage",
    "implementation inventory",
    "interface inventory",
    "findings",
    "no finding notes",
    "open questions",
)
REQUIRED_SECTION_LIST = list(REQUIRED_SECTIONS)
IMPLEMENTATION_INVENTORY_HEADERS = (
    "file/unit",
    "contract id",
    "contract/responsibility",
    "entrypoints/source anchors",
    "implementation/data/side-effect trace",
    "failure/edge/permission/recovery trace",
    "verification evidence",
    "result",
)
IMPLEMENTATION_RESULTS = {"PASS", "GAP", "BLOCKED"}
BATCH_IMPLEMENTATION_CONTRACT_ID_RE = re.compile(r"^(batch_\d{3,}):C\d{3,}$")
LEAD_IMPLEMENTATION_CONTRACT_ID_RE = re.compile(r"^lead:C\d{3,}$")
IMPLEMENTATION_BASIS_KINDS = {
    "user-requirement",
    "acceptance-criterion",
    "recorded-decision",
    "public-contract",
    "interface-promise",
    "caller-contract",
    "schema-invariant",
    "operational-contract",
    "source-inferred",
}
IMPLEMENTATION_BASIS_RE = re.compile(
    r"\bBasis:\s*([a-z][a-z-]*)\s*(?:[-–—:])\s*(.+?)(?=\s+Discovery:|$)",
    re.IGNORECASE,
)
IMPLEMENTATION_DISCOVERY_RE = re.compile(
    r"\bDiscovery:\s*(parsed|manual)\s*(?:[-–—:])\s*(.+?)\s*$",
    re.IGNORECASE,
)
IMPLEMENTATION_EVIDENCE_TYPE_RE = re.compile(
    r"\bevidence-type:\s*(test|runtime|source-only)\b",
    re.IGNORECASE,
)
IMPLEMENTATION_EVIDENCE_REF_RE = re.compile(
    r"\bevidence-ref:\s*([^;|]+)",
    re.IGNORECASE,
)
IMPLEMENTATION_EVIDENCE_OUTCOME_RE = re.compile(
    r"\b(?:outcome|result):\s*([^;|]+)",
    re.IGNORECASE,
)
BROWSER_TEST_REFERENCE_RE = re.compile(
    r"(?:^|/)[^`/]*browser[^`/]*\.test\.(?:[cm]?[jt]sx?)$",
    re.IGNORECASE,
)
BROWSER_TEST_COMMAND_RE = re.compile(
    r"\bcommand:\s*[^;|]*(?:node|npm|playwright)\b",
    re.IGNORECASE,
)
BROWSER_TEST_PASS_RE = re.compile(
    r"\b(?:pass(?:ed|ing)?\s*(?:=|:)?\s*[1-9]\d*|[1-9]\d*\s+pass(?:ing|ed)?)\b",
    re.IGNORECASE,
)
BROWSER_TEST_ZERO_FAILURE_RE = re.compile(
    r"\b(?:fail(?:ed|ures?)?\s*(?:=|:)?\s*0|0\s+(?:fail(?:ed|ures?)?))\b",
    re.IGNORECASE,
)
IMPLEMENTATION_EXPECTATION_RE = re.compile(
    r"\b(counterfactual|invariance):\s*([^;|]+(?:\s+[^;|]+)*)",
    re.IGNORECASE,
)
SOURCE_ONLY_DISALLOWED_CLAIM_RE = re.compile(
    r"\b(?:persist(?:s|ed|ing|ence|ent)?|durab(?:le|ility)|database|transaction|"
    r"read[- ]?back|sav(?:e|es|ed|ing)|stor(?:e|es|ed|ing)|writ(?:e|es|ten|ing)|"
    r"integration|external[- ](?:effect|service|dependency|system|call)|side[- ]effect|"
    r"gateway|webhook|enqueue(?:s|d|ing)?|publish(?:es|ed|ing)?|emit(?:s|ted|ting)?|"
    r"send(?:s|ing)?|sent|upload(?:s|ed|ing)?|success(?:ful|fully)?|"
    r"succeed(?:s|ed|ing)?)\b",
    re.IGNORECASE,
)
IMPLEMENTATION_VERIFICATION_BEHAVIOR_RE = re.compile(
    r"\b(?:assert(?:s|ed|ing)?|accept(?:s|ed|ing)?|block(?:s|ed|ing)?|"
    r"calculat(?:e|es|ed|ing)|call(?:s|ed|ing)?|compar(?:e|es|ed|ing)|"
    r"confirm(?:s|ed|ing)?|cover(?:s|ed|ing)?|creat(?:e|es|ed|ing)|"
    r"delet(?:e|es|ed|ing)|emit(?:s|ted|ting)?|exercis(?:e|es|ed|ing)|"
    r"fail(?:s|ed|ing)?|invok(?:e|es|ed|ing)|load(?:s|ed|ing)?|"
    r"persist(?:s|ed|ing)?|prov(?:e|es|ed|ing)|read(?:s|ing)?|"
    r"register(?:s|ed|ing)?|reject(?:s|ed|ing)?|render(?:s|ed|ing)?|"
    r"report(?:s|ed|ing)?|return(?:s|ed|ing)?|sav(?:e|es|ed|ing)|"
    r"updat(?:e|es|ed|ing)|validat(?:e|es|ed|ing)|verif(?:y|ies|ied|ying)|"
    r"writ(?:e|es|ten|ing))\b",
    re.IGNORECASE,
)
IMPLEMENTATION_OUTCOME_BEHAVIOR_RE = re.compile(
    r"\b(?:accepted|asserted|blocked|calculated|called|created|deleted|"
    r"emitted|failed|loaded|matched|persisted|produced|read|registered|"
    r"rejected|rendered|returned|saved|sent|updated|validated|verified|wrote)\b",
    re.IGNORECASE,
)
GENERIC_IMPLEMENTATION_EVIDENCE_PHRASES = (
    "manual source tracing is bound to manifest",
    "manifest sha-256",
    "source-defined output or side effect",
    "owned source logic",
    "fixture report",
)
LEAD_RECONCILIATION_SECTIONS = (
    "run id",
    "worker",
    "cross-file contract trace",
    "findings",
    "open questions",
)
LEAD_RECONCILIATION_HEADERS = (
    "contract id",
    "batch contract ids",
    "contract/source anchors",
    "entry-registration",
    "core-logic",
    "data-lifecycle",
    "integration-boundary",
    "authorization-trust",
    "failure-recovery",
    "observable-outcome",
    "operational-lifecycle",
    "verification",
    "result",
)
LEAD_RECONCILIATION_TRACE_FIELDS = LEAD_RECONCILIATION_HEADERS[3:-1]
LEAD_RECONCILIATION_EMPTY_SENTINEL = "No source-backed implementation contracts were queued."
LEAD_RECONCILIATION_TRACE_STATUS_RE = re.compile(
    r"^\s*(pass|gap|blocked|not applicable)\s*(?:[-–—:])\s*(.+?)\s*$",
    re.IGNORECASE,
)
IMPLEMENTATION_TRACE_STATUS_RE = LEAD_RECONCILIATION_TRACE_STATUS_RE
JOURNEY_REPORT_SECTIONS = {
    "journey_source_worker": (
        "run id",
        "worker",
        "journey sources",
        "proposed journeys",
        "ui source journey checks",
        "findings",
        "open questions",
    ),
    "visual_journey_worker": (
        "run id",
        "worker",
        "visual tooling",
        "visual journey checks",
        "changed visual review",
        "findings",
        "open questions",
    ),
}
JOURNEY_WORKER_LABELS = {
    "journey_source_worker": "journey_source",
    "visual_journey_worker": "visual_journey",
}
JOURNEY_PROMPT_FIELDS = {
    "journey_source_worker": "source_prompt",
    "visual_journey_worker": "visual_prompt",
}
JOURNEY_REPORT_FIELDS = {
    "journey_source_worker": "source_report",
    "visual_journey_worker": "visual_report",
}
SOURCE_JOURNEY_TABLE_HEADERS = {
    "journey",
    "step",
    "files",
    "primary navigation/decision elements",
    "relevance estimate",
    "required information",
    "interaction and metadata checklist",
    "mobile/desktop availability",
    "test mode evidence",
}
VISUAL_JOURNEY_TABLE_HEADERS = {
    "journey",
    "viewport",
    "route/screen",
    "evidence",
    "navigation visibility",
    "decision information",
    "interaction and metadata checklist",
    "visual quality",
    "result",
}
CHANGED_VISUAL_REVIEW_HEADERS = {
    "review cell",
    "trigger/status",
    "initial viewport evidence",
    "full-page evidence",
    "decision",
    "note",
}
ALLOWED_RELEVANCE_VALUES = {
    "critical-always",
    "primary-frequent",
    "secondary-occasional",
    "rare-under-5-percent",
}
ALLOWED_PRUNED_REVIEW_DECISIONS = {
    "excluded-with-rationale",
    "out-of-scope-with-user-confirmation",
    "requeued",
}
ALLOWED_TRACKED_DELETION_REVIEW_DECISIONS = {
    "verified-removal",
    "finding-recorded",
    "blocked",
}
ALLOWED_CLAIM_BASES = {"runtime-attested", "tool-schema-inspected", "self-reported", "manual-fallback"}


def validate_effort_claim(row: dict, *, basis_field: str, label_field: str, prefix: str) -> list[dict]:
    issues: list[dict] = []
    basis = row.get(basis_field)
    label = row.get(label_field)
    if basis not in ALLOWED_CLAIM_BASES:
        issues.append({"field": f"{prefix}.{basis_field}", "expected": sorted(ALLOWED_CLAIM_BASES), "actual": basis})
        return issues
    expected_label = "runtime-attested" if basis == "runtime-attested" else ("manual-fallback" if basis == "manual-fallback" else "ledger-recorded-unverified")
    if label != expected_label:
        issues.append({"field": f"{prefix}.{label_field}", "expected": expected_label, "actual": label})
    return issues
FINDING_HEADING_RE = re.compile(r"^###\s+P[0-3]\s+-\s+\S", re.IGNORECASE)
FINDING_FIELD_RE = re.compile(r"^-\s*([^:]+):")
REQUIRED_FINDING_FIELDS = {
    "files",
    "evidence",
    "interface evidence",
    "expected behavior/standard",
    "gap",
    "suggested direction",
}
PATH_IN_BACKTICKS_RE = re.compile(r"`([^`]+)`")
PLACEHOLDER_COMMENT_RE = re.compile(
    r"(?:#|//|/\*|<!--)\s*(?:TODO|FIXME|XXX)\b",
    re.IGNORECASE,
)
CODE_STUB_RE = re.compile(
    r"\b(?:throw|raise)\s+(?:new\s+)?(?:NotImplemented(?:Error|Exception)\b|"
    r"(?:Error|Exception)\s*\(\s*['\"](?:TODO|FIXME|not implemented|placeholder|stub)\b|"
    r"['\"]?(?:TODO|FIXME|not implemented|placeholder|stub)\b)"
    r"|\bpanic!\s*\(\s*['\"](?:TODO|FIXME|not implemented|placeholder|stub)\b"
    r"|\b(?:todo|unimplemented)!\s*\(",
    re.IGNORECASE,
)
CONSOLE_LOG_RE = re.compile(r"console\.(?:log|warn|error)\s*\([^)\n]*\)", re.IGNORECASE)
UI_CODE_EXTENSIONS = {".astro", ".jsx", ".mdx", ".svelte", ".tsx", ".vue"}
CODE_STUB_EXTENSIONS = {
    ".c",
    ".cc",
    ".cpp",
    ".cs",
    ".dart",
    ".go",
    ".h",
    ".hpp",
    ".java",
    ".js",
    ".jsx",
    ".kt",
    ".mjs",
    ".php",
    ".py",
    ".rb",
    ".rs",
    ".scala",
    ".swift",
    ".ts",
    ".tsx",
}
MESSAGE_CATALOG_HINT_EXTENSIONS = {
    ".arb",
    ".ftl",
    ".json",
    ".po",
    ".pot",
    ".properties",
    ".resx",
    ".strings",
    ".xlf",
    ".xliff",
}
VISIBLE_JSON_KEY_TOKENS = {
    "action",
    "aria",
    "banner",
    "button",
    "cancel",
    "caption",
    "close",
    "confirm",
    "continue",
    "copy",
    "create",
    "cta",
    "delete",
    "description",
    "empty",
    "error",
    "heading",
    "help",
    "hint",
    "label",
    "loading",
    "login",
    "logout",
    "message",
    "name",
    "next",
    "notice",
    "ok",
    "placeholder",
    "previous",
    "remove",
    "save",
    "search",
    "submit",
    "success",
    "subtitle",
    "text",
    "title",
    "toast",
    "tooltip",
    "update",
    "warning",
    "welcome",
}
INTERNAL_JSON_KEY_TOKENS = {
    "analytics",
    "api",
    "class",
    "classname",
    "code",
    "debug",
    "endpoint",
    "feature",
    "flag",
    "href",
    "icon",
    "id",
    "key",
    "metadata",
    "permission",
    "route",
    "slug",
    "style",
    "telemetry",
    "token",
    "url",
}
VISIBLE_HINT_EXTENSIONS = {
    ".arb",
    ".astro",
    ".axaml",
    ".cshtml",
    ".ejs",
    ".ftl",
    ".hbs",
    ".handlebars",
    ".html",
    ".j2",
    ".jinja",
    ".jinja2",
    ".json",
    ".jsx",
    ".liquid",
    ".md",
    ".mdx",
    ".mustache",
    ".njk",
    ".po",
    ".pot",
    ".properties",
    ".pug",
    ".razor",
    ".resx",
    ".strings",
    ".svelte",
    ".svg",
    ".storyboard",
    ".tpl",
    ".tsx",
    ".twig",
    ".vue",
    ".xaml",
    ".xib",
    ".xml",
    ".xlf",
    ".xliff",
    ".yaml",
    ".yml",
}
GENERIC_PURPOSE_VALUES = {
    "component",
    "config",
    "config file",
    "file",
    "fixture source",
    "message catalog",
    "misc",
    "script",
    "source",
    "source file",
    "test",
    "test file",
    "utility",
}
DIRECTORY_ONLY_PURPOSE_RE = re.compile(
    r"\b(?:files?|sources?|items?|entries|everything)\s+(?:under|in|from|inside)\b"
    r"|\b(?:directory|folder)\s+(?:of|for|under|inside|containing)\b",
    re.IGNORECASE,
)
SIMPLE_KEY_VALUE_RE = re.compile(r"(?m)^\s*[-A-Za-z0-9_.]+\s*[:=]\s*['\"]?([^'\"\n#{}\[\]]{2,100})['\"]?\s*$")
MARKDOWN_H1_RE = re.compile(r"(?m)^#\s+(.{2,100})\s*$")
PROPERTIES_VALUE_RE = re.compile(r"(?m)^\s*[^#!\s][^:=\n]*[:=]\s*([^\n#]{2,100})\s*$")
PO_VALUE_RE = re.compile(r'(?m)^\s*msg(?:id|str)\s+"([^"]{2,100})"\s*$')
APPLE_STRINGS_VALUE_RE = re.compile(r'(?m)=\s*"([^"]{2,100})"\s*;')
FTL_VALUE_RE = re.compile(r"(?m)^\s*[-A-Za-z0-9_.]+\s*=\s*([^\n{#]{2,100})\s*$")
NO_FINDINGS_SENTINELS = {"no findings", "no confirmed findings"}
AUDIT_EXECUTION_ONLY_GAP_RE = re.compile(
    r"\b(?:external test[- ]environment blocker|audit[- ]execution blocker|"
    r"audit evidence cannot establish|not (?:an? )?source(?:-level)? defect)\b",
    re.IGNORECASE,
)
AUDIT_EXECUTION_BLOCKER_HEADING_RE = re.compile(
    r"\baudit[- ]execution (?:blocked|blocker)\b",
    re.IGNORECASE,
)
BOILERPLATE_VALUES = {
    "concrete evidence",
    "exact visible text",
    "fixture expected path",
    "fixture implementation note",
    "fixture report",
    "fixture visible text",
    "implemented",
    "n/a",
    "none",
    "not applicable",
    "todo",
}
GENERIC_INTERFACE_PHRASES = (
    "trace `",
    "source text was inspected",
    "handler/state/api/persistence/verification expected",
    "implemented/missing behavior evidence",
)
VISIBLE_KEY_RE = re.compile(
    r"(?im)^\s*(display_name|short_description|default_prompt|label|title|placeholder|description|tooltip|helper_text|empty_state|error_message|success_message)\s*:\s*['\"]([^'\"\n#]+)['\"]"
)
VISIBLE_ATTR_RE = re.compile(
    r"""(?ix)
    \b(?:aria-label|title|placeholder|alt|label|content|android:text|text)\s*=\s*
    (?:"([^"]{2,100})"|'([^']{2,100})')
    """
)
VISIBLE_TEXT_RE = re.compile(r">\s*([^<>{}\n][^<>{}]{1,100}?)\s*<")
EMPTY_HANDLER_RE = re.compile(
    r"\bon[A-Z][A-Za-z]*\s*=\s*{\s*(?:\(\s*\)\s*=>\s*\{\s*\}|function\s*\([^)]*\)\s*\{\s*\})\s*}",
    re.DOTALL,
)
NOOP_FUNCTION_RE = re.compile(
    r"\b(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(?:\([^)]*\)|[A-Za-z_$][\w$]*)\s*=>\s*\{\s*\}"
    r"|\bfunction\s+([A-Za-z_$][\w$]*)\s*\([^)]*\)\s*\{\s*\}",
    re.DOTALL,
)
EVENT_HANDLER_REF_RE = re.compile(r"\bon[A-Z][A-Za-z]*\s*=\s*{\s*([A-Za-z_$][\w$]*)\s*}")
DEAD_HREF_RE = re.compile(r"""(?is)<a\b[^>]*\bhref\s*=\s*(?:["']#["']|{\s*["']#["']\s*})[^>]*>""")
BUTTON_TAG_RE = re.compile(r"(?is)<button\b(.*?)(?<![=])>(.*?)</button>")
SELF_CLOSING_BUTTON_RE = re.compile(r"(?is)<button\b((?:=>|[^>])*?)/\s*>")
COMPONENT_BUTTON_TAG_RE = re.compile(r"(?is)<([A-Z][A-Za-z0-9_.]*Button)\b(.*?)>(.*?)</\1>")
SELF_CLOSING_COMPONENT_BUTTON_RE = re.compile(r"(?is)<([A-Z][A-Za-z0-9_.]*Button)\b((?:=>|[^>])*?)/\s*>")
DISABLED_ATTR_PATTERN = r"""(?<![\w:-])disabled(?![\w:-])(?:\s*=\s*(?:{\s*true\s*}|["']disabled["']|["']true["'])|(?=\s*(?:/?>|$)))"""
DISABLED_ATTR_RE = re.compile(DISABLED_ATTR_PATTERN, re.IGNORECASE)
STATIC_DISABLED_CONTROL_RE = re.compile(
    rf"(?is)<(?:button|a|[A-Z][A-Za-z0-9_.]*Button)\b[^>]*{DISABLED_ATTR_PATTERN}[^>]*>"
)
ROLE_BUTTON_RE = re.compile(r"""(?is)<([A-Za-z][\w:.-]*)\b([^>]*)\brole\s*=\s*(?:"button"|'button'|{\s*["']button["']\s*})([^>]*)>(.*?)</\1>""")
FORM_FIELD_TAG_RE = re.compile(r"(?is)<(input|select|textarea)\b(.*?)(?<![=])>(.*?)</\1>|<(input)\b(.*?)(?<![=])/?>")
FORM_TAG_RE = re.compile(r"(?is)<form\b([^>]*)>")
FORM_SUBMIT_ATTR_RE = re.compile(
    r"(?is)(?<![\w:-])(?:action|onsubmit|onSubmit|@submit|v-on:submit|on:submit|\(submit\))\s*="
)
ROLE_INTERACTIVE_RE = re.compile(
    r"""(?is)<([A-Za-z][\w:.-]*)\b([^>]*)\brole\s*=\s*(?:"(?:checkbox|menuitem|switch|tab)"|'(?:checkbox|menuitem|switch|tab)'|{\s*["'](?:checkbox|menuitem|switch|tab)["']\s*})([^>]*)>(.*?)</\1>"""
)
INTERACTIVE_HINT_EXTENSIONS = {
    ".axaml",
    ".astro",
    ".cshtml",
    ".cts",
    ".ejs",
    ".handlebars",
    ".hbs",
    ".html",
    ".j2",
    ".jinja",
    ".jinja2",
    ".js",
    ".jsx",
    ".liquid",
    ".mjs",
    ".mdx",
    ".mts",
    ".mustache",
    ".njk",
    ".pug",
    ".razor",
    ".svelte",
    ".storyboard",
    ".tpl",
    ".ts",
    ".tsx",
    ".twig",
    ".vue",
    ".xaml",
    ".xib",
    ".xml",
}
NATIVE_DISABLED_CONTROL_RE = re.compile(
    r"""(?is)<(?:Button|button|[A-Za-z:]+Button)\b[^>]*\b(?:IsEnabled|isEnabled|enabled|android:enabled)\s*=\s*(?:"false"|'false'|{\s*false\s*})[^>]*>"""
)
ASSET_EVIDENCE_TERMS = (
    "asset",
    "dimension",
    "height",
    "image/",
    "mime",
    "referenc",
    "render",
    "usage",
    "visual",
    "width",
)
ASSET_MIME_BY_SUFFIX = {
    ".gif": "image/gif",
    ".ico": "image/x-icon",
    ".jpeg": "image/jpeg",
    ".jpg": "image/jpeg",
    ".png": "image/png",
    ".svg": "image/svg+xml",
    ".webp": "image/webp",
    ".woff": "font/woff",
    ".woff2": "font/woff2",
    ".ttf": "font/ttf",
    ".eot": "application/vnd.ms-fontobject",
}
ASSET_METADATA_READ_LIMIT = 512 * 1024
SVG_ROOT_RE = re.compile(r"<svg\b(?P<attrs>[^>]*)>", re.IGNORECASE)
SVG_ATTR_RE = re.compile(r"([A-Za-z_:][-A-Za-z0-9_:.]*)\s*=\s*(['\"])(.*?)\2", re.DOTALL)
SVG_NUMBER_RE = re.compile(r"^\s*([+-]?(?:\d+(?:\.\d*)?|\.\d+))")
VISUAL_EVIDENCE_TOOL_TERMS = (
    "browser",
    "command",
    "cypress",
    "mcp",
    "npx",
    "npm",
    "playwright",
    "pnpm",
    "ran",
    "storybook",
    "yarn",
)
VISUAL_EVIDENCE_ARTIFACT_TERMS = (
    ".jpeg",
    ".jpg",
    ".png",
    "artifact",
    "playwright-report",
    "recording",
    "screenshot",
    "trace",
    "video",
)
WEB_UI_EXTENSIONS = {".astro", ".css", ".html", ".jsx", ".mdx", ".scss", ".svelte", ".tsx", ".vue"}
VISUAL_DANGER_RE = re.compile(
    r"\b(overloaded?|crowded|cramped|unreadable|invisible|low[- ]contrast|clipped|cropped|truncated|"
    r"overflow|hidden overflow|no scroll|without scroll|unscannable|ambiguous hierarchy|oversized|"
    r"excessive detail|debug detail|raw status|overexposed|over-prescribed|overprescribed|"
    r"duplicate summaries?|duplicative summaries?|duplicate severity|vague labels?|unclear labels?|"
    r"detail controls?|source-model leakage|data-model leakage|dominates|dominating|buried|below the fold|"
    r"nested (?:cards?|blocks?|containers?|frames?)|card[- ]in[- ]card|blocks? inside blocks?|"
    r"border stacks?|background stacks?|visual noise|noisy surfaces?|misaligned|misalignment|"
    r"random(?:ly)? placed|random placement|weak grid|poor grid|grid drift|inconsistent gutters?|"
    r"unstable (?:expander|expansion|disclosure)|jumps? horizontally|width changes?|different widths?|"
    r"collapsed and expanded [^|.\n]*(?:different widths?|mismatch)|expanded and collapsed [^|.\n]*(?:different widths?|mismatch)|"
	    r"meaningless icons?|unclear icons?|unintuitive icons?|decorative clutter|avatar clutter|unnecessary avatars?|"
	    r"tiny icon[- ]only target|icon[- ]only (?:target|activation)|row [^|.\n]*(?:not clickable|not activated|not interactive)|"
	    r"(?:navigate|navigation|destination|click) [^|.\n]*(?:no pointer|cursor missing|unpredictable|missing destination|wrong destination)|"
	    r"(?:popover|flyout|expanded panel|temporary panel) [^|.\n]*(?:no timeout|no lifecycle|never closes|stays open|does not close)|"
	    r"(?:badge|flag) [^|.\n]*(?:not interactive|no hover|no click|no popover|no detail)|"
	    r"(?:hover|focus|click) [^|.\n]*(?:missing|absent|no feedback)|popover [^|.\n]*(?:missing|absent|not available)|"
	    r"(?:expander|expand icon|disclosure icon|chevron) [^|.\n]*(?:interferes?|overlaps?|collides?|fights?) [^|.\n]*(?:scroll|scrollbar)|"
	    r"scrollbar [^|.\n]*(?:interferes?|overlaps?|collides?|fights?) [^|.\n]*(?:expander|expand icon|disclosure icon|chevron)|"
	    r"copy (?:button|icon|control)s? [^|.\n]*(?:always visible|permanent|not reachable|disappears|unstable)|"
	    r"duplicate (?:status|error|duration)|status [^|.\n]*twice|error [^|.\n]*twice|duration [^|.\n]*(?:unnecessary|steals attention)|"
	    r"sender labels?|authorship labels?|routing labels?|selectable timestamps?|timestamp [^|.\n]*selectable|"
    r"permanent (?:helper|instruction)|instruction noise|helper text|low[- ]importance|"
    r"decision[- ]critical [^|.\n]*(?:hidden|weak|buried)|hidden [^|.\n]*decision[- ]critical|"
    r"weakly placed [^|.\n]*decision)\b",
    re.IGNORECASE,
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Compare subagent Markdown File Coverage tables to a full-repo-audit manifest."
    )
    parser.add_argument("--manifest", required=True, help="Path to manifest.json from build_audit_batches.py.")
    parser.add_argument(
        "--reports",
        required=True,
        nargs="+",
        help="One or more Markdown report files or directories containing reports.",
    )
    parser.add_argument(
        "--batch-id",
        help=(
            "Validate one exact batch report without requiring lead reconciliation, "
            "other batch reports, or a completed effort ledger."
        ),
    )
    parser.add_argument(
        "--skip-current-hash-check",
        action="store_true",
        help="Do not compare manifest SHA-256 fingerprints to the current repo files.",
    )
    parser.add_argument(
        "--receipt-out",
        help=(
            "Write a stable verification receipt only when verification passes and every "
            "manifest-authorized report remains byte-identical throughout the run."
        ),
    )
    parser.add_argument("--json", action="store_true", help="Emit machine-readable JSON.")
    return parser.parse_args()


def iter_report_files(paths: list[str]) -> list[Path]:
    files: list[Path] = []
    for raw_path in paths:
        path = Path(raw_path).expanduser().resolve()
        if path.is_dir():
            files.extend(
                sorted(
                    item
                    for item in path.glob("*.md")
                    if item.is_file()
                    and (
                        REPORT_FILENAME_RE.match(item.name)
                        or item.name == LEAD_RECONCILIATION_REPORT_NAME
                    )
                )
            )
        elif path.is_file():
            if not REPORT_FILENAME_RE.match(path.name) and path.name != LEAD_RECONCILIATION_REPORT_NAME:
                raise ValueError(
                    "Report file must use exact batch_###.md or lead_reconciliation.md filename: "
                    f"{path}"
                )
            files.append(path)
        else:
            raise FileNotFoundError(f"Report path does not exist: {path}")
    for reports_parent in {
        path.parent for path in files if REPORT_FILENAME_RE.match(path.name)
    }:
        lead_report = reports_parent / LEAD_RECONCILIATION_REPORT_NAME
        if lead_report.is_file() and lead_report not in files:
            files.append(lead_report)
    return sorted(set(files))


def validate_markdown_safe_manifest_token(value: str, field_name: str) -> None:
    if value != value.strip():
        raise ValueError(f"Manifest {field_name} must not have leading or trailing whitespace.")
    if any(ord(char) < 32 or ord(char) == 127 for char in value):
        raise ValueError(f"Manifest {field_name} must not contain ASCII control characters.")
    unsafe = sorted(char for char in MARKDOWN_UNSAFE_PATH_CHARS if char in value)
    if unsafe:
        raise ValueError(f"Manifest {field_name} must not contain Markdown table/code delimiters: {unsafe}.")


def validate_repo_relative_path(value: str, field_name: str) -> None:
    validate_markdown_safe_manifest_token(value, field_name)
    if "\\" in value:
        raise ValueError(f"Manifest {field_name} must use POSIX repo-relative paths.")
    path = PurePosixPath(value)
    if path.is_absolute() or not path.parts or any(part in {"", ".", ".."} for part in path.parts):
        raise ValueError(f"Manifest {field_name} must be a repo-relative path without '.' or '..' segments.")


def duplicate_values(values: list[str]) -> list[str]:
    counts = Counter(values)
    return sorted(value for value, count in counts.items() if count > 1)


def validate_manifest_count(manifest: dict, field_name: str, expected: int) -> None:
    actual = manifest.get(field_name)
    if not isinstance(actual, int) or isinstance(actual, bool):
        raise ValueError(f"Manifest {field_name} must be an integer; actual: {actual}")
    if actual != expected:
        raise ValueError(f"Manifest {field_name} must equal {expected}; actual: {actual}")


def load_manifest(manifest_path: Path) -> dict:
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ValueError(f"Manifest is not valid JSON: {manifest_path}") from exc
    if not isinstance(manifest, dict):
        raise ValueError("Manifest must be a JSON object.")

    source_files = manifest.get("source_files")
    if not isinstance(source_files, list):
        raise ValueError("Manifest field source_files must be a list.")
    source_paths: list[str] = []
    for index, item in enumerate(source_files):
        if not isinstance(item, dict):
            raise ValueError(f"Manifest source_files[{index}] must be an object.")
        if not isinstance(item.get("rel_path"), str) or not item["rel_path"]:
            raise ValueError(f"Manifest source_files[{index}].rel_path must be a non-empty string.")
        validate_repo_relative_path(item["rel_path"], f"source_files[{index}].rel_path")
        source_paths.append(item["rel_path"])
        if "sha256" in item and item["sha256"] is not None:
            if not isinstance(item["sha256"], str):
                raise ValueError(f"Manifest source_files[{index}].sha256 must be a string when present.")
            if not SHA256_RE.match(item["sha256"]):
                raise ValueError(f"Manifest source_files[{index}].sha256 must be a 64-character SHA-256 hex digest.")
    duplicate_source_paths = duplicate_values(source_paths)
    if duplicate_source_paths:
        raise ValueError(f"Manifest source_files rel_path values must be unique; duplicates: {duplicate_source_paths}")
    source_path_set = set(source_paths)
    source_hash_by_path = {
        item["rel_path"]: item.get("sha256")
        for item in source_files
        if isinstance(item, dict)
    }

    raw_coverage_units = manifest.get("coverage_units")
    if raw_coverage_units is None:
        coverage_units = [
            {
                "unit_id": item["rel_path"],
                "rel_path": item["rel_path"],
                "sha256": item.get("sha256"),
                "start_line": None,
                "end_line": None,
            }
            for item in source_files
        ]
    else:
        if not isinstance(raw_coverage_units, list):
            raise ValueError("Manifest field coverage_units must be a list when present.")
        coverage_units = raw_coverage_units
    coverage_unit_ids: list[str] = []
    coverage_unit_paths: list[str] = []
    for index, unit in enumerate(coverage_units):
        if not isinstance(unit, dict):
            raise ValueError(f"Manifest coverage_units[{index}] must be an object.")
        if not isinstance(unit.get("unit_id"), str) or not unit["unit_id"]:
            raise ValueError(f"Manifest coverage_units[{index}].unit_id must be a non-empty string.")
        if not isinstance(unit.get("rel_path"), str) or not unit["rel_path"]:
            raise ValueError(f"Manifest coverage_units[{index}].rel_path must be a non-empty string.")
        validate_markdown_safe_manifest_token(unit["unit_id"], f"coverage_units[{index}].unit_id")
        validate_repo_relative_path(unit["rel_path"], f"coverage_units[{index}].rel_path")
        if unit["rel_path"] not in source_path_set:
            raise ValueError(f"Manifest coverage_units[{index}].rel_path is absent from source_files: {unit['rel_path']}")
        if "sha256" in unit and unit["sha256"] is not None:
            if not isinstance(unit["sha256"], str):
                raise ValueError(f"Manifest coverage_units[{index}].sha256 must be a string when present.")
            if not SHA256_RE.match(unit["sha256"]):
                raise ValueError(f"Manifest coverage_units[{index}].sha256 must be a 64-character SHA-256 hex digest.")
        start_line = unit.get("start_line")
        end_line = unit.get("end_line")
        start_byte = unit.get("start_byte")
        end_byte = unit.get("end_byte")
        has_line_range = start_line is not None or end_line is not None
        has_byte_range = start_byte is not None or end_byte is not None
        if has_line_range and has_byte_range:
            raise ValueError(f"Manifest coverage_units[{index}] must not mix line and byte ranges.")
        if has_line_range:
            if (
                not isinstance(start_line, int)
                or isinstance(start_line, bool)
                or not isinstance(end_line, int)
                or isinstance(end_line, bool)
                or start_line < 1
                or end_line < start_line
            ):
                raise ValueError(f"Manifest coverage_units[{index}] line range must use positive start_line/end_line integers.")
        if has_byte_range:
            if (
                not isinstance(start_byte, int)
                or isinstance(start_byte, bool)
                or not isinstance(end_byte, int)
                or isinstance(end_byte, bool)
                or start_byte < 1
                or end_byte < start_byte
            ):
                raise ValueError(f"Manifest coverage_units[{index}] byte range must use positive start_byte/end_byte integers.")
        coverage_unit_ids.append(unit["unit_id"])
        coverage_unit_paths.append(unit["rel_path"])
    duplicate_unit_ids = duplicate_values(coverage_unit_ids)
    if duplicate_unit_ids:
        raise ValueError(f"Manifest coverage_units unit_id values must be unique; duplicates: {duplicate_unit_ids}")

    batches = manifest.get("batches")
    if not isinstance(batches, list):
        raise ValueError("Manifest field batches must be a list.")
    batch_ids: list[str] = []
    assigned_paths: list[str] = []
    assigned_units: list[str] = []
    coverage_unit_set = set(coverage_unit_ids)
    for index, batch in enumerate(batches):
        if not isinstance(batch, dict):
            raise ValueError(f"Manifest batches[{index}] must be an object.")
        if not isinstance(batch.get("id"), str) or not batch["id"]:
            raise ValueError(f"Manifest batches[{index}].id must be a non-empty string.")
        batch_ids.append(batch["id"])
        files = batch.get("files")
        if not isinstance(files, list) or not all(isinstance(item, str) for item in files):
            raise ValueError(f"Manifest batches[{index}].files must be a list of strings.")
        for file_index, rel_path in enumerate(files):
            validate_repo_relative_path(rel_path, f"batches[{index}].files[{file_index}]")
        duplicate_batch_files = duplicate_values(files)
        if duplicate_batch_files:
            raise ValueError(
                f"Manifest batches[{index}].files must not contain duplicates: {duplicate_batch_files}"
            )
        assigned_paths.extend(files)
        batch_units = batch.get("coverage_units")
        if batch_units is None:
            batch_units = files
        if not isinstance(batch_units, list) or not all(isinstance(item, str) for item in batch_units):
            raise ValueError(f"Manifest batches[{index}].coverage_units must be a list of strings when present.")
        for unit_index, unit_id in enumerate(batch_units):
            validate_markdown_safe_manifest_token(unit_id, f"batches[{index}].coverage_units[{unit_index}]")
        duplicate_batch_units = duplicate_values(batch_units)
        if duplicate_batch_units:
            raise ValueError(
                f"Manifest batches[{index}].coverage_units must not contain duplicates: {duplicate_batch_units}"
            )
        assigned_units.extend(batch_units)
    duplicate_batch_ids = duplicate_values(batch_ids)
    if duplicate_batch_ids:
        raise ValueError(f"Manifest batch ids must be unique; duplicates: {duplicate_batch_ids}")
    assigned_path_set = set(assigned_paths)
    unknown_assigned = sorted(assigned_path_set - source_path_set)
    unassigned = sorted(source_path_set - assigned_path_set)
    assigned_unit_set = set(assigned_units)
    unknown_assigned_units = sorted(assigned_unit_set - coverage_unit_set)
    unassigned_units = sorted(coverage_unit_set - assigned_unit_set)
    duplicate_assignments = duplicate_values(assigned_units)
    if unknown_assigned:
        raise ValueError(f"Manifest batches reference files absent from source_files: {unknown_assigned}")
    if unassigned:
        raise ValueError(f"Manifest source_files are not assigned to a batch: {unassigned}")
    if unknown_assigned_units:
        raise ValueError(f"Manifest batches reference coverage units absent from coverage_units: {unknown_assigned_units}")
    if unassigned_units:
        raise ValueError(f"Manifest coverage_units are not assigned to a batch: {unassigned_units}")
    if duplicate_assignments:
        raise ValueError(f"Manifest batch coverage unit assignments must be unique: {duplicate_assignments}")
    scope_warnings = manifest.get("scope_warnings", [])
    if not isinstance(scope_warnings, list):
        raise ValueError("Manifest field scope_warnings must be a list when present.")
    pruned_hints = manifest.get("pruned_directory_review_hints", [])
    if not isinstance(pruned_hints, list):
        raise ValueError("Manifest field pruned_directory_review_hints must be a list when present.")
    tracked_deletions = manifest.get("tracked_deletions", [])
    if not isinstance(tracked_deletions, list):
        raise ValueError("Manifest field tracked_deletions must be a list when present.")
    tracked_deletion_paths: set[str] = set()
    for index, removal in enumerate(tracked_deletions):
        if not isinstance(removal, dict):
            raise ValueError(f"Manifest tracked_deletions[{index}] must be an object.")
        path = removal.get("path")
        if not isinstance(path, str) or not path:
            raise ValueError(f"Manifest tracked_deletions[{index}].path must be a non-empty string.")
        validate_repo_relative_path(path, f"tracked_deletions[{index}].path")
        if path in tracked_deletion_paths:
            raise ValueError(f"Manifest tracked_deletions contains duplicate path: {path}")
        tracked_deletion_paths.add(path)
        available = removal.get("baseline_available")
        if not isinstance(available, bool):
            raise ValueError(f"Manifest tracked_deletions[{index}].baseline_available must be boolean.")
        digest = removal.get("baseline_sha256")
        size = removal.get("baseline_size_bytes")
        if available:
            if not isinstance(digest, str) or SHA256_RE.fullmatch(digest) is None:
                raise ValueError(f"Manifest tracked_deletions[{index}].baseline_sha256 must be a SHA-256 digest when baseline evidence is available.")
            if not isinstance(size, int) or isinstance(size, bool) or size < 0:
                raise ValueError(f"Manifest tracked_deletions[{index}].baseline_size_bytes must be a non-negative integer when baseline evidence is available.")
        elif digest is not None or size is not None:
            raise ValueError(f"Manifest tracked_deletions[{index}] must omit baseline digest and size when evidence is unavailable.")
    journey_audit = manifest.get("journey_audit")
    if journey_audit is not None and not isinstance(journey_audit, dict):
        raise ValueError("Manifest field journey_audit must be an object when present.")
    lead_reconciliation = manifest.get("lead_reconciliation")
    if not isinstance(lead_reconciliation, dict):
        raise ValueError("Manifest field lead_reconciliation must be an object.")
    expected_lead_reconciliation = {
        "required": True,
        "worker": "lead_reconciliation",
        "prompt": "lead_reconciliation.md",
        "report": "reports/lead_reconciliation.md",
    }
    if lead_reconciliation != expected_lead_reconciliation:
        raise ValueError(
            "Manifest field lead_reconciliation must exactly declare the required lead prompt/report contract."
        )

    validate_manifest_count(manifest, "source_file_count", len(source_files))
    if "coverage_unit_count" in manifest:
        validate_manifest_count(manifest, "coverage_unit_count", len(coverage_units))
    validate_manifest_count(manifest, "batch_count", len(batches))
    validate_manifest_count(
        manifest,
        "interface_file_count",
        sum(1 for item in source_files if isinstance(item, dict) and item.get("interface_relevant") is True),
    )
    validate_manifest_count(manifest, "scope_warning_count", len(scope_warnings))
    validate_manifest_count(manifest, "pruned_directory_review_hint_count", len(pruned_hints))
    validate_manifest_count(manifest, "tracked_deletion_count", len(tracked_deletions))

    manifest["expected_files"] = {item["rel_path"] for item in manifest.get("source_files", [])}
    manifest["expected_hashes"] = {
        unit["unit_id"]: unit.get("sha256") or source_hash_by_path.get(unit["rel_path"])
        for unit in coverage_units
    }
    manifest["expected_unit_to_file"] = {unit["unit_id"]: unit["rel_path"] for unit in coverage_units}
    manifest["coverage_units_normalized"] = coverage_units
    manifest["expected_by_batch"] = {
        batch["id"]: set(batch.get("coverage_units") or batch.get("files", [])) for batch in manifest.get("batches", [])
    }
    manifest["expected_files_by_batch"] = {
        batch["id"]: {
            (manifest["expected_unit_to_file"].get(unit_id, unit_id))
            for unit_id in (batch.get("coverage_units") or batch.get("files", []))
        }
        for batch in manifest.get("batches", [])
    }
    return manifest


def verify_completion_marker(manifest_path: Path, manifest: dict) -> list[dict]:
    marker_path = manifest_path.parent / "queue_complete.json"
    if not marker_path.is_file():
        return [{"path": str(marker_path), "reason": "queue_complete.json is missing"}]
    legacy_marker_path = manifest_path.parent / "audit_complete.json"
    if legacy_marker_path.is_file():
        return [
            {
                "path": str(legacy_marker_path),
                "reason": "legacy audit_complete.json must not be used as a queue or verification marker",
            }
        ]
    try:
        marker = json.loads(marker_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError:
        return [{"path": str(marker_path), "reason": f"{marker_path.name} is not valid JSON"}]
    if not isinstance(marker, dict):
        return [{"path": str(marker_path), "reason": f"{marker_path.name} must be a JSON object"}]

    mismatches: list[dict] = []
    expected = {
        "run_id": manifest.get("run_id"),
        "phase": "queue_generated",
        "audit_verified": False,
        "batch_count": manifest.get("batch_count"),
        "source_file_count": manifest.get("source_file_count"),
        "manifest": "manifest.json",
        "audit_index": "audit_index.md",
        "effort_ledger": "effort_ledger.json",
        "excluded_files": "excluded_files.json",
        "reports_dir": "reports",
        "ownership_marker": ".full-repo-audit-artifacts.json",
        "marker_semantics": "Queue artifacts were generated; subagent reports and effort ledger still require verifier completion.",
    }
    for field, expected_value in expected.items():
        if expected_value is not None and marker.get(field) != expected_value:
            mismatches.append(
                {
                    "path": str(marker_path),
                    "field": field,
                    "expected": expected_value,
                    "actual": marker.get(field),
                }
            )
    return mismatches


def load_excluded_files(manifest_path: Path) -> list[dict]:
    excluded_path = manifest_path.parent / "excluded_files.json"
    if not excluded_path.is_file():
        raise ValueError(f"excluded_files.json is missing: {excluded_path}")
    try:
        excluded = json.loads(excluded_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ValueError(f"excluded_files.json is not valid JSON: {excluded_path}") from exc
    if not isinstance(excluded, list):
        raise ValueError("excluded_files.json must be a JSON list.")
    for index, item in enumerate(excluded):
        if not isinstance(item, dict):
            raise ValueError(f"excluded_files[{index}] must be an object.")
        if not isinstance(item.get("path"), str) or not item["path"]:
            raise ValueError(f"excluded_files[{index}].path must be a non-empty string.")
        if not isinstance(item.get("reason"), str) or not item["reason"]:
            raise ValueError(f"excluded_files[{index}].reason must be a non-empty string.")
        if "scope_warning" in item and not isinstance(item["scope_warning"], bool):
            raise ValueError(f"excluded_files[{index}].scope_warning must be boolean when present.")
    return excluded


def canonical_json_sha256(data: dict | list) -> str:
    payload = json.dumps(data, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(payload).hexdigest()


def verify_excluded_files(manifest_path: Path, manifest: dict) -> tuple[list[dict], list[dict]]:
    excluded = load_excluded_files(manifest_path)
    manifest_warnings = manifest.get("scope_warnings", [])
    if not isinstance(manifest_warnings, list):
        raise ValueError("Manifest field scope_warnings must be a list when present.")
    excluded_warnings = [item for item in excluded if item.get("scope_warning")]
    issues = []
    manifest_warning_count = manifest.get("scope_warning_count", len(manifest_warnings))
    if manifest_warning_count != len(manifest_warnings):
        issues.append(
            {
                "path": str(manifest_path),
                "field": "scope_warning_count",
                "expected": len(manifest_warnings),
                "actual": manifest_warning_count,
            }
        )
    manifest_excluded_count = manifest.get("excluded_file_count")
    if manifest_excluded_count != len(excluded):
        issues.append(
            {
                "path": str(manifest_path),
                "field": "excluded_file_count",
                "expected": len(excluded),
                "actual": manifest_excluded_count,
            }
        )
    manifest_excluded_digest = manifest.get("excluded_files_sha256")
    excluded_digest = canonical_json_sha256(excluded)
    if manifest_excluded_digest != excluded_digest:
        issues.append(
            {
                "path": str(manifest_path.parent / "excluded_files.json"),
                "field": "excluded_files_sha256",
                "expected": manifest_excluded_digest,
                "actual": excluded_digest,
                "reason": "excluded_files.json content differs from manifest digest",
            }
        )
    manifest_warning_rows = set()
    for index, item in enumerate(manifest_warnings):
        if not isinstance(item, dict):
            raise ValueError(f"Manifest scope_warnings[{index}] must be an object.")
        if not isinstance(item.get("path"), str) or not item["path"]:
            raise ValueError(f"Manifest scope_warnings[{index}].path must be a non-empty string.")
        if not isinstance(item.get("reason"), str) or not item["reason"]:
            raise ValueError(f"Manifest scope_warnings[{index}].reason must be a non-empty string.")
        manifest_warning_rows.add((item["path"], item["reason"]))
    excluded_warning_rows = {(item.get("path"), item.get("reason")) for item in excluded_warnings}
    if manifest_warning_rows != excluded_warning_rows:
        def warning_dicts(rows: set[tuple[str | None, str | None]]) -> list[dict]:
            return [
                {"path": path, "reason": reason}
                for path, reason in sorted(rows, key=lambda row: (row[0] or "", row[1] or ""))
            ]

        issues.append(
            {
                "path": str(manifest_path.parent / "excluded_files.json"),
                "reason": "scope warning rows differ between manifest and excluded_files.json",
                "manifest": warning_dicts(manifest_warning_rows),
                "excluded_files": warning_dicts(excluded_warning_rows),
            }
        )
    if manifest_warning_count != len(excluded_warnings):
        issues.append(
            {
                "path": str(manifest_path.parent / "excluded_files.json"),
                "field": "scope_warning_count",
                "expected": manifest_warning_count,
                "actual": len(excluded_warnings),
            }
        )
    return excluded_warnings, issues


def validate_journey_report(
    report_path: Path,
    *,
    expected_run_id: str | None,
    worker_key: str,
    interface_files: set[str],
    known_source_files: set[str],
) -> list[dict]:
    required_sections = JOURNEY_REPORT_SECTIONS[worker_key]
    worker_label = JOURNEY_WORKER_LABELS[worker_key]
    try:
        text = report_path.read_text(encoding="utf-8")
    except UnicodeDecodeError as exc:
        return [{"path": str(report_path), "reason": "journey report is not valid UTF-8", "error": str(exc)}]
    except OSError as exc:
        return [{"path": str(report_path), "reason": "journey report could not be read", "error": str(exc)}]

    section_order = [
        match.group(1).strip().lower()
        for line in text.splitlines()
        if (match := SECTION_RE.match(line.strip()))
    ]
    bodies = section_bodies(text)
    issues: list[dict] = []
    if section_order != list(required_sections):
        issues.append(
            {
                "path": str(report_path),
                "reason": "journey report sections must match the required order exactly",
                "expected": list(required_sections),
                "actual": section_order,
            }
        )
    run_ids = declared_run_ids(text)
    if len(run_ids) != 1 or (expected_run_id and run_ids and run_ids[0] != expected_run_id):
        issues.append(
            {
                "path": str(report_path),
                "field": "run id",
                "expected": expected_run_id,
                "actual": run_ids,
            }
        )
    worker_body = bodies.get("worker", "").strip().splitlines()
    worker_value = worker_body[0].strip() if worker_body else ""
    if worker_value != worker_label:
        issues.append(
            {
                "path": str(report_path),
                "field": "worker",
                "expected": worker_label,
                "actual": worker_value,
            }
        )
    for section in required_sections:
        if not bodies.get(section, "").strip():
            issues.append({"path": str(report_path), "section": section, "reason": "section body is empty"})
    findings_issues = validate_findings_schema(bodies.get("findings", ""))
    if findings_issues:
        issues.append({"path": str(report_path), "section": "findings", "issues": findings_issues})
    semantic_findings_issues = validate_journey_findings_semantics(
        bodies.get("findings", ""),
        known_source_files,
        interface_files,
    )
    if semantic_findings_issues:
        issues.append({"path": str(report_path), "section": "findings", "issues": semantic_findings_issues})
    report_text = f"{bodies.get('journey sources', '')}\n{bodies.get('ui source journey checks', '')}\n{bodies.get('visual journey checks', '')}\n{bodies.get('visual tooling', '')}"
    not_applicable_text = normalized_text(report_text)
    report_is_not_applicable = worker_key == "visual_journey_worker" and "not applicable" in not_applicable_text and (
        "no repo-owned" in not_applicable_text
        or "host-owned" in not_applicable_text
        or "no visual ui" in not_applicable_text
        or "no rendered ui" in not_applicable_text
    )
    missing_interface_mentions = sorted(
        rel_path
        for rel_path in interface_files
        if rel_path not in report_text
    )
    if missing_interface_mentions:
        issues.append(
            {
                "path": str(report_path),
                "section": "journey coverage",
                "reason": "journey report must mention each manifest interface file, including visual not-applicable reports",
                "files": missing_interface_mentions,
            }
        )
    unknown_finding_refs: set[str] = set()
    for block in parse_finding_blocks(bodies.get("findings", "")):
        refs = PATH_IN_BACKTICKS_RE.findall(block["fields"].get("files", ""))
        unknown_finding_refs.update(ref for ref in refs if ref not in known_source_files)
    if unknown_finding_refs:
        issues.append(
            {
                "path": str(report_path),
                "section": "findings",
                "reason": "journey finding Files fields must reference manifest source files",
                "files": sorted(unknown_finding_refs),
            }
        )
    if worker_key == "journey_source_worker":
        proposed_body = normalized_text(bodies.get("proposed journeys", ""))
        if "confirmed" not in proposed_body and "draft-needs-user-confirmation" not in proposed_body:
            issues.append(
                {
                    "path": str(report_path),
                    "section": "proposed journeys",
                    "reason": "source journey report must list confirmed journeys or draft-needs-user-confirmation journeys",
                }
            )
        checks_body = bodies.get("ui source journey checks", "")
        if "| journey |" not in normalized_text(checks_body) or "relevance" not in normalized_text(checks_body):
            issues.append(
                {
                    "path": str(report_path),
                    "section": "ui source journey checks",
                    "reason": "journey source report must include the required table with relevance estimates",
                }
            )
        table_rows = parse_markdown_table_dicts(checks_body)
        if table_rows and set(table_rows[0]) != SOURCE_JOURNEY_TABLE_HEADERS:
            issues.append(
                {
                    "path": str(report_path),
                    "section": "ui source journey checks",
                    "reason": "journey source table headers must exactly match the required columns",
                    "expected": sorted(SOURCE_JOURNEY_TABLE_HEADERS),
                    "actual": sorted(table_rows[0]),
                }
            )
        if table_rows:
            mentioned_in_rows = {
                rel_path
                for row in table_rows
                for rel_path in interface_files
                if rel_path in " ".join(row.values())
            }
            missing_rows = sorted(interface_files - mentioned_in_rows)
            if missing_rows:
                issues.append(
                    {
                        "path": str(report_path),
                        "section": "ui source journey checks",
                        "reason": "journey source table must cover each manifest interface file",
                        "files": missing_rows,
                    }
                )
            row_issues = []
            for index, row in enumerate(table_rows, start=1):
                relevance_values = {
                    normalized_text(value)
                    for value in re.split(r"[,;/]", row.get("relevance estimate", ""))
                    if normalized_text(value)
                }
                if not relevance_values or not relevance_values <= ALLOWED_RELEVANCE_VALUES:
                    row_issues.append({"row": index, "field": "relevance estimate", "actual": row.get("relevance estimate", "")})
                for field in ("primary navigation/decision elements", "required information", "mobile/desktop availability", "test mode evidence"):
                    if is_boilerplate_value(row.get(field, "")):
                        row_issues.append({"row": index, "field": field, "actual": row.get(field, "")})
            if row_issues:
                issues.append(
                    {
                        "path": str(report_path),
                        "section": "ui source journey checks",
                        "reason": "journey source rows must include allowed relevance values and non-boilerplate decision/test-mode fields",
                        "rows": row_issues,
                    }
                )
        else:
            issues.append(
                {
                    "path": str(report_path),
                    "section": "ui source journey checks",
                    "reason": "journey source report must include at least one journey table row",
                }
            )
    if worker_key == "visual_journey_worker":
        tooling_body = normalized_text(bodies.get("visual tooling", ""))
        checks_body_raw = bodies.get("visual journey checks", "")
        checks_body = normalized_text(checks_body_raw)
        changed_review_body = bodies.get("changed visual review", "")
        danger_failure = False
        require_mobile = any(Path(rel_path).suffix.lower() in WEB_UI_EXTENSIONS for rel_path in interface_files)
        if not any(term in tooling_body for term in ("test mode", "fixture", "playwright", "cypress", "storybook", "browser", "not applicable", "no visual")):
            issues.append(
                {
                    "path": str(report_path),
                    "section": "visual tooling",
                    "reason": "visual report must identify visual tooling/test mode or explicitly explain why it is not applicable",
                }
            )
        if "| journey |" not in checks_body or "viewport" not in checks_body:
            issues.append(
                {
                    "path": str(report_path),
                    "section": "visual journey checks",
                    "reason": "visual report must include the required table with viewport checks",
                }
            )
        if interface_files and not report_is_not_applicable:
            checklist_text = f"{checks_body_raw}\n{bodies.get('findings', '')}"
            missing_labels = common.interaction_checklist_missing(checklist_text)
            if missing_labels:
                issues.append(
                    {
                        "path": str(report_path),
                        "section": "visual journey checks",
                        "reason": "rendered visual journey report must mark every interaction checklist label pass/gap/blocked/not-applicable",
                        "missing": missing_labels,
                    }
                )
        visual_rows = parse_markdown_table_dicts(checks_body_raw)
        if visual_rows and set(visual_rows[0]) != VISUAL_JOURNEY_TABLE_HEADERS:
            issues.append(
                {
                    "path": str(report_path),
                    "section": "visual journey checks",
                    "reason": "visual journey table headers must exactly match the required columns",
                    "expected": sorted(VISUAL_JOURNEY_TABLE_HEADERS),
                    "actual": sorted(visual_rows[0]),
                }
            )
        if not visual_rows:
            issues.append(
                {
                    "path": str(report_path),
                    "section": "visual journey checks",
                    "reason": "visual report must include at least one journey/viewport table row",
                }
            )
        if visual_rows and not report_is_not_applicable:
            rendered_rows = [
                row
                for row in visual_rows
                if normalized_text(row.get("result", "")) not in {"blocked", "not applicable", "not-applicable", "n/a"}
            ]
            evidence_records, evidence_issues = audit_evidence.validate_visual_evidence_manifest(
                report_path.parent.parent,
                expected_run_id or "",
                required=bool(rendered_rows),
            )
            for issue in evidence_issues:
                issues.append({"path": str(report_path), "section": "visual evidence", **issue})
            if rendered_rows:
                required_kinds = {"screenshot"}
                if any(Path(rel_path).suffix.lower() in WEB_UI_EXTENSIONS for rel_path in interface_files):
                    required_kinds.update({"formal-web-verifier", "review-queue", "manual-review"})
                for issue in audit_evidence.validate_references(text, evidence_records, required_kinds=required_kinds):
                    issues.append({"path": str(report_path), "section": "visual evidence", **issue})
                for index, row in enumerate(rendered_rows, start=1):
                    row_ids = audit_evidence.evidence_references(row.get("evidence", ""))
                    screenshots = [
                        evidence_records[item]
                        for item in row_ids
                        if item in evidence_records and evidence_records[item].get("kind") in {"screenshot", "native-snapshot"}
                    ]
                    if not screenshots:
                        issues.append(
                            {
                                "path": str(report_path),
                                "section": "visual evidence",
                                "row": index,
                                "reason": "each rendered viewport row must bind a real screenshot/native snapshot with evidence:<id>",
                            }
                        )
                        continue
                    route = normalized_text(row.get("route/screen", ""))
                    viewport = normalized_text(row.get("viewport", ""))
                    if not any(normalized_text(str(item.get("route", ""))) == route for item in screenshots):
                        issues.append({"path": str(report_path), "section": "visual evidence", "row": index, "reason": "screenshot route metadata does not match the report row"})
                    if not any(
                        viewport in normalized_text(str(item.get("viewport", {}).get("label", "")))
                        or normalized_text(str(item.get("viewport", {}).get("label", ""))) in viewport
                        for item in screenshots
                    ):
                        issues.append({"path": str(report_path), "section": "visual evidence", "row": index, "reason": "screenshot viewport metadata does not match the report row"})
            changed_rows = parse_markdown_table_dicts(changed_review_body)
            if require_mobile and not changed_rows:
                issues.append({"path": str(report_path), "section": "changed visual review", "reason": "rendered web UI requires a changed visual-review table"})
            if changed_rows and set(changed_rows[0]) != CHANGED_VISUAL_REVIEW_HEADERS:
                issues.append({
                    "path": str(report_path),
                    "section": "changed visual review",
                    "reason": "changed visual-review table headers must exactly match the required columns",
                    "expected": sorted(CHANGED_VISUAL_REVIEW_HEADERS),
                    "actual": sorted(changed_rows[0]),
                })
            allowed_review_decisions = {
                "pass", "gap", "blocked", "carried-pass", "carried-gap", "carried-blocked", "not applicable",
            }
            for index, row in enumerate(changed_rows, start=1):
                decision = normalized_text(row.get("decision", ""))
                if decision not in allowed_review_decisions:
                    issues.append({"path": str(report_path), "section": "changed visual review", "row": index, "field": "decision", "actual": decision})
                    continue
                initial = row.get("initial viewport evidence", "")
                full_page = row.get("full-page evidence", "")
                if decision.startswith("carried-"):
                    if "not reopened" not in normalized_text(initial) or "not reopened" not in normalized_text(full_page):
                        issues.append({"path": str(report_path), "section": "changed visual review", "row": index, "reason": "carried unchanged screenshots must explicitly remain unopened"})
                elif decision != "not applicable":
                    initial_refs = audit_evidence.evidence_references(initial)
                    full_refs = audit_evidence.evidence_references(full_page)
                    if not any(evidence_records.get(item, {}).get("kind") in {"screenshot", "native-snapshot"} for item in initial_refs):
                        issues.append({"path": str(report_path), "section": "changed visual review", "row": index, "reason": "queued cell requires initial-viewport screenshot evidence"})
                    if not any(evidence_records.get(item, {}).get("kind") in {"screenshot", "native-snapshot"} for item in full_refs):
                        issues.append({"path": str(report_path), "section": "changed visual review", "row": index, "reason": "queued cell requires full-page screenshot evidence"})
                if decision in {"gap", "blocked", "carried-gap", "carried-blocked"} and normalized_text(bodies.get("findings", "")) in {"", "no findings."}:
                    issues.append({"path": str(report_path), "section": "changed visual review", "row": index, "reason": "review gap/blocked status requires a finding"})
            if require_mobile and changed_rows:
                changed_kinds = {
                    evidence_records[item].get("kind")
                    for item in audit_evidence.evidence_references(changed_review_body)
                    if item in evidence_records
                }
                missing_review_kinds = sorted({"formal-web-verifier", "review-queue", "manual-review"} - changed_kinds)
                if missing_review_kinds:
                    issues.append({"path": str(report_path), "section": "changed visual review", "reason": "web UI review must cite the complete formal review chain", "missing_kinds": missing_review_kinds})
            viewports_by_journey: dict[str, set[str]] = defaultdict(set)
            for row in visual_rows:
                journey = normalized_text(row.get("journey", ""))
                viewport = normalized_text(row.get("viewport", ""))
                if journey:
                    viewports_by_journey[journey].add(viewport)
            incomplete = []
            for journey, viewports in sorted(viewports_by_journey.items()):
                has_desktop = any("desktop" in viewport for viewport in viewports)
                has_mobile = any(
                    term in viewport
                    for viewport in viewports
                    for term in ("mobile", "narrow", "small", "phone")
                )
                if not has_desktop or (require_mobile and not has_mobile):
                    incomplete.append(
                        {
                            "journey": journey,
                            "viewports": sorted(viewports),
                            "missing": [
                                label
                                for label, ok in (("desktop", has_desktop), ("narrow mobile", has_mobile or not require_mobile))
                                if not ok
                            ],
                        }
                    )
            if incomplete:
                issues.append(
                    {
                        "path": str(report_path),
                        "section": "visual journey checks",
                        "reason": (
                            "visual report must include desktop and narrow-mobile viewport rows per journey unless checks are not applicable"
                            if require_mobile
                            else "visual report must include desktop/native viewport rows per journey unless checks are not applicable"
                        ),
                        "journeys": incomplete,
                    }
                )
            evidence_text = f"{tooling_body}\n{checks_body}"
            has_tool_or_command = any(term in evidence_text for term in VISUAL_EVIDENCE_TOOL_TERMS)
            has_artifact_evidence = any(term in evidence_text for term in VISUAL_EVIDENCE_ARTIFACT_TERMS)
            if not has_tool_or_command or not has_artifact_evidence:
                issues.append(
                    {
                        "path": str(report_path),
                        "section": "visual tooling",
                        "reason": "visual report must cite commands/tools run and screenshot/trace/artifact evidence when visual checks are applicable",
                        "has_tool_or_command": has_tool_or_command,
                        "has_artifact_evidence": has_artifact_evidence,
                    }
                )
        if visual_rows:
            row_issues = []
            for index, row in enumerate(visual_rows, start=1):
                for field in ("journey", "viewport", "route/screen", "evidence"):
                    if is_boilerplate_value(row.get(field, "")) or len(plain_cell(row.get(field, ""))) < 4:
                        row_issues.append({"row": index, "field": field, "actual": row.get(field, "")})
                if not report_is_not_applicable:
                    for field in ("navigation visibility", "decision information", "visual quality", "result"):
                        if is_boilerplate_value(row.get(field, "")) or len(plain_cell(row.get(field, ""))) < 4:
                            row_issues.append({"row": index, "field": field, "actual": row.get(field, "")})
                    result = normalized_text(row.get("result", ""))
                    if result in {"pass", "passed", "matched"} and VISUAL_DANGER_RE.search(" ".join(row.values())):
                        danger_failure = True
                        row_issues.append(
                            {
                                "row": index,
                                "field": "result",
                                "reason": "visual danger terms such as overload, nested frames, unstable disclosure, meaningless icons, instruction noise, unreadable text, clipping, overflow, or low contrast cannot be marked pass without a finding",
                                "actual": row.get("result", ""),
                            }
                        )
            if row_issues:
                issues.append(
                    {
                        "path": str(report_path),
                        "section": "visual journey checks",
                        "reason": "visual rows must include non-boilerplate route, evidence, navigation, decision, quality, and result details",
                        "rows": row_issues,
                    }
                )
            if danger_failure and not has_visual_danger_finding(bodies.get("findings", "")):
                issues.append(
                    {
                        "path": str(report_path),
                        "section": "findings",
                        "reason": "visual danger terms require a visual/usability finding",
                    }
                )
    return issues


def verify_effort_ledger(
    manifest_path: Path,
    manifest: dict,
    known_batch_ids: set[str],
    report_rel_by_batch: dict[str, str],
) -> list[dict]:
    ledger_path = manifest_path.parent / "effort_ledger.json"
    if not ledger_path.is_file():
        return [{"path": str(ledger_path), "reason": "effort_ledger.json is missing"}]
    try:
        ledger = json.loads(ledger_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError:
        return [{"path": str(ledger_path), "reason": "effort_ledger.json is not valid JSON"}]
    if not isinstance(ledger, dict):
        return [{"path": str(ledger_path), "reason": "effort_ledger.json must be a JSON object"}]

    issues: list[dict] = []
    if ledger.get("run_id") != manifest.get("run_id"):
        issues.append({"path": str(ledger_path), "field": "run_id", "expected": manifest.get("run_id"), "actual": ledger.get("run_id")})
    if not isinstance(ledger.get("provenance_scope"), str) or not ledger.get("provenance_scope"):
        issues.append({"path": str(ledger_path), "field": "provenance_scope", "expected": "non-empty string", "actual": ledger.get("provenance_scope")})
    if ledger.get("effort_verification_scope") != "ledger-recorded":
        issues.append({"path": str(ledger_path), "field": "effort_verification_scope", "expected": "ledger-recorded", "actual": ledger.get("effort_verification_scope")})

    capability = ledger.get("subagent_capability_check")
    if not isinstance(capability, dict) or capability.get("status") != "completed":
        issues.append({"path": str(ledger_path), "field": "subagent_capability_check.status", "expected": "completed", "actual": capability.get("status") if isinstance(capability, dict) else None})
    capability_can_set = capability.get("can_set_reasoning_effort") if isinstance(capability, dict) else None
    if not isinstance(capability_can_set, bool):
        issues.append({"path": str(ledger_path), "field": "subagent_capability_check.can_set_reasoning_effort", "expected": "boolean", "actual": capability_can_set})
    if not isinstance(capability, dict) or not isinstance(capability.get("spawn_tool"), str) or not capability.get("spawn_tool"):
        issues.append({"path": str(ledger_path), "field": "subagent_capability_check.spawn_tool", "expected": "non-empty string", "actual": capability.get("spawn_tool") if isinstance(capability, dict) else None})
    if not isinstance(capability, dict) or not isinstance(capability.get("notes"), str) or not capability.get("notes"):
        issues.append({"path": str(ledger_path), "field": "subagent_capability_check.notes", "expected": "non-empty string", "actual": capability.get("notes") if isinstance(capability, dict) else None})
    if isinstance(capability, dict):
        for issue in validate_effort_claim(capability, basis_field="claim_basis", label_field="claim_label", prefix="subagent_capability_check"):
            issues.append({"path": str(ledger_path), **issue})
        if not isinstance(capability.get("evidence"), str) or len(capability.get("evidence", "").strip()) < 12:
            issues.append({"path": str(ledger_path), "field": "subagent_capability_check.evidence", "expected": "concrete capability evidence", "actual": capability.get("evidence")})

    fallback = ledger.get("fallback_mode")
    if not isinstance(fallback, dict):
        issues.append({"path": str(ledger_path), "field": "fallback_mode", "expected": "object", "actual": type(fallback).__name__})
        fallback = {}
    fallback_active_value = fallback.get("active")
    if not isinstance(fallback_active_value, bool):
        issues.append({"path": str(ledger_path), "field": "fallback_mode.active", "expected": "boolean", "actual": fallback_active_value})
    fallback_active = fallback_active_value is True
    if fallback_active and not fallback.get("reason"):
        issues.append({"path": str(ledger_path), "field": "fallback_mode.reason", "expected": "non-empty reason when fallback is active", "actual": fallback.get("reason")})
    if not fallback_active and fallback.get("reason"):
        issues.append({"path": str(ledger_path), "field": "fallback_mode.reason", "expected": None, "actual": fallback.get("reason")})
    if capability_can_set is True and fallback_active:
        issues.append({"path": str(ledger_path), "field": "fallback_mode.active", "expected": False, "actual": True})
    if capability_can_set is False and not fallback_active:
        issues.append({"path": str(ledger_path), "field": "fallback_mode.active", "expected": True, "actual": False})

    raw_pruned_hint_count = manifest.get("pruned_directory_review_hint_count", 0)
    if isinstance(raw_pruned_hint_count, int) and not isinstance(raw_pruned_hint_count, bool):
        pruned_hint_count = raw_pruned_hint_count
    else:
        pruned_hint_count = 0
        issues.append(
            {
                "path": str(ledger_path),
                "field": "pruned_directory_review_hint_count",
                "expected": "integer manifest count",
                "actual": raw_pruned_hint_count,
            }
        )
    pruned_hints = manifest.get("pruned_directory_review_hints", [])
    expected_pruned_paths = {
        hint.get("path")
        for hint in pruned_hints
        if isinstance(hint, dict) and isinstance(hint.get("path"), str)
    }
    pruned_review = ledger.get("pruned_directory_review")
    if pruned_hint_count:
        if not isinstance(pruned_review, dict):
            issues.append({"path": str(ledger_path), "field": "pruned_directory_review", "expected": "object", "actual": type(pruned_review).__name__})
        else:
            if pruned_review.get("status") != "completed":
                issues.append({"path": str(ledger_path), "field": "pruned_directory_review.status", "expected": "completed", "actual": pruned_review.get("status")})
            if pruned_review.get("hint_count") != pruned_hint_count:
                issues.append({"path": str(ledger_path), "field": "pruned_directory_review.hint_count", "expected": pruned_hint_count, "actual": pruned_review.get("hint_count")})
            if not isinstance(pruned_review.get("notes"), str) or not pruned_review.get("notes"):
                issues.append({"path": str(ledger_path), "field": "pruned_directory_review.notes", "expected": "non-empty review notes", "actual": pruned_review.get("notes")})
            decisions = pruned_review.get("decisions")
            if not isinstance(decisions, list):
                issues.append({"path": str(ledger_path), "field": "pruned_directory_review.decisions", "expected": "one decision per pruned hint", "actual": type(decisions).__name__})
            else:
                observed_paths = set()
                decision_issues = []
                for index, decision in enumerate(decisions):
                    if not isinstance(decision, dict):
                        decision_issues.append({"index": index, "reason": "decision row must be an object", "actual": type(decision).__name__})
                        continue
                    path = decision.get("path")
                    if not isinstance(path, str):
                        decision_issues.append({"index": index, "field": "path", "expected": "string path from manifest pruned_directory_review_hints", "actual": path})
                    else:
                        observed_paths.add(path)
                    if not isinstance(path, str) or path not in expected_pruned_paths:
                        decision_issues.append({"index": index, "field": "path", "reason": "path is not in manifest pruned_directory_review_hints", "actual": path})
                    decision_value = decision.get("decision")
                    if decision_value not in ALLOWED_PRUNED_REVIEW_DECISIONS:
                        decision_issues.append(
                            {
                                "index": index,
                                "field": "decision",
                                "expected": sorted(ALLOWED_PRUNED_REVIEW_DECISIONS),
                                "actual": decision_value,
                            }
                        )
                    rationale = decision.get("rationale")
                    if not isinstance(rationale, str) or is_boilerplate_value(rationale) or len(plain_cell(rationale)) < 12:
                        decision_issues.append({"index": index, "field": "rationale", "expected": "non-boilerplate rationale", "actual": rationale})
                missing_decision_paths = sorted(expected_pruned_paths - observed_paths)
                extra_decision_paths = sorted(path for path in observed_paths - expected_pruned_paths if path is not None)
                if missing_decision_paths or extra_decision_paths:
                    decision_issues.append(
                        {
                            "field": "path coverage",
                            "missing": missing_decision_paths,
                            "extra": extra_decision_paths,
                        }
                    )
                if decision_issues:
                    issues.append(
                        {
                            "path": str(ledger_path),
                            "field": "pruned_directory_review.decisions",
                            "reason": "each pruned directory hint needs a structured lead decision and rationale",
                            "issues": decision_issues,
                        }
                    )
    elif isinstance(pruned_review, dict) and pruned_review.get("status") != "not-applicable":
        issues.append({"path": str(ledger_path), "field": "pruned_directory_review.status", "expected": "not-applicable", "actual": pruned_review.get("status")})

    raw_tracked_deletion_count = manifest.get("tracked_deletion_count", 0)
    if isinstance(raw_tracked_deletion_count, int) and not isinstance(raw_tracked_deletion_count, bool):
        tracked_deletion_count = raw_tracked_deletion_count
    else:
        tracked_deletion_count = 0
        issues.append(
            {
                "path": str(ledger_path),
                "field": "tracked_deletion_count",
                "expected": "integer manifest count",
                "actual": raw_tracked_deletion_count,
            }
        )
    tracked_deletions = manifest.get("tracked_deletions", [])
    expected_tracked_deletions = {
        removal.get("path"): removal
        for removal in tracked_deletions
        if isinstance(removal, dict) and isinstance(removal.get("path"), str)
    }
    tracked_deletion_review = ledger.get("tracked_deletion_review")
    if tracked_deletion_count:
        if not isinstance(tracked_deletion_review, dict):
            issues.append({"path": str(ledger_path), "field": "tracked_deletion_review", "expected": "object", "actual": type(tracked_deletion_review).__name__})
        else:
            if tracked_deletion_review.get("status") != "completed":
                issues.append({"path": str(ledger_path), "field": "tracked_deletion_review.status", "expected": "completed", "actual": tracked_deletion_review.get("status")})
            if tracked_deletion_review.get("removal_count") != tracked_deletion_count:
                issues.append({"path": str(ledger_path), "field": "tracked_deletion_review.removal_count", "expected": tracked_deletion_count, "actual": tracked_deletion_review.get("removal_count")})
            if not isinstance(tracked_deletion_review.get("notes"), str) or not tracked_deletion_review.get("notes"):
                issues.append({"path": str(ledger_path), "field": "tracked_deletion_review.notes", "expected": "non-empty review notes", "actual": tracked_deletion_review.get("notes")})
            decisions = tracked_deletion_review.get("decisions")
            if not isinstance(decisions, list):
                issues.append({"path": str(ledger_path), "field": "tracked_deletion_review.decisions", "expected": "one decision per tracked deletion", "actual": type(decisions).__name__})
            else:
                observed_paths = set()
                decision_issues = []
                for index, decision in enumerate(decisions):
                    if not isinstance(decision, dict):
                        decision_issues.append({"index": index, "reason": "decision row must be an object", "actual": type(decision).__name__})
                        continue
                    path = decision.get("path")
                    if not isinstance(path, str):
                        decision_issues.append({"index": index, "field": "path", "expected": "string path from manifest tracked_deletions", "actual": path})
                    else:
                        observed_paths.add(path)
                    expected_removal = expected_tracked_deletions.get(path) if isinstance(path, str) else None
                    if expected_removal is None:
                        decision_issues.append({"index": index, "field": "path", "reason": "path is not in manifest tracked_deletions", "actual": path})
                    elif decision.get("baseline_sha256") != expected_removal.get("baseline_sha256"):
                        decision_issues.append({"index": index, "field": "baseline_sha256", "expected": expected_removal.get("baseline_sha256"), "actual": decision.get("baseline_sha256")})
                    decision_value = decision.get("decision")
                    if decision_value not in ALLOWED_TRACKED_DELETION_REVIEW_DECISIONS:
                        decision_issues.append(
                            {
                                "index": index,
                                "field": "decision",
                                "expected": sorted(ALLOWED_TRACKED_DELETION_REVIEW_DECISIONS),
                                "actual": decision_value,
                            }
                        )
                    for field in ("rationale", "evidence"):
                        value = decision.get(field)
                        if not isinstance(value, str) or is_boilerplate_value(value) or len(plain_cell(value)) < 12:
                            decision_issues.append({"index": index, "field": field, "expected": "non-boilerplate evidence-backed text", "actual": value})
                missing_paths = sorted(set(expected_tracked_deletions) - observed_paths)
                extra_paths = sorted(path for path in observed_paths - set(expected_tracked_deletions) if path is not None)
                if missing_paths or extra_paths:
                    decision_issues.append({"field": "path coverage", "missing": missing_paths, "extra": extra_paths})
                if decision_issues:
                    issues.append(
                        {
                            "path": str(ledger_path),
                            "field": "tracked_deletion_review.decisions",
                            "reason": "each tracked deletion needs a structured evidence-backed lead disposition",
                            "issues": decision_issues,
                        }
                    )
    elif isinstance(tracked_deletion_review, dict) and tracked_deletion_review.get("status") != "not-applicable":
        issues.append({"path": str(ledger_path), "field": "tracked_deletion_review.status", "expected": "not-applicable", "actual": tracked_deletion_review.get("status")})

    expected_high_risk = {
        item.get("rel_path"): item
        for item in manifest.get("high_risk_files", [])
        if isinstance(item, dict) and isinstance(item.get("rel_path"), str)
    }
    high_risk_review = ledger.get("lead_high_risk_review")
    if expected_high_risk:
        if not isinstance(high_risk_review, dict) or high_risk_review.get("status") != "completed":
            issues.append({"path": str(ledger_path), "field": "lead_high_risk_review.status", "expected": "completed", "actual": high_risk_review.get("status") if isinstance(high_risk_review, dict) else None})
        else:
            rows = high_risk_review.get("files")
            if not isinstance(rows, list):
                issues.append({"path": str(ledger_path), "field": "lead_high_risk_review.files", "expected": "one row per high-risk file", "actual": type(rows).__name__})
            else:
                by_path = {row.get("rel_path"): row for row in rows if isinstance(row, dict) and isinstance(row.get("rel_path"), str)}
                if set(by_path) != set(expected_high_risk):
                    issues.append({"path": str(ledger_path), "field": "lead_high_risk_review.files", "missing": sorted(set(expected_high_risk) - set(by_path)), "extra": sorted(set(by_path) - set(expected_high_risk))})
                for rel_path, expected in expected_high_risk.items():
                    row = by_path.get(rel_path, {})
                    if row.get("status") != "completed" or row.get("sha256") != expected.get("sha256") or row.get("risk_reasons") != expected.get("risk_reasons"):
                        issues.append({"path": str(ledger_path), "field": f"lead_high_risk_review.files[{rel_path}]", "reason": "status/hash/risk reasons must match the manifest", "actual": row})
                    if not isinstance(row.get("evidence"), str) or len(plain_cell(row.get("evidence", ""))) < 20:
                        issues.append({"path": str(ledger_path), "field": f"lead_high_risk_review.files[{rel_path}].evidence", "reason": "lead review needs concrete source/risk evidence"})
                    if not isinstance(row.get("notes"), str) or len(plain_cell(row.get("notes", ""))) < 20:
                        issues.append({"path": str(ledger_path), "field": f"lead_high_risk_review.files[{rel_path}].notes", "reason": "lead review needs concrete risk notes"})
    elif not isinstance(high_risk_review, dict) or high_risk_review.get("status") != "not-applicable":
        issues.append({"path": str(ledger_path), "field": "lead_high_risk_review.status", "expected": "not-applicable", "actual": high_risk_review.get("status") if isinstance(high_risk_review, dict) else None})

    lead = ledger.get("lead")
    if not isinstance(lead, dict):
        issues.append({"path": str(ledger_path), "field": "lead", "expected": "object", "actual": type(lead).__name__})
    else:
        if lead.get("required_reasoning_effort") != "xhigh":
            issues.append({"path": str(ledger_path), "field": "lead.required_reasoning_effort", "expected": "xhigh", "actual": lead.get("required_reasoning_effort")})

    if isinstance(lead, dict):
        if lead.get("status") != "completed":
            issues.append({"path": str(ledger_path), "field": "lead.status", "expected": "completed", "actual": lead.get("status")})
        if lead.get("actual_reasoning_effort") != "xhigh":
            issues.append({"path": str(ledger_path), "field": "lead.actual_reasoning_effort", "expected": "xhigh", "actual": lead.get("actual_reasoning_effort")})
        if not lead.get("agent_id"):
            issues.append({"path": str(ledger_path), "field": "lead.agent_id", "expected": "agent id", "actual": lead.get("agent_id")})
        for issue in validate_effort_claim(lead, basis_field="effort_claim_basis", label_field="effort_claim_label", prefix="lead"):
            issues.append({"path": str(ledger_path), **issue})
        if not isinstance(lead.get("runtime_provenance"), str) or len(lead.get("runtime_provenance", "").strip()) < 12:
            issues.append({"path": str(ledger_path), "field": "lead.runtime_provenance", "expected": "concrete runtime provenance", "actual": lead.get("runtime_provenance")})

    expected_lead_reconciliation = manifest.get("lead_reconciliation", {})
    lead_reconciliation = ledger.get("lead_reconciliation")
    if not isinstance(lead_reconciliation, dict):
        issues.append(
            {
                "path": str(ledger_path),
                "field": "lead_reconciliation",
                "expected": "object",
                "actual": type(lead_reconciliation).__name__,
            }
        )
    else:
        if lead_reconciliation.get("status") != "completed":
            issues.append(
                {
                    "path": str(ledger_path),
                    "field": "lead_reconciliation.status",
                    "expected": "completed",
                    "actual": lead_reconciliation.get("status"),
                }
            )
        for ledger_field, manifest_field in (("prompt", "prompt"), ("report", "report")):
            expected_value = expected_lead_reconciliation.get(manifest_field)
            actual_value = lead_reconciliation.get(ledger_field)
            if actual_value != expected_value:
                issues.append(
                    {
                        "path": str(ledger_path),
                        "field": f"lead_reconciliation.{ledger_field}",
                        "expected": expected_value,
                        "actual": actual_value,
                    }
                )
            if isinstance(actual_value, str) and not (manifest_path.parent / actual_value).is_file():
                issues.append(
                    {
                        "path": str(ledger_path),
                        "field": f"lead_reconciliation.{ledger_field}",
                        "reason": "referenced lead reconciliation artifact is missing",
                        "actual": actual_value,
                    }
                )

    journey = manifest.get("journey_audit") if isinstance(manifest.get("journey_audit"), dict) else {}
    journey_required = bool(journey.get("required"))
    for worker_key in ("journey_source_worker", "visual_journey_worker"):
        worker = ledger.get(worker_key)
        if not isinstance(worker, dict):
            issues.append({"path": str(ledger_path), "field": worker_key, "expected": "object", "actual": type(worker).__name__})
            continue
        if not journey_required:
            if worker.get("status") != "not-applicable":
                issues.append({"path": str(ledger_path), "field": f"{worker_key}.status", "expected": "not-applicable", "actual": worker.get("status")})
            continue
        if worker.get("status") != "completed":
            issues.append({"path": str(ledger_path), "field": f"{worker_key}.status", "expected": "completed", "actual": worker.get("status")})
        if worker.get("required_reasoning_effort") != "low":
            issues.append({"path": str(ledger_path), "field": f"{worker_key}.required_reasoning_effort", "expected": "low", "actual": worker.get("required_reasoning_effort")})
        expected_prompt = journey.get(JOURNEY_PROMPT_FIELDS[worker_key])
        prompt = worker.get("prompt")
        if prompt != expected_prompt:
            issues.append({"path": str(ledger_path), "field": f"{worker_key}.prompt", "expected": expected_prompt, "actual": prompt})
        if not isinstance(prompt, str) or not prompt:
            issues.append({"path": str(ledger_path), "field": f"{worker_key}.prompt", "expected": "prompt path", "actual": prompt})
        else:
            prompt_path = manifest_path.parent / prompt
            if not prompt_path.is_file():
                issues.append({"path": str(ledger_path), "field": f"{worker_key}.prompt", "expected": f"existing file {prompt}", "actual": prompt})
        expected_report = journey.get(JOURNEY_REPORT_FIELDS[worker_key])
        report = worker.get("report")
        if report != expected_report:
            issues.append({"path": str(ledger_path), "field": f"{worker_key}.report", "expected": expected_report, "actual": report})
        if not isinstance(report, str) or not report:
            issues.append({"path": str(ledger_path), "field": f"{worker_key}.report", "expected": "report path", "actual": report})
        else:
            report_path = manifest_path.parent / report
            if not report_path.is_file():
                issues.append({"path": str(ledger_path), "field": f"{worker_key}.report", "expected": f"existing file {report}", "actual": report})
            else:
                for issue in validate_journey_report(
                    report_path,
                    expected_run_id=manifest.get("run_id"),
                    worker_key=worker_key,
                    interface_files=set(journey.get("interface_files") or []),
                    known_source_files=set(manifest.get("expected_files", set())),
                ):
                    issues.append({"path": str(ledger_path), "field": worker_key, **issue})
        if fallback_active:
            if worker.get("actual_reasoning_effort") != "manual-fallback":
                issues.append({"path": str(ledger_path), "field": f"{worker_key}.actual_reasoning_effort", "expected": "manual-fallback", "actual": worker.get("actual_reasoning_effort")})
        else:
            if not worker.get("agent_id"):
                issues.append({"path": str(ledger_path), "field": f"{worker_key}.agent_id", "expected": "agent id", "actual": worker.get("agent_id")})
            if worker.get("actual_reasoning_effort") != "low":
                issues.append({"path": str(ledger_path), "field": f"{worker_key}.actual_reasoning_effort", "expected": "low", "actual": worker.get("actual_reasoning_effort")})
        if not isinstance(worker.get("runtime_provenance"), str) or not worker.get("runtime_provenance"):
            issues.append({"path": str(ledger_path), "field": f"{worker_key}.runtime_provenance", "expected": "non-empty string", "actual": worker.get("runtime_provenance")})
        for issue in validate_effort_claim(worker, basis_field="effort_claim_basis", label_field="effort_claim_label", prefix=worker_key):
            issues.append({"path": str(ledger_path), **issue})

    batches = ledger.get("batches")
    if not isinstance(batches, list):
        issues.append({"path": str(ledger_path), "field": "batches", "expected": "list", "actual": type(batches).__name__})
        return issues

    expected_prompt_by_batch = {
        batch["id"]: batch.get("prompt")
        for batch in manifest.get("batches", [])
        if isinstance(batch, dict) and isinstance(batch.get("id"), str)
    }
    seen_batch_ids: set[str] = set()
    batch_id_counts: Counter[str] = Counter()
    for index, batch in enumerate(batches):
        if not isinstance(batch, dict):
            issues.append({"path": str(ledger_path), "field": f"batches[{index}]", "expected": "object", "actual": type(batch).__name__})
            continue
        batch_id = batch.get("batch_id")
        if not isinstance(batch_id, str):
            issues.append({"path": str(ledger_path), "field": f"batches[{index}].batch_id", "expected": "string batch id", "actual": batch_id})
            continue
        seen_batch_ids.add(batch_id)
        batch_id_counts[batch_id] += 1
        if batch_id not in known_batch_ids:
            issues.append({"path": str(ledger_path), "field": f"batches[{index}].batch_id", "expected": "known batch id", "actual": batch_id})
        expected_prompt = expected_prompt_by_batch.get(batch_id)
        if batch.get("prompt") != expected_prompt:
            issues.append({"path": str(ledger_path), "field": f"batches[{index}].prompt", "expected": expected_prompt, "actual": batch.get("prompt")})
        if batch.get("required_reasoning_effort") != "low":
            issues.append({"path": str(ledger_path), "field": f"batches[{index}].required_reasoning_effort", "expected": "low", "actual": batch.get("required_reasoning_effort")})
        if batch.get("status") != "completed":
            issues.append({"path": str(ledger_path), "field": f"batches[{index}].status", "expected": "completed", "actual": batch.get("status")})
        expected_report = f"reports/{batch_id}.md"
        actual_report = batch.get("report")
        if actual_report != expected_report:
            issues.append({"path": str(ledger_path), "field": f"batches[{index}].report", "expected": expected_report, "actual": actual_report})
        parsed_report = report_rel_by_batch.get(batch_id)
        if parsed_report and actual_report != parsed_report:
            issues.append({"path": str(ledger_path), "field": f"batches[{index}].report", "expected": parsed_report, "actual": actual_report})
        if fallback_active:
            if batch.get("actual_reasoning_effort") != "manual-fallback":
                issues.append({"path": str(ledger_path), "field": f"batches[{index}].actual_reasoning_effort", "expected": "manual-fallback", "actual": batch.get("actual_reasoning_effort")})
        else:
            if not batch.get("agent_id"):
                issues.append({"path": str(ledger_path), "field": f"batches[{index}].agent_id", "expected": "agent id", "actual": batch.get("agent_id")})
            if batch.get("actual_reasoning_effort") != "low":
                issues.append({"path": str(ledger_path), "field": f"batches[{index}].actual_reasoning_effort", "expected": "low", "actual": batch.get("actual_reasoning_effort")})
        if not isinstance(batch.get("runtime_provenance"), str) or not batch.get("runtime_provenance"):
            issues.append({"path": str(ledger_path), "field": f"batches[{index}].runtime_provenance", "expected": "non-empty string", "actual": batch.get("runtime_provenance")})
        for issue in validate_effort_claim(batch, basis_field="effort_claim_basis", label_field="effort_claim_label", prefix=f"batches[{index}]"):
            issues.append({"path": str(ledger_path), **issue})

    missing = sorted(known_batch_ids - seen_batch_ids)
    extra = sorted(item for item in seen_batch_ids - known_batch_ids if item is not None)
    duplicate_batch_ids = sorted(batch_id for batch_id, count in batch_id_counts.items() if count > 1)
    if missing:
        issues.append({"path": str(ledger_path), "field": "batches", "reason": "missing batch ledger rows", "batches": missing})
    if extra:
        issues.append({"path": str(ledger_path), "field": "batches", "reason": "unknown batch ledger rows", "batches": extra})
    if duplicate_batch_ids:
        issues.append({"path": str(ledger_path), "field": "batches", "reason": "duplicate batch ledger rows", "batches": duplicate_batch_ids})
    return issues


def filename_batch_id(report_path: Path) -> str | None:
    match = BATCH_ID_RE.search(report_path.name)
    if not match:
        return None
    return f"batch_{match.group(1)}".lower()


def declared_batch_ids(text: str) -> list[str]:
    ids: list[str] = []
    lines = text.splitlines()
    for index, line in enumerate(lines):
        section = SECTION_RE.match(line.strip())
        if not section or section.group(1).strip().lower() != "batch id":
            continue
        for value in lines[index + 1 :]:
            stripped = value.strip()
            if stripped.startswith("## "):
                break
            if not stripped:
                continue
            match = BATCH_ID_RE.search(stripped)
            if match:
                ids.append(f"batch_{match.group(1)}".lower())
    return ids


def declared_run_ids(text: str) -> list[str]:
    ids: list[str] = []
    lines = text.splitlines()
    for index, line in enumerate(lines):
        section = SECTION_RE.match(line.strip())
        if not section or section.group(1).strip().lower() != "run id":
            continue
        for value in lines[index + 1 :]:
            stripped = value.strip()
            if stripped.startswith("## "):
                break
            if stripped:
                ids.append(stripped.strip("`"))
    return ids


def section_bodies(text: str) -> dict[str, str]:
    bodies: dict[str, list[str]] = {}
    current: str | None = None
    for line in text.splitlines():
        section = SECTION_RE.match(line.strip())
        if section:
            current = section.group(1).strip().lower()
            bodies.setdefault(current, [])
            continue
        if current:
            bodies[current].append(line)
    return {section: "\n".join(lines).strip() for section, lines in bodies.items()}


def split_markdown_row(row: str) -> list[str]:
    stripped = row.strip()
    if stripped.startswith("|"):
        stripped = stripped[1:]
    if stripped.endswith("|"):
        stripped = stripped[:-1]

    columns: list[str] = []
    current: list[str] = []
    escaped = False
    for char in stripped:
        if escaped:
            current.append(char if char == "|" else f"\\{char}")
            escaped = False
            continue
        if char == "\\":
            escaped = True
            continue
        if char == "|":
            columns.append("".join(current).strip())
            current = []
            continue
        current.append(char)
    if escaped:
        current.append("\\")
    columns.append("".join(current).strip())
    return columns


def is_separator_row(columns: list[str]) -> bool:
    return bool(columns) and all(re.fullmatch(r":?-{3,}:?", column.strip()) for column in columns)


def plain_cell(value: str) -> str:
    return value.strip().strip("`").strip()


def normalized_text(value: str) -> str:
    return re.sub(r"\s+", " ", plain_cell(value).lower()).strip()


def parse_markdown_table_dicts(text: str) -> list[dict[str, str]]:
    rows: list[dict[str, str]] = []
    headers: list[str] | None = None
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped.startswith("|"):
            continue
        columns = split_markdown_row(stripped)
        if is_separator_row(columns):
            continue
        if headers is None:
            headers = [normalized_text(column) for column in columns]
            continue
        if len(columns) != len(headers):
            continue
        rows.append({header: columns[index] for index, header in enumerate(headers)})
    return rows


def loose_normalized_text(value: str) -> str:
    return re.sub(r"[^a-z0-9_.!#()-]+", " ", value.lower()).strip()


def is_boilerplate_value(value: str) -> bool:
    normalized = normalized_text(value)
    punctuationless = re.sub(r"[^\w\s]+", "", normalized).strip()
    return not normalized or normalized in BOILERPLATE_VALUES or punctuationless in BOILERPLATE_VALUES


def source_contains_visible_text(source: str, visible_text: str) -> bool:
    """Match extractor-normalized interface text back to its source.

    ``dedupe_hints`` collapses whitespace so TSX generic/type syntax cannot
    accidentally create multiline table cells. Validate against both the raw
    source and the same whitespace-normalized representation.
    """
    evidence = plain_cell(visible_text)
    return evidence in source or evidence in re.sub(r"\s+", " ", source)


def dedupe_hints(hints: list[str]) -> list[str]:
    cleaned = []
    seen = set()
    for hint in hints:
        normalized = re.sub(r"\s+", " ", hint).strip().strip("'\"")
        if (
            len(normalized) < 2
            or len(normalized) > 100
            or "{{" in normalized
            or "}}" in normalized
            or normalized in seen
        ):
            continue
        seen.add(normalized)
        cleaned.append(normalized)
    return cleaned


def split_json_key_tokens(key: str) -> set[str]:
    spaced = re.sub(r"([a-z0-9])([A-Z])", r"\1 \2", key)
    return {item.lower() for item in re.split(r"[^A-Za-z0-9]+", spaced) if item}


def looks_like_visible_json_string(key_stack: list[str], value: str) -> bool:
    stripped = value.strip()
    if not stripped:
        return False
    if re.match(r"(?i)^(?:[a-z][a-z0-9+.-]*:)?//", stripped) or stripped.startswith(("/", "./", "../")):
        return False
    if re.match(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", stripped):
        return False
    tokens: set[str] = set()
    for key in key_stack:
        tokens.update(split_json_key_tokens(key))
    if tokens & INTERNAL_JSON_KEY_TOKENS:
        return False
    if tokens & VISIBLE_JSON_KEY_TOKENS:
        return True
    if any(char.isspace() for char in stripped) or any(char.isupper() for char in stripped):
        return True
    return False


def json_string_hints(text: str) -> list[str]:
    try:
        payload = json.loads(text)
    except json.JSONDecodeError:
        return []
    hints: list[str] = []

    def walk(value: object, key_stack: list[str]) -> None:
        if isinstance(value, str):
            if looks_like_visible_json_string(key_stack, value):
                hints.append(value)
        elif isinstance(value, list):
            for item in value:
                walk(item, key_stack)
        elif isinstance(value, dict):
            for key, item in value.items():
                walk(item, [*key_stack, str(key)])

    walk(payload, [])
    return hints


def xml_element_text_hints(text: str, element_names: set[str]) -> list[str]:
    try:
        root = ET.fromstring(text)
    except ET.ParseError:
        return []
    hints: list[str] = []
    for element in root.iter():
        local_name = element.tag.rsplit("}", 1)[-1].lower()
        if local_name not in element_names:
            continue
        value = " ".join(part.strip() for part in element.itertext() if part.strip())
        if 2 <= len(value) <= 100:
            hints.append(value)
    return hints


def message_catalog_hints(suffix: str, text: str) -> list[str]:
    if suffix in {".json", ".arb"}:
        return json_string_hints(text)
    if suffix in {".po", ".pot"}:
        return [match.group(1).strip() for match in PO_VALUE_RE.finditer(text)]
    if suffix == ".properties":
        return [match.group(1).strip() for match in PROPERTIES_VALUE_RE.finditer(text)]
    if suffix == ".strings":
        return [match.group(1).strip() for match in APPLE_STRINGS_VALUE_RE.finditer(text)]
    if suffix == ".ftl":
        return [match.group(1).strip() for match in FTL_VALUE_RE.finditer(text)]
    if suffix == ".resx":
        return xml_element_text_hints(text, {"value"})
    if suffix in {".xlf", ".xliff"}:
        return xml_element_text_hints(text, {"source", "target"})
    return []


def visible_text_hints(rel_path: str, text: str) -> list[str]:
    suffix = PurePosixPath(rel_path).suffix.lower()
    if suffix not in VISIBLE_HINT_EXTENSIONS:
        return []
    hints: list[str] = []
    if suffix == ".md":
        h1_match = MARKDOWN_H1_RE.search(text)
        if h1_match:
            hints.append(h1_match.group(1).strip())
    if suffix in MESSAGE_CATALOG_HINT_EXTENSIONS:
        hints.extend(message_catalog_hints(suffix, text))
    if suffix in {".yaml", ".yml"}:
        hints.extend(match.group(1).strip() for match in SIMPLE_KEY_VALUE_RE.finditer(text))
    for match in VISIBLE_KEY_RE.finditer(text):
        hints.append(match.group(2).strip())
    for match in VISIBLE_ATTR_RE.finditer(text):
        hints.append((match.group(1) or match.group(2)).strip())
    for match in VISIBLE_TEXT_RE.finditer(text):
        if not appears_inside_quoted_literal(text, match.start()):
            hints.append(match.group(1).strip())
    return dedupe_hints(hints)


def appears_inside_quoted_literal(line: str, index: int) -> bool:
    escaped = False
    quote_counts = {"'": 0, '"': 0, "`": 0}
    for char in line[:index]:
        if escaped:
            escaped = False
            continue
        if char == "\\":
            escaped = True
            continue
        if char in quote_counts:
            quote_counts[char] += 1
    return any(count % 2 for count in quote_counts.values())


def placeholder_markers_for(rel_path: str, text: str) -> list[str]:
    suffix = PurePosixPath(rel_path).suffix.lower()
    markers = set()
    for line in text.splitlines():
        for match in PLACEHOLDER_COMMENT_RE.finditer(line):
            if not appears_inside_quoted_literal(line, match.start()):
                markers.add(line.strip()[:160])
        if suffix in CODE_STUB_EXTENSIONS:
            for match in CODE_STUB_RE.finditer(line):
                if not appears_inside_quoted_literal(line, match.start()):
                    markers.add(match.group(0).strip())
    if suffix in UI_CODE_EXTENSIONS:
        for line in text.splitlines():
            for match in CONSOLE_LOG_RE.finditer(line):
                if not appears_inside_quoted_literal(line, match.start()):
                    markers.add(match.group(0).strip())
    return sorted(markers)


def placeholder_marker_terms(marker: str) -> set[str]:
    normalized = loose_normalized_text(marker)
    terms = {normalized}
    backticked = PATH_IN_BACKTICKS_RE.findall(marker)
    terms.update(loose_normalized_text(item) for item in backticked)
    return {term for term in terms if len(term) >= 4}


def marker_is_covered_by_block(block: dict, marker: str, term_builder) -> bool:
    block_text = loose_normalized_text(block.get("text", ""))
    return any(term in block_text for term in term_builder(marker))


def uncovered_placeholder_markers(blocks: list[dict], markers: list[str]) -> list[str]:
    return [
        marker
        for marker in markers
        if not any(marker_is_covered_by_block(block, marker, placeholder_marker_terms) for block in blocks)
    ]


def strip_tags(value: str) -> str:
    return re.sub(r"<[^>]+>", "", value)


def has_accessible_label(attrs: str, body: str = "") -> bool:
    if re.search(
        r"""(?i)\b(?:aria-label|aria-labelledby|title|content|contentdescription|android:text|text|label|accessibilityLabel)\s*=\s*(?:"[^"]+"|'[^']+'|{[^}]+})""",
        attrs,
    ):
        return True
    text_body = strip_tags(body).strip()
    if text_body:
        return True
    return bool(re.search(r"{[^{}]*[A-Za-z_$][^{}]*}", body))


def is_wrapped_in_label(text: str, start: int, end: int) -> bool:
    """Recognize native fields whose accessible name comes from a parent label."""
    open_label = text.rfind("<label", 0, start)
    closed_label = text.rfind("</label>", 0, start)
    next_close = text.find("</label>", end)
    return open_label > closed_label and next_close >= end


def has_associated_html_label(text: str, attrs: str) -> bool:
    """Recognize labels that name a sibling field through a shared JSX id expression."""
    field_id = attr_value(attrs, "id")
    if not field_id:
        return False
    label_values = re.findall(
        r"(?is)\bhtmlFor\s*=\s*(?:\{\s*)?['\"]?([^}\s'\"]+)",
        text,
    )
    return field_id in label_values


def label_wrapper_component_names(text: str) -> set[str]:
    """Find same-file JSX components that render children inside a native label."""
    definitions = list(re.finditer(r"\b(?:function|const)\s+([A-Z][A-Za-z0-9_]*)\b", text))
    names: set[str] = set()
    for index, match in enumerate(definitions):
        end = definitions[index + 1].start() if index + 1 < len(definitions) else len(text)
        body = text[match.start():end]
        if re.search(r"(?is)<label\b[^>]*>.*?\{\s*children\s*\}.*?</label>", body):
            names.add(match.group(1))
    return names


def is_wrapped_in_label_component(
    text: str,
    start: int,
    end: int,
    label_components: set[str],
) -> bool:
    """Recognize native controls passed as children to a same-file label wrapper."""
    if not label_components:
        return False
    openings = list(re.finditer(r"<([A-Z][A-Za-z0-9_]*)\b[^>]*>", text[:start]))
    for opening in reversed(openings):
        name = opening.group(1)
        if name not in label_components:
            continue
        close = text.find(f"</{name}>", opening.end())
        if close >= end:
            return True
    return False


def accessible_button_component_names(text: str) -> set[str]:
    """Find same-file Button components whose implementation supplies a usable name."""
    definitions = list(re.finditer(r"\b(?:function|const)\s+([A-Z][A-Za-z0-9_]*Button)\b", text))
    names: set[str] = set()
    for index, match in enumerate(definitions):
        end = definitions[index + 1].start() if index + 1 < len(definitions) else len(text)
        body = text[match.start():end]
        if not re.search(r"(?is)<(?:button|a|Link)\b", body):
            continue
        if re.search(r"(?is)\baria-label\s*=|\{\s*(?:children|label|[A-Za-z_$][\w$]*\.title)\s*\}|\bt\s*\(", body):
            names.add(match.group(1))
    return names


def attr_value(attrs: str, name: str) -> str | None:
    match = re.search(
        rf"""(?is)\b{re.escape(name)}\s*=\s*(?:"([^"]*)"|'([^']*)'|{{\s*["']?([^}}"']+)["']?\s*}})""",
        attrs,
    )
    if not match:
        return None
    return next((value for value in match.groups() if value is not None), None)


def has_event_handler(attrs: str) -> bool:
    return bool(re.search(r"\bon(?:Click|Change|Input|Submit|KeyDown|KeyUp|KeyPress|CheckedChange|ValueChange)\s*=", attrs))


def has_static_disabled_attr(attrs: str) -> bool:
    return bool(DISABLED_ATTR_RE.search(attrs))


def form_field_kind(match: re.Match[str]) -> tuple[str, str, str]:
    if match.group(1):
        return match.group(1).lower(), match.group(2) or "", match.group(3) or ""
    return (match.group(4) or "input").lower(), match.group(5) or "", ""


def no_op_handler_names(text: str) -> set[str]:
    names: set[str] = set()
    for match in NOOP_FUNCTION_RE.finditer(text):
        name = match.group(1) or match.group(2)
        if name:
            names.add(name)
    return names


def source_only_claim_requires_stronger_evidence(claim_text: str) -> bool:
    """Distinguish a semantic side-effect claim from backticked source names."""
    semantic_text = PATH_IN_BACKTICKS_RE.sub("", claim_text)
    return bool(SOURCE_ONLY_DISALLOWED_CLAIM_RE.search(semantic_text))


def is_static_validation_script(rel_path: str) -> bool:
    """Return true for non-rendered check scripts that may embed source fixtures."""
    normalized = PurePosixPath(rel_path).as_posix()
    return bool(
        re.search(
            r"(?:^|/)scripts/check-[^/]+\.[cm]?[jt]sx?$",
            normalized,
            flags=re.IGNORECASE,
        )
    )


def interface_control_markers_for(rel_path: str, text: str) -> list[str]:
    suffix = PurePosixPath(rel_path).suffix.lower()
    if suffix not in INTERACTIVE_HINT_EXTENSIONS or re.search(
        r"(?i)\.(?:test|spec)\.[cm]?[jt]sx?$",
        rel_path,
    ) or is_static_validation_script(rel_path):
        return []
    markers: set[str] = set()
    noop_names = no_op_handler_names(text)
    label_components = label_wrapper_component_names(text)
    named_button_components = accessible_button_component_names(text)
    for match in EMPTY_HANDLER_RE.finditer(text):
        markers.add(f"empty handler `{match.group(0).strip()[:80]}`")
    for match in EVENT_HANDLER_REF_RE.finditer(text):
        handler_name = match.group(1)
        if handler_name in noop_names:
            markers.add(f"named no-op handler `{handler_name}`")
    for match in DEAD_HREF_RE.finditer(text):
        markers.add(f"dead link `{match.group(0).strip()[:120]}`")
    for match in STATIC_DISABLED_CONTROL_RE.finditer(text):
        markers.add(f"static disabled control `{match.group(0).strip()[:120]}`")
    for match in NATIVE_DISABLED_CONTROL_RE.finditer(text):
        markers.add(f"static disabled control `{match.group(0).strip()[:120]}`")
    for match in FORM_TAG_RE.finditer(text):
        attrs = match.group(1) or ""
        if not FORM_SUBMIT_ATTR_RE.search(attrs):
            markers.add(f"form without submit handler or action `{match.group(0).strip()[:120]}`")
    for match in FORM_FIELD_TAG_RE.finditer(text):
        tag_name, attrs, body = form_field_kind(match)
        field_type = (attr_value(attrs, "type") or "").strip().lower()
        if tag_name == "input" and field_type == "hidden":
            continue
        if has_static_disabled_attr(attrs):
            markers.add(f"static disabled form field `{match.group(0).strip()[:120]}`")
        if (
            not has_accessible_label(attrs, body)
            and not is_wrapped_in_label(text, match.start(), match.end())
            and not has_associated_html_label(text, attrs)
            and not is_wrapped_in_label_component(
                text,
                match.start(),
                match.end(),
                label_components,
            )
        ):
            markers.add(f"unlabeled form field `{match.group(0).strip()[:120]}`")
        if tag_name in {"input", "select", "textarea"} and not has_event_handler(attrs):
            name_or_id = attr_value(attrs, "name") or attr_value(attrs, "id") or attr_value(attrs, "placeholder")
            if not name_or_id and not (
                is_wrapped_in_label(text, match.start(), match.end())
                or has_associated_html_label(text, attrs)
                or is_wrapped_in_label_component(
                    text,
                    match.start(),
                    match.end(),
                    label_components,
                )
            ):
                markers.add(f"untracked form field `{match.group(0).strip()[:120]}`")
    for match in BUTTON_TAG_RE.finditer(text):
        attrs = match.group(1) or ""
        body = match.group(2) or ""
        if not has_accessible_label(attrs, body):
            markers.add(f"unlabeled button `{match.group(0).strip()[:120]}`")
    for match in SELF_CLOSING_BUTTON_RE.finditer(text):
        attrs = match.group(1) or ""
        if not has_accessible_label(attrs):
            markers.add(f"unlabeled button `{match.group(0).strip()[:120]}`")
    for match in COMPONENT_BUTTON_TAG_RE.finditer(text):
        name = match.group(1).split(".")[-1]
        attrs = match.group(2) or ""
        body = match.group(3) or ""
        if name in named_button_components:
            continue
        if not has_accessible_label(attrs, body):
            markers.add(f"unlabeled button component `{match.group(0).strip()[:120]}`")
    for match in SELF_CLOSING_COMPONENT_BUTTON_RE.finditer(text):
        name = match.group(1).split(".")[-1]
        attrs = match.group(2) or ""
        if name in named_button_components:
            continue
        if not has_accessible_label(attrs):
            markers.add(f"unlabeled button component `{match.group(0).strip()[:120]}`")
    for match in ROLE_BUTTON_RE.finditer(text):
        attrs = f"{match.group(2) or ''} {match.group(3) or ''}"
        body = match.group(4) or ""
        if not has_accessible_label(attrs, body):
            markers.add(f"unlabeled role button `{match.group(0).strip()[:120]}`")
        if not re.search(r"\bon(?:Click|KeyDown|KeyUp|KeyPress)\s*=", attrs):
            markers.add(f"role button without click or keyboard handler `{match.group(0).strip()[:120]}`")
    for match in ROLE_INTERACTIVE_RE.finditer(text):
        attrs = f"{match.group(2) or ''} {match.group(3) or ''}"
        body = match.group(4) or ""
        if not has_accessible_label(attrs, body):
            markers.add(f"unlabeled interactive role `{match.group(0).strip()[:120]}`")
        if not has_event_handler(attrs):
            markers.add(f"interactive role without handler `{match.group(0).strip()[:120]}`")
    return sorted(markers)


def interface_control_marker_terms(marker: str) -> set[str]:
    normalized = loose_normalized_text(marker)
    terms = {normalized}
    backticked = PATH_IN_BACKTICKS_RE.findall(marker)
    terms.update(loose_normalized_text(item) for item in backticked)
    return {term for term in terms if len(term) >= 4}


def uncovered_interface_control_markers(blocks: list[dict], markers: list[str]) -> list[str]:
    return [
        marker
        for marker in markers
        if not any(marker_is_covered_by_block(block, marker, interface_control_marker_terms) for block in blocks)
    ]


def file_purpose_tokens(rel_path: str) -> set[str]:
    path = PurePosixPath(rel_path)
    tokens = set(part.lower() for part in re.split(r"[^A-Za-z0-9]+", path.stem) if part)
    tokens.update(part.lower() for part in path.parts[:-1] if part)
    if path.suffix:
        tokens.add(path.suffix.lower().lstrip("."))
    return tokens


def validate_file_coverage_purposes(rows: list[dict]) -> list[dict]:
    issues: list[dict] = []
    purpose_counts = Counter(normalized_text(row["purpose"]) for row in rows)
    for row in rows:
        purpose = row["purpose"]
        normalized = normalized_text(purpose)
        tokens = file_purpose_tokens(row["file"])
        purpose_words = set(re.split(r"[^a-z0-9]+", normalized))
        if DIRECTORY_ONLY_PURPOSE_RE.search(purpose):
            issues.append(
                {
                    "section": "file coverage",
                    "file": row["file"],
                    "reason": "file coverage purpose must describe the file role, not only a directory or group",
                    "actual": purpose,
                }
            )
        elif normalized in GENERIC_PURPOSE_VALUES:
            issues.append(
                {
                    "section": "file coverage",
                    "file": row["file"],
                    "reason": "file coverage purpose is too generic",
                    "actual": purpose,
                }
            )
        elif purpose_counts[normalized] >= 3 and not (tokens & purpose_words):
            issues.append(
                {
                    "section": "file coverage",
                    "file": row["file"],
                    "reason": "repeated file coverage purpose must include file-specific role detail",
                    "actual": purpose,
                }
            )
    return issues


def png_dimensions(data: bytes) -> tuple[int, int] | None:
    if len(data) < 24 or not data.startswith(b"\x89PNG\r\n\x1a\n") or data[12:16] != b"IHDR":
        return None
    width, height = struct.unpack(">II", data[16:24])
    return width, height


def gif_dimensions(data: bytes) -> tuple[int, int] | None:
    if len(data) < 10 or data[:6] not in {b"GIF87a", b"GIF89a"}:
        return None
    width, height = struct.unpack("<HH", data[6:10])
    return width, height


def jpeg_dimensions(data: bytes) -> tuple[int, int] | None:
    if len(data) < 4 or not data.startswith(b"\xff\xd8"):
        return None
    index = 2
    while index + 9 < len(data):
        if data[index] != 0xFF:
            index += 1
            continue
        marker = data[index + 1]
        index += 2
        if marker in {0xD8, 0xD9}:
            continue
        if index + 2 > len(data):
            return None
        segment_length = int.from_bytes(data[index:index + 2], "big")
        if segment_length < 2 or index + segment_length > len(data):
            return None
        if marker in {0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF}:
            if segment_length < 7:
                return None
            height = int.from_bytes(data[index + 3:index + 5], "big")
            width = int.from_bytes(data[index + 5:index + 7], "big")
            return width, height
        index += segment_length
    return None


def webp_dimensions(data: bytes) -> tuple[int, int] | None:
    if len(data) < 30 or data[:4] != b"RIFF" or data[8:12] != b"WEBP":
        return None
    chunk = data[12:16]
    if chunk == b"VP8X" and len(data) >= 30:
        width = 1 + int.from_bytes(data[24:27], "little")
        height = 1 + int.from_bytes(data[27:30], "little")
        return width, height
    if chunk == b"VP8 " and len(data) >= 30 and data[23:26] == b"\x9d\x01\x2a":
        width = int.from_bytes(data[26:28], "little") & 0x3FFF
        height = int.from_bytes(data[28:30], "little") & 0x3FFF
        return width, height
    if chunk == b"VP8L" and len(data) >= 25 and data[20] == 0x2F:
        bits = int.from_bytes(data[21:25], "little")
        width = (bits & 0x3FFF) + 1
        height = ((bits >> 14) & 0x3FFF) + 1
        return width, height
    return None


def read_asset_prefix(path: Path, limit: int = ASSET_METADATA_READ_LIMIT) -> bytes:
    with path.open("rb") as handle:
        return handle.read(limit)


def svg_dimension_number(raw_value: str) -> int | float | None:
    match = SVG_NUMBER_RE.match(raw_value)
    if not match:
        return None
    try:
        value = float(match.group(1))
    except ValueError:
        return None
    if value <= 0:
        return None
    return int(value) if value.is_integer() else value


def parse_svg_metadata(data: bytes) -> tuple[int | float | None, int | float | None] | None:
    text = data.decode("utf-8", errors="ignore")
    match = SVG_ROOT_RE.search(text)
    if not match:
        return None
    attrs = {
        name.lower(): value.strip()
        for name, _quote, value in SVG_ATTR_RE.findall(match.group("attrs"))
    }
    width = svg_dimension_number(attrs.get("width", ""))
    height = svg_dimension_number(attrs.get("height", ""))
    if width is not None and height is not None:
        return width, height
    viewbox = attrs.get("viewbox")
    if viewbox:
        parts = re.split(r"[\s,]+", viewbox.strip())
        if len(parts) == 4:
            viewbox_width = svg_dimension_number(parts[2])
            viewbox_height = svg_dimension_number(parts[3])
            if viewbox_width is not None and viewbox_height is not None:
                return viewbox_width, viewbox_height
    return None, None


def asset_metadata_for(path: Path) -> dict:
    suffix = path.suffix.lower()
    metadata = {
        "mime": ASSET_MIME_BY_SUFFIX.get(suffix),
        "width": None,
        "height": None,
        "valid": False,
        "reason": None,
    }
    if suffix not in ASSET_MIME_BY_SUFFIX:
        metadata["reason"] = f"unsupported asset extension {suffix}"
        return metadata
    try:
        if suffix in {".woff", ".woff2", ".ttf", ".eot", ".ico"}:
            metadata["valid"] = path.stat().st_size > 0
            return metadata
        data = read_asset_prefix(path)
    except OSError as exc:
        metadata["reason"] = str(exc)
        return metadata
    if suffix == ".svg":
        dimensions = parse_svg_metadata(data)
        if dimensions is None:
            metadata["reason"] = "could not parse SVG root metadata"
            return metadata
        metadata["width"], metadata["height"] = dimensions
        metadata["valid"] = True
        return metadata
    dimensions = None
    if suffix == ".png":
        dimensions = png_dimensions(data)
    elif suffix in {".jpg", ".jpeg"}:
        dimensions = jpeg_dimensions(data)
    elif suffix == ".gif":
        dimensions = gif_dimensions(data)
    elif suffix == ".webp":
        dimensions = webp_dimensions(data)
    if not dimensions:
        metadata["reason"] = "could not parse asset dimensions"
        return metadata
    metadata["width"], metadata["height"] = dimensions
    metadata["valid"] = True
    return metadata


def asset_evidence_matches_metadata(text: str, metadata: dict) -> bool:
    normalized = normalized_text(text)
    mime = metadata.get("mime")
    width = metadata.get("width")
    height = metadata.get("height")
    if mime and mime.lower() not in normalized:
        return False
    if width is not None and height is not None:
        width_text = str(int(width)) if isinstance(width, float) and width.is_integer() else str(width)
        height_text = str(int(height)) if isinstance(height, float) and height.is_integer() else str(height)
        compact = f"{width_text}x{height_text}"
        spaced = f"{width_text} x {height_text}"
        named = f"width {width_text} height {height_text}"
        if compact not in normalized and spaced not in normalized and named not in normalized:
            return False
    return True


def parse_interface_inventory_rows(text: str, report_path: Path) -> tuple[list[dict], list[str]]:
    rows: list[dict] = []
    malformed_rows: list[str] = []
    in_inventory = False
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.lower().startswith("## interface inventory"):
            in_inventory = True
            continue
        if in_inventory and stripped.startswith("## ") and not stripped.lower().startswith("## interface inventory"):
            break
        if not in_inventory or not stripped.startswith("|"):
            continue
        columns = split_markdown_row(stripped)
        if [column.lower() for column in columns[:5]] == [
            "file",
            "surface",
            "visible text/control/message",
            "expected behavior path",
            "actual implementation notes",
        ]:
            continue
        if is_separator_row(columns):
            continue
        if len(columns) != 5 or any(not column for column in columns):
            malformed_rows.append(stripped)
            continue
        rows.append(
            {
                "file": columns[0].strip("`").strip(),
                "surface": columns[1],
                "visible_text": columns[2],
                "expected_behavior_path": columns[3],
                "actual_implementation_notes": columns[4],
                "report": str(report_path),
            }
        )
    return rows, malformed_rows


def parse_implementation_inventory_rows(text: str, report_path: Path) -> tuple[list[dict], list[str]]:
    rows: list[dict] = []
    malformed_rows: list[str] = []
    in_inventory = False
    table_lines: list[tuple[str, list[str]]] = []
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.lower().startswith("## implementation inventory"):
            in_inventory = True
            continue
        if in_inventory and stripped.startswith("## ") and not stripped.lower().startswith(
            "## implementation inventory"
        ):
            break
        if not in_inventory or not stripped.startswith("|"):
            continue
        table_lines.append((stripped, split_markdown_row(stripped)))

    if not table_lines or tuple(column.lower() for column in table_lines[0][1]) != IMPLEMENTATION_INVENTORY_HEADERS:
        malformed_rows.append(
            "missing or invalid exact 8-column implementation inventory header; "
            f"expected columns {IMPLEMENTATION_INVENTORY_HEADERS!r}"
        )
    if (
        len(table_lines) < 2
        or len(table_lines[1][1]) != len(IMPLEMENTATION_INVENTORY_HEADERS)
        or not is_separator_row(table_lines[1][1])
    ):
        malformed_rows.append(
            "missing or invalid exact 8-column implementation inventory separator immediately after the header"
        )

    for stripped, columns in table_lines[2:]:
        if tuple(column.lower() for column in columns) == IMPLEMENTATION_INVENTORY_HEADERS or is_separator_row(columns):
            malformed_rows.append(f"unexpected repeated implementation inventory header/separator: {stripped}")
            continue
        if len(columns) != len(IMPLEMENTATION_INVENTORY_HEADERS) or any(not column for column in columns):
            malformed_rows.append(stripped)
            continue
        rows.append(
            {
                "file": columns[0].strip("`").strip(),
                "contract_id": columns[1].strip("`").strip(),
                "contract": columns[2],
                "anchors": columns[3],
                "implementation_trace": columns[4],
                "failure_trace": columns[5],
                "verification_evidence": columns[6],
                "result": columns[7].strip().upper(),
                "report": str(report_path),
            }
        )
    return rows, malformed_rows


def implementation_definition_patterns(rel_path: str) -> list[str]:
    suffix = PurePosixPath(rel_path).suffix.casefold()
    patterns: list[str] = []
    if suffix == ".py":
        patterns = [
            r"(?m)^\s*(?:async\s+)?def\s+([A-Za-z_]\w*)\s*\(",
            r"(?m)^\s*class\s+([A-Za-z_]\w*)\b",
        ]
    elif suffix in {".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".vue", ".svelte"}:
        patterns = [
            r"(?m)^\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s+([A-Za-z_$][\w$]*)\s*\(",
            r"(?m)^\s*(?:export\s+)?(?:default\s+)?class\s+([A-Za-z_$][\w$]*)\b",
            r"(?m)^\s*(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(?:async\s+)?(?:\([^\n]*\)|[A-Za-z_$][\w$]*)\s*=>",
            r"(?m)^\s*(?:(?:public|private|protected|static|async|abstract|override|readonly|declare|get|set)\s+)*([A-Za-z_$][\w$]*)\s*(?:<[^\n>{}]+>)?\s*\([^;\n{}]*\)\s*(?::\s*[^={\n]+)?\s*\{",
        ]
    elif suffix == ".go":
        patterns = [r"(?m)^\s*func\s+(?:\([^\n)]*\)\s*)?([A-Za-z_]\w*)\s*\("]
    elif suffix == ".rs":
        patterns = [
            r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_]\w*)\s*\(",
            r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:struct|enum|trait)\s+([A-Za-z_]\w*)\b",
        ]
    elif suffix == ".swift":
        patterns = [
            r"(?m)^\s*(?:public\s+|private\s+|internal\s+|fileprivate\s+|open\s+)*(?:static\s+|class\s+)?func\s+([A-Za-z_]\w*)\s*\(",
            r"(?m)^\s*(?:public\s+|private\s+|internal\s+|fileprivate\s+|open\s+)*(?:class|struct|enum|protocol)\s+([A-Za-z_]\w*)\b",
        ]
    elif suffix in {".rb", ".rake"}:
        patterns = [
            r"(?m)^\s*def\s+(?:self\.)?([A-Za-z_]\w*[!?=]?)",
            r"(?m)^\s*(?:class|module)\s+([A-Za-z_]\w*(?:::[A-Za-z_]\w*)*)",
        ]
    elif suffix in {".sh", ".bash", ".zsh"}:
        patterns = [r"(?m)^\s*(?:function\s+)?([A-Za-z_]\w*)\s*(?:\(\s*\))?\s*\{"]
    elif suffix == ".java":
        patterns = [
            r"(?m)^\s*(?:public\s+|protected\s+|private\s+|static\s+|final\s+|abstract\s+)*(?:class|interface|enum|record)\s+([A-Za-z_]\w*)\b",
            r"(?m)^\s*(?:public\s+|protected\s+|private\s+|static\s+|final\s+|abstract\s+|synchronized\s+|native\s+)+[A-Za-z_][\w<>,.?\[\]\s]*\s+([A-Za-z_]\w*)\s*\([^;{}]*\)\s*(?:throws\s+[^{}]+)?\{",
        ]
    elif suffix in {".kt", ".kts"}:
        patterns = [
            r"(?m)^\s*(?:(?:public|private|protected|internal|open|abstract|sealed|data|value|enum|annotation)\s+)*(?:class|interface|object)\s+([A-Za-z_]\w*)\b",
            r"(?m)^\s*(?:(?:public|private|protected|internal|open|override|suspend|inline|operator|tailrec|infix|external)\s+)*fun\s+(?:[\w<>?.]+\.)?([A-Za-z_]\w*)\s*\(",
        ]
    elif suffix in {".c", ".h", ".cc", ".cpp", ".cxx", ".hpp", ".hh"}:
        patterns = [
            r"(?m)^\s*(?:class|struct|enum)\s+([A-Za-z_]\w*)\b",
            r"(?m)^\s*(?:[A-Za-z_]\w*[\s*&:<>,\[\]]+)+([A-Za-z_]\w*)\s*\([^;{}]*\)\s*(?:const\s*)?\{",
        ]
    elif suffix == ".cs":
        patterns = [
            r"(?m)^\s*(?:(?:public|private|protected|internal|static|abstract|sealed|partial)\s+)*(?:class|interface|struct|record|enum)\s+([A-Za-z_]\w*)\b",
            r"(?m)^\s*(?:(?:public|private|protected|internal|static|virtual|override|abstract|async|sealed|partial|extern)\s+)+[A-Za-z_][\w<>,.?\[\]\s]*\s+([A-Za-z_]\w*)\s*\([^;{}]*\)\s*\{",
        ]
    elif suffix in {".php", ".phtml"}:
        patterns = [
            r"(?mi)^\s*(?:(?:final|abstract|readonly)\s+)*(?:class|interface|trait|enum)\s+([A-Za-z_]\w*)\b",
            r"(?mi)^\s*(?:(?:public|private|protected|static|final|abstract)\s+)*function\s+&?\s*([A-Za-z_]\w*)\s*\(",
        ]
    elif suffix == ".dart":
        patterns = [
            r"(?m)^\s*(?:(?:abstract|base|final|interface|sealed)\s+)*(?:class|mixin|enum|extension)\s+([A-Za-z_]\w*)\b",
            r"(?m)^\s*(?:[A-Za-z_][\w<>?,\[\]]*\s+)+([A-Za-z_]\w*)\s*\([^;{}]*\)\s*(?:async\*?|sync\*)?\s*\{",
        ]
    elif suffix == ".lua":
        patterns = [r"(?m)^\s*(?:local\s+)?function\s+([A-Za-z_]\w*(?:[.:][A-Za-z_]\w*)*)\s*\("]
    elif suffix in {".ex", ".exs"}:
        patterns = [r"(?m)^\s*defp?\s+([a-z_]\w*[!?]?)\s*(?:\(|do\b)"]
    elif suffix == ".sql":
        patterns = [
            r"(?im)^\s*CREATE\s+(?:OR\s+(?:REPLACE|ALTER)\s+)?(?:FUNCTION|PROCEDURE|TRIGGER|(?:MATERIALIZED\s+)?VIEW)\s+(?:IF\s+NOT\s+EXISTS\s+)?((?:\"?[A-Za-z_]\w*\"?\.)?\"?[A-Za-z_]\w*\"?)"
        ]
    return patterns


def implementation_responsibility_occurrences(
    rel_path: str,
    source_text: str,
    *,
    start_line: int | None = 1,
    start_byte: int | None = None,
) -> list[dict[str, object]]:
    """Return every recognized definition with an occurrence-aware source anchor.

    Line/column anchors are absolute for whole-file and line-range units. Byte
    units use an absolute byte offset because a decoded byte slice need not
    begin at a source line boundary. The manifest hash makes either coordinate
    stable for the exact audited source snapshot.
    """

    control_keywords = {
        "catch",
        "do",
        "else",
        "finally",
        "for",
        "if",
        "return",
        "switch",
        "try",
        "while",
        "with",
    }
    occurrences: list[dict[str, object]] = []
    seen: set[tuple[str, int]] = set()
    for pattern in implementation_definition_patterns(rel_path):
        for match in re.finditer(pattern, source_text):
            value = match.group(1)
            key = (value, match.start(1))
            if value.casefold() in control_keywords or key in seen:
                continue
            seen.add(key)
            if isinstance(start_byte, int):
                byte_offset = start_byte + len(source_text[: match.start(1)].encode("utf-8"))
                anchor = f"{value}@B{byte_offset}"
                coordinate = {"byte": byte_offset}
            else:
                relative_line = source_text.count("\n", 0, match.start(1)) + 1
                line_number = (start_line if isinstance(start_line, int) else 1) + relative_line - 1
                line_start = source_text.rfind("\n", 0, match.start(1)) + 1
                column_number = match.start(1) - line_start + 1
                anchor = f"{value}@L{line_number}:C{column_number}"
                coordinate = {"line": line_number, "column": column_number}
            occurrences.append(
                {
                    "name": value,
                    "anchor": anchor,
                    "start": match.start(1),
                    **coordinate,
                }
            )
    occurrences.sort(key=lambda item: (int(item["start"]), str(item["name"]), str(item["anchor"])))
    return occurrences


def implementation_responsibility_hints(rel_path: str, source_text: str) -> list[str]:
    """Return unique high-confidence names for diagnostics and compatibility.

    Exact inventory coverage uses :func:`implementation_responsibility_occurrences`
    so repeated method names cannot collapse into one row.
    """

    hints: list[str] = []
    for occurrence in implementation_responsibility_occurrences(rel_path, source_text):
        value = str(occurrence["name"])
        if value not in hints:
            hints.append(value)
    return hints


def is_manifest_test_path(rel_path: str) -> bool:
    path = PurePosixPath(rel_path)
    parts = {part.casefold() for part in path.parts[:-1]}
    name = path.name.casefold()
    stem = path.stem.casefold()
    return bool(
        parts & {"test", "tests", "__tests__", "spec", "specs"}
        or name in {"self_test.py", "self-test.py"}
        or stem.startswith(("test_", "spec_"))
        or stem.endswith(("_test", "_tests", "_spec", ".test", ".spec"))
        or re.search(r"(?:^|[._-])(?:test|tests|spec)(?:[._-]|$)", name)
    )


def manifest_source_path_for_reference(
    reference: str,
    known_source_files: set[str],
) -> str | None:
    """Resolve an exact manifest path or a path with a precise source suffix."""

    if reference in known_source_files:
        return reference
    base, separator, suffix = reference.partition("#")
    if not separator or base not in known_source_files or not suffix:
        return None
    if re.fullmatch(r"L\d+(?:-L?\d+)?", suffix, re.IGNORECASE):
        return base
    if re.fullmatch(r"B\d+(?:-B?\d+)?", suffix, re.IGNORECASE):
        return base
    if re.fullmatch(r"[A-Za-z_$][A-Za-z0-9_$.:@-]*(?:\(\))?", suffix):
        return base
    return None


def validate_typed_implementation_evidence(
    *,
    verification: str,
    evidence_type: str | None,
    result: str,
    known_source_files: set[str],
    valid_bound_evidence_ids: set[str],
    context: str,
) -> list[dict]:
    """Bind test/runtime labels to real files or attested audit artifacts."""

    if evidence_type not in {"test", "runtime"}:
        return []
    issues: list[dict] = []
    reference_matches = list(IMPLEMENTATION_EVIDENCE_REF_RE.finditer(verification))
    evidence_refs: list[str] = []
    if len(reference_matches) == 1:
        reference_detail = reference_matches[0].group(1)
        evidence_refs = [plain_cell(value) for value in PATH_IN_BACKTICKS_RE.findall(reference_detail)]
        unexplained = re.sub(r"`[^`]+`", "", reference_detail).strip(" ,\t")
        if not evidence_refs or unexplained:
            evidence_refs = []
    if len(reference_matches) != 1 or not evidence_refs:
        issues.append(
            {
                "reason": f"{context} evidence-type {evidence_type} requires exactly one evidence-ref declaration containing only concrete backticked references",
                "verification": verification,
            }
        )
    elif evidence_type == "test":
        valid_test_paths = {path for path in known_source_files if is_manifest_test_path(path)}
        invalid_refs = sorted(set(evidence_refs) - valid_test_paths)
        if invalid_refs:
            issues.append(
                {
                    "reason": f"{context} test evidence-ref values must resolve to manifest-owned test source files",
                    "invalid_evidence_refs": invalid_refs,
                    "available_test_files": sorted(valid_test_paths),
                }
            )
    else:
        runtime_ids = []
        invalid_syntax = []
        for reference in evidence_refs:
            match = re.fullmatch(r"evidence:([A-Za-z][A-Za-z0-9_-]{0,63})", reference)
            if match:
                runtime_ids.append(match.group(1))
            else:
                invalid_syntax.append(reference)
        unresolved = sorted(set(runtime_ids) - valid_bound_evidence_ids)
        if invalid_syntax or unresolved:
            issues.append(
                {
                    "reason": f"{context} runtime evidence-ref values must resolve to valid bound audit evidence:<id> artifacts",
                    "invalid_evidence_refs": sorted(invalid_syntax),
                    "unresolved_evidence_ids": unresolved,
                }
            )
    if result == "PASS":
        outcome_matches = list(IMPLEMENTATION_EVIDENCE_OUTCOME_RE.finditer(verification))
        outcome = plain_cell(outcome_matches[0].group(1)) if len(outcome_matches) == 1 else ""
        if (
            len(outcome_matches) != 1
            or len(outcome) < 12
            or is_boilerplate_value(outcome)
            or not IMPLEMENTATION_OUTCOME_BEHAVIOR_RE.search(outcome)
        ):
            issues.append(
                {
                    "reason": f"{context} PASS with test or runtime evidence requires exactly one explicit outcome: or result: statement naming the observed result",
                    "verification": verification,
                }
            )
    return issues


def finding_cites_implementation_contract(
    block: dict,
    *,
    contract_id: str,
    unit_id: str,
    rel_path: str,
) -> bool:
    refs = set(PATH_IN_BACKTICKS_RE.findall(block.get("text", "")))
    return contract_id in refs and (unit_id == rel_path or unit_id in refs)


def parse_finding_blocks(findings_body: str) -> list[dict]:
    lines = findings_body.splitlines()
    heading_indexes = [index for index, line in enumerate(lines) if FINDING_HEADING_RE.match(line.strip())]
    blocks = []
    for position, start in enumerate(heading_indexes):
        end = heading_indexes[position + 1] if position + 1 < len(heading_indexes) else len(lines)
        block_lines = lines[start + 1 : end]
        fields = {}
        for line in block_lines:
            match = re.match(r"^-\s*([^:]+):\s*(.*)$", line.strip())
            if match:
                fields[match.group(1).strip().lower()] = match.group(2).strip()
        blocks.append(
            {
                "heading": lines[start].strip(),
                "fields": fields,
                "text": "\n".join([lines[start], *block_lines]).strip(),
            }
        )
    return blocks


def has_visual_danger_finding(findings_body: str) -> bool:
    normalized = re.sub(r"[\s.]+", " ", findings_body.strip().lower()).strip()
    if not normalized or normalized in NO_FINDINGS_SENTINELS:
        return False
    for block in parse_finding_blocks(findings_body):
        text = normalized_text(block.get("text", ""))
        if VISUAL_DANGER_RE.search(text) or any(
            token in text
            for token in ("journey usability", "decision path", "rendered journey", "readability", "contrast", "scannable")
        ):
            return True
    return False


def validate_findings_schema(findings_body: str) -> list[dict]:
    stripped = findings_body.strip()
    if not stripped:
        return []
    normalized = re.sub(r"[\s.]+", " ", stripped.lower()).strip()
    if normalized in NO_FINDINGS_SENTINELS:
        return []

    lines = findings_body.splitlines()
    heading_indexes = [index for index, line in enumerate(lines) if FINDING_HEADING_RE.match(line.strip())]
    malformed_headings = [
        line.strip()
        for line in lines
        if line.strip().startswith("###") and not FINDING_HEADING_RE.match(line.strip())
    ]
    issues: list[dict] = []
    if malformed_headings:
        issues.append(
            {
                "section": "findings",
                "reason": "finding headings must match '### P0/P1/P2/P3 - Short title'",
                "headings": malformed_headings,
            }
        )
    if not heading_indexes:
        issues.append(
            {
                "section": "findings",
                "reason": "findings must use severity subsections or an explicit no-findings sentinel",
            }
        )
        return issues

    for block in parse_finding_blocks(findings_body):
        missing = sorted(REQUIRED_FINDING_FIELDS - set(block["fields"]))
        if missing:
            issues.append(
                {
                    "section": "findings",
                    "heading": block["heading"],
                    "reason": "finding is missing required fields",
                    "missing": missing,
                }
            )
        gap = block["fields"].get("gap", "")
        if (
            AUDIT_EXECUTION_ONLY_GAP_RE.search(gap)
            and not AUDIT_EXECUTION_BLOCKER_HEADING_RE.search(block["heading"])
        ):
            issues.append(
                {
                    "section": "findings",
                    "heading": block["heading"],
                    "reason": "audit-execution-only gap must use an 'Audit execution blocked' heading instead of a product-defect title",
                }
            )
    return issues


def validate_journey_findings_semantics(
    findings_body: str,
    known_source_files: set[str],
    interface_files: set[str],
) -> list[dict]:
    stripped = findings_body.strip()
    if not stripped:
        return []
    normalized = re.sub(r"[\s.]+", " ", stripped.lower()).strip()
    if normalized in NO_FINDINGS_SENTINELS:
        return []

    issues: list[dict] = []
    for block in parse_finding_blocks(findings_body):
        fields = block["fields"]
        file_refs = PATH_IN_BACKTICKS_RE.findall(fields.get("files", ""))
        if not file_refs:
            issues.append(
                {
                    "section": "findings",
                    "heading": block["heading"],
                    "reason": "Files field must contain one or more backticked manifest source files",
                }
            )
            continue
        unknown_refs = sorted(set(file_refs) - known_source_files)
        if unknown_refs:
            issues.append(
                {
                    "section": "findings",
                    "heading": block["heading"],
                    "reason": "Files field references files outside the manifest source inventory",
                    "files": unknown_refs,
                }
            )
        evidence = fields.get("evidence", "")
        if is_boilerplate_value(evidence) or len(plain_cell(evidence)) < 12:
            issues.append(
                {
                    "section": "findings",
                    "heading": block["heading"],
                    "reason": "Evidence field must contain concrete non-boilerplate journey/source detail",
                    "actual": evidence,
                }
            )
        for field_name in REQUIRED_FINDING_FIELDS - {"files", "evidence", "interface evidence"}:
            value = fields.get(field_name, "")
            if is_boilerplate_value(value) or len(plain_cell(value)) < 12:
                issues.append(
                    {
                        "section": "findings",
                        "heading": block["heading"],
                        "field": field_name,
                        "reason": "required journey finding field must contain meaningful non-boilerplate content",
                        "actual": value,
                    }
                )
        if set(file_refs) & interface_files:
            interface_evidence = fields.get("interface evidence", "")
            if is_boilerplate_value(interface_evidence) or len(plain_cell(interface_evidence)) < 4:
                issues.append(
                    {
                        "section": "findings",
                        "heading": block["heading"],
                        "reason": "Interface journey findings must include concrete visible label/control/message evidence",
                        "actual": interface_evidence,
                    }
                )
    return issues


def validate_lead_reconciliation_report(
    report_path: Path,
    *,
    expected_run_id: str,
    known_source_files: set[str],
    source_file_count: int,
    source_text_for_file,
    batch_contracts: dict[str, dict],
    source_hashes: dict[str, str],
    valid_bound_evidence_ids: set[str],
) -> tuple[list[dict], list[dict]]:
    issues: list[dict] = []
    rows: list[dict] = []
    try:
        text = report_path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as exc:
        return ([{"reason": f"lead reconciliation report could not be read as UTF-8: {exc}"}], rows)

    bodies = section_bodies(text)
    ordered_sections = [
        match.group(1).strip().lower()
        for line in text.splitlines()
        if (match := SECTION_RE.match(line.strip()))
    ]
    if ordered_sections != list(LEAD_RECONCILIATION_SECTIONS):
        issues.append(
            {
                "reason": "lead reconciliation report sections must exactly match the required order",
                "expected": list(LEAD_RECONCILIATION_SECTIONS),
                "actual": ordered_sections,
            }
        )
    run_ids = declared_run_ids(text)
    if run_ids != [expected_run_id]:
        issues.append(
            {
                "reason": "lead reconciliation report must declare the exact audit Run ID once",
                "expected": expected_run_id,
                "actual": run_ids,
            }
        )
    if bodies.get("worker", "").strip() != "lead_reconciliation":
        issues.append(
            {
                "reason": "lead reconciliation Worker must be exactly lead_reconciliation",
                "actual": bodies.get("worker", "").strip(),
            }
        )

    trace_body = bodies.get("cross-file contract trace", "").strip()
    if source_file_count == 0:
        if trace_body != LEAD_RECONCILIATION_EMPTY_SENTINEL:
            issues.append(
                {
                    "reason": "empty manifests require the exact lead reconciliation sentinel",
                    "expected": LEAD_RECONCILIATION_EMPTY_SENTINEL,
                    "actual": trace_body,
                }
            )
    else:
        table_lines = [
            (line.strip(), split_markdown_row(line.strip()))
            for line in trace_body.splitlines()
            if line.strip().startswith("|")
        ]
        if not table_lines or tuple(column.lower() for column in table_lines[0][1]) != LEAD_RECONCILIATION_HEADERS:
            issues.append(
                {
                    "reason": "lead reconciliation requires the exact 13-column Cross-File Contract Trace header",
                    "expected": LEAD_RECONCILIATION_HEADERS,
                }
            )
        if (
            len(table_lines) < 2
            or len(table_lines[1][1]) != len(LEAD_RECONCILIATION_HEADERS)
            or not is_separator_row(table_lines[1][1])
        ):
            issues.append(
                {
                    "reason": "lead reconciliation requires the exact 13-column table separator immediately after the header"
                }
            )
        for raw, columns in table_lines[2:]:
            if tuple(column.lower() for column in columns) == LEAD_RECONCILIATION_HEADERS or is_separator_row(columns):
                issues.append({"reason": "lead reconciliation contains a repeated header/separator", "row": raw})
                continue
            if len(columns) != len(LEAD_RECONCILIATION_HEADERS) or any(not column for column in columns):
                issues.append({"reason": "malformed lead reconciliation contract row", "row": raw})
                continue
            rows.append(
                {
                    "contract_id": columns[0].strip("`").strip(),
                    "batch_contract_ids_cell": columns[1],
                    "anchors": columns[2],
                    **{
                        field: columns[index]
                        for index, field in enumerate(LEAD_RECONCILIATION_TRACE_FIELDS, start=3)
                    },
                    "result": columns[12].strip().upper(),
                }
            )
        if not rows:
            issues.append(
                {"reason": "non-empty manifests require at least one lead cross-file/public/operational contract row"}
            )

    contract_id_counts = Counter(row["contract_id"] for row in rows)
    mapped_batch_counts: Counter[str] = Counter()
    for row in rows:
        contract_id = row["contract_id"]
        if not LEAD_IMPLEMENTATION_CONTRACT_ID_RE.fullmatch(contract_id):
            issues.append(
                {
                    "reason": "lead reconciliation Contract ID must use the lead:C### namespace with at least three digits",
                    "contract_id": contract_id,
                }
            )
        if contract_id_counts[contract_id] > 1:
            issues.append(
                {
                    "reason": "lead reconciliation Contract IDs must be unique",
                    "contract_id": contract_id,
                }
            )
        result = row["result"]
        if result not in IMPLEMENTATION_RESULTS:
            issues.append(
                {
                    "reason": "lead reconciliation Result must be PASS, GAP, or BLOCKED",
                    "contract_id": contract_id,
                    "actual": result,
                }
            )
        raw_batch_refs = PATH_IN_BACKTICKS_RE.findall(row["batch_contract_ids_cell"])
        batch_refs = [
            reference
            for reference in raw_batch_refs
            if BATCH_IMPLEMENTATION_CONTRACT_ID_RE.fullmatch(reference)
        ]
        if not batch_refs or len(batch_refs) != len(raw_batch_refs) or len(batch_refs) != len(set(batch_refs)):
            issues.append(
                {
                    "reason": "lead reconciliation Batch Contract IDs must contain unique exact backticked batch_<3+ digits>:C<3+ digits> IDs",
                    "contract_id": contract_id,
                    "actual": row["batch_contract_ids_cell"],
                }
            )
        row["batch_contract_ids"] = batch_refs
        mapped_batch_counts.update(batch_refs)
        unknown_batch_refs = sorted(set(batch_refs) - set(batch_contracts))
        if unknown_batch_refs:
            issues.append(
                {
                    "reason": "lead reconciliation maps unknown batch Contract IDs",
                    "contract_id": contract_id,
                    "batch_contract_ids": unknown_batch_refs,
                }
            )
        anchor_refs = set(PATH_IN_BACKTICKS_RE.findall(row["anchors"]))
        source_refs = anchor_refs & known_source_files
        symbol_refs = {
            reference
            for reference in anchor_refs - known_source_files - {contract_id}
            if len(reference.strip()) >= 2 and not is_boilerplate_value(reference)
        }
        mapped_anchor_tokens = {
            token
            for batch_id in batch_refs
            if batch_id in batch_contracts
            for token in batch_contracts[batch_id].get("anchor_tokens", [])
        }
        if not source_refs or not symbol_refs:
            issues.append(
                {
                    "reason": "lead reconciliation anchors must cite a repo source file and a concrete backticked symbol/token",
                    "contract_id": contract_id,
                    "anchors": row["anchors"],
                }
            )
        elif not any(
            symbol in source_text_for_file(source_file)
            or symbol.casefold() == str(source_hashes.get(source_file, "")).casefold()
            or symbol in mapped_anchor_tokens
            for source_file in source_refs
            for symbol in symbol_refs
        ):
            issues.append(
                {
                    "reason": "lead reconciliation anchor token must occur in at least one cited manifest source file",
                    "contract_id": contract_id,
                    "source_files": sorted(source_refs),
                    "tokens": sorted(symbol_refs),
                }
            )
        trace_statuses: list[str] = []
        trace_status_by_field: dict[str, str] = {}
        for field in LEAD_RECONCILIATION_TRACE_FIELDS:
            value = row[field]
            status_match = LEAD_RECONCILIATION_TRACE_STATUS_RE.match(value)
            if not status_match:
                issues.append(
                    {
                        "reason": "lead reconciliation trace cells must begin with pass, gap, blocked, or not applicable plus evidence",
                        "contract_id": contract_id,
                        "field": field,
                        "actual": value,
                    }
                )
                continue
            trace_status = status_match.group(1).casefold()
            trace_statuses.append(trace_status)
            trace_status_by_field[field] = trace_status
            evidence = status_match.group(2)
            if is_boilerplate_value(evidence) or len(plain_cell(evidence)) < 12:
                issues.append(
                    {
                        "reason": "lead reconciliation trace cells require concrete evidence or justified non-applicability",
                        "contract_id": contract_id,
                        "field": field,
                        "actual": evidence,
                    }
                )
        derived_result = (
            "GAP"
            if "gap" in trace_statuses
            else "BLOCKED"
            if "blocked" in trace_statuses
            else "PASS"
        )
        if result in IMPLEMENTATION_RESULTS and trace_statuses and result != derived_result:
            issues.append(
                {
                    "reason": "lead reconciliation Result contradicts its trace statuses",
                    "contract_id": contract_id,
                    "expected": derived_result,
                    "actual": result,
                }
            )
        if result == "PASS":
            non_pass_clean_fields = {
                field: trace_status_by_field.get(field)
                for field in ("observable-outcome", "verification")
                if trace_status_by_field.get(field) != "pass"
            }
            if non_pass_clean_fields:
                issues.append(
                    {
                        "reason": "lead reconciliation PASS requires pass observable-outcome and verification trace statuses",
                        "contract_id": contract_id,
                        "actual": non_pass_clean_fields,
                    }
                )
        mapped_results = {
            batch_contracts[batch_id]["result"]
            for batch_id in batch_refs
            if batch_id in batch_contracts
        }
        if "GAP" in mapped_results and result != "GAP":
            issues.append(
                {
                    "reason": "lead reconciliation cannot hide a mapped batch GAP",
                    "contract_id": contract_id,
                    "actual": result,
                }
            )
        elif "BLOCKED" in mapped_results and result == "PASS":
            issues.append(
                {
                    "reason": "lead reconciliation cannot hide a mapped batch BLOCKED result",
                    "contract_id": contract_id,
                    "actual": result,
                }
            )
        required_source_refs = {
            batch_contracts[batch_id]["source_file"]
            for batch_id in batch_refs
            if batch_id in batch_contracts
        }
        missing_source_refs = sorted(required_source_refs - source_refs)
        if missing_source_refs:
            issues.append(
                {
                    "reason": "lead reconciliation anchors must cover every mapped batch contract source file",
                    "contract_id": contract_id,
                    "missing_source_files": missing_source_refs,
                }
            )
        lead_evidence_refs = {
            "Contract/source anchors": anchor_refs,
            "observable-outcome": set(PATH_IN_BACKTICKS_RE.findall(row["observable-outcome"])),
            "verification": set(PATH_IN_BACKTICKS_RE.findall(row["verification"])),
        }
        missing_anchor_evidence: dict[str, dict[str, list[str]]] = {}
        for batch_id in batch_refs:
            if batch_id not in batch_contracts:
                continue
            batch_anchor_tokens = set(batch_contracts[batch_id].get("anchor_tokens", []))
            missing_cells = {
                field: sorted(batch_anchor_tokens)
                for field, evidence_refs in lead_evidence_refs.items()
                if not batch_anchor_tokens or not (batch_anchor_tokens & evidence_refs)
            }
            if missing_cells:
                missing_anchor_evidence[batch_id] = missing_cells
        if missing_anchor_evidence:
            issues.append(
                {
                    "reason": "lead reconciliation must carry a concrete source anchor token for every mapped batch contract through anchors, observable outcome, and verification",
                    "contract_id": contract_id,
                    "missing_batch_anchor_evidence": missing_anchor_evidence,
                }
            )
        verification = row["verification"]
        lead_evidence_type_matches = list(
            IMPLEMENTATION_EVIDENCE_TYPE_RE.finditer(verification)
        )
        lead_evidence_type = (
            lead_evidence_type_matches[0].group(1).casefold()
            if len(lead_evidence_type_matches) == 1
            else None
        )
        if len(lead_evidence_type_matches) != 1:
            issues.append(
                {
                    "reason": "lead reconciliation verification requires exactly one evidence-type: test|runtime|source-only declaration",
                    "contract_id": contract_id,
                    "verification": verification,
                }
            )
        for typed_issue in validate_typed_implementation_evidence(
            verification=verification,
            evidence_type=lead_evidence_type,
            result=result,
            known_source_files=known_source_files,
            valid_bound_evidence_ids=valid_bound_evidence_ids,
            context="lead reconciliation verification",
        ):
            issues.append({"contract_id": contract_id, **typed_issue})
        lead_expectation_matches = list(
            IMPLEMENTATION_EXPECTATION_RE.finditer(verification)
        )
        if len(lead_expectation_matches) != 1 or len(
            plain_cell(lead_expectation_matches[0].group(2))
        ) < 12:
            issues.append(
                {
                    "reason": "lead reconciliation verification requires exactly one concrete counterfactual: ... or invariance: ... statement",
                    "contract_id": contract_id,
                    "verification": verification,
                }
            )
        lead_claim_text = " ".join(
            row[field] for field in LEAD_RECONCILIATION_TRACE_FIELDS
        )
        if (
            result == "PASS"
            and lead_evidence_type == "source-only"
            and source_only_claim_requires_stronger_evidence(lead_claim_text)
        ):
            issues.append(
                {
                    "reason": "lead PASS persistence, integration, external-effect, or success claims require test or runtime evidence, not source-only",
                    "contract_id": contract_id,
                    "verification": verification,
                }
            )
        if (
            not IMPLEMENTATION_VERIFICATION_BEHAVIOR_RE.search(verification)
            or not PATH_IN_BACKTICKS_RE.search(verification)
            or any(
                phrase in normalized_text(verification)
                for phrase in GENERIC_IMPLEMENTATION_EVIDENCE_PHRASES
            )
        ):
            issues.append(
                {
                    "reason": "lead reconciliation verification must name a behavior-specific test, runtime check, or source proof",
                    "contract_id": contract_id,
                    "verification": verification,
                }
            )

    missing_batch_contracts = sorted(set(batch_contracts) - set(mapped_batch_counts))
    duplicate_batch_contracts = sorted(
        contract_id for contract_id, count in mapped_batch_counts.items() if count > 1
    )
    if missing_batch_contracts:
        issues.append(
            {
                "reason": "lead reconciliation must map every batch implementation Contract ID exactly once",
                "missing_batch_contract_ids": missing_batch_contracts,
            }
        )
    if duplicate_batch_contracts:
        issues.append(
            {
                "reason": "lead reconciliation must not map a batch implementation Contract ID more than once",
                "duplicate_batch_contract_ids": duplicate_batch_contracts,
            }
        )

    findings_body = bodies.get("findings", "")
    issues.extend(validate_findings_schema(findings_body))
    normalized_findings = re.sub(r"[\s.]+", " ", findings_body.strip().lower()).strip()
    finding_blocks = parse_finding_blocks(findings_body)
    gap_ids = {
        row["contract_id"] for row in rows if row["result"] in {"GAP", "BLOCKED"}
    }
    blocks_by_contract: dict[str, list[dict]] = defaultdict(list)
    if normalized_findings in NO_FINDINGS_SENTINELS:
        if gap_ids:
            issues.append(
                {
                    "reason": "lead GAP or BLOCKED contracts require atomic finding blocks",
                    "contract_ids": sorted(gap_ids),
                }
            )
    else:
        for block in finding_blocks:
            fields = block["fields"]
            file_refs = PATH_IN_BACKTICKS_RE.findall(fields.get("files", ""))
            unknown_files = sorted(set(file_refs) - known_source_files)
            if not file_refs or unknown_files:
                issues.append(
                    {
                        "reason": "lead reconciliation finding Files must cite one or more manifest source files",
                        "heading": block["heading"],
                        "unknown_files": unknown_files,
                    }
                )
            cited_ids = sorted(
                ref
                for ref in set(PATH_IN_BACKTICKS_RE.findall(block.get("text", "")))
                if LEAD_IMPLEMENTATION_CONTRACT_ID_RE.fullmatch(ref)
            )
            if len(cited_ids) != 1 or cited_ids[0] not in gap_ids:
                issues.append(
                    {
                        "reason": "each atomic lead finding must cite exactly one GAP or BLOCKED lead Contract ID",
                        "heading": block["heading"],
                        "contract_ids": cited_ids,
                    }
                )
            else:
                blocks_by_contract[cited_ids[0]].append(block)
            for field_name in REQUIRED_FINDING_FIELDS - {"files", "interface evidence"}:
                value = fields.get(field_name, "")
                if is_boilerplate_value(value) or len(plain_cell(value)) < 12:
                    issues.append(
                        {
                            "reason": "lead reconciliation finding fields require concrete atomic evidence",
                            "heading": block["heading"],
                            "field": field_name,
                            "actual": value,
                        }
                    )
            interface_evidence = fields.get("interface evidence", "")
            if len(plain_cell(interface_evidence)) < 4:
                issues.append(
                    {
                        "reason": "lead reconciliation finding requires interface evidence or Not applicable",
                        "heading": block["heading"],
                    }
                )
        for contract_id in sorted(gap_ids):
            if len(blocks_by_contract.get(contract_id, [])) != 1:
                issues.append(
                    {
                        "reason": "each lead GAP or BLOCKED Contract ID requires exactly one atomic finding block",
                        "contract_id": contract_id,
                        "finding_count": len(blocks_by_contract.get(contract_id, [])),
                    }
                )

    if not bodies.get("open questions", "").strip():
        issues.append({"reason": "lead reconciliation Open Questions must not be empty"})
    return issues, rows


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def manifest_authorized_report_names(manifest: dict) -> list[str]:
    """Return the exact batch, journey, and lead report basenames owned by the manifest."""
    return merge_findings.manifest_report_names(manifest)


def manifest_reports_root(manifest_path: Path, manifest: dict) -> Path:
    declared = manifest.get("reports_dir")
    owned_path = manifest_path.parent / "reports"
    if owned_path.is_symlink():
        raise ValueError(f"Manifest-owned reports directory must not be a symlink: {owned_path}")
    reports_root = owned_path.resolve()
    if not reports_root.is_dir():
        raise ValueError(f"Manifest-owned reports directory is missing: {reports_root}")
    if not isinstance(declared, str) or Path(declared).expanduser().resolve() != reports_root:
        raise ValueError("Manifest reports_dir must resolve to its exact manifest-owned reports directory.")
    return reports_root


def snapshot_authorized_report_state(
    manifest_path: Path,
    manifest: dict,
) -> tuple[Path, dict[str, str], dict[str, tuple[int, ...]]]:
    reports_root = manifest_reports_root(manifest_path, manifest)
    hashes: dict[str, str] = {}
    identities: dict[str, tuple[int, ...]] = {}
    for name in manifest_authorized_report_names(manifest):
        report_path = reports_root / name
        report_bytes, identity = merge_findings.read_stable_regular_file(
            report_path,
            f"manifest-authorized report {name}",
        )
        hashes[name] = hashlib.sha256(report_bytes).hexdigest()
        identities[name] = identity
    return reports_root, hashes, identities


def snapshot_authorized_report_hashes(
    manifest_path: Path,
    manifest: dict,
) -> tuple[Path, dict[str, str]]:
    reports_root, hashes, _identities = snapshot_authorized_report_state(
        manifest_path,
        manifest,
    )
    return reports_root, hashes


def build_verification_receipt(
    *,
    manifest: dict,
    manifest_sha256: str,
    reports_root: Path,
    report_sha256: dict[str, str],
    verifier_result: dict,
) -> dict:
    if verifier_result.get("ok") is not True:
        raise ValueError("A verification receipt requires a passing verifier result.")
    return {
        "schema_version": 1,
        "audit_kind": "full-repo-audit",
        "run_id": manifest.get("run_id"),
        "repo_root": manifest.get("repo_root"),
        "manifest_sha256": manifest_sha256,
        "reports_dir": str(reports_root),
        "report_sha256": report_sha256,
        "verifier_result_sha256": canonical_json_sha256(verifier_result),
    }


def verify_with_receipt_data(
    manifest_path: Path,
    reports: list[Path],
    *,
    skip_current_hash_check: bool = False,
) -> tuple[dict, dict | None]:
    manifest_bytes_before, manifest_identity_before = merge_findings.read_stable_regular_file(
        manifest_path,
        "audit manifest",
    )
    manifest_sha256 = hashlib.sha256(manifest_bytes_before).hexdigest()
    manifest = load_manifest(manifest_path)
    reports_root_before, report_hashes_before, report_identities_before = snapshot_authorized_report_state(
        manifest_path,
        manifest,
    )
    result = verify(
        manifest_path,
        reports,
        skip_current_hash_check=skip_current_hash_check,
    )
    if result.get("ok") is not True:
        return result, None

    manifest_bytes_after, manifest_identity_after = merge_findings.read_stable_regular_file(
        manifest_path,
        "audit manifest",
    )
    reports_root_after, report_hashes_after, report_identities_after = snapshot_authorized_report_state(
        manifest_path,
        manifest,
    )
    if (
        manifest_bytes_after != manifest_bytes_before
        or manifest_identity_after != manifest_identity_before
    ):
        raise ValueError("Audit manifest changed during verification; refusing verification receipt.")
    if (
        reports_root_after != reports_root_before
        or report_hashes_after != report_hashes_before
        or report_identities_after != report_identities_before
    ):
        raise ValueError(
            "Manifest-authorized reports changed during verification; refusing verification receipt."
        )
    receipt = build_verification_receipt(
        manifest=manifest,
        manifest_sha256=manifest_sha256,
        reports_root=reports_root_before,
        report_sha256=report_hashes_before,
        verifier_result=result,
    )
    return result, receipt


def resolved_receipt_output_path(raw_path: str | Path) -> Path:
    path = Path(raw_path).expanduser()
    if not path.name or path.name in {".", ".."}:
        raise ValueError("Verification receipt output must name a file.")
    parent = path.parent.resolve()
    if not parent.is_dir():
        raise ValueError(f"Verification receipt output parent is missing: {parent}")
    return parent / path.name


def prepare_receipt_output(
    raw_path: str | Path,
    *,
    manifest_path: Path,
    manifest: dict,
) -> Path:
    """Validate and invalidate a prior receipt before a new verification attempt."""
    target = resolved_receipt_output_path(raw_path)
    expected_target = manifest_path.parent.resolve() / "verification_receipt.json"
    if target != expected_target:
        raise ValueError(
            "Verification receipt output must be exactly <manifest-dir>/verification_receipt.json."
        )
    # Validate the manifest-owned report root and allowlist before invalidating a prior receipt.
    manifest_reports_root(manifest_path, manifest)
    manifest_authorized_report_names(manifest)
    if target.is_symlink():
        raise ValueError(f"Verification receipt output must not be a symlink: {target}")
    if target.exists():
        if not target.is_file():
            raise ValueError(f"Verification receipt output must be a regular file: {target}")
        target.unlink()
    return target


def write_json_atomic(path: Path, payload: dict) -> None:
    if path.is_symlink():
        raise ValueError(f"Verification receipt output must not be a symlink: {path}")
    if path.exists() and not path.is_file():
        raise ValueError(f"Verification receipt output must be a regular file: {path}")
    file_descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.",
        suffix=".tmp",
        dir=path.parent,
    )
    temporary_path = Path(temporary_name)
    try:
        with os.fdopen(file_descriptor, "w", encoding="utf-8") as handle:
            json.dump(payload, handle, indent=2, sort_keys=True)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        if path.is_symlink():
            raise ValueError(f"Verification receipt output became a symlink: {path}")
        os.replace(temporary_path, path)
    finally:
        if temporary_path.exists():
            temporary_path.unlink()


def parse_report(report_path: Path, known_batch_ids: set[str]) -> dict:
    try:
        text = report_path.read_text(encoding="utf-8")
    except UnicodeDecodeError as exc:
        raise ValueError(f"Report file is not valid UTF-8: {report_path}") from exc
    except OSError as exc:
        raise ValueError(f"Report file could not be read: {report_path}: {exc}") from exc
    rows: list[dict] = []
    malformed_rows: list[str] = []
    in_coverage = False
    bodies = section_bodies(text)
    ordered_sections = [
        match.group(1).strip().lower()
        for line in text.splitlines()
        if (match := SECTION_RE.match(line.strip()))
    ]
    sections = set(ordered_sections)
    run_ids = declared_run_ids(text)
    declared_ids = declared_batch_ids(text)
    file_batch_id = filename_batch_id(report_path)
    known_declared_ids = [batch_id for batch_id in declared_ids if batch_id in known_batch_ids]
    batch_id = known_declared_ids[0] if len(known_declared_ids) == 1 else None
    implementation_rows, malformed_implementation_rows = parse_implementation_inventory_rows(text, report_path)
    interface_rows, malformed_interface_rows = parse_interface_inventory_rows(text, report_path)

    for line in text.splitlines():
        stripped = line.strip()
        if stripped.lower().startswith("## file coverage"):
            in_coverage = True
            continue
        if in_coverage and stripped.startswith("## ") and not stripped.lower().startswith("## file coverage"):
            break
        if not in_coverage or not stripped.startswith("|"):
            continue
        columns = split_markdown_row(stripped)
        if [column.lower() for column in columns[:4]] == ["file", "status", "sha-256", "purpose"]:
            continue
        if is_separator_row(columns):
            continue
        if len(columns) != 4 or not columns[0] or not columns[1] or not columns[2] or not columns[3]:
            malformed_rows.append(stripped)
            continue
        sha_value = columns[2].strip("`").strip()
        if not SHA256_RE.match(sha_value):
            malformed_rows.append(f"{stripped} (invalid SHA-256 digest)")
            continue
        rows.append(
            {
                "file": columns[0].strip("`").strip(),
                "status": columns[1].strip().upper(),
                "sha256": sha_value.lower(),
                "purpose": columns[3],
                "report": str(report_path),
            }
        )

    narrative_issues = []
    for section in ("batch summary", "findings", "no finding notes", "open questions"):
        if not bodies.get(section):
            narrative_issues.append({"section": section, "reason": "section body is empty"})
    narrative_issues.extend(validate_file_coverage_purposes(rows))
    narrative_issues.extend(validate_findings_schema(bodies.get("findings", "")))
    evidence_text = f"{bodies.get('findings', '')}\n{bodies.get('no finding notes', '')}"
    missing_evidence_files = sorted(
        row["file"]
        for row in rows
        if row.get("status") == "CHECKED" and row["file"] not in evidence_text
    )
    if missing_evidence_files:
        narrative_issues.append(
            {
                "section": "findings/no finding notes",
                "reason": "checked files must be referenced in Findings or No Finding Notes",
                "files": missing_evidence_files,
            }
        )

    return {
        "report": str(report_path),
        "run_ids": run_ids,
        "batch_id": batch_id,
        "filename_batch_id": file_batch_id,
        "declared_batch_ids": declared_ids,
        "rows": rows,
        "malformed_rows": malformed_rows,
        "implementation_rows": implementation_rows,
        "malformed_implementation_rows": malformed_implementation_rows,
        "implementation_inventory_body": bodies.get("implementation inventory", ""),
        "interface_rows": interface_rows,
        "malformed_interface_rows": malformed_interface_rows,
        "interface_inventory_body": bodies.get("interface inventory", ""),
        "findings_body": bodies.get("findings", ""),
        "finding_blocks": parse_finding_blocks(bodies.get("findings", "")),
        "missing_sections": [section for section in REQUIRED_SECTIONS if section not in sections],
        "section_order": ordered_sections,
        "section_shape_mismatch": ordered_sections != REQUIRED_SECTION_LIST,
        "narrative_issues": narrative_issues,
    }


def report_location_issue(manifest_path: Path, parsed: dict) -> dict | None:
    report_path = Path(parsed["report"]).resolve()
    expected_root = (manifest_path.parent / "reports").resolve()
    batch_id = parsed["filename_batch_id"] or parsed["batch_id"]
    expected_rel = f"reports/{batch_id}.md" if batch_id else "reports/batch_###.md"
    try:
        rel_path = report_path.relative_to(manifest_path.parent.resolve()).as_posix()
    except ValueError:
        return {
            "report": str(report_path),
            "expected": expected_rel,
            "reason": "report is outside the audit output directory",
        }
    if report_path.parent != expected_root:
        return {
            "report": str(report_path),
            "expected": expected_rel,
            "actual": rel_path,
            "reason": "report must be directly under the audit output reports directory",
        }
    if batch_id and rel_path != expected_rel:
        return {
            "report": str(report_path),
            "expected": expected_rel,
            "actual": rel_path,
            "reason": "report filename/path does not match its batch id",
        }
    return None


def interface_evidence_is_source_backed(
    interface_evidence: str,
    file_refs: list[str],
    interface_files: set[str],
    source_text_getter,
    inventory_visible_by_file: dict[str, set[str]],
) -> bool:
    evidence = plain_cell(interface_evidence)
    normalized = normalized_text(evidence)
    if not evidence or is_boilerplate_value(evidence):
        return False
    if normalized.startswith("visible control text "):
        evidence = evidence[len("visible control text ") :].strip(" `\"'")
    if normalized.startswith("visible text "):
        evidence = evidence[len("visible text ") :].strip(" `\"'")
    if normalized.startswith("visible label "):
        evidence = evidence[len("visible label ") :].strip(" `\"'")
    if normalized.startswith("source anchor "):
        evidence = evidence[len("source anchor ") :].strip(" `\"'")
    if normalized.startswith("exact visible text "):
        evidence = evidence[len("exact visible text ") :].strip(" `\"'")
    evidence = evidence.strip().strip(".")
    backticked = PATH_IN_BACKTICKS_RE.findall(evidence)
    if backticked:
        evidence = backticked[-1]
    evidence = evidence.strip("`\"'.")
    normalized_evidence = normalized_text(evidence)
    if not evidence or normalized_evidence in {"source-backed", "source anchor"}:
        return False
    for rel_path in file_refs:
        if rel_path not in interface_files:
            continue
        if evidence in inventory_visible_by_file.get(rel_path, set()):
            return True
        if source_text_getter and source_contains_visible_text(source_text_getter(rel_path), evidence):
            return True
    return False


def validate_finding_bindings(
    parsed: dict,
    expected_batch: set[str],
    interface_files: set[str],
    source_text_getter=None,
    inventory_visible_by_file: dict[str, set[str]] | None = None,
) -> list[dict]:
    inventory_visible_by_file = inventory_visible_by_file or {}
    issues = []
    for block in parsed["finding_blocks"]:
        fields = block["fields"]
        file_refs = PATH_IN_BACKTICKS_RE.findall(fields.get("files", ""))
        if not file_refs:
            issues.append(
                {
                    "section": "findings",
                    "heading": block["heading"],
                    "reason": "Files field must contain one or more backticked repo-relative paths",
                }
            )
            continue
        unknown_refs = sorted(set(file_refs) - expected_batch)
        if unknown_refs:
            issues.append(
                {
                    "section": "findings",
                    "heading": block["heading"],
                    "reason": "Files field references files outside this batch",
                    "files": unknown_refs,
                }
            )
        evidence = fields.get("evidence", "")
        if is_boilerplate_value(evidence) or len(plain_cell(evidence)) < 12:
            issues.append(
                {
                    "section": "findings",
                    "heading": block["heading"],
                    "reason": "Evidence field must contain concrete non-boilerplate detail",
                    "actual": evidence,
                }
            )
        for field_name in REQUIRED_FINDING_FIELDS - {"files", "evidence", "interface evidence"}:
            value = fields.get(field_name, "")
            if is_boilerplate_value(value) or len(plain_cell(value)) < 12:
                issues.append(
                    {
                        "section": "findings",
                        "heading": block["heading"],
                        "field": field_name,
                        "reason": "required finding field must contain meaningful non-boilerplate content",
                        "actual": value,
                    }
                )
        if set(file_refs) & interface_files:
            interface_evidence = fields.get("interface evidence", "")
            if is_boilerplate_value(interface_evidence):
                issues.append(
                    {
                        "section": "findings",
                        "heading": block["heading"],
                        "reason": "Interface findings must include concrete visible label/control/message evidence",
                        "actual": interface_evidence,
                    }
                )
            elif not interface_evidence_is_source_backed(
                interface_evidence,
                file_refs,
                interface_files,
                source_text_getter,
                inventory_visible_by_file,
            ):
                issues.append(
                    {
                        "section": "findings",
                        "heading": block["heading"],
                        "reason": "Interface evidence must match source-visible text/control/message or an accepted interface inventory row",
                        "actual": interface_evidence,
                    }
                )
    return issues


def verify(
    manifest_path: Path,
    reports: list[Path],
    *,
    skip_current_hash_check: bool = False,
    batch_id: str | None = None,
) -> dict:
    verification_batch_id = batch_id
    manifest = load_manifest(manifest_path)
    expected_source_files = manifest["expected_files"]
    expected_hashes = manifest["expected_hashes"]
    expected_by_batch = manifest["expected_by_batch"]
    expected_files_by_batch = manifest.get("expected_files_by_batch", expected_by_batch)
    expected_unit_to_file = manifest.get("expected_unit_to_file", {})
    coverage_unit_by_id = {
        unit.get("unit_id"): unit
        for unit in manifest.get("coverage_units_normalized", [])
        if isinstance(unit, dict) and isinstance(unit.get("unit_id"), str)
    }
    if verification_batch_id is not None:
        if verification_batch_id not in expected_by_batch:
            raise ValueError(f"--batch-id is not present in the manifest: {verification_batch_id}")
        expected_by_batch = {verification_batch_id: expected_by_batch[verification_batch_id]}
        expected_files_by_batch = {
            verification_batch_id: expected_files_by_batch[verification_batch_id]
        }
        selected_units = set(expected_by_batch[verification_batch_id])
        expected_hashes = {
            unit_id: sha256
            for unit_id, sha256 in expected_hashes.items()
            if unit_id in selected_units
        }
        expected_source_files = sorted(set(expected_files_by_batch[batch_id]))
    expected = set(expected_hashes)
    expected_source_hashes = {
        item["rel_path"]: item.get("sha256")
        for item in manifest.get("source_files", [])
        if isinstance(item, dict)
        and isinstance(item.get("rel_path"), str)
        and (verification_batch_id is None or item["rel_path"] in expected_source_files)
    }
    known_batch_ids = set(expected_by_batch)
    expected_run_id = manifest.get("run_id")
    bound_evidence_records, bound_evidence_issues = audit_evidence.validate_visual_evidence_manifest(
        manifest_path.parent,
        expected_run_id or "",
        required=False,
    )
    invalid_bound_evidence_ids = {
        issue.get("record")
        for issue in bound_evidence_issues
        if isinstance(issue.get("record"), str)
    }
    has_global_bound_evidence_issue = any(
        not isinstance(issue.get("record"), str) for issue in bound_evidence_issues
    )
    valid_bound_evidence_ids = (
        set()
        if has_global_bound_evidence_issue
        else set(bound_evidence_records) - invalid_bound_evidence_ids
    )
    repo_root_raw = manifest.get("repo_root")
    repo_root_text = repo_root_raw.strip() if isinstance(repo_root_raw, str) else ""
    repo_root = Path(repo_root_text).expanduser() if repo_root_text else None
    source_text_checks_enabled = repo_root is not None and repo_root.exists() and repo_root.is_dir()
    source_text_cache: dict[str, str] = {}
    source_bytes_cache: dict[str, bytes] = {}

    def source_text(rel_path: str) -> str:
        if not source_text_checks_enabled or repo_root is None:
            return ""
        if rel_path not in source_text_cache:
            path = repo_root / rel_path
            try:
                source_text_cache[rel_path] = path.read_text(encoding="utf-8", errors="ignore")
            except OSError:
                source_text_cache[rel_path] = ""
        return source_text_cache[rel_path]

    def source_bytes(rel_path: str) -> bytes:
        if not source_text_checks_enabled or repo_root is None:
            return b""
        if rel_path not in source_bytes_cache:
            try:
                source_bytes_cache[rel_path] = (repo_root / rel_path).read_bytes()
            except OSError:
                source_bytes_cache[rel_path] = b""
        return source_bytes_cache[rel_path]

    def source_bytes_for_unit(unit_id: str) -> bytes:
        rel_path = expected_unit_to_file.get(unit_id, unit_id)
        unit = coverage_unit_by_id.get(unit_id, {})
        data = source_bytes(rel_path)
        start_byte = unit.get("start_byte") if isinstance(unit, dict) else None
        end_byte = unit.get("end_byte") if isinstance(unit, dict) else None
        if isinstance(start_byte, int) and isinstance(end_byte, int):
            return data[start_byte - 1 : end_byte]
        start_line = unit.get("start_line") if isinstance(unit, dict) else None
        end_line = unit.get("end_line") if isinstance(unit, dict) else None
        if isinstance(start_line, int) and isinstance(end_line, int):
            return b"".join(data.splitlines(keepends=True)[start_line - 1 : end_line])
        return data

    def unit_requires_source_token_anchor(unit_id: str) -> bool:
        data = source_bytes_for_unit(unit_id)
        if not data or b"\0" in data:
            return False
        try:
            text = data.decode("utf-8")
        except UnicodeDecodeError:
            return False
        return bool(text.strip())

    def source_text_for_unit(unit_id: str) -> str:
        rel_path = expected_unit_to_file.get(unit_id, unit_id)
        unit = coverage_unit_by_id.get(unit_id, {})
        start_byte = unit.get("start_byte") if isinstance(unit, dict) else None
        end_byte = unit.get("end_byte") if isinstance(unit, dict) else None
        if (
            source_text_checks_enabled
            and repo_root is not None
            and isinstance(start_byte, int)
            and isinstance(end_byte, int)
            and start_byte >= 1
            and end_byte >= start_byte
        ):
            try:
                with (repo_root / rel_path).open("rb") as handle:
                    handle.seek(start_byte - 1)
                    return handle.read(end_byte - start_byte + 1).decode("utf-8", errors="ignore")
            except OSError:
                return ""
        text = source_text(rel_path)
        start_line = unit.get("start_line") if isinstance(unit, dict) else None
        end_line = unit.get("end_line") if isinstance(unit, dict) else None
        if not isinstance(start_line, int) or not isinstance(end_line, int):
            return text
        lines = text.splitlines()
        return "\n".join(lines[start_line - 1 : end_line])

    def responsibility_occurrences_for_unit(unit_id: str) -> list[dict[str, object]]:
        rel_path = expected_unit_to_file.get(unit_id, unit_id)
        unit = coverage_unit_by_id.get(unit_id, {})
        start_line = unit.get("start_line") if isinstance(unit, dict) else None
        start_byte = unit.get("start_byte") if isinstance(unit, dict) else None
        return implementation_responsibility_occurrences(
            rel_path,
            source_text_for_unit(unit_id),
            start_line=start_line if isinstance(start_line, int) else 1,
            start_byte=start_byte if isinstance(start_byte, int) else None,
        )

    if verification_batch_id is None:
        unresolved_scope_warnings, excluded_file_mismatches = verify_excluded_files(manifest_path, manifest)
    else:
        unresolved_scope_warnings, excluded_file_mismatches = [], []
    completion_marker_mismatches = verify_completion_marker(manifest_path, manifest)

    lead_report_paths = [
        report for report in reports if report.name == LEAD_RECONCILIATION_REPORT_NAME
    ]
    batch_report_paths = [
        report for report in reports if report.name != LEAD_RECONCILIATION_REPORT_NAME
    ]
    parsed_reports = [parse_report(report, known_batch_ids) for report in batch_report_paths]
    batch_contracts: dict[str, dict] = {}
    for parsed in parsed_reports:
        for row in parsed["implementation_rows"]:
            contract_id = row.get("contract_id")
            unit_id = row.get("file")
            if (
                isinstance(contract_id, str)
                and BATCH_IMPLEMENTATION_CONTRACT_ID_RE.fullmatch(contract_id)
                and isinstance(unit_id, str)
            ):
                source_file = expected_unit_to_file.get(unit_id, unit_id)
                expected_unit_hash = expected_hashes.get(unit_id, "")
                excluded_anchor_refs = {unit_id, source_file, contract_id}
                unit_text = source_text_for_unit(unit_id)
                requires_source_token = unit_requires_source_token_anchor(unit_id)
                occurrence_anchors = {
                    str(item["anchor"])
                    for item in responsibility_occurrences_for_unit(unit_id)
                }
                anchor_tokens = sorted(
                    {
                        plain_cell(reference)
                        for reference in PATH_IN_BACKTICKS_RE.findall(row.get("anchors", ""))
                        if plain_cell(reference) not in excluded_anchor_refs
                        and (
                            (
                                plain_cell(reference) in unit_text
                                or plain_cell(reference) in occurrence_anchors
                            )
                            if requires_source_token
                            else (
                                plain_cell(reference) in unit_text
                                or plain_cell(reference) in occurrence_anchors
                                or (
                                    expected_unit_hash
                                    and plain_cell(reference).casefold()
                                    == expected_unit_hash.casefold()
                                )
                            )
                        )
                    }
                )
                batch_contracts[contract_id] = {
                    "unit": unit_id,
                    "source_file": source_file,
                    "result": row.get("result"),
                    "anchor_tokens": anchor_tokens,
                }
    lead_reconciliation_issues: list[dict] = []
    lead_reconciliation_rows: list[dict] = []
    expected_lead_path = (manifest_path.parent / "reports" / LEAD_RECONCILIATION_REPORT_NAME).resolve()
    if verification_batch_id is not None:
        lead_reconciliation_issues = []
    elif len(lead_report_paths) != 1:
        lead_reconciliation_issues.append(
            {
                "reason": "exactly one reports/lead_reconciliation.md artifact is required",
                "reports": [str(path) for path in lead_report_paths],
            }
        )
    else:
        actual_lead_path = lead_report_paths[0].resolve()
        if actual_lead_path != expected_lead_path:
            lead_reconciliation_issues.append(
                {
                    "reason": "lead reconciliation report must use the manifest-owned exact report path",
                    "expected": str(expected_lead_path),
                    "actual": str(actual_lead_path),
                }
            )
        report_issues, lead_reconciliation_rows = validate_lead_reconciliation_report(
            lead_report_paths[0],
            expected_run_id=expected_run_id,
            known_source_files=set(expected_source_files),
            source_file_count=len(expected_source_files),
            source_text_for_file=source_text,
            batch_contracts=batch_contracts,
            source_hashes=expected_source_hashes,
            valid_bound_evidence_ids=valid_bound_evidence_ids,
        )
        lead_reconciliation_issues.extend(report_issues)
    source_interface_relevance = {
        item["rel_path"]: item.get("interface_relevant") is True
        for item in manifest.get("source_files", [])
        if isinstance(item, dict) and isinstance(item.get("rel_path"), str)
    }
    source_kind_by_file = {
        item["rel_path"]: item.get("kind")
        for item in manifest.get("source_files", [])
        if isinstance(item, dict) and isinstance(item.get("rel_path"), str)
    }
    expected_interface_by_batch = {
        batch_id: {rel_path for rel_path in files if source_interface_relevance.get(rel_path)}
        for batch_id, files in expected_files_by_batch.items()
    }
    report_rel_by_batch = {}
    report_location_mismatches = []
    for parsed in parsed_reports:
        location_issue = report_location_issue(manifest_path, parsed)
        if location_issue:
            report_location_mismatches.append(location_issue)
        batch_id = parsed["batch_id"]
        if not batch_id:
            continue
        try:
            report_rel = Path(parsed["report"]).resolve().relative_to(manifest_path.parent).as_posix()
        except ValueError:
            report_rel = str(Path(parsed["report"]).resolve())
        report_rel_by_batch[batch_id] = report_rel
    effort_ledger_mismatches = (
        verify_effort_ledger(manifest_path, manifest, known_batch_ids, report_rel_by_batch)
        if verification_batch_id is None
        else []
    )
    observed: dict[str, list[dict]] = defaultdict(list)
    for parsed in parsed_reports:
        for row in parsed["rows"]:
            observed[row["file"]].append(row)

    observed_paths = set(observed)
    missing = sorted(expected - observed_paths)
    extra = sorted(observed_paths - expected)
    duplicate = sorted(path for path, rows in observed.items() if len(rows) > 1)
    unchecked = sorted(
        path
        for path in expected & observed_paths
        if any(row["status"] != "CHECKED" for row in observed[path])
    )

    batch_counts = Counter(parsed["batch_id"] for parsed in parsed_reports if parsed["batch_id"])
    missing_batch_reports = sorted(batch_id for batch_id in known_batch_ids if batch_counts[batch_id] == 0)
    duplicate_batch_reports = sorted(batch_id for batch_id, count in batch_counts.items() if count > 1)
    unassigned_reports = sorted(parsed["report"] for parsed in parsed_reports if not parsed["batch_id"])
    run_id_mismatches = []
    for parsed in parsed_reports:
        run_ids = parsed["run_ids"]
        reasons = []
        if len(run_ids) != 1:
            reasons.append(f"expected exactly one Run ID, found {len(run_ids)}")
        elif expected_run_id and run_ids[0] != expected_run_id:
            reasons.append(f"declared Run ID {run_ids[0]} does not match manifest {expected_run_id}")
        if reasons:
            run_id_mismatches.append({"report": parsed["report"], "run_ids": run_ids, "reasons": reasons})

    report_hash_mismatches = []
    for path, rows in observed.items():
        expected_hash = expected_hashes.get(path)
        if not expected_hash:
            continue
        for row in rows:
            if row.get("sha256") != expected_hash:
                report_hash_mismatches.append(
                    {
                        "file": path,
                        "report": row["report"],
                        "expected": expected_hash,
                        "reported": row.get("sha256"),
                    }
                )

    current_hash_mismatches = []
    current_hash_errors = []
    source_text_errors = []
    verification_warnings = []
    if not source_text_checks_enabled:
        source_text_errors.append(
            {
                "repo_root": str(repo_root) if repo_root is not None else None,
                "reason": "manifest repo_root is missing or not a directory; source-backed implementation, interface, placeholder, dead-control, and asset checks cannot run",
            }
        )
    if skip_current_hash_check:
        verification_warnings.append(
            {
                "flag": "--skip-current-hash-check",
                "reason": "current source fingerprint freshness check was skipped; verification is degraded",
            }
        )
    elif repo_root is None or not repo_root.exists() or not repo_root.is_dir():
        current_hash_errors.append(
            {
                "repo_root": str(repo_root) if repo_root is not None else None,
                "reason": "manifest repo_root is missing or not a directory; current SHA-256 check cannot run",
            }
        )
    else:
        for rel_path, expected_hash in expected_source_hashes.items():
            if not expected_hash:
                continue
            fs_path = repo_root / rel_path
            if not fs_path.is_file():
                current_hash_mismatches.append(
                    {"file": rel_path, "expected": expected_hash, "current": None, "reason": "missing"}
                )
                continue
            try:
                current_hash = sha256_file(fs_path)
            except OSError as exc:
                current_hash_errors.append(
                    {
                        "file": rel_path,
                        "expected": expected_hash,
                        "current": None,
                        "reason": f"current SHA-256 check could not read file: {exc}",
                    }
                )
                continue
            if current_hash != expected_hash:
                current_hash_mismatches.append(
                    {"file": rel_path, "expected": expected_hash, "current": current_hash, "reason": "changed"}
                )
    batch_id_mismatches = []
    for parsed in parsed_reports:
        declared_ids = parsed["declared_batch_ids"]
        file_batch_id = parsed["filename_batch_id"]
        mismatch_reasons = []
        if len(declared_ids) != 1:
            mismatch_reasons.append(f"expected exactly one declared Batch ID, found {len(declared_ids)}")
        elif declared_ids[0] not in known_batch_ids:
            mismatch_reasons.append(f"declared unknown Batch ID {declared_ids[0]}")
        if file_batch_id and declared_ids and declared_ids[0] != file_batch_id:
            mismatch_reasons.append(f"filename Batch ID {file_batch_id} does not match declared {declared_ids[0]}")
        if mismatch_reasons:
            batch_id_mismatches.append(
                {
                    "report": parsed["report"],
                    "filename_batch_id": file_batch_id,
                    "declared_batch_ids": declared_ids,
                    "reasons": mismatch_reasons,
                }
            )
    malformed = [
        {"report": parsed["report"], "rows": parsed["malformed_rows"]}
        for parsed in parsed_reports
        if parsed["malformed_rows"]
    ]
    implementation_inventory_issues = []
    contract_id_counts = Counter(
        row["contract_id"]
        for parsed in parsed_reports
        for row in parsed["implementation_rows"]
        if row.get("contract_id")
    )
    duplicate_contract_ids = {
        contract_id for contract_id, count in contract_id_counts.items() if count > 1
    }
    for parsed in parsed_reports:
        batch_id = parsed["batch_id"]
        if not batch_id or batch_id not in expected_by_batch:
            continue
        expected_units = set(expected_by_batch[batch_id])
        observed_units = [row["file"] for row in parsed["implementation_rows"]]
        observed_unit_set = set(observed_units)
        issues = []
        if parsed["malformed_implementation_rows"]:
            issues.append(
                {
                    "reason": "malformed implementation inventory rows",
                    "rows": parsed["malformed_implementation_rows"],
                }
            )
        missing_units = sorted(expected_units - observed_unit_set)
        extra_units = sorted(observed_unit_set - expected_units)
        if missing_units:
            issues.append({"reason": "missing implementation inventory rows", "units": missing_units})
        if extra_units:
            issues.append({"reason": "implementation inventory rows outside this batch", "units": extra_units})

        responsibility_occurrences_by_unit = {
            unit_id: responsibility_occurrences_for_unit(unit_id)
            for unit_id in sorted(expected_units)
        }
        for unit_id in sorted(expected_units):
            rel_path = expected_unit_to_file.get(unit_id, unit_id)
            responsibility_anchors = [
                str(item["anchor"])
                for item in responsibility_occurrences_by_unit[unit_id]
            ]
            hint_contracts: dict[str, set[str]] = defaultdict(set)
            for row in parsed["implementation_rows"]:
                if row["file"] != unit_id:
                    continue
                anchor_refs = set(PATH_IN_BACKTICKS_RE.findall(row["anchors"]))
                trace_refs = set(PATH_IN_BACKTICKS_RE.findall(row["implementation_trace"]))
                verification_refs = set(PATH_IN_BACKTICKS_RE.findall(row["verification_evidence"]))
                for hint in responsibility_anchors:
                    if hint in anchor_refs and hint in trace_refs and hint in verification_refs:
                        hint_contracts[hint].add(row["contract_id"])
            missing_responsibilities = sorted(set(responsibility_anchors) - set(hint_contracts))
            if missing_responsibilities:
                issues.append(
                    {
                        "reason": "implementation inventory omitted occurrence-aware named source responsibilities",
                        "unit": unit_id,
                        "missing_source_anchors": missing_responsibilities,
                    }
                )
            contract_hints: dict[str, set[str]] = defaultdict(set)
            for hint, contract_ids in hint_contracts.items():
                for contract_id in contract_ids:
                    contract_hints[contract_id].add(hint)
            # Definitions that jointly implement one outcome may share a row.
            # Coverage remains exact because every occurrence-aware anchor must
            # still be present in anchors, trace, and verification.

        finding_blocks_by_file: dict[str, list[dict]] = defaultdict(list)
        for block in parsed["finding_blocks"]:
            for file_ref in PATH_IN_BACKTICKS_RE.findall(block["fields"].get("files", "")):
                finding_blocks_by_file[file_ref].append(block)

        gap_rows = [
            row
            for row in parsed["implementation_rows"]
            if row["file"] in expected_units and row["result"] in {"GAP", "BLOCKED"}
        ]
        gap_rows_by_contract = {
            row["contract_id"]: row
            for row in gap_rows
            if (
                (match := BATCH_IMPLEMENTATION_CONTRACT_ID_RE.fullmatch(row["contract_id"]))
                and match.group(1) == batch_id
            )
        }
        finding_blocks_by_contract: dict[str, list[dict]] = defaultdict(list)
        for block in parsed["finding_blocks"]:
            cited_ids = sorted(
                {
                    reference
                    for reference in PATH_IN_BACKTICKS_RE.findall(block.get("text", ""))
                    if BATCH_IMPLEMENTATION_CONTRACT_ID_RE.fullmatch(reference)
                }
            )
            if len(cited_ids) != 1 or cited_ids[0] not in gap_rows_by_contract:
                issues.append(
                    {
                        "reason": "each atomic batch finding must cite exactly one GAP or BLOCKED implementation Contract ID",
                        "heading": block.get("heading"),
                        "contract_ids": cited_ids,
                    }
                )
            else:
                finding_blocks_by_contract[cited_ids[0]].append(block)

        for row in parsed["implementation_rows"]:
            unit_id = row["file"]
            if unit_id not in expected_units:
                continue
            rel_path = expected_unit_to_file.get(unit_id, unit_id)
            contract_id = row["contract_id"]
            result = row["result"]
            trace_statuses: list[str] = []
            trace_status_by_field: dict[str, str] = {}
            for trace_field in (
                "implementation_trace",
                "failure_trace",
                "verification_evidence",
            ):
                status_match = IMPLEMENTATION_TRACE_STATUS_RE.match(row[trace_field])
                if not status_match:
                    issues.append(
                        {
                            "reason": "implementation trace fields must begin with pass, gap, blocked, or not applicable plus evidence",
                            "unit": unit_id,
                            "contract_id": contract_id,
                            "field": trace_field,
                            "actual": row[trace_field],
                        }
                    )
                    continue
                trace_status = status_match.group(1).casefold()
                trace_statuses.append(trace_status)
                trace_status_by_field[trace_field] = trace_status
            derived_result = (
                "GAP"
                if "gap" in trace_statuses
                else "BLOCKED"
                if "blocked" in trace_statuses
                else "PASS"
            )
            if result in IMPLEMENTATION_RESULTS and trace_statuses and result != derived_result:
                issues.append(
                    {
                        "reason": "implementation inventory Result contradicts its trace statuses",
                        "unit": unit_id,
                        "contract_id": contract_id,
                        "expected": derived_result,
                        "actual": result,
                    }
                )
            if result == "PASS":
                non_pass_clean_fields = {
                    field: trace_status_by_field.get(field)
                    for field in ("implementation_trace", "verification_evidence")
                    if trace_status_by_field.get(field) != "pass"
                }
                if non_pass_clean_fields:
                    issues.append(
                        {
                            "reason": "implementation inventory PASS requires pass implementation and verification trace statuses",
                            "unit": unit_id,
                            "contract_id": contract_id,
                            "actual": non_pass_clean_fields,
                        }
                    )
            contract_id_match = BATCH_IMPLEMENTATION_CONTRACT_ID_RE.fullmatch(contract_id)
            if (
                contract_id_match is None
                or contract_id_match.group(1) != batch_id
                or is_boilerplate_value(contract_id)
            ):
                issues.append(
                    {
                        "reason": "implementation inventory Contract ID must use the owning batch_<3+ digits>:C<3+ digits> namespace",
                        "unit": unit_id,
                        "contract_id": contract_id,
                        "expected_batch": batch_id,
                    }
                )
            if contract_id in duplicate_contract_ids:
                issues.append(
                    {
                        "reason": "implementation inventory Contract IDs must be unique across the audit",
                        "unit": unit_id,
                        "contract_id": contract_id,
                    }
                )
            if result not in IMPLEMENTATION_RESULTS:
                issues.append(
                    {
                        "reason": "implementation inventory Result must be PASS, GAP, or BLOCKED",
                        "unit": unit_id,
                        "actual": result,
                    }
                )
            field_minimums = {
                "contract": 12,
                "anchors": 3,
                "implementation_trace": 12,
                "failure_trace": 12,
                "verification_evidence": 24,
            }
            for field, minimum in field_minimums.items():
                value = row[field]
                if is_boilerplate_value(value) or len(plain_cell(value)) < minimum:
                    issues.append(
                        {
                            "reason": "implementation inventory field must contain concrete non-boilerplate evidence",
                            "unit": unit_id,
                            "field": field,
                            "actual": value,
                        }
                    )

            contract_text = row["contract"]
            basis_matches = list(IMPLEMENTATION_BASIS_RE.finditer(contract_text))
            if len(basis_matches) != 1:
                issues.append(
                    {
                        "reason": "implementation responsibility requires exactly one 'Basis: <kind> — <backticked reference>' statement",
                        "unit": unit_id,
                        "contract_id": contract_id,
                        "actual": contract_text,
                    }
                )
                basis_kind = None
                basis_refs: set[str] = set()
            else:
                basis_kind = basis_matches[0].group(1).casefold()
                basis_detail = basis_matches[0].group(2)
                basis_refs = {
                    plain_cell(reference)
                    for reference in PATH_IN_BACKTICKS_RE.findall(basis_detail)
                    if len(plain_cell(reference)) >= 2
                    and not is_boilerplate_value(plain_cell(reference))
                }
                if basis_kind not in IMPLEMENTATION_BASIS_KINDS:
                    issues.append(
                        {
                            "reason": "implementation responsibility Basis kind is not an allowed authoritative or source-inferred kind",
                            "unit": unit_id,
                            "contract_id": contract_id,
                            "actual": basis_kind,
                            "allowed": sorted(IMPLEMENTATION_BASIS_KINDS),
                        }
                    )
                if not basis_refs:
                    issues.append(
                        {
                            "reason": "implementation responsibility Basis must cite a concrete backticked reference",
                            "unit": unit_id,
                            "contract_id": contract_id,
                            "actual": basis_detail,
                        }
                    )
                if basis_kind in IMPLEMENTATION_BASIS_KINDS - {"source-inferred"}:
                    unresolved_basis_refs = sorted(
                        reference
                        for reference in basis_refs
                        if manifest_source_path_for_reference(
                            reference,
                            set(expected_source_files),
                        )
                        is None
                    )
                    if unresolved_basis_refs:
                        issues.append(
                            {
                                "reason": "authoritative implementation Basis references must resolve to manifest-owned source files, optionally with a precise #line, #byte, or #symbol suffix",
                                "unit": unit_id,
                                "contract_id": contract_id,
                                "unresolved_basis_references": unresolved_basis_refs,
                            }
                        )

            discovery_matches = list(IMPLEMENTATION_DISCOVERY_RE.finditer(contract_text))
            if len(discovery_matches) != 1:
                issues.append(
                    {
                        "reason": "implementation responsibility requires exactly one 'Discovery: parsed|manual — <backticked reference>' statement",
                        "unit": unit_id,
                        "contract_id": contract_id,
                        "actual": contract_text,
                    }
                )
                discovery_kind = None
                discovery_refs: set[str] = set()
            else:
                discovery_kind = discovery_matches[0].group(1).casefold()
                discovery_detail = discovery_matches[0].group(2)
                discovery_refs = {
                    plain_cell(reference)
                    for reference in PATH_IN_BACKTICKS_RE.findall(discovery_detail)
                    if len(plain_cell(reference)) >= 2
                    and not is_boilerplate_value(plain_cell(reference))
                }
                if not discovery_refs:
                    issues.append(
                        {
                            "reason": "implementation responsibility Discovery must cite a concrete backticked reference",
                            "unit": unit_id,
                            "contract_id": contract_id,
                            "actual": discovery_detail,
                        }
                    )
                if discovery_kind == "parsed":
                    recognized_refs = {
                        str(item["anchor"])
                        for item in responsibility_occurrences_by_unit.get(unit_id, [])
                    }
                    row_anchor_refs = {
                        plain_cell(reference)
                        for reference in PATH_IN_BACKTICKS_RE.findall(row["anchors"])
                    }
                    if not (discovery_refs & recognized_refs & row_anchor_refs):
                        issues.append(
                            {
                                "reason": "parsed discovery requires a recognized occurrence-aware named-definition anchor in the assigned unit anchors",
                                "unit": unit_id,
                                "contract_id": contract_id,
                                "recognized": sorted(recognized_refs),
                                "actual": sorted(discovery_refs),
                            }
                        )
            trace_text = row["implementation_trace"]
            if "->" not in trace_text and "→" not in trace_text:
                issues.append(
                    {
                        "reason": "implementation/data/side-effect trace must show an ordered path with '->' or '→'",
                        "unit": unit_id,
                        "actual": trace_text,
                    }
                )
            valid_anchor_values: list[str] = []
            if source_text_checks_enabled:
                unit_text = source_text_for_unit(unit_id)
                expected_hash = expected_hashes.get(unit_id, "")
                anchor_values = [plain_cell(value) for value in PATH_IN_BACKTICKS_RE.findall(row["anchors"])]
                source_anchor_values = [
                    value
                    for value in anchor_values
                    if len(value) >= 1
                    and value not in {unit_id, rel_path, contract_id, expected_hash}
                    and (
                        value in unit_text
                        or value
                        in {
                            str(item["anchor"])
                            for item in responsibility_occurrences_by_unit.get(unit_id, [])
                        }
                    )
                ]
                hash_anchor_values = [
                    value for value in anchor_values
                    if expected_hash and value.casefold() == expected_hash.casefold()
                ]
                requires_source_token = unit_requires_source_token_anchor(unit_id)
                valid_anchor_values = source_anchor_values if requires_source_token else (source_anchor_values or hash_anchor_values)
                if requires_source_token and not source_anchor_values:
                    issues.append(
                        {
                            "reason": "non-empty text implementation units require a backticked source token; a file or unit hash is coverage evidence, not an implementation anchor",
                            "unit": unit_id,
                            "anchors": row["anchors"],
                        }
                    )
                elif not requires_source_token and not valid_anchor_values:
                    issues.append(
                        {
                            "reason": "empty or non-text implementation units require a backticked assigned-unit SHA-256 or concrete decoded source token",
                            "unit": unit_id,
                            "anchors": row["anchors"],
                        }
                    )

            if basis_kind == "source-inferred" and basis_refs and not (
                basis_refs & set(valid_anchor_values)
            ):
                issues.append(
                    {
                        "reason": "source-inferred Basis must cite a validated source anchor from the assigned unit",
                        "unit": unit_id,
                        "contract_id": contract_id,
                        "basis_references": sorted(basis_refs),
                        "validated_anchors": sorted(valid_anchor_values),
                    }
                )
            if discovery_refs and not (discovery_refs & set(valid_anchor_values)):
                issues.append(
                    {
                        "reason": "implementation responsibility Discovery must cite a validated source anchor from the assigned unit",
                        "unit": unit_id,
                        "contract_id": contract_id,
                        "discovery_references": sorted(discovery_refs),
                        "validated_anchors": sorted(valid_anchor_values),
                    }
                )

            trace_refs = set(PATH_IN_BACKTICKS_RE.findall(trace_text))
            if not valid_anchor_values or not any(anchor in trace_refs for anchor in valid_anchor_values):
                issues.append(
                    {
                        "reason": "implementation/data/side-effect trace must repeat a validated backticked source anchor from the assigned unit",
                        "unit": unit_id,
                        "contract_id": contract_id,
                        "actual": trace_text,
                    }
                )

            verification_evidence = row["verification_evidence"]
            evidence_type_matches = list(
                IMPLEMENTATION_EVIDENCE_TYPE_RE.finditer(verification_evidence)
            )
            evidence_type = (
                evidence_type_matches[0].group(1).casefold()
                if len(evidence_type_matches) == 1
                else None
            )
            if len(evidence_type_matches) != 1:
                issues.append(
                    {
                        "reason": "implementation verification requires exactly one evidence-type: test|runtime|source-only declaration",
                        "unit": unit_id,
                        "contract_id": contract_id,
                        "verification_evidence": verification_evidence,
                    }
                )
            for typed_issue in validate_typed_implementation_evidence(
                verification=verification_evidence,
                evidence_type=evidence_type,
                result=result,
                known_source_files=set(expected_source_files),
                valid_bound_evidence_ids=valid_bound_evidence_ids,
                context="implementation verification",
            ):
                issues.append(
                    {
                        "unit": unit_id,
                        "contract_id": contract_id,
                        **typed_issue,
                    }
                )
            expectation_matches = list(
                IMPLEMENTATION_EXPECTATION_RE.finditer(verification_evidence)
            )
            if len(expectation_matches) != 1 or len(
                plain_cell(expectation_matches[0].group(2))
            ) < 12:
                issues.append(
                    {
                        "reason": "implementation verification requires exactly one concrete counterfactual: ... or invariance: ... statement",
                        "unit": unit_id,
                        "contract_id": contract_id,
                        "verification_evidence": verification_evidence,
                    }
                )
            claim_text = f"{row['contract']} {row['implementation_trace']}"
            if (
                result == "PASS"
                and evidence_type == "source-only"
                and not is_manifest_test_path(rel_path)
                and source_only_claim_requires_stronger_evidence(claim_text)
            ):
                issues.append(
                    {
                        "reason": "PASS persistence, integration, external-effect, or success claims require test or runtime evidence, not source-only",
                        "unit": unit_id,
                        "contract_id": contract_id,
                        "verification_evidence": verification_evidence,
                    }
                )
            verification_refs = set(PATH_IN_BACKTICKS_RE.findall(verification_evidence))
            browser_test_refs = sorted(
                reference
                for reference in verification_refs
                if BROWSER_TEST_REFERENCE_RE.search(reference)
            )
            if (
                result == "PASS"
                and evidence_type == "test"
                and browser_test_refs
                and not (
                    BROWSER_TEST_COMMAND_RE.search(verification_evidence)
                    and BROWSER_TEST_PASS_RE.search(verification_evidence)
                    and BROWSER_TEST_ZERO_FAILURE_RE.search(verification_evidence)
                )
            ):
                issues.append(
                    {
                        "reason": "browser-test PASS evidence must record the exact command plus a completed nonzero pass and zero-failure summary",
                        "unit": unit_id,
                        "contract_id": contract_id,
                        "browser_test_references": browser_test_refs,
                        "verification_evidence": verification_evidence,
                    }
                )
            generic_verification = any(
                phrase in normalized_text(verification_evidence)
                for phrase in GENERIC_IMPLEMENTATION_EVIDENCE_PHRASES
            )
            if (
                not valid_anchor_values
                or not any(anchor in verification_refs for anchor in valid_anchor_values)
                or not IMPLEMENTATION_VERIFICATION_BEHAVIOR_RE.search(verification_evidence)
                or generic_verification
            ):
                issues.append(
                    {
                        "reason": "implementation verification must be behavior-specific, cite a validated source anchor, and name what a test, runtime check, or manual source trace proves",
                        "unit": unit_id,
                        "contract_id": contract_id,
                        "verification_evidence": verification_evidence,
                    }
                )

            file_bound_blocks = finding_blocks_by_file.get(rel_path, [])
            bound_blocks = [
                block
                for block in file_bound_blocks
                if finding_cites_implementation_contract(
                    block,
                    contract_id=contract_id,
                    unit_id=unit_id,
                    rel_path=rel_path,
                )
            ]
            if result in {"GAP", "BLOCKED"}:
                if not bound_blocks:
                    issues.append(
                        {
                            "reason": "GAP or BLOCKED implementation inventory row must have a file-bound finding that cites its exact Contract ID and range unit when applicable",
                            "unit": unit_id,
                            "source_file": rel_path,
                            "contract_id": contract_id,
                            "result": result,
                        }
                    )

        for contract_id, row in sorted(gap_rows_by_contract.items()):
            finding_count = len(finding_blocks_by_contract.get(contract_id, []))
            if finding_count != 1:
                issues.append(
                    {
                        "reason": "each GAP or BLOCKED implementation Contract ID requires exactly one atomic batch finding",
                        "unit": row["file"],
                        "contract_id": contract_id,
                        "finding_count": finding_count,
                    }
                )

        gap_rows_by_file: dict[str, list[dict]] = defaultdict(list)
        for row in gap_rows:
            gap_rows_by_file[expected_unit_to_file.get(row["file"], row["file"])].append(row)
        for rel_path, blocks in finding_blocks_by_file.items():
            if rel_path not in expected_files_by_batch.get(batch_id, set()):
                continue
            gap_rows = gap_rows_by_file.get(rel_path, [])
            for block in blocks:
                if not any(
                    finding_cites_implementation_contract(
                        block,
                        contract_id=row["contract_id"],
                        unit_id=row["file"],
                        rel_path=rel_path,
                    )
                    for row in gap_rows
                ):
                    issues.append(
                        {
                            "reason": "every file-bound finding must cite a GAP or BLOCKED implementation Contract ID and exact range unit when applicable",
                            "source_file": rel_path,
                            "heading": block.get("heading"),
                        }
                    )
        if issues:
            implementation_inventory_issues.append(
                {"batch": batch_id, "report": parsed["report"], "issues": issues}
            )
    interface_inventory_issues = []
    for parsed in parsed_reports:
        batch_id = parsed["batch_id"]
        if not batch_id or batch_id not in expected_by_batch:
            continue
        expected_interface_files = expected_interface_by_batch.get(batch_id, set())
        observed_interface_files = [row["file"] for row in parsed["interface_rows"]]
        observed_interface_set = set(observed_interface_files)
        issues = []
        if parsed["malformed_interface_rows"]:
            issues.append({"reason": "malformed interface inventory rows", "rows": parsed["malformed_interface_rows"]})
        missing_interface_files = sorted(expected_interface_files - observed_interface_set)
        extra_interface_files = sorted(observed_interface_set - expected_interface_files)
        if expected_interface_files:
            if missing_interface_files:
                issues.append({"reason": "missing interface inventory rows", "files": missing_interface_files})
            if extra_interface_files:
                issues.append({"reason": "interface inventory rows for non-interface files", "files": extra_interface_files})
            visible_by_file: dict[str, set[str]] = defaultdict(set)
            for row in parsed["interface_rows"]:
                if row["file"] not in expected_interface_files:
                    continue
                visible_by_file[row["file"]].add(plain_cell(row["visible_text"]))
                for field in ("visible_text", "expected_behavior_path", "actual_implementation_notes"):
                    if is_boilerplate_value(row[field]):
                        issues.append(
                            {
                                "reason": "interface inventory row contains boilerplate text",
                                "file": row["file"],
                                "field": field,
                                "actual": row[field],
                            }
                        )
                expected_path_note = normalized_text(row["expected_behavior_path"])
                implementation_note = normalized_text(row["actual_implementation_notes"])
                if any(phrase in expected_path_note for phrase in GENERIC_INTERFACE_PHRASES) or any(
                    phrase in implementation_note for phrase in GENERIC_INTERFACE_PHRASES
                ):
                    issues.append(
                        {
                            "reason": "interface inventory row must trace behavior to concrete implementation anchors or static metadata rationale",
                            "file": row["file"],
                            "expected_behavior_path": row["expected_behavior_path"],
                            "actual_implementation_notes": row["actual_implementation_notes"],
                        }
                    )
                visible_text = plain_cell(row["visible_text"])
                if (
                    source_text_checks_enabled
                    and normalized_text(visible_text) != "none found"
                    and not source_contains_visible_text(source_text(row["file"]), visible_text)
                ):
                    issues.append(
                        {
                            "reason": "visible text/control/message is not present in the source file",
                            "file": row["file"],
                            "visible_text": row["visible_text"],
                        }
                    )
                if source_text_checks_enabled and repo_root is not None and source_kind_by_file.get(row["file"]) == "source/ui-asset":
                    asset_evidence = f"{row['expected_behavior_path']} {row['actual_implementation_notes']}"
                    metadata = asset_metadata_for(repo_root / row["file"])
                    if not metadata["valid"]:
                        issues.append(
                            {
                                "reason": "source-backed UI asset could not be parsed for verifier-backed metadata",
                                "file": row["file"],
                                "metadata": metadata,
                            }
                        )
                    elif not asset_evidence_matches_metadata(asset_evidence, metadata):
                        issues.append(
                            {
                                "reason": "source-backed UI asset inventory must cite verifier-backed MIME/type and dimensions when available",
                                "file": row["file"],
                                "expected_metadata": metadata,
                                "expected_behavior_path": row["expected_behavior_path"],
                                "actual_implementation_notes": row["actual_implementation_notes"],
                            }
                        )
            if source_text_checks_enabled:
                for rel_path in sorted(expected_interface_files):
                    missing_hints = [
                        hint for hint in visible_text_hints(rel_path, source_text(rel_path))
                        if hint not in visible_by_file.get(rel_path, set())
                    ]
                    if missing_hints:
                        issues.append(
                            {
                                "reason": "interface inventory is missing visible text/control/message hints",
                                "file": rel_path,
                                "missing_visible_text": missing_hints,
                            }
                        )
        else:
            body = parsed["interface_inventory_body"].strip()
            sentinel = "No interface-relevant files in this batch."
            if observed_interface_files:
                issues.append(
                    {
                        "reason": "non-interface batch should not include interface inventory rows",
                        "files": sorted(observed_interface_set),
                    }
                )
            if body != sentinel:
                issues.append(
                    {
                        "reason": "non-interface batch must use exact no-interface sentinel",
                        "expected": sentinel,
                        "actual": parsed["interface_inventory_body"].strip(),
                    }
                )
        if issues:
            interface_inventory_issues.append({"batch": batch_id, "report": parsed["report"], "issues": issues})
    finding_schema_issues = []
    placeholder_omissions = []
    interface_control_omissions = []
    for parsed in parsed_reports:
        batch_id = parsed["batch_id"]
        if not batch_id or batch_id not in expected_by_batch:
            continue
        expected_batch = expected_files_by_batch.get(batch_id, set())
        interface_files = expected_interface_by_batch.get(batch_id, set())
        inventory_visible_by_file: dict[str, set[str]] = defaultdict(set)
        for row in parsed["interface_rows"]:
            inventory_visible_by_file[row["file"]].add(plain_cell(row["visible_text"]))
        binding_issues = validate_finding_bindings(
            parsed,
            expected_batch,
            interface_files,
            source_text if source_text_checks_enabled else None,
            inventory_visible_by_file,
        )
        if binding_issues:
            finding_schema_issues.append({"batch": batch_id, "report": parsed["report"], "issues": binding_issues})
        findings_body = parsed["findings_body"]
        normalized_findings = re.sub(r"[\s.]+", " ", findings_body.strip().lower()).strip()
        finding_files = {
            file_ref
            for block in parsed["finding_blocks"]
            for file_ref in PATH_IN_BACKTICKS_RE.findall(block["fields"].get("files", ""))
        }
        for unit_id in sorted(expected_by_batch[batch_id]):
            if not source_text_checks_enabled:
                continue
            rel_path = expected_unit_to_file.get(unit_id, unit_id)
            unit_text = source_text_for_unit(unit_id)
            markers = placeholder_markers_for(rel_path, unit_text)
            marker_blocks = [
                block
                for block in parsed["finding_blocks"]
                if rel_path in PATH_IN_BACKTICKS_RE.findall(block["fields"].get("files", ""))
            ]
            missing_markers = uncovered_placeholder_markers(marker_blocks, markers)
            if markers and (
                normalized_findings in NO_FINDINGS_SENTINELS
                or rel_path not in finding_files
                or missing_markers
            ):
                placeholder_omissions.append(
                    {
                        "batch": batch_id,
                        "report": parsed["report"],
                        "file": unit_id,
                        "source_file": rel_path,
                        "markers": markers,
                        "missing_markers": missing_markers or markers,
                        "reason": "source file contains placeholder markers but report Findings do not cover the marker details",
                    }
                )
            control_markers = interface_control_markers_for(rel_path, unit_text)
            control_blocks = marker_blocks
            missing_control_markers = uncovered_interface_control_markers(control_blocks, control_markers)
            if control_markers and (
                normalized_findings in NO_FINDINGS_SENTINELS
                or rel_path not in finding_files
                or missing_control_markers
            ):
                interface_control_omissions.append(
                    {
                        "batch": batch_id,
                        "report": parsed["report"],
                        "file": unit_id,
                        "source_file": rel_path,
                        "markers": control_markers,
                        "missing_markers": missing_control_markers or control_markers,
                        "reason": "interface file contains dead, no-op, or unlabeled controls but report Findings do not cover them",
                    }
                )
    missing_sections = [
        {"report": parsed["report"], "sections": parsed["missing_sections"]}
        for parsed in parsed_reports
        if parsed["missing_sections"]
    ]
    section_shape_mismatches = [
        {
            "report": parsed["report"],
            "expected": REQUIRED_SECTION_LIST,
            "actual": parsed["section_order"],
        }
        for parsed in parsed_reports
        if parsed["section_shape_mismatch"]
    ]
    semantic_report_issues = [
        {"report": parsed["report"], "issues": parsed["narrative_issues"]}
        for parsed in parsed_reports
        if parsed["narrative_issues"]
    ]

    batch_mismatches = []
    for parsed in parsed_reports:
        batch_id = parsed["batch_id"]
        if not batch_id or batch_id not in expected_by_batch:
            continue
        expected_batch = expected_by_batch[batch_id]
        observed_batch = {row["file"] for row in parsed["rows"]}
        missing_in_batch = sorted(expected_batch - observed_batch)
        extra_in_batch = sorted(observed_batch - expected_batch)
        unchecked_in_batch = sorted(
            row["file"]
            for row in parsed["rows"]
            if row["file"] in expected_batch and row["status"] != "CHECKED"
        )
        if missing_in_batch or extra_in_batch or unchecked_in_batch:
            batch_mismatches.append(
                {
                    "batch": batch_id,
                    "report": parsed["report"],
                    "missing": missing_in_batch,
                    "extra": extra_in_batch,
                    "unchecked": unchecked_in_batch,
                }
            )

    ok = not any(
        [
            missing,
            extra,
            duplicate,
            unchecked,
            missing_batch_reports,
            duplicate_batch_reports,
            unassigned_reports,
            report_location_mismatches,
            run_id_mismatches,
            report_hash_mismatches,
            current_hash_mismatches,
            current_hash_errors,
            source_text_errors,
            verification_warnings,
            unresolved_scope_warnings,
            excluded_file_mismatches,
            completion_marker_mismatches,
            effort_ledger_mismatches,
            batch_id_mismatches,
            malformed,
            lead_reconciliation_issues,
            implementation_inventory_issues,
            interface_inventory_issues,
            finding_schema_issues,
            placeholder_omissions,
            interface_control_omissions,
            missing_sections,
            section_shape_mismatches,
            semantic_report_issues,
            batch_mismatches,
        ]
    )

    return {
        "expected_count": len(expected),
        "reported_count": len(observed_paths),
        "expected_batch_count": len(known_batch_ids),
        "verification_scope": (
            f"batch:{verification_batch_id}" if verification_batch_id else "complete"
        ),
        "report_files": [str(path) for path in reports],
        "effort_ledger_provenance_note": (
            "Ledger-recorded effort consistency only; scheduler effort settings are not independently verified by this script."
        ),
        "effort_verification_scope": "ledger-recorded",
        "missing": missing,
        "extra": extra,
        "duplicate": duplicate,
        "unchecked": unchecked,
        "missing_batch_reports": missing_batch_reports,
        "duplicate_batch_reports": duplicate_batch_reports,
        "unassigned_reports": unassigned_reports,
        "report_location_mismatches": report_location_mismatches,
        "run_id_mismatches": run_id_mismatches,
        "report_hash_mismatches": report_hash_mismatches,
        "current_hash_mismatches": current_hash_mismatches,
        "current_hash_errors": current_hash_errors,
        "source_text_errors": source_text_errors,
        "current_hash_check_skipped": skip_current_hash_check,
        "verification_warnings": verification_warnings,
        "unresolved_scope_warnings": unresolved_scope_warnings,
        "excluded_file_mismatches": excluded_file_mismatches,
        "completion_marker_mismatches": completion_marker_mismatches,
        "effort_ledger_mismatches": effort_ledger_mismatches,
        "batch_id_mismatches": batch_id_mismatches,
        "malformed_rows": malformed,
        "lead_reconciliation_contract_count": len(lead_reconciliation_rows),
        "lead_reconciliation_issues": lead_reconciliation_issues,
        "implementation_inventory_issues": implementation_inventory_issues,
        "interface_inventory_issues": interface_inventory_issues,
        "finding_schema_issues": finding_schema_issues,
        "placeholder_omissions": placeholder_omissions,
        "interface_control_omissions": interface_control_omissions,
        "missing_sections": missing_sections,
        "section_shape_mismatches": section_shape_mismatches,
        "semantic_report_issues": semantic_report_issues,
        "batch_mismatches": batch_mismatches,
        "ok": ok,
    }


def print_values(key: str, values: list) -> None:
    print(f"{key}: {len(values)}")
    for value in values:
        print(f"  - {value}")


def print_human(result: dict) -> None:
    print(f"Expected files: {result['expected_count']}")
    print(f"Reported files: {result['reported_count']}")
    print(f"Expected batches: {result['expected_batch_count']}")
    print(f"Reports read: {len(result['report_files'])}")
    print(f"Effort ledger provenance: {result['effort_ledger_provenance_note']}")
    for key in (
        "missing",
        "unchecked",
        "duplicate",
        "extra",
        "missing_batch_reports",
        "duplicate_batch_reports",
        "unassigned_reports",
        "report_location_mismatches",
        "run_id_mismatches",
        "report_hash_mismatches",
        "current_hash_mismatches",
        "current_hash_errors",
        "source_text_errors",
        "verification_warnings",
        "unresolved_scope_warnings",
        "excluded_file_mismatches",
        "completion_marker_mismatches",
        "effort_ledger_mismatches",
        "batch_id_mismatches",
        "malformed_rows",
        "lead_reconciliation_issues",
        "implementation_inventory_issues",
        "interface_inventory_issues",
        "finding_schema_issues",
        "placeholder_omissions",
        "interface_control_omissions",
        "missing_sections",
        "section_shape_mismatches",
        "semantic_report_issues",
        "batch_mismatches",
    ):
        print_values(key, result[key])
    print(f"ok: {str(result['ok']).lower()}")


def emit_json(payload: dict) -> None:
    print(json.dumps(payload, indent=2))


def main() -> int:
    args = parse_args()
    manifest_path = Path(args.manifest).expanduser().resolve()
    receipt = None
    receipt_path = None
    if args.receipt_out:
        receipt_manifest = load_manifest(manifest_path)
        receipt_path = prepare_receipt_output(
            args.receipt_out,
            manifest_path=manifest_path,
            manifest=receipt_manifest,
        )
        if args.skip_current_hash_check:
            raise ValueError(
                "--receipt-out cannot be combined with --skip-current-hash-check; "
                "a verification receipt requires current source freshness."
            )
        if args.batch_id:
            raise ValueError("--receipt-out cannot be combined with --batch-id; receipts require complete verification.")
    reports = iter_report_files(args.reports)
    if args.receipt_out:
        result, receipt = verify_with_receipt_data(
            manifest_path,
            reports,
            skip_current_hash_check=args.skip_current_hash_check,
        )
    else:
        result = verify(
            manifest_path,
            reports,
            skip_current_hash_check=args.skip_current_hash_check,
            batch_id=args.batch_id,
        )
    if receipt is not None:
        assert receipt_path is not None
        write_json_atomic(receipt_path, receipt)
    if args.json:
        emit_json(result)
    else:
        print_human(result)
    return 0 if result["ok"] else 1


def wants_json_output(argv: list[str]) -> bool:
    return "--json" in argv


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (FileNotFoundError, OSError, ValueError) as exc:
        if wants_json_output(sys.argv[1:]):
            emit_json({"ok": False, "error": {"type": type(exc).__name__, "message": str(exc)}})
        else:
            print(str(exc), file=sys.stderr)
        raise SystemExit(2)
