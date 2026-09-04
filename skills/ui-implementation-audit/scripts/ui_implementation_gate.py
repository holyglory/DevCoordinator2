#!/usr/bin/env python3
"""Shared mechanical checks for explicit implemented-UI evidence."""

from __future__ import annotations

import re
from pathlib import Path


STYLE_OR_CATALOG_EXTENSIONS = {
    ".css",
    ".less",
    ".po",
    ".pot",
    ".sass",
    ".scss",
    ".styl",
    ".strings",
    ".xcstrings",
}
DOCUMENT_OR_CONFIG_EXTENSIONS = {
    ".json",
    ".jsonc",
    ".lock",
    ".markdown",
    ".md",
    ".rst",
    ".toml",
    ".txt",
    ".yaml",
    ".yml",
}
ASSET_EXTENSIONS = {
    ".avif",
    ".bmp",
    ".gif",
    ".ico",
    ".jpeg",
    ".jpg",
    ".pdf",
    ".png",
    ".svg",
    ".tif",
    ".tiff",
    ".webp",
}
EVIDENCE_ONLY_DIRS = {
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
}
EVIDENCE_ONLY_NAME_RE = re.compile(
    r"(?:^|[._-])(fixture|mockup|prototype|snapshot|spec|stories?|test)(?:[._-]|$)",
    re.IGNORECASE,
)
OUTBOUND_RENDER_SYMBOL_RE = re.compile(
    r"\b(?:class|def|function|struct)\s+[A-Za-z0-9_]+(?:Email|Mail|Notification)(?:Template)?\b|"
    r"\b(?:const|let|val|var)\s+[A-Za-z0-9_]+(?:Email|Mail|Notification)(?:Template)?\s*=",
)
WEB_MARKUP_EXTENSIONS = {
    ".astro",
    ".cshtml",
    ".ejs",
    ".hbs",
    ".handlebars",
    ".htm",
    ".html",
    ".j2",
    ".jinja",
    ".jinja2",
    ".js",
    ".jsx",
    ".liquid",
    ".mdx",
    ".mjs",
    ".mustache",
    ".njk",
    ".php",
    ".pug",
    ".razor",
    ".svelte",
    ".tpl",
    ".tsx",
    ".twig",
    ".vue",
}
NATIVE_MARKUP_EXTENSIONS = {".axaml", ".xaml", ".xml"}
WEB_TAG_RE = re.compile(
    r"<\s*(?P<tag>[A-Z][A-Za-z0-9_.:-]*|[a-z][a-z0-9]*-[a-z0-9-]+|"
    r"(?i:a|article|aside|button|details|dialog|div|figure|footer|form|h[1-6]|header|img|input|label|li|main|nav|ol|p|section|select|span|summary|table|tbody|td|textarea|th|thead|tr|ul))\b"
)
WEB_CONTROL_TAGS = {"a", "button", "dialog", "form", "input", "select", "table", "textarea"}
WEB_CONTENT_TAGS = {
    "article",
    "aside",
    "details",
    "div",
    "figure",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "img",
    "label",
    "li",
    "main",
    "nav",
    "ol",
    "p",
    "section",
    "span",
    "summary",
    "tbody",
    "td",
    "th",
    "thead",
    "tr",
    "ul",
}
SCAFFOLD_TAGS = {
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
}
STARTER_SIGNATURE_GROUPS = (
    ("vite + react", "edit src/app.tsx and save to test hmr"),
    ("get started by editing app/page", "save and see your changes instantly"),
    ("learn react", "logo.svg"),
)
STRONG_PLACEHOLDER_RE = re.compile(
    r"\b(?:coming soon|under construction|work in progress|not implemented|placeholder)\b",
    re.IGNORECASE,
)
GENERIC_EXAMPLE_RE = re.compile(r"^\s*example\s*$", re.IGNORECASE)
UI_STRING_LABEL_RE = re.compile(
    r"\b(?:Button|JButton|JLabel|Label|QLabel|QPushButton|Text|TextBlock|TextView)\s*\(\s*['\"]([^'\"]+)['\"]",
    re.IGNORECASE,
)
ACTION_STRING_LABEL_RE = re.compile(
    r"\b(?:Button|JButton|QPushButton|st\.button|gr\.Button)\s*\(\s*['\"]([^'\"]+)['\"]",
    re.IGNORECASE,
)
DETECTOR_VERSION = 2
MAX_BASIS_LENGTH = 180
GENERIC_BASIS_VALUES = {
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
}
UI_KIND_SUFFIXES = ("UI", "View", "Widget", "Screen", "Panel", "Surface", "Component", "Window", "Dialog", "Page", "Canvas", "Layout")
NON_UI_KIND_SUFFIXES = ("ViewModel", "Model", "State", "Data", "Record", "Policy", "Service", "Controller")
UI_CONTROL_KIND_NAMES = {"Button", "Checkbox", "Combo", "Input", "Label", "List", "Menu", "Radio", "Select", "Table", "Text", "Tree"}


def is_evidence_only_interface_path(rel_path: str) -> bool:
    path = Path(rel_path)
    parts = {part.lower() for part in path.parts[:-1]}
    return bool(parts & EVIDENCE_ONLY_DIRS or EVIDENCE_ONLY_NAME_RE.search(path.name.lower()))


def _without_comments(text: str) -> str:
    text = re.sub(r"/\*.*?\*/", " ", text, flags=re.DOTALL)
    text = re.sub(r"<!--.*?-->", " ", text, flags=re.DOTALL)
    text = re.sub(r"(?m)^\s*(?://|#).*$", " ", text)
    return text


def _without_string_literals(text: str) -> str:
    return re.sub(
        r'''(?s)(?:"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`(?:\\.|[^`\\])*`)''',
        lambda match: " " * len(match.group(0)),
        text,
    )


