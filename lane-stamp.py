#!/usr/bin/env python3
"""Lane-stamping pass for ~/.config/substrate/_global/todos.md.

Schema: lane:<project>/<component>/<thread>
Subject-prefix encoding: [lane:proj/comp/_] leads the subject.
Canonical project ∈ {client projects (from the yadm-excluded lane-map.json), substrate, _global}
Canonical component for substrate ∈ binaries registry.

CANONICAL DEFINITION: todo/SPEC.md §"Lane tags (canonical) [#lmbg]".
This is the WRITE side of that single source; the TUI lane-view (fleet-tui
lane_section_key) is the READ side. This stamper MUST emit only what that
regex accepts: exactly 3 parts [lane:p/c/t]. read==write is selftest-enforced
(empirical, not structural — 3 languages, no shared parser; see SPEC R2).
Durable here (substrate-distro/todo/) per #4lyn — was in /tmp (#85q7 hygiene:
the write-side of a "single source" must not live in ephemeral /tmp).

Mode: --dry runs classifier, prints proposed rewrites, touches nothing.
"""
import json
import re
import sys
from pathlib import Path

TODOS = Path.home() / ".config/substrate/_global/todos.md"

# (project, component, regex) — first match wins. order = priority.
LANE_MAP = Path.home() / ".config/substrate/lane-map.json"


def _load_client_lanes():
    """Client company→component→regex rows from the yadm-EXCLUDED lane-map.json,
    keeping company info out of shared source (#bfyz9x). Missing/invalid → empty
    (client items fall to unsorted; substrate lanes unaffected)."""
    try:
        return [tuple(row) for row in json.loads(LANE_MAP.read_text())]
    except Exception:
        return []


# Substrate infra lanes (not client info) stay inline.
SUBSTRATE_LANES = [
    ("substrate", "token-monitor",     r"token[-\s]?monitor|en0x"),
    ("substrate", "fleet-tui",         r"fleet[-\s]?tui|\bTUI\b"),
    ("substrate", "fleet-digest",      r"fleet[-\s]?(visibility|digest)|evolution[-\s]?digest"),
    ("substrate", "idle-scout",        r"idle[-\s]?scout"),
    ("substrate", "idle-work",         r"idle[-\s]?work"),
    ("substrate", "recruit",           r"\brecruit\b|\bretune\b"),
    ("substrate", "gate",              r"\bgate\b|gate-"),
    ("substrate", "switchboard",       r"switchboard"),
    ("substrate", "skill-router",      r"skill[-\s]?router|router[-\s]?(rule|schema|side[-_\s]?effect)|\bF16\b"),
    ("substrate", "todo",              r"\[ledger\]|\btodo\b|todos\.md"),
    ("substrate", "doctrine",          r"doctrine|§\d|verified[-\s]?done|decorative"),
    ("substrate", "prune-daemon",      r"prune[-\s]?daemon|presence[-\s]?prune"),
    ("substrate", "substrate-cli",     r"\bsubstrate (validate|deploy|infer|survey|enroll|primitives)"),
    ("substrate", "device-claim",      r"device[-\s]?claim"),
    ("substrate", "resources",         r"\bresources\b binary"),
    ("substrate", "spec-prepper",      r"\bprepper\b|SPEC-role-prepper"),
    ("substrate", "spec-time",         r"time[-\s]?awareness|time[-\s]?embodiment|temporal"),
    ("substrate", "spec-identity",     r"identity\.toml|identity[-\s]?per[-\s]?project"),
    ("substrate", "spec-docs",         r"docs\.toml|SPEC-docs"),
    ("substrate", "burn-governor",     r"burn[-\s]?(governor|binary)|idle[-\s]?pool[-\s]?burn"),
    ("substrate", "egress-audit",      r"egress[-\s]?events|PostToolUse.*egress"),
    ("substrate", "claude-md",         r"CLAUDE\.md"),
    ("substrate", "stream-holder",     r"stream[-\s]?holder|consumer[-\s]?dead"),
    ("substrate", "complaint-meta",    r"complaint[-\s]?(sweep|capture)"),
]

LANES = _load_client_lanes() + SUBSTRATE_LANES

# Already-stamped check
STAMPED_RE = re.compile(r"\[lane:[^\]]+\]")
LINE_RE = re.compile(r"^(- \[([ x])\] \[#([0-9a-z]+)\] )(.*)$")


def classify(subject: str):
    s = subject.lower()
    for proj, comp, pat in LANES:
        if re.search(pat, s, re.I):
            return (proj, comp)
    return None


def main():
    dry = "--dry" in sys.argv
    lines = TODOS.read_text().splitlines(keepends=False)
    out_lines = []
    stamped = 0
    skipped_done = 0
    already = 0
    unsorted_ids = []
    by_lane = {}
    samples = []

    for line in lines:
        m = LINE_RE.match(line)
        if not m:
            out_lines.append(line)
            continue
        prefix, mark, tid, subject = m.group(1), m.group(2), m.group(3), m.group(4)
        if mark == "x":  # closed item — leave alone
            skipped_done += 1
            out_lines.append(line)
            continue
        if STAMPED_RE.search(subject):
            already += 1
            out_lines.append(line)
            continue
        lane = classify(subject)
        if lane is None:
            unsorted_ids.append((tid, subject[:90]))
            tag = "[lane:_/_/_]"
        else:
            tag = f"[lane:{lane[0]}/{lane[1]}/_]"
            by_lane[(lane[0], lane[1])] = by_lane.get((lane[0], lane[1]), 0) + 1
        new_subject = f"{tag} {subject}"
        out_lines.append(f"{prefix}{new_subject}")
        stamped += 1
        if len(samples) < 5:
            samples.append((tid, tag, subject[:80]))

    print(f"=== LANE STAMP DRY RUN ===")
    print(f"Stamped: {stamped} (auto-classified + unsorted)")
    print(f"Already stamped (skipped): {already}")
    print(f"Closed (left alone): {skipped_done}")
    print(f"Unsorted (lane:_/_/_): {len(unsorted_ids)}")
    print()
    print(f"=== LANE COUNTS ===")
    for k, v in sorted(by_lane.items(), key=lambda x: (-x[1], x[0])):
        print(f"  {k[0]:10s}/{k[1]:18s} {v}")
    print()
    print(f"=== SAMPLE STAMPS (first 5) ===")
    for tid, tag, subj in samples:
        print(f"  #{tid}  {tag}  {subj}")
    print()
    print(f"=== UNSORTED IDS (need manual lane) ===")
    for tid, subj in unsorted_ids:
        print(f"  #{tid}  {subj}")
    if not dry:
        TODOS.write_text("\n".join(out_lines) + "\n")
        print(f"\nWROTE {TODOS}")
    else:
        print(f"\nDRY RUN — no file written. Pass without --dry to commit.")


if __name__ == "__main__":
    main()
