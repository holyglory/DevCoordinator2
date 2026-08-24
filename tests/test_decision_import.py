"""decision_import parses the established DecisionHistory format and records
entries idempotently through the reviewed interface (REQ-PLAN-10)."""

import importlib.util
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "decision_import", Path(__file__).resolve().parents[1] / "scripts" /
    "decision_import.py")
decision_import = importlib.util.module_from_spec(spec)
sys.modules["decision_import"] = decision_import
spec.loader.exec_module(decision_import)

SAMPLE = """# Decision History

Compact record of owner-level decisions.

Format: `XX-YYYY-MM-DD-TOPIC — Decision`.

## XX-2026-01-01-CONSOLE-COLORS — Buttons use one accent color

**Decision.** Every primary button on the console page uses the accent color
so actions are easy to spot.

**Alternatives.** Per-view colors were rejected as noise.

**Owner context.** The owner saw both variants in a preview.

## XX-2026-01-02-EDGE-ROUTES — The edge proxies by route document

**Decision.** The edge serves domains from the last valid route document and
proxies each deployment by its leased port.

**Alternatives.** Restarting the edge per change was rejected.

**Owner context.** Verbatim: "deploys must not drop traffic".
"""


def test_parse_extracts_refs_titles_bodies_and_aspects():
    entries = decision_import.parse(SAMPLE)
    assert [e.ref for e in entries] == ["XX-2026-01-01-CONSOLE-COLORS",
                                       "XX-2026-01-02-EDGE-ROUTES"]
    first, second = entries
    assert first.title == "Buttons use one accent color"
    assert "**" not in first.body and "Alternatives" in first.body
    assert first.aspect == "ui"          # console/button keywords
    assert second.aspect == "deployment"  # edge/route/proxy keywords
    assert first.technical_note is None
    # The intro before the first section is not an entry.
    assert all(e.ref.startswith("XX-") for e in entries)


def test_parse_clips_long_titles_and_overflows_body_to_technical_note():
    long_title = "T" * 200
    long_body = ("word " * 1200).strip()  # ~6000 chars
    text = f"## XX-LONG — {long_title}\n\n**Decision.** {long_body}\n"
    (entry,) = decision_import.parse(text)
    assert len(entry.title) == decision_import.TITLE_MAX
    assert len(entry.body) <= decision_import.BODY_MAX
    assert entry.body.endswith("(continued in the technical note)")
    assert entry.technical_note and entry.technical_note.endswith("word")


def test_import_records_and_skips_duplicates(monkeypatch, capsys, tmp_path):
    (tmp_path / "DecisionHistory.md").write_text(SAMPLE)
    seen = []

    def fake_call(socket_path, command, args, **kw):
        assert command == "decision.record"
        if any(s["ref"] == args["ref"] for s in seen):
            return {"ok": False, "error": {"code": "args_invalid",
                                           "message": f"ref {args['ref']!r} is"
                                                      " already used"}}
        seen.append(args)
        return {"ok": True, "result": {"decision_id": "n1", "seq": len(seen)}}

    monkeypatch.setattr(decision_import, "call", fake_call)
    monkeypatch.setattr(decision_import, "load_instance_config",
                        lambda: type("C", (), {"socket_path": "/nowhere"}))
    rc = decision_import.main(["--path", str(tmp_path),
                               "--aspect", "XX-2026-01-02-EDGE-ROUTES=process"])
    assert rc == 0
    assert [s["ref"] for s in seen] == ["XX-2026-01-01-CONSOLE-COLORS",
                                       "XX-2026-01-02-EDGE-ROUTES"]
    assert seen[1]["aspect"] == "process"  # override wins over the guess
    rc = decision_import.main(["--path", str(tmp_path)])  # idempotent second run
    assert rc == 0
    assert len(seen) == 2
    out = capsys.readouterr().out
    assert "2 imported" in out and "2 already present" in out


def test_dry_run_prints_plan_without_calling(monkeypatch, capsys, tmp_path):
    (tmp_path / "DecisionHistory.md").write_text(SAMPLE)
    monkeypatch.setattr(decision_import, "call",
                        lambda *a, **k: (_ for _ in ()).throw(AssertionError))
    rc = decision_import.main(["--path", str(tmp_path), "--dry-run"])
    assert rc == 0
    out = capsys.readouterr().out
    assert "aspect=ui" in out and "nothing recorded" in out