ESCAPED_WHITESPACE_RE = re.compile(
    r"\\(?:[fnrtv]|x(?:09|0a|0b|0c|0d|20)|u(?:0009|000a|000b|000c|000d|0020)|"
    r"U(?:00000009|0000000a|0000000b|0000000c|0000000d|00000020))",
    re.IGNORECASE,
)


def _literal_has_visible_content(value: str) -> bool:
    return bool(ESCAPED_WHITESPACE_RE.sub("", value).strip())


def _visible_labels(code: str) -> list[str]:
    labels = [re.sub(r"\s+", " ", item).strip(" \t\r\n.,:;!?-_") for item in re.findall(r">([^<>{}]+)<", code)]
    labels.extend(
        match.group(1).strip()
        for match in re.finditer(
            r"\b(?:aria-label|placeholder|title|value)\s*=\s*['\"]([^'\"]+)['\"]",
            code,
            flags=re.IGNORECASE,
        )
    )
    return [item for item in labels if _literal_has_visible_content(item)]


def _is_scaffold_tag(tag: str) -> bool:
    folded = tag.casefold()
    leaf = folded.rsplit(".", 1)[-1]
    return bool(
        leaf in SCAFFOLD_TAGS | {"layout", "page", "root", "screen"}
        or leaf.endswith(("boundary", "guard", "layout", "provider", "router", "shell"))
        or leaf.startswith(("require", "router"))
    )


def _data_bound_content(code: str) -> bool:
    return bool(
        re.search(
            r"(?:\.map\s*\(|\bforeach\b|\bfor\s*\(|\bv-for\s*=|\*ngFor\s*=|\{#each\b|\bForEach\s*\(|\bitems\s*\()",
            code,
            flags=re.IGNORECASE,
        )
    )


def _bound_value_output(code: str) -> bool:
    return bool(
        re.search(
            r"<\?=|\{\{[^{}]*[A-Za-z_$][^{}]*\}\}|\$\{[^{}]*[A-Za-z_$][^{}]*\}|"
            r"\{\s*[A-Za-z_$][\w.$\[\]]+\s*\}",
            code,
        )
    )


def _provably_empty_aliases(code: str) -> set[str]:
    aliases: set[str] = set()
    assignment = re.compile(
        r"(?:^|[;{])\s*(?:(?:const|let|val|var|String|string)\s+)?"
        r"(?P<name>[A-Za-z_]\w*)(?:\s*:\s*[^=;\n]+)?\s*=\s*"
        r"(?P<quote>['\"])(?P<value>.*?)(?P=quote)",
        re.MULTILINE,
    )
    for match in assignment.finditer(code):
        if not _literal_has_visible_content(match.group("value")):
            aliases.add(match.group("name"))
    null_assignment = re.compile(
        r"(?:^|[;{])\s*(?:(?:const|let|val|var|String|string)\s+)?"
        r"(?P<name>[A-Za-z_]\w*)(?:\s*:\s*[^=;\n]+)?\s*=\s*(?:nil|null|None)\b",
        re.MULTILINE,
    )
    aliases.update(match.group("name") for match in null_assignment.finditer(code))
    empty_constructor = re.compile(
        r"(?:^|[;{])\s*(?:(?:const|let|val|var|String|string)\s+)?"
        r"(?P<name>[A-Za-z_]\w*)(?:\s*:\s*[^=;\n]+)?\s*=\s*"
        r"(?:Data|NSData|NSImage|String|UIImage)\s*\(\s*\)",
        re.MULTILINE,
    )
    aliases.update(match.group("name") for match in empty_constructor.finditer(code))
    alias_assignment = re.compile(
        r"(?:^|[;{])\s*(?:(?:const|let|val|var|String|string)\s+)?"
        r"(?P<name>[A-Za-z_]\w*)(?:\s*:\s*[^=;\n]+)?\s*=\s*(?P<source>[A-Za-z_]\w*)\b",
        re.MULTILINE,
    )
    changed = True
    while changed:
        changed = False
        for match in alias_assignment.finditer(code):
            if match.group("source") in aliases and match.group("name") not in aliases:
                aliases.add(match.group("name"))
                changed = True
    return aliases


def _dynamic_value_is_substantive(value: str, source_code: str) -> bool:
    root = re.match(r"[A-Za-z_$][\w$]*", value)
    return bool(
        root
        and root.group(0) not in {"nil", "null", "None"}
        and root.group(0) not in _provably_empty_aliases(source_code)
    )


def _empty_constructor_follows(code: str, end: int, value: str) -> bool:
    root = value.split(".", 1)[0]
    return bool(
        root in {"Data", "NSData", "NSImage", "String", "UIImage"}
        and re.match(r"\s*\(\s*\)", code[end:])
    )


def _dynamic_ui_content(executable_code: str, source_code: str) -> bool:
    patterns = (
        r"\b(?:Text|Markdown)\s*\(\s*(?P<value>[A-Za-z_$][\w.$\[\]]*)",
        r"\b(?:st|gr)\.(?:data_editor|dataframe|markdown|table|text|title|write)\s*\(\s*"
        r"(?P<value>[A-Za-z_$][\w.$\[\]]*)",
        r"\.\s*(?:innerHTML|textContent|innerText|text)\s*=\s*(?P<value>[A-Za-z_$][\w.$\[\]]*)",
        r"\b(?:Button|Entry|Label)\s*\([^)]*\btext\s*=\s*(?P<value>[A-Za-z_$][\w.$\[\]]*)",
        r"\bsetTitle\s*\(\s*(?P<value>[A-Za-z_$][\w.$\[\]]*)",
        r"\b(?:NSImageView|UIImageView)\s*\(\s*image\s*:\s*(?P<value>[A-Za-z_$][\w.$\[\]]*)",
    )
    return any(
        _dynamic_value_is_substantive(match.group("value"), source_code)
        and not _empty_constructor_follows(executable_code, match.end("value"), match.group("value"))
        for pattern in patterns
        for match in re.finditer(pattern, executable_code)
    )


