//! Conservative applicability gate for the explicit UI implementation audit.

use std::collections::BTreeSet;
use std::path::Path;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::audit_ledger::read_bytes_nofollow;

pub const DETECTOR_VERSION: u64 = 2;
const MAX_BASIS_LENGTH: usize = 180;

const EVIDENCE_ONLY_DIRS: &[&str] = &[
    "__snapshots__",
    "docs",
    "documentation",
    "examples",
    "fixtures",
    "mockup",
    "mockups",
    "prototype",
    "prototypes",
    "screenshots",
    "snapshots",
    "stories",
    "storybook",
    "test",
    "tests",
];
const STYLE_OR_CATALOG_EXTENSIONS: &[&str] = &[
    "css",
    "less",
    "po",
    "pot",
    "sass",
    "scss",
    "styl",
    "strings",
    "xcstrings",
];
const DOCUMENT_OR_CONFIG_EXTENSIONS: &[&str] = &[
    "json", "jsonc", "lock", "markdown", "md", "rst", "toml", "txt", "yaml", "yml",
];
const ASSET_EXTENSIONS: &[&str] = &[
    "avif", "bmp", "gif", "ico", "jpeg", "jpg", "pdf", "png", "svg", "tif", "tiff", "webp",
];
const WEB_MARKUP_EXTENSIONS: &[&str] = &[
    "astro",
    "cshtml",
    "ejs",
    "hbs",
    "handlebars",
    "htm",
    "html",
    "j2",
    "jinja",
    "jinja2",
    "js",
    "jsx",
    "liquid",
    "mdx",
    "mjs",
    "mustache",
    "njk",
    "php",
    "pug",
    "razor",
    "svelte",
    "tpl",
    "tsx",
    "twig",
    "vue",
];
const NATIVE_MARKUP_EXTENSIONS: &[&str] = &["axaml", "xaml", "xml"];
const WEB_CONTROL_TAGS: &[&str] = &[
    "a", "button", "dialog", "form", "input", "select", "table", "textarea",
];
const WEB_CONTENT_TAGS: &[&str] = &[
    "article", "aside", "details", "div", "figure", "footer", "h1", "h2", "h3", "h4", "h5", "h6",
    "header", "img", "label", "li", "main", "nav", "ol", "p", "section", "span", "summary",
    "tbody", "td", "th", "thead", "tr", "ul",
];
const SCAFFOLD_TAGS: &[&str] = &[
    "app",
    "fragment",
    "navigate",
    "outlet",
    "router-view",
    "routerprovider",
    "routes",
    "route",
    "slot",
    "strictmode",
    "suspense",
];
const UI_KIND_SUFFIXES: &[&str] = &[
    "UI",
    "View",
    "Widget",
    "Screen",
    "Panel",
    "Surface",
    "Component",
    "Window",
    "Dialog",
    "Page",
    "Canvas",
    "Layout",
];
const NON_UI_KIND_SUFFIXES: &[&str] = &[
    "ViewModel",
    "Model",
    "State",
    "Data",
    "Record",
    "Policy",
    "Service",
    "Controller",
];
const BACKEND_TOKENS: &[&str] = &[
    "data",
    "database",
    "db",
    "diesel",
    "mongodb",
    "mysql",
    "orm",
    "persistence",
    "postgres",
    "prisma",
    "sql",
    "sqlalchemy",
    "sqlite",
    "sqlx",
    "storage",
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExplicitUiBasis {
    pub ui_kind: String,
    pub source_anchor: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Qualification {
    pub method: String,
    pub detector_version: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_anchor: Option<String>,
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static UI gate regex")
}

fn suffix(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

fn words(value: &str) -> BTreeSet<String> {
    re(r"[A-Za-z0-9]+")
        .find_iter(value)
        .map(|item| item.as_str().to_ascii_lowercase())
        .collect()
}

pub fn is_evidence_only_interface_path(rel_path: &str) -> bool {
    let path = Path::new(rel_path);
    let parent_is_evidence = path
        .parent()
        .into_iter()
        .flat_map(Path::components)
        .filter_map(|part| part.as_os_str().to_str())
        .map(str::to_ascii_lowercase)
        .any(|part| EVIDENCE_ONLY_DIRS.contains(&part.as_str()));
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    parent_is_evidence
        || re(r"(?i)(?:^|[._-])(?:fixture|mockup|prototype|snapshot|spec|stories?|test)(?:[._-]|$)")
            .is_match(&name)
}

fn without_comments(text: &str) -> String {
    let block = re(r"(?s)/\*.*?\*/|<!--.*?-->");
    let lines = re(r"(?m)^\s*(?://|#).*$");
    lines
        .replace_all(&block.replace_all(text, " "), " ")
        .into_owned()
}

fn quoted_ranges(text: &str) -> Vec<(usize, usize, String)> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let quote = bytes[index];
        if !matches!(quote, b'\'' | b'"' | b'`') {
            index += 1;
            continue;
        }
        let start = index;
        let triple = quote != b'`'
            && index + 2 < bytes.len()
            && bytes[index + 1] == quote
            && bytes[index + 2] == quote;
        index += if triple { 3 } else { 1 };
        let body_start = index;
        while index < bytes.len() {
            if bytes[index] == b'\\' {
                index = (index + 2).min(bytes.len());
                continue;
            }
            let ends = if triple {
                index + 2 < bytes.len()
                    && bytes[index] == quote
                    && bytes[index + 1] == quote
                    && bytes[index + 2] == quote
            } else {
                bytes[index] == quote
            };
            if ends {
                let body = String::from_utf8_lossy(&bytes[body_start..index]).into_owned();
                index += if triple { 3 } else { 1 };
                ranges.push((start, index, body));
                break;
            }
            index += 1;
        }
        if index >= bytes.len() && ranges.last().is_none_or(|range| range.0 != start) {
            break;
        }
    }
    ranges
}

fn quoted_values(text: &str) -> Vec<String> {
    quoted_ranges(text)
        .into_iter()
        .map(|(_, _, body)| body)
        .collect()
}

fn without_string_literals(text: &str) -> String {
    let mut bytes = text.as_bytes().to_vec();
    for (start, end, _) in quoted_ranges(text) {
        bytes[start..end].fill(b' ');
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn literal_has_visible_content(value: &str) -> bool {
    let escaped = re(
        r"(?i)\\(?:[fnrtv]|x(?:09|0a|0b|0c|0d|20)|u(?:0009|000a|000b|000c|000d|0020)|U(?:00000009|0000000a|0000000b|0000000c|0000000d|00000020))",
    );
    !escaped.replace_all(value, "").trim().is_empty()
}

fn placeholder(value: &str) -> bool {
    re(r"(?i)\b(?:coming soon|under construction|work in progress|not implemented|placeholder)\b")
        .is_match(value)
}

fn visible_labels(code: &str) -> Vec<String> {
    let mut labels = re(r">([^<>{}]+)<")
        .captures_iter(code)
        .filter_map(|capture| capture.get(1))
        .map(|value| {
            value
                .as_str()
                .trim_matches(|ch: char| " \t\r\n.,:;!?-_".contains(ch))
                .to_owned()
        })
        .collect::<Vec<_>>();
    let attrs = re(r#"(?i)\b(?:aria-label|placeholder|title|value)\s*=\s*['\"]([^'\"]+)['\"]"#);
    labels.extend(
        attrs
            .captures_iter(code)
            .filter_map(|capture| capture.get(1))
            .map(|value| value.as_str().trim().to_owned()),
    );
    labels.retain(|value| literal_has_visible_content(value));
    labels
}

fn framework_labels(code: &str) -> Vec<String> {
    let mut values = Vec::new();
    let patterns = [
        r#"(?i)\b(?:Button|JButton|JLabel|Label|QLabel|QPushButton|Text|TextBlock|TextView)\s*\(\s*['\"]([^'\"]+)['\"]"#,
        r#"(?i)\b(?:st|gr)\.[A-Za-z_]\w*\s*\(\s*['\"]([^'\"]+)['\"]"#,
        r#"(?i)\.\s*(?:innerHTML|textContent|innerText|text)\s*=\s*['\"]([^'\"]+)['\"]"#,
        r#"(?i)\bsetTitle\s*\(\s*['\"]([^'\"]+)['\"]"#,
        r#"(?i)\.\s*(?:stringValue|text)\s*=\s*['\"]([^'\"]+)['\"]"#,
        r#"(?i)\b(?:Button|Entry|Label)\s*\([^)]*\btext\s*=\s*['\"]([^'\"]+)['\"]"#,
        r#"(?i)\b(?:android:text|contentDescription)\s*=\s*['\"]([^'\"]+)['\"]"#,
        r#"(?i)\bNSTextField\s*\(\s*labelWithString\s*:\s*['\"]([^'\"]+)['\"]"#,
    ];
    for pattern in patterns {
        values.extend(
            re(pattern)
                .captures_iter(code)
                .filter_map(|capture| capture.get(1))
                .map(|value| value.as_str().trim().to_owned()),
        );
    }
    values.retain(|value| literal_has_visible_content(value));
    values
}

fn is_scaffold_tag(tag: &str) -> bool {
    let folded = tag.to_ascii_lowercase();
    let leaf = folded.rsplit('.').next().unwrap_or(&folded);
    SCAFFOLD_TAGS.contains(&leaf)
        || ["layout", "page", "root", "screen"].contains(&leaf)
        || ["boundary", "guard", "layout", "provider", "router", "shell"]
            .iter()
            .any(|suffix| leaf.ends_with(suffix))
        || ["require", "router"]
            .iter()
            .any(|prefix| leaf.starts_with(prefix))
}

fn data_bound_content(code: &str) -> bool {
    re(r"(?i)(?:\.map\s*\(|\bforeach\b|\bfor\s*\(|\bv-for\s*=|\*ngFor\s*=|\{#each\b|\bForEach\s*\(|\bitems\s*\()")
        .is_match(code)
}

fn bound_value_output(code: &str) -> bool {
    re(r"<\?=|\{\{[^{}]*[A-Za-z_$][^{}]*\}\}|\$\{[^{}]*[A-Za-z_$][^{}]*\}|\{\s*[A-Za-z_$][\w.$\[\]]+\s*\}")
        .is_match(code)
}

fn provably_empty_aliases(code: &str) -> BTreeSet<String> {
    let mut aliases = BTreeSet::new();
    let assignment = re(
        r#"(?m)(?:^|[;{])\s*(?:(?:const|let|val|var|String|string)\s+)?([A-Za-z_]\w*)(?:\s*:\s*[^=;\n]+)?\s*=\s*['\"]([^'\"]*)['\"]"#,
    );
    for capture in assignment.captures_iter(code) {
        if !literal_has_visible_content(capture.get(2).unwrap().as_str()) {
            aliases.insert(capture.get(1).unwrap().as_str().to_owned());
        }
    }
    for capture in re(r"(?m)(?:^|[;{])\s*(?:(?:const|let|val|var|String|string)\s+)?([A-Za-z_]\w*)(?:\s*:\s*[^=;\n]+)?\s*=\s*(?:nil|null|None)\b")
        .captures_iter(code)
    {
        aliases.insert(capture.get(1).unwrap().as_str().to_owned());
    }
    for capture in re(r"(?m)(?:^|[;{])\s*(?:(?:const|let|val|var|String|string)\s+)?([A-Za-z_]\w*)(?:\s*:\s*[^=;\n]+)?\s*=\s*(?:Data|NSData|NSImage|String|UIImage)\s*\(\s*\)")
        .captures_iter(code)
    {
        aliases.insert(capture.get(1).unwrap().as_str().to_owned());
    }
    let alias_assignment = re(
        r"(?m)(?:^|[;{])\s*(?:(?:const|let|val|var|String|string)\s+)?([A-Za-z_]\w*)(?:\s*:\s*[^=;\n]+)?\s*=\s*([A-Za-z_]\w*)\b",
    );
    loop {
        let before = aliases.len();
        for capture in alias_assignment.captures_iter(code) {
            if aliases.contains(capture.get(2).unwrap().as_str()) {
                aliases.insert(capture.get(1).unwrap().as_str().to_owned());
            }
        }
        if aliases.len() == before {
            break;
        }
    }
    aliases
}

fn dynamic_value_is_substantive(value: &str, source_code: &str) -> bool {
    let Some(root) = re(r"^[A-Za-z_$][\w$]*").find(value) else {
        return false;
    };
    !["nil", "null", "None"].contains(&root.as_str())
        && !provably_empty_aliases(source_code).contains(root.as_str())
}

fn empty_constructor_follows(code: &str, end: usize, value: &str) -> bool {
    let root = value.split('.').next().unwrap_or(value);
    ["Data", "NSData", "NSImage", "String", "UIImage"].contains(&root)
        && re(r"^\s*\(\s*\)").is_match(&code[end..])
}

fn dynamic_ui_content(executable_code: &str, source_code: &str) -> bool {
    let patterns = [
        r"\b(?:Text|Markdown)\s*\(\s*([A-Za-z_$][\w.$\[\]]*)",
        r"\b(?:st|gr)\.(?:data_editor|dataframe|markdown|table|text|title|write)\s*\(\s*([A-Za-z_$][\w.$\[\]]*)",
        r"\.\s*(?:innerHTML|textContent|innerText|text)\s*=\s*([A-Za-z_$][\w.$\[\]]*)",
        r"\b(?:Button|Entry|Label)\s*\([^)]*\btext\s*=\s*([A-Za-z_$][\w.$\[\]]*)",
        r"\bsetTitle\s*\(\s*([A-Za-z_$][\w.$\[\]]*)",
        r"\b(?:NSImageView|UIImageView)\s*\(\s*image\s*:\s*([A-Za-z_$][\w.$\[\]]*)",
    ];
    patterns.iter().any(|pattern| {
        re(pattern).captures_iter(executable_code).any(|capture| {
            let value = capture.get(1).unwrap();
            dynamic_value_is_substantive(value.as_str(), source_code)
                && !empty_constructor_follows(executable_code, value.end(), value.as_str())
        })
    })
}

fn override_dynamic_content(executable_code: &str, source_code: &str) -> bool {
    dynamic_ui_content(executable_code, source_code)
        || re(r"\.\w+\s*\(\s*([A-Za-z_$][\w.$\[\]]*)\s*\)")
            .captures_iter(executable_code)
            .any(|capture| {
                let value = capture.get(1).unwrap();
                dynamic_value_is_substantive(value.as_str(), source_code)
                    && !empty_constructor_follows(executable_code, value.end(), value.as_str())
            })
}

fn has_substantive_code_content(code: &str, executable_code: &str) -> bool {
    !framework_labels(code).is_empty()
        || data_bound_content(code)
        || dynamic_ui_content(executable_code, code)
        || re(r"\b(?:JList|JTable|NSCollectionView|NSTableView|QListView|QTableView|UICollectionView|UITableView)\s*\(")
            .is_match(executable_code)
}

fn is_product_component_tag(tag: &str) -> bool {
    let leaf = tag
        .to_ascii_lowercase()
        .rsplit('.')
        .next()
        .unwrap_or(tag)
        .replace(['-', '_'], "");
    [
        "button",
        "calendar",
        "card",
        "chart",
        "contacts",
        "dashboard",
        "details",
        "editor",
        "feed",
        "form",
        "gallery",
        "grid",
        "list",
        "map",
        "menu",
        "profile",
        "table",
        "timeline",
        "toolbar",
        "viewer",
    ]
    .iter()
    .any(|ending| leaf.ends_with(ending))
}

fn interactive_product_ui_signal(code: &str) -> bool {
    re(r"\bon[A-Z][A-Za-z]*\s*=|\bv-on:|@click\s*=|<\s*(?:form|input|select|textarea)\b|\b(?:Button|TextField)\s*\(|\bst\.button\s*\(")
        .is_match(code)
}

fn outbound_renderer_path(rel_path: &Path) -> bool {
    rel_path
        .components()
        .filter_map(|part| part.as_os_str().to_str())
        .flat_map(|part| words(part).into_iter())
        .any(|token| {
            [
                "email",
                "emails",
                "mail",
                "mails",
                "notification",
                "notifications",
            ]
            .contains(&token.as_str())
        })
}

fn web_surface_issue(code: &str, executable_code: &str, extension: &str) -> Option<Option<String>> {
    if !WEB_MARKUP_EXTENSIONS.contains(&extension) {
        return None;
    }
    let tags = re(r"<\s*([A-Za-z][A-Za-z0-9_.:-]*)\b")
        .captures_iter(executable_code)
        .filter_map(|capture| capture.get(1))
        .map(|value| value.as_str().to_owned())
        .collect::<Vec<_>>();
    if tags.is_empty() {
        return None;
    }
    let labels = visible_labels(code);
    let product_tags = tags
        .iter()
        .filter(|tag| !is_scaffold_tag(tag))
        .collect::<Vec<_>>();
    let intrinsic = product_tags
        .iter()
        .map(|tag| tag.to_ascii_lowercase())
        .filter(|tag| {
            WEB_CONTROL_TAGS.contains(&tag.as_str()) || WEB_CONTENT_TAGS.contains(&tag.as_str())
        })
        .collect::<Vec<_>>();
    let custom = product_tags
        .iter()
        .filter(|tag| {
            let folded = tag.to_ascii_lowercase();
            !WEB_CONTROL_TAGS.contains(&folded.as_str())
                && !WEB_CONTENT_TAGS.contains(&folded.as_str())
        })
        .collect::<Vec<_>>();
    let handler = re(r"\b(?:action|href|on[A-Z][A-Za-z]*|v-on:|@click)\s*=").is_match(code);
    let no_op_handler =
        re(r"\bon[A-Z][A-Za-z]*\s*=\s*\{\s*(?:\([^)]*\)\s*=>\s*)?\{\s*\}\s*\}").is_match(code);
    let functional_control = intrinsic
        .iter()
        .any(|tag| WEB_CONTROL_TAGS.contains(&tag.as_str()))
        && handler
        && !no_op_handler;
    let bound = data_bound_content(code);
    let nonplaceholder = labels.iter().filter(|label| !placeholder(label)).count();
    if labels.iter().any(|label| placeholder(label))
        && !bound
        && !(functional_control && nonplaceholder >= 2)
    {
        return Some(Some(
            "contains a placeholder-only target surface".to_owned(),
        ));
    }
    if !labels.is_empty()
        && labels
            .iter()
            .all(|label| label.trim().eq_ignore_ascii_case("example"))
        && !functional_control
    {
        return Some(Some(
            "contains a placeholder-only target surface".to_owned(),
        ));
    }
    if product_tags.is_empty() {
        return Some(Some(
            "contains only route, provider, mount, or framework scaffolding".to_owned(),
        ));
    }
    if (re(r"\b(?:createRoot|ReactDOM\.render)\s*\(").is_match(executable_code)
        || re(r"<\s*Outlet\b").is_match(executable_code))
        && intrinsic.is_empty()
    {
        return Some(Some(
            "contains only wrapped route, provider, mount, or framework scaffolding".to_owned(),
        ));
    }
    let visible_input = intrinsic
        .iter()
        .any(|tag| ["input", "select", "textarea"].contains(&tag.as_str()))
        && re(r"(?i)\b(?:id|name|placeholder|type|value)\s*=").is_match(code);
    if intrinsic
        .iter()
        .any(|tag| WEB_CONTROL_TAGS.contains(&tag.as_str()))
        && (!labels.is_empty() || visible_input)
    {
        return Some(None);
    }
    if intrinsic.len() >= 2 && (!labels.is_empty() || bound) {
        return Some(None);
    }
    if bound
        && !intrinsic.is_empty()
        && (intrinsic
            .iter()
            .any(|tag| ["article", "li", "p", "td", "tr"].contains(&tag.as_str()))
            || bound_value_output(code))
    {
        return Some(None);
    }
    if custom.len() >= 2 && (!labels.is_empty() || bound) {
        return Some(None);
    }
    if !custom.is_empty()
        && tags.len() >= 2
        && custom.iter().any(|tag| is_product_component_tag(tag))
    {
        return Some(None);
    }
    Some(Some(
        "does not construct substantive visible product content or controls".to_owned(),
    ))
}

fn embedded_web_surface_issue(code: &str, extension: &str) -> Option<Option<String>> {
    let angular = code.contains("@Component") && code.contains("template");
    let lit_component = (code.contains("LitElement") || code.contains("ReactiveElement"))
        && re(r"\brender\s*\(").is_match(code);
    let lit_function = re(r#"\bfrom\s*['\"](?:lit|lit-element|lit-html)(?:/[^'\"]*)?['\"]"#)
        .is_match(code)
        && (re(r"(?s)\bfunction\s+[A-Z][A-Za-z0-9_]*\s*\([^)]*\)\s*\{.*?\breturn\s+html\s*`")
            .is_match(code)
            || re(r"(?s)\b(?:const|let)\s+[A-Z][A-Za-z0-9_]*\s*=.*?=>\s*html\s*`").is_match(code));
    let python_route =
        extension == "py" && re(r"(?i)@[A-Za-z_]\w*\.(?:get|route)\s*\(").is_match(code);
    if !(angular || lit_component || lit_function || python_route) {
        return None;
    }
    let mut checked = false;
    let mut issue = None;
    for body in quoted_values(code)
        .into_iter()
        .filter(|body| body.contains('<'))
    {
        if let Some(result) = web_surface_issue(&body, &without_comments(&body), "html") {
            checked = true;
            if result.is_none() {
                return Some(None);
            }
            issue = result;
        }
    }
    checked.then_some(issue)
}

fn code_placeholder_only(code: &str) -> bool {
    let labels = framework_labels(code);
    if labels.iter().any(|label| placeholder(label)) {
        if data_bound_content(code) {
            return false;
        }
        let action_labels = re(r#"(?i)\b(?:Button|JButton|QPushButton|st\.button|gr\.Button)\s*\(\s*['\"]([^'\"]+)['\"]"#)
            .captures_iter(code)
            .filter_map(|capture| capture.get(1))
            .map(|value| value.as_str())
            .collect::<Vec<_>>();
        return !action_labels
            .iter()
            .any(|label| !label.trim().is_empty() && !placeholder(label));
    }
    !labels.is_empty()
        && labels
            .iter()
            .all(|label| label.trim().eq_ignore_ascii_case("example"))
}

fn parenthesized_extent(code: &str, open_index: usize) -> (&str, usize) {
    let bytes = code.as_bytes();
    let mut index = open_index;
    let mut depth = 0_i64;
    let mut quote = None;
    while index < bytes.len() {
        if let Some(current) = quote {
            if bytes[index] == b'\\' {
                index = (index + 2).min(bytes.len());
                continue;
            }
            if bytes[index] == current {
                quote = None;
            }
            index += 1;
            continue;
        }
        if matches!(bytes[index], b'\'' | b'"' | b'`') {
            quote = Some(bytes[index]);
            index += 1;
            continue;
        }
        match bytes[index] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return (&code[open_index + 1..index], index + 1);
                }
            }
            _ => {}
        }
        index += 1;
    }
    (&code[open_index.saturating_add(1)..], code.len())
}

fn control_args_are_substantive(kind: &str, args: &str, source_code: &str) -> bool {
    let leaf = kind.rsplit("::").next().unwrap_or(kind);
    if [
        "JList",
        "JTable",
        "NSCollectionView",
        "NSTableView",
        "QListView",
        "QTableView",
        "UICollectionView",
        "UITableView",
    ]
    .contains(&leaf)
    {
        return true;
    }
    let nonblank_literal = quoted_values(args)
        .iter()
        .any(|value| literal_has_visible_content(value));
    if ["NSImageView", "UIImageView"].contains(&leaf) {
        if nonblank_literal {
            return true;
        }
        let executable = without_string_literals(args);
        if re(r"\bimage\s*:\s*(?:NSImage|UIImage)\s*\(\s*(?:data\s*:\s*(?:Data|NSData)\s*\(\s*\)?)?\s*\)?\s*$")
            .is_match(&executable)
        {
            return false;
        }
        return re(r"\bimage\s*:\s*([A-Za-z_$][\w.$\[\]]*)")
            .captures(&executable)
            .and_then(|capture| capture.get(1))
            .is_some_and(|value| dynamic_value_is_substantive(value.as_str(), source_code));
    }
    nonblank_literal
        || (["JButton", "JLabel", "QLabel", "QPushButton"].contains(&leaf)
            && re(r"^\s*([A-Za-z_$][\w.$\[\]]*)")
                .captures(&without_string_literals(args))
                .and_then(|capture| capture.get(1))
                .is_some_and(|value| dynamic_value_is_substantive(value.as_str(), source_code)))
}

fn named_control_binding_is_substantive(code: &str, name: &str, source_code: &str) -> bool {
    let escaped = regex::escape(name);
    let literal = re(&format!(
        r#"(?s)\b{escaped}\s*\.\s*(?:image|innerHTML|innerText|placeholder|stringValue|text|textContent|value)\s*=\s*['\"]([^'\"]*)['\"]|\b{escaped}\s*\.\s*(?:setText|setTitle)\s*\(\s*['\"]([^'\"]*)['\"]"#
    ));
    if literal.captures_iter(code).any(|capture| {
        capture
            .get(1)
            .or_else(|| capture.get(2))
            .is_some_and(|value| literal_has_visible_content(value.as_str()))
    }) {
        return true;
    }
    let executable = without_string_literals(code);
    let dynamic = re(&format!(
        r"\b{escaped}\s*\.\s*(?:image|innerHTML|innerText|placeholder|stringValue|text|textContent|value)\s*=\s*([A-Za-z_$][\w.$\[\]]*)|\b{escaped}\s*\.\s*(?:setText|setTitle)\s*\(\s*([A-Za-z_$][\w.$\[\]]*)"
    ));
    dynamic.captures_iter(&executable).any(|capture| {
        let Some(value) = capture.get(1).or_else(|| capture.get(2)) else {
            return false;
        };
        dynamic_value_is_substantive(value.as_str(), source_code)
            && !empty_constructor_follows(&executable, value.end(), value.as_str())
    })
}

fn attached_control_is_substantive(
    code: &str,
    constructors: &[&str],
    attach_methods: &[&str],
) -> bool {
    for attach in attach_methods {
        let direct = re(&format!(
            r"\b{}\s*\(\s*(?:new\s+)?([A-Za-z_]\w*)\s*\(",
            regex::escape(attach)
        ));
        for capture in direct.captures_iter(code) {
            let kind = capture.get(1).unwrap().as_str();
            if !constructors.contains(&kind) {
                continue;
            }
            let open = capture.get(0).unwrap().end() - 1;
            let (args, _) = parenthesized_extent(code, open);
            if control_args_are_substantive(kind, args, code) {
                return true;
            }
        }
    }
    let declaration = re(
        r"\b(?:let|var|const|[A-Za-z_]\w*(?:<[^>]+>)?)\s+([A-Za-z_]\w*)(?:\s*:\s*[A-Za-z_][\w.<>,? ]*)?\s*=\s*(?:new\s+)?([A-Za-z_]\w*)\s*\(",
    );
    for capture in declaration.captures_iter(code) {
        let name = capture.get(1).unwrap().as_str();
        let kind = capture.get(2).unwrap().as_str();
        if !constructors.contains(&kind) {
            continue;
        }
        let open = capture.get(0).unwrap().end() - 1;
        let (args, end) = parenthesized_extent(code, open);
        let tail = &code[end..];
        let attached = attach_methods.iter().any(|attach| {
            re(&format!(
                r"\b{}\s*\(\s*{}\s*\)",
                regex::escape(attach),
                regex::escape(name)
            ))
            .is_match(tail)
        });
        if attached
            && (control_args_are_substantive(kind, args, code)
                || named_control_binding_is_substantive(tail, name, code))
        {
            return true;
        }
    }
    false
}

fn swiftui_has_content(code: &str, executable_code: &str) -> bool {
    if data_bound_content(code) || !framework_labels(code).is_empty() {
        return true;
    }
    let dynamic = re(r"\b(?:Button|Canvas|ForEach|Image|Label|Picker|Table|Text|TextField)\s*\(\s*(?:[A-Za-z_]\w*\s*:\s*)?([A-Za-z_$][\w.$\[\]]*)\s*(?:[,)]|$)")
        .captures_iter(executable_code)
        .filter_map(|capture| capture.get(1))
        .any(|value| dynamic_value_is_substantive(value.as_str(), code));
    if dynamic {
        return true;
    }
    let custom = re(r"\b([A-Z][A-Za-z0-9_]+)\s*\(([^)]*)\)");
    custom.captures_iter(executable_code).any(|capture| {
        let name = capture.get(1).unwrap().as_str();
        let args = capture.get(2).unwrap().as_str();
        ![
            "Button",
            "Canvas",
            "ForEach",
            "Image",
            "Label",
            "Picker",
            "Table",
            "Text",
            "TextField",
            "HStack",
            "LazyHGrid",
            "LazyVGrid",
            "List",
            "ScrollView",
            "VStack",
            "ZStack",
        ]
        .contains(&name)
            && (name.ends_with("List")
                || name.ends_with("Screen")
                || name.ends_with("View")
                || name.ends_with("Widget")
                || (!args.trim().is_empty() && re(r"[A-Za-z_$]").is_match(args)))
    })
}

fn declarative_native_has_content(code: &str) -> bool {
    let collection = re(r"(?is)<\s*(?:[A-Za-z_][\w.-]*(?::|\.))?(?:CollectionView|ListView|RecyclerView|TableView)\b([^>]*)>")
        .captures(code)
        .and_then(|capture| capture.get(1))
        .map(|attrs| attrs.as_str().to_owned());
    if let Some(attrs) = collection {
        let value = re(r#"(?is)\b(?:ItemsSource|adapter|android:entries|data|entries|items|itemTemplate)\s*=\s*['\"](.*?)['\"]"#)
            .captures(&attrs)
            .and_then(|capture| capture.get(1))
            .map(|value| value.as_str().to_owned());
        if value.is_some_and(|value| literal_has_visible_content(&value)) {
            return true;
        }
    }
    re(r#"(?is)<\s*(?:[A-Za-z_][\w.-]*:)?(?:Button|Image|Label|TextBlock|TextField|TextView)\b[^>]*(?:android:text|aria-label|contentDescription|src|text)\s*=\s*['\"][^'\"]+['\"]"#)
        .is_match(code)
        || re(r"(?is)<\s*(?:[A-Za-z_][\w.-]*:)?(?:Button|Label|TextBlock|TextView)\b[^>]*>\s*[^<{][^<]*<")
            .is_match(code)
}

fn code_surface_kind(code: &str, executable_code: &str, extension: &str) -> Option<&'static str> {
    if re(r"\bvar\s+body\s*:\s*some\s+View\b").is_match(executable_code)
        && swiftui_has_content(code, executable_code)
    {
        return Some("swiftui-view");
    }
    let native_controls = [
        "NSButton",
        "NSCollectionView",
        "NSImageView",
        "NSTableView",
        "NSTextField",
        "UIButton",
        "UICollectionView",
        "UIImageView",
        "UILabel",
        "UITableView",
        "UITextField",
    ];
    if re(r"\b(?:NSViewRepresentable|NSViewControllerRepresentable|UIViewRepresentable|UIViewControllerRepresentable)\b")
        .is_match(executable_code)
        && re(r"\bmake(?:NS|UI)View(?:Controller)?\s*\(").is_match(executable_code)
        && attached_control_is_substantive(code, &native_controls, &["addArrangedSubview", "addSubview"])
    {
        return Some("native-view-representable");
    }
    if re(r"\b(?:NSViewController|UIViewController)\b").is_match(executable_code)
        && re(r"\b(?:loadView|viewDidLoad)\s*\(").is_match(executable_code)
        && attached_control_is_substantive(
            code,
            &native_controls,
            &["addArrangedSubview", "addSubview"],
        )
    {
        return Some("native-view-controller");
    }
    if re(r"\b(?:class|struct)\s+\w+[^\n{]{0,120}:\s*(?:NSView|UIView)\b").is_match(executable_code)
        && attached_control_is_substantive(
            code,
            &native_controls,
            &["addArrangedSubview", "addSubview"],
        )
    {
        return Some("native-view-subclass");
    }
    if re(r"\bextends\s+(?:JComponent|JDialog|JFrame|JPanel|JWindow)\b").is_match(executable_code)
        && attached_control_is_substantive(
            code,
            &[
                "JButton",
                "JLabel",
                "JList",
                "JMenu",
                "JTable",
                "JTextArea",
                "JTextField",
            ],
            &["add"],
        )
    {
        return Some("java-swing-view");
    }
    if re(r"@Composable\b").is_match(executable_code)
        && re(r"\b(?:Button|Text|TextField)\s*\(|\bitems\s*\(").is_match(executable_code)
        && has_substantive_code_content(code, executable_code)
    {
        return Some("compose-view");
    }
    if re(r"\bWidget\s+build\s*\(").is_match(executable_code)
        && re(r"\b(?:AppBar|ListView|Text|TextField)\s*\(|\bchildren\s*:").is_match(executable_code)
        && has_substantive_code_content(code, executable_code)
    {
        return Some("flutter-view");
    }
    if (re(r"\b(?:QMainWindow|QWidget)\s*\(|\bclass\s+\w+\s*\(\s*(?:QMainWindow|QWidget)\s*\)")
        .is_match(executable_code))
        && attached_control_is_substantive(
            code,
            &[
                "QPushButton",
                "QLabel",
                "QLineEdit",
                "QListView",
                "QTableView",
            ],
            &["addWidget"],
        )
    {
        return Some("qt-view");
    }
    if re(r"\bTk\s*\(").is_match(executable_code)
        && re(r"\b(?:Button|Entry|Label|Listbox|Text|Treeview)\s*\(").is_match(executable_code)
        && re(r"\.(?:grid|pack|place)\s*\(").is_match(executable_code)
        && has_substantive_code_content(code, executable_code)
    {
        return Some("tk-view");
    }
    if re(r"\bTk\s*\(").is_match(executable_code)
        && re(r"\bCanvas\s*\(").is_match(executable_code)
        && re(r"\.create_(?:arc|image|line|oval|polygon|rectangle|text)\s*\(")
            .is_match(executable_code)
    {
        return Some("tk-canvas-view");
    }
    if (re(r"\bst\.(?:button|chat_input|data_editor|dataframe|file_uploader|header|image|markdown|multiselect|number_input|radio|selectbox|slider|subheader|table|text|text_area|text_input|title|write)\s*\(")
        .is_match(executable_code)
        || re(r"\bgr\.(?:Button|ChatInterface|Checkbox|Dataframe|Dropdown|Gallery|HTML|Image|Interface|Markdown|Radio|Slider|Textbox)\s*\(")
            .is_match(executable_code))
        && has_substantive_code_content(code, executable_code)
    {
        return Some("declarative-python-view");
    }
    if dom_surface_is_substantive(code) {
        return Some("dom-view");
    }
    if NATIVE_MARKUP_EXTENSIONS.contains(&extension) && declarative_native_has_content(code) {
        return Some("declarative-native-view");
    }
    None
}

fn dom_surface_is_substantive(code: &str) -> bool {
    let declaration =
        re(r"\b(?:const|let|var)\s+([A-Za-z_]\w*)\s*=\s*document\.createElement\s*\(");
    declaration.captures_iter(code).any(|capture| {
        let name = capture.get(1).unwrap().as_str();
        let tail = &code[capture.get(0).unwrap().end()..];
        re(&format!(
            r"\b(?:appendChild|replaceChildren)\s*\([^)]*\b{}\b",
            regex::escape(name)
        ))
        .is_match(tail)
            && named_control_binding_is_substantive(tail, name, code)
    })
}

fn candidate_code(repo: &Path, rel_path: &str) -> Result<String, String> {
    let relative = Path::new(rel_path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err("is not a confined repository-relative path".to_owned());
    }
    let path = repo.join(relative);
    let bytes = read_bytes_nofollow(&path, Some(repo))
        .map_err(|error| format!("cannot be read: {error}"))?
        .ok_or_else(|| "is not a regular confined repository file".to_owned())?;
    if is_evidence_only_interface_path(rel_path) {
        return Err(
            "belongs to prototype, story, test, fixture, example, or other evidence-only scope"
                .to_owned(),
        );
    }
    let extension = suffix(rel_path);
    if STYLE_OR_CATALOG_EXTENSIONS.contains(&extension.as_str())
        || ASSET_EXTENSIONS.contains(&extension.as_str())
        || DOCUMENT_OR_CONFIG_EXTENSIONS.contains(&extension.as_str())
    {
        return Err(
            "is documentation, configuration, styling, catalog, or asset source rather than executable UI source"
                .to_owned(),
        );
    }
    let truncated = &bytes[..bytes.len().min(600_000)];
    let text = String::from_utf8_lossy(truncated);
    let code = without_comments(&text);
    let outbound_symbol = re(
        r"\b(?:class|def|function|struct)\s+[A-Za-z0-9_]+(?:Email|Mail|Notification)(?:Template)?\b|\b(?:const|let|val|var)\s+[A-Za-z0-9_]+(?:Email|Mail|Notification)(?:Template)?\s*=",
    )
    .is_match(&code);
    if (outbound_symbol || outbound_renderer_path(relative))
        && !interactive_product_ui_signal(&code)
    {
        return Err(
            "defines an outbound email, mail, or notification renderer rather than a product UI surface"
                .to_owned(),
        );
    }
    let compact = re(r"\s+")
        .replace_all(&code, " ")
        .trim()
        .to_ascii_lowercase();
    if compact.is_empty() {
        return Err("does not contain an implemented surface".to_owned());
    }
    let starter = [
        ["vite + react", "edit src/app.tsx and save to test hmr"],
        [
            "get started by editing app/page",
            "save and see your changes instantly",
        ],
        ["learn react", "logo.svg"],
    ]
    .iter()
    .any(|group| group.iter().all(|signature| compact.contains(signature)));
    if starter {
        let starter_label = re(
            r"(?i)(?:vite\s*\+\s*react|count is \d+|edit src/app\.(?:js|jsx|ts|tsx)|save to test hmr|learn react|read the docs)",
        );
        let meaningful = visible_labels(&code)
            .into_iter()
            .any(|label| !starter_label.is_match(&label));
        if !meaningful && !data_bound_content(&code) {
            return Err("matches a known untouched framework starter".to_owned());
        }
    }
    Ok(code)
}

fn ui_kind_leaf(ui_kind: &str) -> Option<String> {
    let parts = re(r"[A-Za-z_][A-Za-z0-9_]*")
        .find_iter(ui_kind)
        .map(|item| item.as_str().to_owned())
        .collect::<Vec<_>>();
    let leaf = parts.last()?.to_owned();
    if leaf.starts_with("Materialized")
        || NON_UI_KIND_SUFFIXES
            .iter()
            .any(|suffix| leaf.ends_with(suffix))
    {
        return None;
    }
    let namespace_tokens = parts[..parts.len().saturating_sub(1)]
        .iter()
        .flat_map(|part| words(part).into_iter())
        .collect::<BTreeSet<_>>();
    if namespace_tokens
        .iter()
        .any(|token| BACKEND_TOKENS.contains(&token.as_str()))
    {
        return None;
    }
    if UI_KIND_SUFFIXES.iter().any(|suffix| leaf.ends_with(suffix)) {
        return Some(leaf);
    }
    let namespace_ui_hint = parts[..parts.len().saturating_sub(1)].iter().any(|part| {
        ["GUI", "Gui", "UI", "Ui", "View", "Widget"]
            .iter()
            .any(|suffix| part.ends_with(suffix))
    });
    if namespace_ui_hint
        && [
            "Button", "Checkbox", "Combo", "Input", "Label", "List", "Menu", "Radio", "Select",
            "Table", "Text", "Tree",
        ]
        .contains(&leaf.as_str())
    {
        return Some(leaf);
    }
    None
}

fn kind_has_backend_import(code: &str, kind_leaf: &str) -> bool {
    code.lines().any(|line| {
        re(r"\b(?:import|from|include|use)\b").is_match(line)
            && re(&format!(r"\b{}\b", regex::escape(kind_leaf))).is_match(line)
            && words(line)
                .iter()
                .any(|word| BACKEND_TOKENS.contains(&word.as_str()))
    })
}

fn definition_extent(code: &str, start: usize, header_end: usize) -> &str {
    let search_end = (header_end + 800).min(code.len());
    if let Some(relative_brace) = code[header_end..search_end].find('{') {
        let brace = header_end + relative_brace;
        let mut depth = 0_i64;
        for (relative, ch) in code[brace..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &code[start..brace + relative + 1];
                    }
                }
                _ => {}
            }
        }
        return &code[start..];
    }
    let line_start = code[..start].rfind('\n').map_or(0, |index| index + 1);
    let base_indent = code[line_start..start]
        .chars()
        .take_while(|ch| ch.is_whitespace())
        .count();
    let mut end = code[header_end..]
        .find('\n')
        .map_or(code.len(), |index| header_end + index);
    let mut cursor = end.saturating_add(1);
    while cursor < code.len() {
        let line_end = code[cursor..]
            .find('\n')
            .map_or(code.len(), |index| cursor + index);
        let line = &code[cursor..line_end];
        if !line.trim().is_empty() {
            let indent = line.chars().take_while(|ch| ch.is_whitespace()).count();
            if indent <= base_indent {
                break;
            }
            end = line_end;
        }
        cursor = line_end.saturating_add(1);
    }
    &code[start..end]
}

fn find_definition(code: &str, anchor: &str) -> Option<(usize, usize)> {
    let escaped = regex::escape(anchor);
    let patterns = [
        format!(r"\b(?:class|struct|fn|def|function|func)\s+{escaped}\b"),
        format!(r"\b{escaped}\s*[:=]\s*(?:async\s+)?(?:function\b|\([^)]*\)\s*=>)"),
        format!(r"\b(?:[A-Za-z_][\w:<>,*&]*\s+)+{escaped}\s*\([^;{{}}]*\)\s*\{{"),
    ];
    patterns
        .iter()
        .filter_map(|pattern| re(pattern).find(code))
        .min_by_key(|found| found.start())
        .map(|found| (found.start(), found.end()))
}

fn explicit_basis_issue(code: &str, source_code: &str, basis: &ExplicitUiBasis) -> Option<String> {
    let ui_kind = &basis.ui_kind;
    let source_anchor = &basis.source_anchor;
    if ui_kind.trim() != ui_kind || source_anchor.trim() != source_anchor {
        return Some(
            "explicit UI kind and source anchor must not contain surrounding whitespace".to_owned(),
        );
    }
    for (label, value) in [("UI kind", ui_kind), ("source anchor", source_anchor)] {
        let normalized = re(r"[^a-z0-9]+")
            .replace_all(&value.to_ascii_lowercase(), "")
            .into_owned();
        if !(3..=MAX_BASIS_LENGTH).contains(&value.len()) || !re(r"[A-Za-z0-9]").is_match(value) {
            return Some(format!(
                "explicit {} must be a concise exact source token or construct",
                label.to_ascii_lowercase()
            ));
        }
        if [
            "app",
            "component",
            "main",
            "panel",
            "render",
            "screen",
            "surface",
            "ui",
            "view",
            "widget",
            "window",
        ]
        .contains(&normalized.as_str())
        {
            return Some(format!(
                "explicit {} is too generic to identify implemented UI",
                label.to_ascii_lowercase()
            ));
        }
        if !code.contains(value.as_str()) {
            return Some(format!(
                "explicit {} is not present in executable source outside comments",
                label.to_ascii_lowercase()
            ));
        }
    }
    if ui_kind == source_anchor {
        return Some(
            "explicit UI kind and source anchor must identify two distinct source facts".to_owned(),
        );
    }
    let Some(kind_leaf) = ui_kind_leaf(ui_kind) else {
        return Some(
            "explicit UI kind must name a UI-specific framework type or construct".to_owned(),
        );
    };
    if kind_has_backend_import(code, &kind_leaf) {
        return Some(
            "explicit UI kind is imported from a data, database, persistence, or storage module"
                .to_owned(),
        );
    }
    if !re(r"^[A-Za-z_][A-Za-z0-9_]*$").is_match(source_anchor) {
        return Some(
            "explicit source anchor must be one exact named screen or component symbol".to_owned(),
        );
    }
    let imported = code.lines().any(|line| {
        re(r"^\s*(?:import|from|use|include)\b").is_match(line)
            && (line.contains(ui_kind)
                || re(&format!(r"\b{}\b", regex::escape(&kind_leaf))).is_match(line))
    });
    let inherited = re(&format!(
        r"\b(?:extends|implements)\s+[^\n{{;]*\b{}\b|[:<][ \t]*[^\n{{;]*\b{}\b",
        regex::escape(&kind_leaf),
        regex::escape(&kind_leaf)
    ))
    .is_match(code);
    let constructed = re(&format!(r"\b{}\s*(?:::|\()", regex::escape(&kind_leaf))).is_match(code);
    if !(imported || inherited || constructed) {
        return Some(
            "explicit UI kind is not used as an imported, inherited, conformed, or constructed UI type"
                .to_owned(),
        );
    }
    if !(imported || inherited || ui_kind.contains("::") || ui_kind.contains('.')) {
        return Some(
            "explicit UI kind must be imported, inherited, conformed, or namespace-qualified; a local name alone is not framework evidence"
                .to_owned(),
        );
    }
    let Some((start, header_end)) = find_definition(code, source_anchor) else {
        return Some(
            "explicit source anchor is not a named screen or component definition".to_owned(),
        );
    };
    let definition = definition_extent(code, start, header_end);
    if !definition.contains(ui_kind)
        && !re(&format!(r"\b{}\b", regex::escape(&kind_leaf))).is_match(definition)
    {
        return Some(
            "explicit screen or component definition does not use the named UI kind".to_owned(),
        );
    }
    let source_definition = find_definition(source_code, source_anchor)
        .map(|(start, end)| definition_extent(source_code, start, end))
        .unwrap_or(source_code);
    let labels = quoted_values(source_definition)
        .into_iter()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    let has_strong = labels.iter().any(|value| placeholder(value));
    let nonplaceholder = labels.iter().filter(|value| !placeholder(value)).count();
    if has_strong
        && !data_bound_content(source_definition)
        && !override_dynamic_content(
            &without_string_literals(source_definition),
            source_definition,
        )
    {
        let has_action = re(r"(?i)\b(?:button|action|input|select)\s*(?:::|\()")
            .is_match(source_definition)
            && nonplaceholder > 0;
        if !has_action && nonplaceholder < 2 {
            return Some(
                "explicit screen or component definition contains only placeholder UI content"
                    .to_owned(),
            );
        }
    }
    None
}

pub fn qualify_implementation_source(
    repo: &Path,
    rel_path: &str,
    basis: Option<&ExplicitUiBasis>,
) -> Result<Qualification, String> {
    let code = candidate_code(repo, rel_path)?;
    let executable = without_string_literals(&code);
    let extension = suffix(rel_path);
    if let Some(issue) = web_surface_issue(&code, &executable, &extension) {
        if let Some(issue) = issue {
            return Err(issue);
        }
        return Ok(Qualification {
            method: "recognized-ui-signal".to_owned(),
            detector_version: DETECTOR_VERSION,
            ui_kind: None,
            source_anchor: None,
        });
    }
    if let Some(issue) = embedded_web_surface_issue(&code, &extension) {
        if let Some(issue) = issue {
            return Err(issue);
        }
        return Ok(Qualification {
            method: "recognized-ui-signal".to_owned(),
            detector_version: DETECTOR_VERSION,
            ui_kind: None,
            source_anchor: None,
        });
    }
    if code_placeholder_only(&code) {
        return Err("contains a placeholder-only target surface".to_owned());
    }
    if code_surface_kind(&code, &executable, &extension).is_some() {
        return Ok(Qualification {
            method: "recognized-ui-signal".to_owned(),
            detector_version: DETECTOR_VERSION,
            ui_kind: None,
            source_anchor: None,
        });
    }
    let Some(basis) = basis else {
        return Err(
            "does not contain a recognized executable UI construct; use the explicit source-anchor override only after manual inspection of an unrecognized UI framework"
                .to_owned(),
        );
    };
    if let Some(issue) = explicit_basis_issue(&executable, &code, basis) {
        return Err(issue);
    }
    Ok(Qualification {
        method: "explicit-source-anchor".to_owned(),
        detector_version: DETECTOR_VERSION,
        ui_kind: Some(basis.ui_kind.clone()),
        source_anchor: Some(basis.source_anchor.clone()),
    })
}

pub fn implementation_source_issue(
    repo: &Path,
    rel_path: &str,
    basis: Option<&ExplicitUiBasis>,
) -> Option<String> {
    qualify_implementation_source(repo, rel_path, basis).err()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Fixture {
        _directory: tempfile::TempDir,
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            Self {
                root: directory.path().to_owned(),
                _directory: directory,
            }
        }

        fn qualify(&self, path: &str, source: &str) -> Result<Qualification, String> {
            let target = self.root.join(path);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(&target, source).unwrap();
            qualify_implementation_source(&self.root, path, None)
        }

        fn qualify_override(
            &self,
            path: &str,
            source: &str,
            ui_kind: &str,
            source_anchor: &str,
        ) -> Result<Qualification, String> {
            let target = self.root.join(path);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(&target, source).unwrap();
            qualify_implementation_source(
                &self.root,
                path,
                Some(&ExplicitUiBasis {
                    ui_kind: ui_kind.to_owned(),
                    source_anchor: source_anchor.to_owned(),
                }),
            )
        }
    }

    #[test]
    fn recognizes_substantive_web_and_server_rendered_surfaces() {
        let fixture = Fixture::new();
        let cases = [
            (
                "src/SignOut.tsx",
                "export const SignOut = () => <button onClick={signOut}>Sign out</button>;",
            ),
            (
                "src/Dashboard.tsx",
                "export const Dashboard = () => <DashboardShell><ContactToolbar /><ContactList /></DashboardShell>;",
            ),
            (
                "src/VueDashboard.vue",
                "<template><dashboard-shell><contact-list /></dashboard-shell></template>",
            ),
            (
                "src/BrandedPage.tsx",
                "export const BrandedPage = () => <main><h1>Vite + React</h1><button onClick={deploy}>Deploy release</button></main>;",
            ),
            (
                "src/CustomizedStarter.tsx",
                "export const App = ({ contacts }) => <main><h1>Vite + React</h1><p>Edit src/App.tsx and save to test HMR</p><ul>{contacts.map(contact => <li>{contact.name}</li>)}</ul></main>;",
            ),
            (
                "src/NativeContacts.tsx",
                "export const Contacts = ({ contacts }) => <View><Text>Contacts</Text><FlatList data={contacts} renderItem={renderContact} /></View>;",
            ),
            (
                "templates/contacts.php",
                "<main><h1>Contacts</h1><ul><?php foreach ($contacts as $contact): ?><li><?= $contact->name ?></li><?php endforeach; ?></ul></main>",
            ),
            (
                "src/ContactsComponent.ts",
                "@Component({template: `<main><h1>Contacts</h1><ul><li *ngFor=\"let contact of contacts\">{{ contact.name }}</li></ul></main>`}) export class ContactsComponent {}",
            ),
            (
                "src/ContactsElement.ts",
                "export class ContactsElement extends LitElement { render() { return html`<main><h1>Contacts</h1><ul>${this.contacts.map(c => html`<li>${c.name}</li>`)}</ul></main>`; } }",
            ),
            (
                "src/LitFunctionalContacts.ts",
                "import { html } from 'lit-html'; export const Contacts = (contacts) => html`<main><h1>Contacts</h1><ul>${contacts.map(contact => html`<li>${contact.name}</li>`)}</ul></main>`;",
            ),
            (
                "src/contacts_fastapi.py",
                "from fastapi.responses import HTMLResponse\n@app.get('/contacts')\ndef contacts(): return HTMLResponse(status_code=200, content='<main><h1>Contacts</h1><p>Ada Lovelace</p></main>')",
            ),
            (
                "templates/contact-paragraphs.php",
                "<?php foreach ($contacts as $contact): ?><p><?= $contact->name ?></p><?php endforeach; ?>",
            ),
        ];
        for (path, source) in cases {
            assert!(fixture.qualify(path, source).is_ok(), "{path}");
        }
    }

    #[test]
    fn recognizes_substantive_native_and_code_constructed_surfaces() {
        let fixture = Fixture::new();
        let cases = [
            (
                "Sources/ContactsView.swift",
                "import SwiftUI\nstruct ContactsView: View { var body: some View { List { Text(\"Ada Lovelace\"); Button(\"Add contact\") {} } } }",
            ),
            (
                "Sources/ContactStack.swift",
                "import UIKit\nstruct ContactStack: UIViewRepresentable { func makeUIView(context: Context) -> UIStackView { let stack = UIStackView(); let button = UIButton(type: .system); button.setTitle(\"Add contact\", for: .normal); stack.addArrangedSubview(button); return stack } }",
            ),
            (
                "Sources/ReadOnlyContactsController.swift",
                "import UIKit\nclass ContactsController: UIViewController { override func viewDidLoad() { let label = UILabel(); label.text = \"Contacts\"; view.addSubview(label) } }",
            ),
            (
                "Sources/ContactsNativeView.swift",
                "import UIKit\nfinal class ContactsNativeView: UIView { override init(frame: CGRect) { let label = UILabel(); label.text = \"Contacts\"; addSubview(label) } }",
            ),
            (
                "Sources/ProfileImageController.swift",
                "import UIKit\nclass ProfileVC: UIViewController { override func viewDidLoad() { let photo = UIImageView(image: avatar); view.addSubview(photo) } }",
            ),
            (
                "Sources/NamedProfileImageController.swift",
                "import UIKit\nclass ProfileVC: UIViewController { override func viewDidLoad() { let photo = UIImageView(image: UIImage(named: \"avatar\")); view.addSubview(photo) } }",
            ),
            (
                "src/ContactsPanel.java",
                "import javax.swing.*; public final class ContactsPanel extends JPanel { public ContactsPanel() { add(new JButton(\"Add contact\")); } }",
            ),
            (
                "src/ContactsWidget.py",
                "from PyQt6.QtWidgets import *\nclass ContactsWidget(QWidget):\n def __init__(self):\n  layout=QVBoxLayout(self); layout.addWidget(QPushButton('Add contact'))",
            ),
            (
                "src/DynamicCompose.kt",
                "@Composable fun Contacts(name: String) { Text(name) }",
            ),
            (
                "src/DynamicFlutter.dart",
                "class Contacts extends StatelessWidget { Widget build(BuildContext context) { return Text(title); } }",
            ),
            (
                "src/DynamicStreamlit.py",
                "import streamlit as st\nst.dataframe(rows)",
            ),
            (
                "src/DynamicDomNode.js",
                "const node = document.createElement('div'); node.textContent = contact.name; document.body.appendChild(node);",
            ),
            (
                "res/layout/bound-list.xml",
                "<ListView ItemsSource=\"{Binding Contacts}\" />",
            ),
        ];
        for (path, source) in cases {
            assert!(fixture.qualify(path, source).is_ok(), "{path}");
        }
    }

    #[test]
    fn rejects_scaffolds_placeholders_evidence_and_blank_controls() {
        let fixture = Fixture::new();
        let cases = [
            (
                "src/RouteOnly.tsx",
                "export const RouteOnly = () => <Outlet />;",
            ),
            (
                "src/RootMount.tsx",
                "createRoot(document.querySelector('#root')).render(<App />);",
            ),
            (
                "src/AuthRoute.tsx",
                "export const AuthRoute = () => <AuthProvider><Outlet /></AuthProvider>;",
            ),
            (
                "src/ComingSoon.tsx",
                "export const Soon = () => <main><h1>Coming Soon</h1><p>Example</p></main>;",
            ),
            (
                "src/NoOpPlaceholder.tsx",
                "export const Soon = () => <main><h1>Contacts</h1><p>Coming soon</p><button onClick={() => {}}>Continue</button></main>;",
            ),
            ("templates/empty-button.html", "<button></button>"),
            ("templates/empty-link.html", "<a href=\"/\"></a>"),
            (
                "docs/mockups/contacts.html",
                "<main><h1>Contacts</h1></main>",
            ),
            ("src/theme.css", "main { display: grid; }"),
            (
                "stories/ContactList.stories.tsx",
                "export const Example = () => <main><h1>Contacts</h1></main>;",
            ),
            (
                "src/StreamlitConfig.py",
                "import streamlit as st\nst.set_page_config(page_title='Contacts')",
            ),
            (
                "src/EmptyStreamlit.py",
                "import streamlit as st\nst.title('')",
            ),
            (
                "src/EmptyAliasCompose.kt",
                "@Composable fun EmptyScreen() { val title = \"\"; Text(title) }",
            ),
            (
                "src/EmptyFlutter.dart",
                "class Empty extends StatelessWidget { Widget build(BuildContext context) { return Text(\"\"); } }",
            ),
            (
                "src/EmptyDomNode.js",
                "const node = document.createElement('div'); node.textContent = ''; document.body.appendChild(node);",
            ),
            (
                "res/layout/empty-control.xml",
                "<LinearLayout><Button /></LinearLayout>",
            ),
            (
                "res/layout/whitespace-list.xml",
                "<ListView ItemsSource=\"   \" />",
            ),
            (
                "src/WelcomeEmail.ts",
                "export const WelcomeEmail = () => `<main><h1>Welcome</h1></main>`;",
            ),
            (
                "Sources/EmptyContactsView.swift",
                "import SwiftUI\nstruct ContactsView: View {}",
            ),
            (
                "Sources/EmptyTextView.swift",
                "import SwiftUI\nstruct ContactsView: View { var body: some View { Text(\"\") } }",
            ),
            (
                "Sources/UnusedButtonController.swift",
                "import UIKit\nclass ContactsController: UIViewController { override func viewDidLoad() { let button = UIButton(type: .system) } }",
            ),
            (
                "Sources/BlankAttachedButtonController.swift",
                "import UIKit\nclass EmptyVC: UIViewController { override func viewDidLoad() { let blank = UIButton(); view.addSubview(blank) } }",
            ),
            (
                "src/EmptyContactsPanel.java",
                "import javax.swing.JPanel; public final class ContactsPanel extends JPanel {}",
            ),
            (
                "src/BlankAttachedSwingButton.java",
                "import javax.swing.*; public class Empty extends JPanel { public Empty(){ JButton blank = new JButton(\"\"); add(blank); } }",
            ),
        ];
        for (path, source) in cases {
            assert!(fixture.qualify(path, source).is_err(), "{path}");
        }
    }

    #[test]
    fn explicit_override_requires_a_bound_ui_type_and_named_definition() {
        let fixture = Fixture::new();
        let accepted = fixture
            .qualify_override(
                "src/contact_surface.canvas",
                "use canvas_kit::ContactSurface; pub fn build_contacts() -> ContactSurface { ContactSurface::new() }",
                "canvas_kit::ContactSurface",
                "build_contacts",
            )
            .unwrap();
        assert_eq!(accepted.method, "explicit-source-anchor");
        assert_eq!(accepted.detector_version, 2);
        assert!(fixture
            .qualify_override(
                "src/database_view.canvas",
                "use database::MaterializedView; pub fn build_contacts() -> MaterializedView { MaterializedView::new() }",
                "database::MaterializedView",
                "build_contacts",
            )
            .is_err());
        assert!(fixture
            .qualify_override(
                "src/unrelated.canvas",
                "use canvas_kit::ContactSurface; pub fn audit_ledger() -> usize { 0 } pub fn build_contacts() -> ContactSurface { ContactSurface::new() }",
                "canvas_kit::ContactSurface",
                "audit_ledger",
            )
            .unwrap_err()
            .contains("definition does not use"));
        assert!(
            fixture
                .qualify_override(
                    "src/local.py",
                    "class ReportView:\n    pass\n\ndef render_report():\n    return ReportView()",
                    "ReportView",
                    "render_report",
                )
                .unwrap_err()
                .contains("local name alone")
        );
    }
}
