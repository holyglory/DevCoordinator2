//! Inventory and verify user-journey documentation audit artifacts.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use ignore::WalkBuilder;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::audit_ledger::{read_bytes_nofollow, validate_directory_nofollow};

const DOC_EXTENSIONS: &[&str] = &["md", "mdx", "markdown", "rst", "adoc", "asciidoc"];
const SOURCE_HINT_EXTENSIONS: &[&str] = &[
    "ts", "tsx", "js", "jsx", "vue", "svelte", "html", "cshtml", "razor", "swift", "xaml", "axaml",
    "kt", "kts", "dart", "m", "mm", "qml",
];
const EXCLUDED_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "__pycache__",
    ".next",
    ".nuxt",
    ".svelte-kit",
    "build",
    "coverage",
    "dist",
    "node_modules",
    "out",
    "target",
    "vendor",
];
const COMMENT_JOURNEY_TERMS: &[&str] = &[
    "journey",
    "user flow",
    "workflow",
    "onboarding",
    "use case",
    "user story",
];
const JOURNEY_TERMS: &[&str] = &[
    "journey",
    "workflow",
    "flow",
    "onboarding",
    "persona",
    "user",
    "route",
    "screen",
    "mobile",
    "desktop",
    "empty state",
    "error",
    "permission",
    "acceptance",
];
const JOURNEY_STRUCTURE_TERMS: &[&str] = &[
    "acceptance",
    "decision",
    "entry point",
    "failure",
    "goal",
    "primary action",
    "route",
    "screen sequence",
    "success",
    "trigger",
];
const EXPLICIT_JOURNEY_TERMS: &[&str] =
    &["journey", "workflow", "user flow", "persona", "scenario"];
const APP_IDEA_TERMS: &[&str] = &[
    "purpose", "overview", "goal", "mission", "product", "app", "value", "problem",
];
const PRODUCT_STRUCTURE_TERMS: &[&str] = &[
    "app idea",
    "goal",
    "mission",
    "problem",
    "purpose",
    "requirements",
    "target user",
    "users",
    "value",
];
const UX_TERMS: &[&str] = &[
    "navigation",
    "priority",
    "hierarchy",
    "accessibility",
    "responsive",
    "mobile",
    "empty",
    "loading",
    "error",
    "undo",
];
const DECISION_MODEL_TERMS: &[&str] = &[
    "action frequency",
    "decision data",
    "decision-making information",
    "journey decision model",
    "primary decision",
    "primary user goal",
    "required facts",
    "required information",
    "unconfirmed assumptions",
    "unresolved assumptions",
    "warning conditions",
    "warning/flag conditions",
];
const INFORMATION_RELEVANCE_TERMS: &[&str] = &[
    "action frequency",
    "conditional",
    "critical always",
    "critical-always",
    "debug",
    "decision importance",
    "expert only",
    "expert-only",
    "frequent action",
    "frequent actions",
    "importance",
    "information relevance",
    "occasional controls",
    "primary frequent",
    "primary-frequent",
    "rare detail",
    "rare controls",
    "rare-under-5-percent",
    "rare/admin/configuration controls",
    "relative importance",
    "secondary occasional",
    "secondary-occasional",
];
const UI_HANDOFF_TERMS: &[&str] = &[
    "dom measurement",
    "evidence expectations",
    "mockup",
    "rendered state",
    "screenshot",
    "states to verify",
    "ui audit constraint",
    "ui audit constraints",
    "test mode",
    "ui audit handoff",
    "ui handoff constraints",
    "ui implementation audit",
    "viewport measurement",
    "visual audit",
];
const FORMAL_PRIORITY_TERMS: &[&str] = &[
    "primary journey",
    "usage percentage",
    "frequency percent",
    "priority override reason",
    "risk if broken",
];
const FORMAL_HIERARCHY_TERMS: &[&str] = &[
    "initial viewport",
    "primary-content",
    "workflow-surface",
    "blocking-alert",
    "supporting region",
];
const FORMAL_CONTINUATION_TERMS: &[&str] = &[
    "continuation anchor",
    "focus enters",
    "document scroll",
    "expected destination route",
    "without scrolling",
];
const FORMAL_THEME_TERMS: &[&str] = &["theme intent", "light, dark, or mixed", "declared theme"];
const FORMAL_REVIEW_INPUT_TERMS: &[&str] = &[
    "review inputs",
    "ui code, styles, tokens, fonts, and assets",
    "implementation-input ownership",
    "changed visual review",
];
const FORMAL_SCREENSHOT_TERMS: &[&str] = &[
    "initial-viewport screenshot",
    "full-page screenshot",
    "screenshot pair",
];
const FEATURE_TERMS: &[&str] = &[
    "capability",
    "capabilities",
    "feature",
    "features",
    "feature inventory",
    "requirement",
    "requirements",
];
const UI_ELEMENT_TERMS: &[&str] = &[
    "badge",
    "banner",
    "button",
    "control",
    "controls",
    "field",
    "form",
    "menu",
    "screen",
    "state",
    "toast",
    "ui element",
    "ui elements",
];
const IMPLEMENTATION_TERMS: &[&str] = &[
    "api",
    "data path",
    "handler",
    "implementation expectation",
    "implementation expectations",
    "permission",
    "persistence",
    "state change",
    "validation",
];
const TEST_TERMS: &[&str] = &[
    "acceptance criteria",
    "component test",
    "e2e",
    "fixture",
    "qa",
    "test",
    "test expectation",
    "test expectations",
    "test mode",
    "unit test",
    "visual test",
];
const LOW_FREQUENCY_TERMS: &[&str] = &["admin", "configuration", "filter", "filters", "settings"];
const PRIMARY_CONTENT_TERMS: &[&str] = &[
    "chart",
    "dashboard",
    "decision",
    "metric",
    "metrics",
    "primary content",
    "summary",
];
const MOBILE_TERMS: &[&str] = &[
    "mobile",
    "narrow",
    "phone",
    "responsive",
    "screen",
    "viewport",
];
const UI_INTENT_TERMS: &[&str] = &[
    "command center",
    "compact",
    "dashboard",
    "dense",
    "expert ui",
    "overview",
];
const VISIBLE_PRESCRIPTION_TERMS: &[&str] = &[
    "always visible",
    "default visible",
    "must display",
    "must list",
    "must show",
    "required visible decision evidence",
    "required visible evidence",
    "show by default",
    "shown by default",
    "visible by default",
    "visible decision evidence",
];
const DETAIL_HEAVY_TERMS: &[&str] = &[
    "debug",
    "detail",
    "details",
    "evidence",
    "gap breakdown",
    "metadata",
    "next action",
    "owner",
    "raw",
    "raw log",
    "raw status",
    "secondary detail",
    "severity summary",
    "stage",
    "status summary",
];
const INTERACTION_SURFACE_TERMS: &[&str] = &[
    "badge",
    "badges",
    "flag",
    "flags",
    "message",
    "messages",
    "panel",
    "panels",
    "popover",
    "popovers",
    "flyout",
    "flyouts",
    "row",
    "rows",
    "tool block",
    "tool blocks",
    "result block",
    "result blocks",
    "expand",
    "collapse",
    "disclosure",
];
const DISCLOSURE_TERMS: &[&str] = &[
    "click",
    "click target",
    "clickable row",
    "detail path",
    "detail state",
    "dialog",
    "disclosure",
    "drill-down",
    "drill down",
    "drawer",
    "expand",
    "expansion",
    "focus",
    "hover",
    "interactive badge",
    "interactive badges",
    "on demand",
    "popover",
    "row selection",
    "whole row",
    "whole-row",
    "tooltip",
];
const TRANSIENT_TERMS: &[&str] = &[
    "auto collapse",
    "auto-collapse",
    "close on",
    "dismiss",
    "dismissal",
    "focus loss",
    "idle",
    "leave timer",
    "lifecycle",
    "outside click",
    "persistent while hovered",
    "timeout",
];
const NAVIGATION_SURFACE_TERMS: &[&str] = &[
    "destination",
    "go to",
    "jump to",
    "navigate",
    "navigation",
    "open screen",
    "scroll to",
];
const NAVIGATION_AFFORDANCE_TERMS: &[&str] = &[
    "cursor",
    "focus affordance",
    "pointer",
    "predictable destination",
    "target surface",
];
const COPY_TERMS: &[&str] = &["copy", "copy button", "copy control", "clipboard"];
const HOVER_COPY_TERMS: &[&str] = &[
    "hover copy",
    "hover-revealed",
    "hover revealed",
    "reachable",
    "stable position",
    "stay visible",
];
const STATUS_TERMS: &[&str] = &[
    "concise status",
    "duplicate status",
    "duration",
    "error count",
    "result summary",
    "status summary",
    "tool status",
];
const STATUS_MODEL_TERMS: &[&str] = &[
    "default summary",
    "detail only",
    "hidden by default",
    "moved to detail",
    "only when",
    "success indicator",
];
const MESSAGE_METADATA_TERMS: &[&str] = &[
    "author",
    "authorship",
    "message metadata",
    "metadata",
    "routing label",
    "sender",
    "sender label",
    "timestamp",
    "unselectable",
];
const LOW_IMPORTANCE_TERMS: &[&str] = &[
    "conditional",
    "debug",
    "expert-only",
    "expert only",
    "low-frequency",
    "occasional",
    "rare",
    "rare-under-5-percent",
    "secondary",
    "secondary-occasional",
];