def _override_dynamic_content(executable_code: str, source_code: str) -> bool:
    if _dynamic_ui_content(executable_code, source_code):
        return True
    return any(
        _dynamic_value_is_substantive(match.group("value"), source_code)
        and not _empty_constructor_follows(executable_code, match.end("value"), match.group("value"))
        for match in re.finditer(
            r"\.\w+\s*\(\s*(?P<value>[A-Za-z_$][\w.$\[\]]*)\s*\)",
            executable_code,
        )
    )


def _has_substantive_code_content(code: str, executable_code: str) -> bool:
    return bool(
        _framework_labels(code)
        or _data_bound_content(code)
        or _dynamic_ui_content(executable_code, code)
        or re.search(
            r"\b(?:JList|JTable|NSCollectionView|NSTableView|QListView|QTableView|UICollectionView|UITableView)\s*\(",
            executable_code,
        )
    )


def _is_product_component_tag(tag: str) -> bool:
    leaf = tag.casefold().rsplit(".", 1)[-1].replace("-", "").replace("_", "")
    return leaf.endswith(
        (
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
        )
    )


def _interactive_product_ui_signal(code: str) -> bool:
    return bool(
        re.search(
            r"\bon[A-Z][A-Za-z]*\s*=|\bv-on:|@click\s*=|<\s*(?:form|input|select|textarea)\b|"
            r"\b(?:Button|TextField)\s*\(|\bst\.button\s*\(",
            code,
        )
    )


def _outbound_renderer_path(rel_path: Path) -> bool:
    tokens = {
        token
        for part in rel_path.parts
        for token in re.findall(r"[a-z0-9]+", part.casefold().replace("_", " "))
    }
    return bool(tokens & {"email", "emails", "mail", "mails", "notification", "notifications"})


def _web_surface_issue(code: str, executable_code: str, suffix: str) -> tuple[bool, str | None]:
    if suffix not in WEB_MARKUP_EXTENSIONS:
        return False, None
    tags = [match.group("tag") for match in WEB_TAG_RE.finditer(executable_code)]
    if not tags:
        return False, None
    labels = _visible_labels(code)
    product_tags = [tag for tag in tags if not _is_scaffold_tag(tag)]
    intrinsic = [tag.casefold() for tag in product_tags if tag.casefold() in WEB_CONTROL_TAGS | WEB_CONTENT_TAGS]
    custom = [tag for tag in product_tags if tag.casefold() not in WEB_CONTROL_TAGS | WEB_CONTENT_TAGS]
    handler = re.search(r"\b(?:action|href|on[A-Z][A-Za-z]*|v-on:|@click)\s*=", code)
    no_op_handler = re.search(
        r"\bon[A-Z][A-Za-z]*\s*=\s*\{\s*(?:\([^)]*\)\s*=>\s*)?\{\s*\}\s*\}",
        code,
    )
    functional_control = bool(any(tag in WEB_CONTROL_TAGS for tag in intrinsic) and handler and not no_op_handler)
    data_bound_content = _data_bound_content(code)
    nonplaceholder_labels = [label for label in labels if not STRONG_PLACEHOLDER_RE.search(label)]
    if (
        any(STRONG_PLACEHOLDER_RE.search(label) for label in labels)
        and not data_bound_content
        and not (functional_control and len(nonplaceholder_labels) >= 2)
    ):
        return True, "contains a placeholder-only target surface"
    if labels and all(GENERIC_EXAMPLE_RE.fullmatch(label) for label in labels) and not functional_control:
        return True, "contains a placeholder-only target surface"
    if not product_tags:
        return True, "contains only route, provider, mount, or framework scaffolding"
    if (
        re.search(r"\b(?:createRoot|ReactDOM\.render)\s*\(", executable_code)
        or re.search(r"<\s*Outlet\b", executable_code)
    ) and not intrinsic:
        return True, "contains only wrapped route, provider, mount, or framework scaffolding"
    visible_input = bool(
        any(tag in {"input", "select", "textarea"} for tag in intrinsic)
        and re.search(r"\b(?:id|name|placeholder|type|value)\s*=", code, flags=re.IGNORECASE)
    )
    if any(tag in WEB_CONTROL_TAGS for tag in intrinsic) and (labels or visible_input):
        return True, None
    if len(intrinsic) >= 2 and (labels or data_bound_content):
        return True, None
    if data_bound_content and intrinsic and (
        any(tag in {"article", "li", "p", "td", "tr"} for tag in intrinsic)
        or _bound_value_output(code)
    ):
        return True, None
    if len(custom) >= 2 and (labels or data_bound_content):
        return True, None
    if custom and len(tags) >= 2 and any(_is_product_component_tag(tag) for tag in custom):
        return True, None
    return True, "does not construct substantive visible product content or controls"


def _parenthesized_extent(code: str, open_index: int) -> str:
    depth = 0
    index = open_index
    quote = ""
    while index < len(code):
        if quote:
            if code[index] == "\\":
                index += 2
                continue
            if code.startswith(quote, index):
                index += len(quote)
                quote = ""
                continue
            index += 1
            continue
        if code.startswith("'''", index) or code.startswith('\"\"\"', index):
            quote = code[index : index + 3]
            index += 3
            continue
        if code[index] in {"'", '\"'}:
            quote = code[index]
            index += 1
            continue
        if code[index] == "(":
            depth += 1
        elif code[index] == ")":
            depth -= 1
            if depth == 0:
                return code[open_index : index + 1]
        index += 1
    return code[open_index:]


def _embedded_web_surface_issue(code: str, suffix: str) -> tuple[bool, str | None]:
    snippets: list[str] = []
    patterns: list[str] = []
    if re.search(r"@Component\s*\(", code):
        patterns.append(r"\btemplate\s*:\s*(?P<quote>`|'|\")(?P<body>.*?)(?P=quote)")
    lit_component = bool(
        re.search(r"\b(?:LitElement|ReactiveElement)\b", code) and re.search(r"\brender\s*\(", code)
    )
    lit_function = bool(
        re.search(r"\bfrom\s*['\"](?:lit|lit-element|lit-html)(?:/[^'\"]*)?['\"]", code)
        and (
            re.search(r"\bfunction\s+[A-Z][A-Za-z0-9_]*\s*\([^)]*\)\s*\{[^{}]*\breturn\s+html\s*`", code, re.DOTALL)
            or re.search(
                r"\b(?:const|let)\s+[A-Z][A-Za-z0-9_]*\s*=\s*(?:\([^)]*\)|[A-Za-z_]\w*)\s*=>\s*html\s*`",
                code,
                re.DOTALL,
            )
        )
    )
    if lit_component or lit_function:
        patterns.append(r"\bhtml\s*(?P<quote>`)(?P<body>.*?)(?P=quote)")
    if suffix == ".php":
        patterns.append(r"\b(?:echo|print)\s*(?P<quote>'|\")(?P<body>.*?)(?P=quote)")
    python_route = suffix == ".py" and re.search(
        r"@[A-Za-z_]\w*\.(?:get|route)\s*\(", code, re.IGNORECASE
    )
    if python_route:
        patterns.append(
            r"\breturn\s*(?:[rubfRUBF]{1,2})?(?P<quote>'''|\"\"\"|'|\")(?P<body>.*?)(?P=quote)"
        )
        patterns.append(
            r"\b(?:return\s+)?(?:HTMLResponse|Response)\s*\(\s*(?:content\s*=\s*)?"
            r"(?:[rubfRUBF]{1,2})?(?P<quote>'''|\"\"\"|'|\")(?P<body>.*?)(?P=quote)"
        )
        for call_match in re.finditer(r"\b(?:HTMLResponse|Response)\s*\(", code):
            call = _parenthesized_extent(code, call_match.end() - 1)
            for call_pattern in (
                r"^\(\s*(?:[rubfRUBF]{1,2})?(?P<quote>'''|\"\"\"|'|\")(?P<body>.*?)(?P=quote)",
                r"\bcontent\s*=\s*(?:[rubfRUBF]{1,2})?(?P<quote>'''|\"\"\"|'|\")"
                r"(?P<body>.*?)(?P=quote)",
            ):
                snippets.extend(
                    match.group("body") for match in re.finditer(call_pattern, call, flags=re.DOTALL)
                )
        patterns.append(
            r"\b(?:return\s+)?(?:HTMLResponse|Response)\s*\([^)]*?\bcontent\s*=\s*"
            r"(?:[rubfRUBF]{1,2})?(?P<quote>'''|\"\"\"|'|\")(?P<body>.*?)(?P=quote)"
        )
    for pattern in patterns:
        snippets.extend(match.group("body") for match in re.finditer(pattern, code, flags=re.DOTALL))
    checked = False
    last_issue: str | None = None
    for snippet in snippets:
        snippet_checked, issue = _web_surface_issue(snippet, _without_comments(snippet), ".html")
        checked = checked or snippet_checked
        if snippet_checked and issue is None:
            return True, None
        if snippet_checked:
            last_issue = issue
    return checked, last_issue


def _has_nonblank_literal(text: str) -> bool:
    return any(
        _literal_has_visible_content(value)
        for _, value in re.findall(r"(['\"])(.*?)\1", text, flags=re.DOTALL)
    )


def _control_args_are_substantive(kind: str, args: str, source_code: str) -> bool:
    leaf = kind.rsplit("::", 1)[-1]
    if leaf in {
        "JList",
        "JTable",
        "NSCollectionView",
        "NSTableView",
        "QListView",
        "QTableView",
        "UICollectionView",
        "UITableView",
    }:
        return True
    if leaf in {"NSImageView", "UIImageView"}:
        if _has_nonblank_literal(args):
            return True
        executable_args = _without_string_literals(args)
        if re.search(
            r"\bimage\s*:\s*(?:NSImage|UIImage)\s*\(\s*(?:data\s*:\s*(?:Data|NSData)\s*\(\s*)?$",
            executable_args,
        ):
            return False
        image = re.search(r"\bimage\s*:\s*(?P<value>[A-Za-z_$][\w.$\[\]]*)", executable_args)
        return bool(image and _dynamic_value_is_substantive(image.group("value"), source_code))
    if _has_nonblank_literal(args):
        return True
    if leaf in {"JButton", "JLabel", "QLabel", "QPushButton"}:
        value = re.match(
            r"\s*(?![A-Za-z_]\w*\s*[:=])(?P<value>[A-Za-z_$][\w.$\[\]]*)",
            _without_string_literals(args),
        )
        return bool(value and _dynamic_value_is_substantive(value.group("value"), source_code))
    return False