static COMMENT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?://+|#+|\*+|/\*+|<!--)").expect("constant comment regex"));
static HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(#{1,6})\s+(.+?)\s*$").expect("constant heading regex"));
static GITMODULE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*path\s*=\s*(.+?)\s*$").expect("constant gitmodule regex")
});
static ROUTE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(?:route|path|href|to)\s*[:=]\s*[\"']([^\"']+)[\"']"#)
        .expect("constant route regex")
});
static VISIBLE_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)>[^<]{3,}<|aria-label\s*=|placeholder\s*=|title\s*=",
        r#"\b(?:Text|Label|Button|Toggle|TextField|SecureField|navigationTitle|accessibilityLabel)\s*\(\s*[\"'][^\"']{2,}[\"']"#,
        r#"(?i)\b(?:Text|Content|Header|Title|Placeholder|AutomationProperties\.Name)\s*=\s*[\"'][^\"']{2,}[\"']"#,
    ]
    .into_iter()
    .map(|pattern| Regex::new(pattern).expect("constant visible-text regex"))
    .collect()
});
static DECISION_DETAIL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^d-\d{8}-\d{2}\.md$").expect("constant decision detail regex"));
static PLACEHOLDER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:todo|tbd|placeholder|fill\s+this|lorem\s+ipsum)\b|<[^>]+>")
        .expect("constant placeholder regex")
});

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DocRecord {
    pub path: String,
    pub kind: String,
    pub headings: Vec<String>,
    pub app_idea_hits: usize,
    pub journey_hits: usize,
    pub ux_hits: usize,
    pub decision_model_hits: usize,
    pub information_relevance_hits: usize,
    pub ui_handoff_constraint_hits: usize,
    pub prescriptive_ui_risk: bool,
    pub likely_journey_doc: bool,
    pub likely_product_doc: bool,
    pub likely_decision_model_doc: bool,
    pub likely_information_relevance_doc: bool,
    pub likely_ui_handoff_constraint_doc: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceHint {
    pub path: String,
    pub routes: Vec<String>,
    pub visible_text_hits: usize,
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_lowercase()
}

fn read_text(root: &Path, path: &Path) -> String {
    read_bytes_nofollow(path, Some(root))
        .ok()
        .flatten()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

fn count_terms(text: &str, terms: &[&str]) -> usize {
    let lowered = text.to_lowercase();
    terms
        .iter()
        .map(|term| {
            let expression = term
                .to_lowercase()
                .split_whitespace()
                .map(regex::escape)
                .collect::<Vec<_>>()
                .join(r"\s+");
            Regex::new(&expression)
                .expect("escaped term regex")
                .find_iter(&lowered)
                .filter(|found| {
                    let before = found
                        .start()
                        .checked_sub(1)
                        .and_then(|index| lowered.as_bytes().get(index))
                        .is_none_or(|byte| !byte.is_ascii_alphanumeric());
                    let after = lowered
                        .as_bytes()
                        .get(found.end())
                        .is_none_or(|byte| !byte.is_ascii_alphanumeric());
                    before && after
                })
                .count()
        })
        .sum()
}

fn has(text: &str, terms: &[&str]) -> bool {
    count_terms(text, terms) > 0
}

fn policy_or_governance(rel_path: &str) -> bool {
    let path = Path::new(rel_path);
    let parts = rel_path.split('/').collect::<Vec<_>>();
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_lowercase();
    matches!(
        name.as_str(),
        "agents.md" | "claude.md" | "decisionhistory.md"
    ) || (parts.len() == 2
        && parts[0].eq_ignore_ascii_case("DecisionDetails")
        && DECISION_DETAIL_RE.is_match(&name))
}

fn operational(rel_path: &str, text: &str) -> bool {
    let parts = rel_path.split('/').collect::<Vec<_>>();
    (parts.len() >= 2 && parts[0] == "skills")
        || policy_or_governance(rel_path)
        || [
            "agent skill",
            "agent skills",
            "skill directory",
            "skill directories",
            "skills/",
            "full_repo_harness",
            "scripts/validate.py",
        ]
        .iter()
        .any(|marker| text.to_lowercase().contains(marker))
}

fn likely_journey(rel: &str, text: &str, hits: usize) -> bool {
    if operational(rel, text) {
        return false;
    }
    let structure = JOURNEY_STRUCTURE_TERMS
        .iter()
        .filter(|term| has(text, &[*term]))
        .count();
    (has(text, EXPLICIT_JOURNEY_TERMS) && structure >= 1) || (hits >= 3 && structure >= 2)
}

fn likely_product(rel: &str, text: &str, hits: usize) -> bool {
    if operational(rel, text) {
        return false;
    }
    let structure = PRODUCT_STRUCTURE_TERMS
        .iter()
        .filter(|term| has(text, &[*term]))
        .count();
    (has(text, &["product", "requirements", "app idea", "overview"]) && structure >= 2) || hits >= 4
}

fn likely_decision(rel: &str, text: &str) -> bool {
    if operational(rel, text) {
        return false;
    }
    let goal = has(text, &["primary user goal", "primary goal", "goal"]);
    let decision = has(
        text,
        &[
            "primary decision",
            "decision data",
            "decision-making information",
        ],
    );
    let facts = has(
        text,
        &[
            "required facts",
            "required information",
            "warning conditions",
            "warning/flag conditions",
        ],
    );
    let frequency = has(
        text,
        &[
            "action frequency",
            "frequent action",
            "frequent actions",
            "occasional controls",
            "rare controls",
            "rare/admin/configuration controls",
        ],
    );
    goal && decision && facts && (has(text, &["journey decision model"]) || frequency)
}

fn likely_relevance(rel: &str, text: &str) -> bool {
    if operational(rel, text) {
        return false;
    }
    let explicit = has(
        text,
        &[
            "information relevance",
            "critical-always",
            "primary-frequent",
            "secondary-occasional",
            "rare-under-5-percent",
        ],
    );
    let decision = has(
        text,
        &[
            "primary decision",
            "decision data",
            "required facts",
            "required information",
        ],
    );
    let frequency = has(
        text,
        &[
            "action frequency",
            "frequent",
            "occasional",
            "rare",
            "conditional",
        ],
    );
    let classes = has(
        text,
        &[
            "critical-always",
            "primary-frequent",
            "secondary-occasional",
            "rare-under-5-percent",
            "expert-only",
            "debug",
        ],
    );
    (explicit && classes) || (decision && frequency && classes)
}

fn likely_handoff(rel: &str, text: &str) -> bool {
    if operational(rel, text) {
        return false;
    }
    has(
        text,
        &[
            "ui handoff constraints",
            "ui implementation audit",
            "ui audit handoff",
        ],
    ) || (has(
        text,
        &[
            "mockup",
            "screenshot",
            "viewport measurement",
            "rendered state",
            "states to verify",
        ],
    ) && likely_decision(rel, text)
        && likely_relevance(rel, text))
}

fn prescriptive_risk(rel: &str, text: &str) -> bool {
    if operational(rel, text)
        || !has(text, VISIBLE_PRESCRIPTION_TERMS)
        || !has(text, DETAIL_HEAVY_TERMS)
        || !(has(text, UI_INTENT_TERMS) || text.to_lowercase().contains("required visible"))
    {
        return false;
    }
    text.to_lowercase()
        .split("\n\n")
        .flat_map(|block| block.split(['.', '!', '?']))
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .any(|segment| {
            let negated = Regex::new(
                r"\b(?:not|never|does not|do not|without)\b[^.\n]{0,80}\balways visible\b",
            )
            .expect("constant negation regex")
            .is_match(segment);
            !negated
                && has(segment, VISIBLE_PRESCRIPTION_TERMS)
                && has(segment, DETAIL_HEAVY_TERMS)
                && (!has(segment, DISCLOSURE_TERMS)
                    || (has(text, LOW_IMPORTANCE_TERMS)
                        && !has(segment, &["critical-always", "primary-frequent"])))
        })
}

fn classify(rel: &str, text: &str, journey_hits: usize) -> &'static str {
    let lower = rel.to_lowercase();
    if policy_or_governance(rel) {
        "policy-or-decision-history"
    } else if lower.contains("readme") {
        "readme"
    } else if [
        "journey",
        "workflow",
        "flow",
        "persona",
        "product",
        "spec",
        "feature",
        "requirement",
    ]
    .iter()
    .any(|term| lower.contains(term))
    {
        "product-doc"
    } else if ["architecture", "design", "adr"]
        .iter()
        .any(|term| lower.contains(term))
    {
        "architecture-doc"
    } else if lower.contains("test") || lower.contains("qa") {
        "test-doc"
    } else if likely_journey(rel, text, journey_hits) {
        "journey-candidate"
    } else {
        "doc"
    }
}

fn doc_record(repo: &Path, path: &Path) -> DocRecord {
    let text = read_text(repo, path);
    let rel = path
        .strip_prefix(repo)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let app = count_terms(&text, APP_IDEA_TERMS);
    let journey = count_terms(&text, JOURNEY_TERMS);
    DocRecord {
        path: rel.clone(),
        kind: classify(&rel, &text, journey).to_owned(),
        headings: text
            .lines()
            .filter_map(|line| HEADING_RE.captures(line))
            .filter_map(|captures| captures.get(2))
            .map(|value| value.as_str().trim().to_owned())
            .take(20)
            .collect(),
        app_idea_hits: app,
        journey_hits: journey,
        ux_hits: count_terms(&text, UX_TERMS),
        decision_model_hits: count_terms(&text, DECISION_MODEL_TERMS),
        information_relevance_hits: count_terms(&text, INFORMATION_RELEVANCE_TERMS),
        ui_handoff_constraint_hits: count_terms(&text, UI_HANDOFF_TERMS),
        prescriptive_ui_risk: prescriptive_risk(&rel, &text),
        likely_journey_doc: likely_journey(&rel, &text, journey),
        likely_product_doc: likely_product(&rel, &text, app),
        likely_decision_model_doc: likely_decision(&rel, &text),
        likely_information_relevance_doc: likely_relevance(&rel, &text),
        likely_ui_handoff_constraint_doc: likely_handoff(&rel, &text),
    }
}

fn source_hint(repo: &Path, path: &Path) -> Option<SourceHint> {
    let text = read_text(repo, path);
    let routes = ROUTE_RE
        .captures_iter(&text)
        .filter_map(|captures| captures.get(1))
        .map(|value| value.as_str().to_owned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(30)
        .collect::<Vec<_>>();
    let visible = VISIBLE_PATTERNS
        .iter()
        .map(|pattern| pattern.find_iter(&text).count())
        .sum();
    (!routes.is_empty() || visible > 0).then(|| SourceHint {
        path: path
            .strip_prefix(repo)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/"),
        routes,
        visible_text_hits: visible,
    })
}

fn submodules(repo: &Path) -> BTreeSet<String> {
    let text = read_text(repo, &repo.join(".gitmodules"));
    GITMODULE_RE
        .captures_iter(&text)
        .filter_map(|captures| captures.get(1))
        .map(|value| value.as_str().trim().trim_matches('/').to_owned())
        .collect()
}

fn files(repo: &Path) -> Vec<PathBuf> {
    let root = repo.to_path_buf();
    let submodules = submodules(repo);
    let filter_root = root.clone();
    let mut builder = WalkBuilder::new(repo);
    builder
        .standard_filters(false)
        .follow_links(false)
        .sort_by_file_path(|left, right| left.cmp(right));
    builder.filter_entry(move |entry| {
        if entry.depth() == 0 || !entry.file_type().is_some_and(|kind| kind.is_dir()) {
            return true;
        }
        let name = entry.file_name().to_string_lossy();
        if EXCLUDED_DIRS.contains(&name.as_ref()) {
            return false;
        }
        let rel = entry
            .path()
            .strip_prefix(&filter_root)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .replace('\\', "/");
        if submodules
            .iter()
            .any(|module| rel == *module || rel.starts_with(&format!("{module}/")))
        {
            return false;
        }
        entry.path() == filter_root || !entry.path().join(".git").exists()
    });
    builder
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .map(|entry| entry.path().to_owned())
        .collect()
}

fn comment_journey_hits(text: &str) -> usize {
    text.lines()
        .filter(|line| COMMENT_RE.is_match(line))
        .filter(|line| {
            let lower = line.to_lowercase();
            COMMENT_JOURNEY_TERMS
                .iter()
                .any(|term| lower.contains(term))
        })
        .count()
}

pub fn build_inventory(repo: &Path) -> Result<Value, String> {
    let repo = validate_directory_nofollow(repo).map_err(|error| error.to_string())?;
    let mut docs = Vec::new();
    let mut hints = Vec::new();
    let mut source_comment_journey_hits = 0usize;
    for path in files(&repo) {
        let ext = extension(&path);
        if DOC_EXTENSIONS.contains(&ext.as_str()) {
            docs.push(doc_record(&repo, &path));
        } else if SOURCE_HINT_EXTENSIONS.contains(&ext.as_str()) {
            let text = read_text(&repo, &path);
            source_comment_journey_hits += comment_journey_hits(&text);
            if let Some(hint) = source_hint(&repo, &path) {
                hints.push(hint);
            }
        }
    }
    docs.sort_by(|left, right| left.path.cmp(&right.path));
    hints.sort_by(|left, right| left.path.cmp(&right.path));
    hints.truncate(100);
    let product_count = docs.iter().filter(|doc| doc.likely_product_doc).count();
    let journey_count = docs.iter().filter(|doc| doc.likely_journey_doc).count();
    let decision_count = docs
        .iter()
        .filter(|doc| doc.likely_decision_model_doc)
        .count();
    let relevance_count = docs
        .iter()
        .filter(|doc| doc.likely_information_relevance_doc)
        .count();
    let handoff_count = docs
        .iter()
        .filter(|doc| doc.likely_ui_handoff_constraint_doc)
        .count();
    let risk_count = docs.iter().filter(|doc| doc.prescriptive_ui_risk).count();
    let doc_texts = docs
        .iter()
        .filter_map(|doc| {
            let text = read_text(&repo, &repo.join(&doc.path));
            (!operational(&doc.path, &text)).then(|| text.to_lowercase())
        })
        .collect::<Vec<_>>();
    let any = |terms: &[&str]| doc_texts.iter().any(|text| has(text, terms));
    let feature = any(FEATURE_TERMS);
    let ui_elements = any(UI_ELEMENT_TERMS);
    let implementation = any(IMPLEMENTATION_TERMS);
    let tests = any(TEST_TERMS);
    let interaction_surface = any(INTERACTION_SURFACE_TERMS);
    let disclosure = any(DISCLOSURE_TERMS);
    let transient = any(TRANSIENT_TERMS);
    let navigation = any(NAVIGATION_SURFACE_TERMS);
    let navigation_affordance = any(NAVIGATION_AFFORDANCE_TERMS);
    let copy = any(COPY_TERMS);
    let hover_copy = any(HOVER_COPY_TERMS);
    let status = any(STATUS_TERMS);
    let status_model = any(STATUS_MODEL_TERMS);
    let formal_parts = json!({
        "priority":any(FORMAL_PRIORITY_TERMS),"hierarchy":any(FORMAL_HIERARCHY_TERMS),
        "continuation":any(FORMAL_CONTINUATION_TERMS),"theme":any(FORMAL_THEME_TERMS),
        "review_inputs":any(FORMAL_REVIEW_INPUT_TERMS),"screenshots":any(FORMAL_SCREENSHOT_TERMS),
    });
    let formal = formal_parts
        .as_object()
        .expect("object")
        .values()
        .all(|value| value == &json!(true));
    let interaction_model = (!interaction_surface || (disclosure && transient))
        && (!navigation || navigation_affordance)
        && (!copy || hover_copy)
        && (!status || status_model);
    let mut missing = Vec::new();
    let mut risks = Vec::new();
    if product_count == 0 {
        missing.push("No strong app idea/product overview documentation detected.".to_owned());
    }
    if journey_count == 0 {
        missing.push("No strong user journey/workflow/persona documentation detected.".to_owned());
        if source_comment_journey_hits > 0 {
            missing.push(format!("Journey descriptions appear only in source comments ({source_comment_journey_hits} hit(s)); move them into product/journey docs so they can be reviewed and kept current."));
        }
    }
    if docs.iter().map(|doc| doc.ux_hits).sum::<usize>() < 3 {
        missing.push("Very little UI/UX hierarchy, navigation, responsive, or accessibility documentation detected.".to_owned());
    }
    for (present, signal) in [
        (
            feature,
            "No complete feature inventory documentation detected.",
        ),
        (
            ui_elements,
            "No required UI element inventory documentation detected.",
        ),
        (
            implementation,
            "No implementation expectation documentation detected.",
        ),
        (tests, "No test expectation documentation detected."),
    ] {
        if !docs.is_empty() && !present {
            missing.push(signal.to_owned());
        }
    }
    if !docs.is_empty() && (!feature || !ui_elements || !implementation || !tests) {
        risks.push("Docs do not fully define required features, UI elements, implementation expectations, and test expectations.".to_owned());
    }
    if !docs.is_empty() && decision_count == 0 {
        missing.push("No journey decision model documentation detected.".to_owned());
        risks.push("Docs do not define primary goal, primary decision, required facts, warning/flag conditions, action frequency, rare details, and unresolved assumptions for UI implementation.".to_owned());
    }
    if !docs.is_empty() && relevance_count == 0 {
        missing.push("No information relevance inventory documentation detected.".to_owned());
        risks.push("Docs do not classify decision information, warnings, actions, and details as critical-always, primary-frequent, secondary-occasional, rare, conditional, debug, or expert-only.".to_owned());
    }
    if any(MOBILE_TERMS)
        && any(LOW_FREQUENCY_TERMS)
        && any(PRIMARY_CONTENT_TERMS)
        && relevance_count == 0
    {
        risks.push("Docs mention constrained screens plus settings/filters/configuration and primary content, but do not define the decision/relevance model; the UI audit must judge rendered placement rather than inheriting a layout guess.".to_owned());
    }
    if any(UI_INTENT_TERMS) && (decision_count == 0 || relevance_count == 0) {
        risks.push("Docs use UI intent terms such as dense, dashboard, command center, overview, compact, or expert UI without defining the decisions, relevance, action frequency, and assumptions behind that intent.".to_owned());
    }
    if !docs.is_empty() && interaction_surface && !disclosure {
        missing.push("No interaction access model detected for badges, flags, rows, messages, or disclosure surfaces.".to_owned());
        risks.push("Docs mention badges, flags, messages, rows, tool/result blocks, or disclosure surfaces without defining click/hover/focus targets, popover/detail access, or stable expanded/collapsed behavior.".to_owned());
    }
    if !docs.is_empty() && interaction_surface && !transient {
        risks.push("Docs mention interactive or disclosure surfaces without defining a transient panel lifecycle such as explicit close, outside click, focus loss, idle/leave timeout, or documented persistence.".to_owned());
    }
    if !docs.is_empty() && navigation && !navigation_affordance {
        risks.push("Docs mention navigation or cross-surface jumps without defining predictable destinations and pointer/focus affordance for navigational elements.".to_owned());
    }
    if !docs.is_empty() && copy && !hover_copy {
        risks.push(
            "Docs mention copy or clipboard controls without defining whether copy affordances are hidden until hover/focus, where they appear, and how they remain reachable."
                .to_owned(),
        );
    }
    if !docs.is_empty() && status && !status_model {
        risks.push("Docs mention tool/result/status summaries without defining which status, error count, duration, success, or severity signals belong in the concise default state versus detail.".to_owned());
    }
    if !docs.is_empty() && any(MESSAGE_METADATA_TERMS) && relevance_count == 0 {
        risks.push("Docs mention message metadata such as sender labels, routing labels, timestamps, or authorship without classifying whether it is decision content or passive metadata.".to_owned());
    }
    if risk_count > 0 {
        risks.push("Docs appear to turn UI handoff evidence into always-visible layout requirements for lower-importance information; classify decision importance and define inline, hint, selection, expansion, drawer, modal, or detail-view access before treating the docs as UI-audit ready.".to_owned());
    }
    if !hints.is_empty() && decision_count == 0 {
        risks.push("Source hints expose UI surfaces, but docs do not provide a journey decision model; source hints must not substitute for product truth.".to_owned());
    }
    if !docs.is_empty() && !formal {
        missing.push(
            "No complete formal Web UI verification handoff documentation detected.".to_owned(),
        );
        let missing_parts = formal_parts
            .as_object()
            .expect("object")
            .iter()
            .filter_map(|(name, value)| (value != &json!(true)).then_some(name.as_str()))
            .collect::<Vec<_>>();
        risks.push(format!(
            "Docs do not completely define primary journey frequency/risk, initial-viewport region roles, visible/focused continuation, light/dark/mixed theme intent, implementation review-input ownership, and initial/full-page changed-review evidence. Missing handoff parts: {}.",
            missing_parts.join(", ")
        ));
    }
    let ready = decision_count > 0
        && relevance_count > 0
        && handoff_count > 0
        && formal
        && risk_count == 0
        && interaction_model;
    Ok(json!({
        "repo_root":repo.to_string_lossy(),"doc_count":docs.len(),
        "journey_doc_count":journey_count,"product_doc_count":product_count,
        "decision_model_doc_count":decision_count,"information_relevance_doc_count":relevance_count,
        "ui_handoff_constraint_doc_count":handoff_count,"prescriptive_ui_risk_doc_count":risk_count,
        "ui_audit_handoff_ready":ready,"has_feature_inventory":feature,
        "has_ui_element_inventory":ui_elements,"has_implementation_expectations":implementation,
        "has_test_expectations":tests,"has_formal_verification_handoff":formal,
        "formal_verification_handoff_parts":formal_parts,"has_interaction_access_model":interaction_model,
        "source_hint_count":hints.len(),"source_comment_journey_hits":source_comment_journey_hits,
        "docs":docs,"source_hints":hints,"missing_signals":missing,
        "ui_implementation_risk_signals":risks,
    }))
}

pub fn render_inventory(inventory: &Value) -> String {
    let mut lines = vec![
        "# Journey Documentation Inventory".to_owned(),
        String::new(),
        format!(
            "Repo root: `{}`",
            inventory["repo_root"].as_str().unwrap_or("")
        ),
        format!("Docs: **{}**", inventory["doc_count"]),
        format!(
            "Likely journey docs: **{}**",
            inventory["journey_doc_count"]
        ),
        format!(
            "Likely product docs: **{}**",
            inventory["product_doc_count"]
        ),
        format!(
            "Journey decision model docs: **{}**",
            inventory["decision_model_doc_count"]
        ),
        format!(
            "Information relevance docs: **{}**",
            inventory["information_relevance_doc_count"]
        ),
        format!(
            "UI handoff constraint docs: **{}**",
            inventory["ui_handoff_constraint_doc_count"]
        ),
        format!(
            "UI audit handoff ready: **{}**",
            inventory["ui_audit_handoff_ready"]
        ),
        String::new(),
    ];
    for (heading, field) in [
        ("Missing Signals", "missing_signals"),
        (
            "UI Implementation Risk Signals",
            "ui_implementation_risk_signals",
        ),
    ] {
        if let Some(values) = inventory[field]
            .as_array()
            .filter(|values| !values.is_empty())
        {
            lines.push(format!("## {heading}"));
            lines.extend(
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|value| format!("- {value}")),
            );
            lines.push(String::new());
        }
    }
    lines.push("## Docs".to_owned());
    lines.push("| File | Kind | Headings | Journey hits | UX hits | Decision model hits | Relevance hits | Handoff hits |".to_owned());
    lines.push("| --- | --- | --- | ---: | ---: | ---: | ---: | ---: |".to_owned());
    if let Some(docs) = inventory["docs"].as_array() {
        for doc in docs {
            let headings = doc["headings"]
                .as_array()
                .into_iter()
                .flatten()
                .take(5)
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("; ")
                .replace('|', "/");
            lines.push(format!(
                "| `{}` | {} | {} | {} | {} | {} | {} | {} |",
                doc["path"].as_str().unwrap_or(""),
                doc["kind"].as_str().unwrap_or(""),
                if headings.is_empty() {
                    "None"
                } else {
                    &headings
                },
                doc["journey_hits"],
                doc["ux_hits"],
                doc["decision_model_hits"],
                doc["information_relevance_hits"],
                doc["ui_handoff_constraint_hits"],
            ));
        }
    }
    lines.join("\n") + "\n"
}

pub const REQUIRED_REPORT_HEADINGS: [&str; 8] = [
    "Coverage",
    "Confirmation Status",
    "Journey Findings",
    "Decision And Information Gaps",
    "Documentation Findings",
    "Interaction Affordance And Metadata Gaps",
    "Recommended Documentation Plan",
    "Readiness And Open Questions",
];
const REPORT_INTERACTION_TERMS: &[&str] = &[
    "activation target",
    "focus",
    "destination",
    "disclosure lifecycle",
    "detail access",
    "scrollbar",
    "stable dimension",
    "hover-copy",
    "concise status",
    "icon meaning",
    "passive metadata",
];
const REPORT_FORMAL_TERMS: &[&str] = &[
    "primary journey",
    "usage percentage",
    "risk if broken",
    "initial viewport",
    "continuation anchor",
    "theme intent",
    "review inputs",
    "initial-viewport screenshot",
    "full-page screenshot",
    "changed visual review",
];

pub fn verify_report(text: &str) -> Vec<String> {
    let heading_re = Regex::new(r"(?m)^##\s+(.+?)\s*$").expect("constant report heading regex");
    let matches = heading_re.find_iter(text).collect::<Vec<_>>();
    let mut order = Vec::new();
    let mut bodies = std::collections::BTreeMap::new();
    for (index, found) in matches.iter().enumerate() {
        let heading = heading_re
            .captures(found.as_str())
            .and_then(|captures| captures.get(1))
            .map(|value| value.as_str().trim().to_owned())
            .unwrap_or_default();
        let end = matches
            .get(index + 1)
            .map_or(text.len(), |next| next.start());
        order.push(heading.clone());
        bodies.insert(heading, text[found.end()..end].trim().to_owned());
    }
    let mut issues = Vec::new();
    if order != REQUIRED_REPORT_HEADINGS {
        let missing = REQUIRED_REPORT_HEADINGS
            .iter()
            .filter(|heading| !order.iter().any(|actual| actual == **heading))
            .collect::<Vec<_>>();
        issues.push(format!(
            "top-level headings must exactly match the required order; missing={missing:?}"
        ));
    }
    for heading in REQUIRED_REPORT_HEADINGS {
        let body = bodies.get(heading).map(String::as_str).unwrap_or("");
        if body.is_empty() {
            issues.push(format!("section is missing or empty: {heading}"));
        } else if PLACEHOLDER_RE.is_match(body) {
            issues.push(format!("section contains placeholder text: {heading}"));
        }
    }
    let coverage = bodies.get("Coverage").map(String::as_str).unwrap_or("");
    let confirmation = bodies
        .get("Confirmation Status")
        .map(String::as_str)
        .unwrap_or("");
    let journeys = bodies
        .get("Journey Findings")
        .map(String::as_str)
        .unwrap_or("");
    let readiness = bodies
        .get("Readiness And Open Questions")
        .map(String::as_str)
        .unwrap_or("");
    let interaction = bodies
        .get("Interaction Affordance And Metadata Gaps")
        .map(|value| value.to_lowercase())
        .unwrap_or_default();
    if !Regex::new(r"(?i)\b(confirmed|unconfirmed|unavailable|declined)\b")
        .unwrap()
        .is_match(confirmation)
        || !Regex::new(r"(?i)\b(user|docs?|documentation)\b")
            .unwrap()
            .is_match(confirmation)
    {
        issues.push("Confirmation Status must state confirmed/unconfirmed status and identify user or documentation as the source".to_owned());
    }
    if !journeys.is_empty()
        && !Regex::new(r"(?i)\b(confirmed|draft-needs-user-confirmation|draft|rejected|blocked)\b")
            .unwrap()
            .is_match(journeys)
    {
        issues.push("Journey Findings must label each journey confirmed, draft-needs-user-confirmation, rejected, or blocked".to_owned());
    }
    if coverage
        .to_lowercase()
        .contains("journey assumptions unconfirmed")
    {
        if !readiness
            .to_lowercase()
            .contains("journey assumptions unconfirmed")
        {
            issues.push("Readiness And Open Questions must repeat 'journey assumptions unconfirmed' when coverage is unconfirmed".to_owned());
        }
    } else if !coverage.to_lowercase().contains("confirmed") {
        issues.push(
            "Coverage must label journey assumptions confirmed or use the exact unconfirmed phrase"
                .to_owned(),
        );
    }
    if !interaction.contains("no relevant interaction or message-metadata surfaces documented.") {
        let missing = REPORT_INTERACTION_TERMS
            .iter()
            .filter(|term| !interaction.contains(**term))
            .copied()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            issues.push(format!(
                "Interaction Affordance And Metadata Gaps omits required checks: {missing:?}"
            ));
        }
    }
    let report = text.to_lowercase();
    if !report.contains(
        "formal web ui verification handoff not applicable: no web ui surfaces documented.",
    ) {
        let missing = REPORT_FORMAL_TERMS
            .iter()
            .filter(|term| !report.contains(**term))
            .copied()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            issues.push(format!(
                "report omits formal Web UI verification handoff checks: {missing:?}"
            ));
        }
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, relative: &str, text: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    const COMPLETE: &str = r#"# Metrics Dashboard Journey Documentation

## App Idea
Product purpose: help operations analysts understand live metrics.

## Journey Inventory
Primary journey: Review live metrics; usage percentage: 95%; risk if broken: high.
The entry point is /metrics and success means a current-status decision.

## Journey Decision Model
Primary user goal: understand metrics. Primary decision: act or monitor.
Required facts and warning conditions are named. Frequent actions inspect details;
rare/admin/configuration controls cover advanced settings. Unresolved assumptions: none.

## Information Relevance Inventory
The metric list is critical-always and primary-frequent. Filters are
secondary-occasional. Debug settings are rare-under-5-percent and expert-only.
Action frequency and information relevance determine placement.

## Feature Inventory
Features and capabilities include metrics, warnings, retry, and empty state.

## UI Element Inventory
UI elements include a badge, button, field, form, menu, toast, and state.

## Implementation Expectations
Handlers, API data path, persistence, validation, permission, and state change are required.

## Test Expectations
Acceptance criteria, unit test, component test, e2e, fixture, test mode, and visual test are required.

## UI Handoff Constraints
Use a mockup, screenshot, viewport measurement, rendered state, and states to verify
for the UI implementation audit and UI audit handoff.

## Interaction And Metadata Model
The badge and row use a whole-row click target, keyboard focus, hover detail path,
and popover disclosure. The popover dismisses on outside click and focus loss after
an idle timeout. Navigation has a predictable destination, pointer cursor, and focus
affordance. The copy control is hover-revealed, reachable, and stays visible in a
stable position. The concise status uses a default summary and moves duration and
error count to detail only. Sender labels and timestamp are passive metadata.

## Formal Web UI Verification Handoff
Primary journey frequency percent and risk if broken are declared with a priority override reason.
Initial viewport regions are primary-content, workflow-surface, blocking-alert, and supporting region.
The continuation anchor is visible without scrolling; focus enters it and document scroll remains stable at the expected destination route.
Theme intent is light, dark, or mixed and uses a declared theme.
Review inputs map UI code, styles, tokens, fonts, and assets by implementation-input ownership.
Retain an initial-viewport screenshot and full-page screenshot pair for changed visual review.
"#;

    #[test]
    fn weak_complete_and_overprescribed_docs_keep_distinct_readiness() {
        let weak_dir = tempfile::tempdir().unwrap();
        write(
            weak_dir.path(),
            "README.md",
            "# App\n\nA user-facing app with a mobile screen and desktop screen.\n",
        );
        let weak = build_inventory(weak_dir.path()).unwrap();
        assert_eq!(weak["doc_count"], 1);
        assert_eq!(weak["journey_doc_count"], 0);
        assert_eq!(weak["ui_audit_handoff_ready"], false);
        assert!(!weak["missing_signals"].as_array().unwrap().is_empty());

        let complete_dir = tempfile::tempdir().unwrap();
        write(complete_dir.path(), "docs/journey-decision.md", COMPLETE);
        let complete = build_inventory(complete_dir.path()).unwrap();
        assert!(complete["journey_doc_count"].as_u64().unwrap() >= 1);
        assert!(complete["product_doc_count"].as_u64().unwrap() >= 1);
        assert!(complete["decision_model_doc_count"].as_u64().unwrap() >= 1);
        assert!(
            complete["information_relevance_doc_count"]
                .as_u64()
                .unwrap()
                >= 1
        );
        assert!(
            complete["ui_handoff_constraint_doc_count"]
                .as_u64()
                .unwrap()
                >= 1
        );
        assert_eq!(complete["has_formal_verification_handoff"], true);
        assert_eq!(complete["has_interaction_access_model"], true);
        assert_eq!(complete["ui_audit_handoff_ready"], true);
        assert!(
            complete["ui_implementation_risk_signals"]
                .as_array()
                .unwrap()
                .is_empty()
        );

        let prescribed_dir = tempfile::tempdir().unwrap();
        write(
            prescribed_dir.path(),
            "docs/journey.md",
            &format!(
                "{COMPLETE}\nRequired visible decision evidence: raw status, debug details, owner metadata, and severity summary must always be visible on the dense dashboard.\n"
            ),
        );
        let prescribed = build_inventory(prescribed_dir.path()).unwrap();
        assert!(
            prescribed["prescriptive_ui_risk_doc_count"]
                .as_u64()
                .unwrap()
                >= 1
        );
        assert_eq!(prescribed["ui_audit_handoff_ready"], false);
    }

    #[test]
    fn source_hints_comments_policy_and_nested_repositories_are_truthful() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "README.md",
            "# App\n\nA small local tool.\n",
        );
        write(
            directory.path(),
            "src/App.tsx",
            "// User journey: operator reviews health.\n<a href=\"/settings\">Settings</a><button aria-label=\"Save profile\">Save</button>",
        );
        write(
            directory.path(),
            "AGENTS.md",
            "# Policy\n\nJourney Decision Model primary user goal primary decision required facts.",
        );
        write(
            directory.path(),
            ".gitmodules",
            "[submodule \"external/docs\"]\n path = external/docs\n url = https://invalid\n",
        );
        write(
            directory.path(),
            "external/docs/journey.md",
            "# External User Journey\nGoal decision success route acceptance.",
        );
        write(
            directory.path(),
            "packages/nested/.git/HEAD",
            "ref: refs/heads/main\n",
        );
        write(
            directory.path(),
            "packages/nested/docs/journey.md",
            "# Nested User Journey\nGoal decision success route acceptance.",
        );
        let inventory = build_inventory(directory.path()).unwrap();
        assert_eq!(inventory["source_hint_count"], 1);
        assert!(inventory["source_comment_journey_hits"].as_u64().unwrap() >= 1);
        let paths = inventory["docs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|doc| doc["path"].as_str().unwrap())
            .collect::<BTreeSet<_>>();
        assert!(!paths.contains("external/docs/journey.md"));
        assert!(!paths.contains("packages/nested/docs/journey.md"));
        assert_eq!(inventory["journey_doc_count"], 0);
        assert!(
            inventory["missing_signals"]
                .as_array()
                .unwrap()
                .iter()
                .any(|signal| signal.as_str().unwrap().contains("source comments"))
        );
    }

    fn good_report() -> String {
        REQUIRED_REPORT_HEADINGS
            .iter()
            .map(|heading| {
                let body = match *heading {
                    "Coverage" => "Journey assumptions confirmed from documentation and source evidence.",
                    "Confirmation Status" => "Confirmed by the user and repository documentation.",
                    "Journey Findings" => "| Journey | Status | Evidence |\n| --- | --- | --- |\n| Review health | confirmed | user interview |",
                    "Decision And Information Gaps" => "Checked primary journey, usage percentage, risk if broken, initial viewport, continuation anchor, theme intent, review inputs, initial-viewport screenshot, full-page screenshot, and changed visual review.",
                    "Interaction Affordance And Metadata Gaps" => "Checked activation target, focus, destination, disclosure lifecycle, detail access, scrollbar, stable dimension, hover-copy, concise status, icon meaning, and passive metadata.",
                    "Readiness And Open Questions" => "Confirmed readiness from user evidence; there are no open questions.",
                    _ => "Confirmed from documentation with concrete repository file evidence.",
                };
                format!("## {heading}\n\n{body}\n")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn report_verifier_accepts_complete_and_rejects_missing_contracts() {
        let good = good_report();
        assert!(verify_report(&good).is_empty());
        let no_interaction = good.replace(
            "Checked activation target, focus, destination, disclosure lifecycle, detail access, scrollbar, stable dimension, hover-copy, concise status, icon meaning, and passive metadata.",
            "No relevant interaction or message-metadata surfaces documented.",
        );
        assert!(verify_report(&no_interaction).is_empty());
        assert!(
            !verify_report(&good.replace("changed visual review", "ordinary inspection"))
                .is_empty()
        );
        assert!(
            !verify_report(&good.replace(
                "Journey assumptions confirmed from documentation and source evidence.",
                "journey assumptions unconfirmed"
            ))
            .is_empty()
        );
        assert!(
            !verify_report(&good.replace(
                "## Interaction Affordance And Metadata Gaps",
                "## Generic UI Gaps"
            ))
            .is_empty()
        );
    }

    #[test]
    fn canonical_skill_handoff_term_count_matches_legacy_oracle() {
        let text = include_str!("../../../skills/user-journey-docs-audit/SKILL.md");
        let counts = UI_HANDOFF_TERMS
            .iter()
            .map(|term| (*term, count_terms(text, &[*term])))
            .collect::<Vec<_>>();
        assert_eq!(
            counts.iter().map(|(_, count)| count).sum::<usize>(),
            11,
            "{counts:?}"
        );
    }
}