def _named_control_binding_is_substantive(code: str, name: str, source_code: str | None = None) -> bool:
    source_code = source_code or code
    escaped = re.escape(name)
    literal_patterns = (
        rf"\b{escaped}\s*\.\s*(?:image|innerHTML|innerText|placeholder|stringValue|text|textContent|value)"
        rf"\s*=\s*(['\"])(.*?)\1",
        rf"\b{escaped}\s*\.\s*(?:setText|setTitle)\s*\(\s*(['\"])(.*?)\1",
    )
    for pattern in literal_patterns:
        if any(
            _literal_has_visible_content(match.group(2))
            for match in re.finditer(pattern, code, flags=re.DOTALL)
        ):
            return True
    executable_code = _without_string_literals(code)
    dynamic_patterns = (
        (
            rf"\b{escaped}\s*\.\s*(?:image|innerHTML|innerText|placeholder|stringValue|text|textContent|value)"
            rf"\s*=\s*(?P<value>[A-Za-z_$][\w.$\[\]]*)"
        ),
        (
            rf"\b{escaped}\s*\.\s*(?:setText|setTitle)\s*\(\s*"
            rf"(?P<value>[A-Za-z_$][\w.$\[\]]*)"
        ),
    )
    return any(
        _dynamic_value_is_substantive(match.group("value"), source_code)
        and not _empty_constructor_follows(executable_code, match.end("value"), match.group("value"))
        for pattern in dynamic_patterns
        for match in re.finditer(pattern, executable_code)
    )


def _dom_surface_is_substantive(code: str) -> bool:
    for match in re.finditer(
        r"\b(?:const|let|var)\s+(?P<name>[A-Za-z_]\w*)\s*=\s*document\.createElement\s*\(",
        code,
    ):
        name = match.group("name")
        if re.search(
            rf"\b(?:appendChild|replaceChildren)\s*\([^)]*\b{re.escape(name)}\b",
            code[match.end() :],
        ) and _named_control_binding_is_substantive(code[match.end() :], name, code):
            return True
    return False


def _attached_control_is_substantive(
    code: str,
    constructor: str,
    attach: str,
    declaration: str,
) -> bool:
    for match in re.finditer(
        rf"{attach}\s*\(\s*(?:new\s+)?(?P<kind>{constructor})\s*\((?P<args>[^)]*)\)",
        code,
    ):
        if _control_args_are_substantive(match.group("kind"), match.group("args"), code):
            return True
    for match in re.finditer(
        rf"{declaration}(?P<name>[A-Za-z_]\w*)(?:\s*:\s*[A-Za-z_][\w.<>,? ]*)?\s*=\s*"
        rf"(?:new\s+)?(?P<kind>{constructor})\s*\((?P<args>[^)]*)\)",
        code,
    ):
        name = match.group("name")
        if re.search(rf"{attach}\s*\(\s*{re.escape(name)}\s*\)", code[match.end() :]) and (
            _control_args_are_substantive(match.group("kind"), match.group("args"), code)
            or _named_control_binding_is_substantive(code[match.end() :], name, code)
        ):
            return True
    return False


def _swiftui_named_literal_has_content(code: str) -> bool:
    return any(
        _literal_has_visible_content(match.group("value"))
        for match in re.finditer(
            r"\b(?:Image|Label|Text)\s*\(\s*[A-Za-z_]\w*\s*:\s*"
            r"(?P<quote>['\"])(?P<value>.*?)(?P=quote)",
            code,
            flags=re.DOTALL,
        )
    )


def _swiftui_dynamic_has_content(code: str, executable_code: str) -> bool:
    patterns = (
        r"\b(?:Button|Canvas|ForEach|Image|Label|Picker|Table|Text|TextField)\s*\(\s*"
        r"(?![A-Za-z_]\w*\s*:)(?P<value>[A-Za-z_$][\w.$\[\]]*)",
        r"\b(?:Button|Canvas|ForEach|Image|Label|Picker|Table|Text|TextField)\s*\(\s*"
        r"[A-Za-z_]\w*\s*:\s*(?P<value>[A-Za-z_$][\w.$\[\]]*)",
    )
    return any(
        _dynamic_value_is_substantive(match.group("value"), code)
        for pattern in patterns
        for match in re.finditer(pattern, executable_code)
    )


def _swiftui_has_content(code: str, executable_code: str) -> bool:
    if _data_bound_content(code):
        return True
    if any(label for label in _framework_labels(code) if label):
        return True
    return bool(
        _swiftui_dynamic_has_content(code, executable_code)
        or _swiftui_named_literal_has_content(code)
        or re.search(r"\b[A-Z][A-Za-z0-9_]+(?:List|Screen|View|Widget)\s*(?:\(|\{)", executable_code)
        or re.search(
            r"\b(?!Button\b|Canvas\b|ForEach\b|Image\b|Label\b|Picker\b|Table\b|Text\b|TextField\b|"
            r"HStack\b|LazyHGrid\b|LazyVGrid\b|List\b|ScrollView\b|VStack\b|ZStack\b)"
            r"[A-Z][A-Za-z0-9_]+\s*\([^)]*[A-Za-z_$][^)]*\)",
            executable_code,
        )
    )


def _code_surface_kind(code: str, executable_code: str, suffix: str) -> str | None:
    if re.search(r"\bvar\s+body\s*:\s*some\s+View\b", executable_code) and _swiftui_has_content(
        code, executable_code
    ):
        return "swiftui-view"
    if re.search(
        r"\b(?:NSViewRepresentable|NSViewControllerRepresentable|UIViewRepresentable|UIViewControllerRepresentable)\b",
        executable_code,
    ) and re.search(r"\bmake(?:NS|UI)View(?:Controller)?\s*\(", executable_code) and _attached_control_is_substantive(
        code,
        r"(?:NSButton|NSCollectionView|NSImageView|NSTableView|NSTextField|UIButton|UICollectionView|UIImageView|UILabel|UITableView|UITextField)",
        r"(?:addArrangedSubview|addSubview)",
        r"(?:let|var)\s+",
    ):
        return "native-view-representable"
    if re.search(r"\b(?:NSViewController|UIViewController)\b", executable_code) and re.search(
        r"\b(?:loadView|viewDidLoad)\s*\(", executable_code
    ) and _attached_control_is_substantive(
        code,
        r"(?:NSButton|NSCollectionView|NSImageView|NSTableView|NSTextField|UIButton|UICollectionView|UIImageView|UILabel|UITableView|UITextField)",
        r"(?:addArrangedSubview|addSubview)",
        r"(?:let|var)\s+",
    ):
        return "native-view-controller"
    if re.search(r"\b(?:class|struct)\s+\w+[^\n{]{0,120}:\s*(?:NSView|UIView)\b", executable_code) and _attached_control_is_substantive(
        code,
        r"(?:NSButton|NSCollectionView|NSImageView|NSTableView|NSTextField|UIButton|UICollectionView|UIImageView|UILabel|UITableView|UITextField)",
        r"(?:addArrangedSubview|addSubview)",
        r"(?:let|var)\s+",
    ):
        return "native-view-subclass"
    if re.search(r"\bextends\s+(?:JComponent|JDialog|JFrame|JPanel|JWindow)\b", executable_code) and _attached_control_is_substantive(
        code,
        r"(?:JButton|JLabel|JList|JMenu|JTable|JTextArea|JTextField)",
        r"add",
        r"(?:[A-Za-z_]\w*(?:<[^>]+>)?\s+)",
    ):
        return "java-swing-view"
    if re.search(r"@Composable\b", executable_code) and re.search(
        r"\b(?:Button|Text|TextField)\s*\(|\bitems\s*\(", executable_code
    ) and _has_substantive_code_content(code, executable_code):
        return "compose-view"
    if re.search(r"\bWidget\s+build\s*\(", executable_code) and re.search(
        r"\b(?:AppBar|ListView|Text|TextField)\s*\(|\bchildren\s*:",
        executable_code,
    ) and _has_substantive_code_content(code, executable_code):
        return "flutter-view"
    if re.search(
        r"\b(?:QMainWindow|QWidget)\s*\(|\bclass\s+\w+\s*\(\s*(?:QMainWindow|QWidget)\s*\)",
        executable_code,
    ) and _attached_control_is_substantive(
        code,
        r"(?:QPushButton|QLabel|QLineEdit|QListView|QTableView)",
        r"addWidget",
        r"",
    ):
        return "qt-view"
    if re.search(r"\bTk\s*\(", executable_code) and re.search(
        r"\b(?:Button|Entry|Label|Listbox|Text|Treeview)\s*\(", executable_code
    ) and re.search(r"\.(?:grid|pack|place)\s*\(", executable_code) and _has_substantive_code_content(
        code, executable_code
    ):
        return "tk-view"
    if re.search(r"\bTk\s*\(", executable_code) and re.search(r"\bCanvas\s*\(", executable_code) and re.search(
        r"\.create_(?:arc|image|line|oval|polygon|rectangle|text)\s*\(", executable_code
    ):
        return "tk-canvas-view"
    if (
        re.search(
            r"\bst\.(?:button|chat_input|data_editor|dataframe|file_uploader|header|image|markdown|multiselect|number_input|radio|selectbox|slider|subheader|table|text|text_area|text_input|title|write)\s*\(",
            executable_code,
        )
        or re.search(
            r"\bgr\.(?:Button|ChatInterface|Checkbox|Dataframe|Dropdown|Gallery|HTML|Image|Interface|Markdown|Radio|Slider|Textbox)\s*\(",
            executable_code,
        )
    ) and _has_substantive_code_content(code, executable_code):
        return "declarative-python-view"
    if _dom_surface_is_substantive(code):
        return "dom-view"
    if suffix in NATIVE_MARKUP_EXTENSIONS:
        collection = re.search(
            r"<\s*(?:[A-Za-z_][\w.-]*(?::|\.))?"
            r"(?:CollectionView|ListView|RecyclerView|TableView)\b(?P<attrs>[^>]*)>",
            code,
            re.IGNORECASE,
        )
        binding = (
            re.search(
                r"\b(?:ItemsSource|adapter|android:entries|data|entries|items|itemTemplate)\s*=\s*"
                r"(?P<quote>['\"])(?P<value>.*?)(?P=quote)",
                collection.group("attrs"),
                re.IGNORECASE | re.DOTALL,
            )
            if collection
            else None
        )
        if binding and _literal_has_visible_content(binding.group("value")):
            return "declarative-native-view"
    if suffix in NATIVE_MARKUP_EXTENSIONS and (
        re.search(
            r"<\s*(?:[A-Za-z_][\w.-]*:)?(?:Button|Image|Label|TextBlock|TextField|TextView)\b[^>]*"
            r"(?:android:text|aria-label|contentDescription|src|text)\s*=\s*['\"][^'\"]+['\"]",
            code,
            re.IGNORECASE,
        )
        or re.search(
            r"<\s*(?:[A-Za-z_][\w.-]*:)?(?:Button|Label|TextBlock|TextView)\b[^>]*>\s*[^<{][^<]*<",
            code,
            re.IGNORECASE,
        )
    ):
        return "declarative-native-view"
    return None


def _framework_labels(code: str) -> list[str]:
    labels = [match.group(1).strip() for match in UI_STRING_LABEL_RE.finditer(code)]
    patterns = (
        r"\b(?:st|gr)\.[A-Za-z_]\w*\s*\(\s*['\"]([^'\"]+)['\"]",
        r"\.\s*(?:innerHTML|textContent|innerText|text)\s*=\s*['\"]([^'\"]+)['\"]",
        r"\bsetTitle\s*\(\s*['\"]([^'\"]+)['\"]",
        r"\.\s*(?:stringValue|text)\s*=\s*['\"]([^'\"]+)['\"]",
        r"\b(?:Button|Entry|Label)\s*\([^)]*\btext\s*=\s*['\"]([^'\"]+)['\"]",
        r"\b(?:android:text|contentDescription)\s*=\s*['\"]([^'\"]+)['\"]",
        r"\bNSTextField\s*\(\s*labelWithString\s*:\s*['\"]([^'\"]+)['\"]",
    )
    for pattern in patterns:
        labels.extend(match.group(1).strip() for match in re.finditer(pattern, code, flags=re.IGNORECASE))
    return [label for label in labels if _literal_has_visible_content(label)]


def _code_placeholder_only(code: str) -> bool:
    labels = _framework_labels(code)
    strong = [label for label in labels if STRONG_PLACEHOLDER_RE.search(label)]
    if strong:
        if _data_bound_content(code):
            return False
        action_labels = [match.group(1).strip() for match in ACTION_STRING_LABEL_RE.finditer(code)]
        return not any(label and not STRONG_PLACEHOLDER_RE.search(label) for label in action_labels)
    return bool(labels and all(GENERIC_EXAMPLE_RE.fullmatch(label) for label in labels))


def _ui_kind_leaf(ui_kind: str) -> str | None:
    parts = re.findall(r"[A-Za-z_][A-Za-z0-9_]*", ui_kind)
    if not parts:
        return None
    leaf = parts[-1]
    if leaf.startswith("Materialized") or leaf.endswith(NON_UI_KIND_SUFFIXES):
        return None
    namespace_tokens = {
        token
        for part in parts[:-1]
        for token in re.findall(r"[a-z0-9]+", part.casefold().replace("_", " "))
    }
    if namespace_tokens & {
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
        }:
        return None
    folded_tokens = [item.casefold() for item in re.split(r"_+", leaf) if item]
    if leaf.endswith(UI_KIND_SUFFIXES) or (folded_tokens and folded_tokens[-1] in {item.casefold() for item in UI_KIND_SUFFIXES}):
        return leaf
    namespace_ui_hint = any(
        part.endswith(("GUI", "Gui", "UI", "Ui", "View", "Widget"))
        for part in parts[:-1]
    )
    if namespace_ui_hint and leaf in UI_CONTROL_KIND_NAMES:
        return leaf
    return None


def _kind_has_backend_import(code: str, kind_leaf: str) -> bool:
    backend = {
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
    }
    for line in code.splitlines():
        if re.search(r"\b(?:import|from|include|use)\b", line) and re.search(
            rf"\b{re.escape(kind_leaf)}\b", line
        ) and backend.intersection(re.findall(r"[a-z0-9]+", line.casefold().replace("_", " "))):
            return True
    return False


def _definition_extent(code: str, definition: re.Match[str]) -> str:
    header_start = definition.start()
    brace = code.find("{", definition.end(), min(len(code), definition.end() + 800))
    if brace != -1:
        depth = 0
        for index in range(brace, len(code)):
            if code[index] == "{":
                depth += 1
            elif code[index] == "}":
                depth -= 1
                if depth == 0:
                    return code[header_start : index + 1]
        return code[header_start:]

    line_start = code.rfind("\n", 0, header_start) + 1
    line_end = code.find("\n", definition.end())
    if line_end == -1:
        return code[header_start:]
    base_indent = len(code[line_start:header_start]) - len(code[line_start:header_start].lstrip())
    end = line_end
    for match in re.finditer(r"(?m)^(?P<indent>[ \t]*)(?P<body>\S.*)$", code[line_end + 1 :]):
        absolute_start = line_end + 1 + match.start()
        indent = len(match.group("indent").expandtabs(4))
        if indent <= base_indent:
            break
        end = line_end + 1 + match.end()
    return code[header_start:end]


def _candidate_code(repo: Path, rel_path: str) -> tuple[str | None, str | None]:
    relative = Path(rel_path)
    if relative.is_absolute() or ".." in relative.parts:
        return "is not a confined repository-relative path", None
    path = repo / relative
    try:
        if not path.is_file() or path.is_symlink() or not path.resolve().is_relative_to(repo.resolve()):
            return "is not a regular confined repository file", None
        text = path.open("rb").read(600_000).decode("utf-8", errors="ignore")
    except OSError as exc:
        return f"cannot be read: {exc}", None
    if is_evidence_only_interface_path(rel_path):
        return "belongs to prototype, story, test, fixture, example, or other evidence-only scope", None
    suffix = relative.suffix.lower()
    if suffix in STYLE_OR_CATALOG_EXTENSIONS or suffix in ASSET_EXTENSIONS or suffix in DOCUMENT_OR_CONFIG_EXTENSIONS:
        return "is documentation, configuration, styling, catalog, or asset source rather than executable UI source", None

    code = _without_comments(text)
    if (
        OUTBOUND_RENDER_SYMBOL_RE.search(code) or _outbound_renderer_path(relative)
    ) and not _interactive_product_ui_signal(code):
        return "defines an outbound email, mail, or notification renderer rather than a product UI surface", None
    compact = re.sub(r"\s+", " ", code).strip().lower()
    if not compact:
        return "does not contain an implemented surface", None
    if any(all(signature in compact for signature in group) for group in STARTER_SIGNATURE_GROUPS):
        starter_label_re = re.compile(
            r"(?i)(?:vite\s*\+\s*react|count is \d+|edit src/app\.(?:js|jsx|ts|tsx)|save to test hmr|learn react|read the docs)"
        )
        meaningful_labels = [label for label in _visible_labels(code) if not starter_label_re.search(label)]
        if not meaningful_labels and not _data_bound_content(code):
            return "matches a known untouched framework starter", None
    return None, code


def _explicit_basis_issue(code: str, source_code: str, override: dict) -> str | None:
    if not isinstance(override, dict):
        return "has malformed explicit UI source-anchor evidence"
    ui_kind = override.get("ui_kind")
    source_anchor = override.get("source_anchor")
    if not isinstance(ui_kind, str) or not isinstance(source_anchor, str):
        return "explicit UI evidence must contain string ui_kind and source_anchor values"
    if ui_kind != ui_kind.strip() or source_anchor != source_anchor.strip():
        return "explicit UI kind and source anchor must not contain surrounding whitespace"
    for label, value in (("UI kind", ui_kind), ("source anchor", source_anchor)):
        if not 3 <= len(value) <= MAX_BASIS_LENGTH or not re.search(r"[A-Za-z0-9]", value):
            return f"explicit {label.lower()} must be a concise exact source token or construct"
        if re.sub(r"[^a-z0-9]+", "", value.casefold()) in GENERIC_BASIS_VALUES:
            return f"explicit {label.lower()} is too generic to identify implemented UI"
        if value not in code:
            return f"explicit {label.lower()} is not present in executable source outside comments"
    if ui_kind == source_anchor:
        return "explicit UI kind and source anchor must identify two distinct source facts"
    kind_leaf = _ui_kind_leaf(ui_kind)
    if kind_leaf is None:
        return "explicit UI kind must name a UI-specific framework type or construct"
    if _kind_has_backend_import(code, kind_leaf):
        return "explicit UI kind is imported from a data, database, persistence, or storage module"
    if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", source_anchor):
        return "explicit source anchor must be one exact named screen or component symbol"
    import_relation = bool(
        re.search(rf"(?m)^\s*(?:import|from|use|include)\b[^\n]*{re.escape(ui_kind)}", code)
        or re.search(rf"(?m)^\s*(?:import|from|use|include)\b[^\n]*\b{re.escape(kind_leaf)}\b", code)
    )
    inheritance_relation = bool(
        re.search(rf"\b(?:extends|implements)\s+[^\n{{;]*\b{re.escape(kind_leaf)}\b", code)
        or re.search(rf"[:<][ \t]*[^\n{{;]*\b{re.escape(kind_leaf)}\b", code)
    )
    construction_relation = bool(re.search(rf"\b{re.escape(kind_leaf)}\s*(?:::|\()", code))
    kind_relation = import_relation or inheritance_relation or construction_relation
    if not kind_relation:
        return "explicit UI kind is not used as an imported, inherited, conformed, or constructed UI type"
    if not (import_relation or inheritance_relation or re.search(r"(?:::|\.)", ui_kind)):
        return "explicit UI kind must be imported, inherited, conformed, or namespace-qualified; a local name alone is not framework evidence"
    definition_pattern = (
        rf"\b(?:class|struct|fn|def|function|func)\s+{re.escape(source_anchor)}\b|"
        rf"\b{re.escape(source_anchor)}\s*[:=]\s*(?:async\s+)?(?:function\b|\([^)]*\)\s*=>)|"
        rf"\b(?:[A-Za-z_][\w:<>,*&]*\s+)+{re.escape(source_anchor)}\s*\([^;{{}}]*\)\s*\{{"
    )
    definition = re.search(definition_pattern, code)
    if not definition:
        return "explicit source anchor is not a named screen or component definition"
    definition_source = _definition_extent(code, definition)
    if ui_kind not in definition_source and not re.search(rf"\b{re.escape(kind_leaf)}\b", definition_source):
        return "explicit screen or component definition does not use the named UI kind"
    source_definition_match = re.search(definition_pattern, source_code)
    source_definition = _definition_extent(source_code, source_definition_match) if source_definition_match else source_code
    quoted_labels = [
        value.strip()
        for _, value in re.findall(r"(['\"])(.*?)\1", source_definition, flags=re.DOTALL)
        if value.strip()
    ]
    strong_labels = [value for value in quoted_labels if STRONG_PLACEHOLDER_RE.search(value)]
    nonplaceholder_labels = [value for value in quoted_labels if not STRONG_PLACEHOLDER_RE.search(value)]
    if strong_labels and not (
        _data_bound_content(source_definition)
        or _override_dynamic_content(_without_string_literals(source_definition), source_definition)
    ):
        has_action = bool(
            re.search(r"\b(?:button|action|input|select)\s*(?:::|\()", source_definition, re.IGNORECASE)
            and nonplaceholder_labels
        )
        if not has_action and len(nonplaceholder_labels) < 2:
            return "explicit screen or component definition contains only placeholder UI content"
    return None


def qualify_implementation_source(
    repo: Path,
    rel_path: str,
    override: dict | None = None,
) -> tuple[str | None, dict | None]:
    issue, code = _candidate_code(repo, rel_path)
    if issue or code is None:
        return issue, None
    executable_code = _without_string_literals(code)
    web_checked, web_issue = _web_surface_issue(
        code,
        executable_code,
        Path(rel_path).suffix.lower(),
    )
    if web_checked and web_issue:
        return web_issue, None
    embedded_checked, embedded_issue = _embedded_web_surface_issue(code, Path(rel_path).suffix.lower())
    if embedded_checked and embedded_issue:
        return embedded_issue, None
    if _code_placeholder_only(code):
        return "contains a placeholder-only target surface", None
    code_kind = _code_surface_kind(code, executable_code, Path(rel_path).suffix.lower())
    if (web_checked and web_issue is None) or (embedded_checked and embedded_issue is None) or code_kind is not None:
        return None, {
            "method": "recognized-ui-signal",
            "detector_version": DETECTOR_VERSION,
        }
    if override is None:
        return (
            "does not contain a recognized executable UI construct; use the explicit source-anchor override only after manual inspection of an unrecognized UI framework",
            None,
        )
    issue = _explicit_basis_issue(executable_code, code, override)
    if issue:
        return issue, None
    return None, {
        "method": "explicit-source-anchor",
        "detector_version": DETECTOR_VERSION,
        "ui_kind": override["ui_kind"],
        "source_anchor": override["source_anchor"],
    }


def implementation_source_issue(repo: Path, rel_path: str, override: dict | None = None) -> str | None:
    return qualify_implementation_source(repo, rel_path, override)[0]
