//! Parse `todos.md` → `Todos`. Format per SPEC.md v1.1:
//!
//! ```text
//! # todos — <company>
//! <!-- ... preamble comments ... -->
//!
//! ## P0
//! - [ ] [#abc1] subject text
//! - [x] [#def2] closed item (closed 2026-06-22)
//!
//! ## P1
//! - [ ] subject without explicit ID (parser back-fills on next write)
//! ```
//!
//! Rules (matching SPEC.md "format rules" section):
//! 1. `## P<N>` headings define priority buckets (N is non-negative int).
//! 2. `- [ ]` / `- [x]` checkbox lines are items. First-token-after-checkbox
//!    is the ID iff it looks like `[#<token>]`; otherwise that token is part
//!    of the subject and the parser back-fills an ID on the next write.
//! 3. Trailing `(closed YYYY-MM-DD)` is preserved into `Item.closed_on`.
//! 4. HTML comments / blank lines outside buckets are preserved as preamble.

use crate::model::{IdleOkState, Item, Lane, ParkState, Priority, Todos};

pub fn parse(source: &str, company: &str) -> Todos {
    let mut todos = Todos::empty(company);
    let mut current_priority: Option<Priority> = None;
    let mut in_preamble = true;

    for (idx, raw_line) in source.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim_end();

        if let Some(rest) = line.strip_prefix("## ") {
            // Priority heading — close preamble.
            in_preamble = false;
            current_priority = Priority::parse(rest.trim());
            continue;
        }

        if in_preamble {
            todos.preamble.push(raw_line.to_string());
            continue;
        }

        if let Some(item) = parse_item_line(line, line_no, current_priority.as_ref()) {
            todos.items.push(item);
        }
        // Lines that aren't items, headings, or preamble are silently
        // skipped — they may be operator notes interleaved between
        // items. Writer round-trip is not bit-exact; this is the
        // intentional information loss point.
    }

    todos
}

fn parse_item_line(line: &str, line_no: usize, priority: Option<&Priority>) -> Option<Item> {
    let priority = priority.cloned()?;
    let trimmed = line.trim_start();
    let (open, rest) = if let Some(rest) = trimmed.strip_prefix("- [ ] ") {
        (true, rest)
    } else if let Some(rest) = trimmed.strip_prefix("- [x] ") {
        (false, rest)
    } else if let Some(rest) = trimmed.strip_prefix("- [X] ") {
        (false, rest)
    } else {
        return None;
    };

    // Optional `[#id]` prefix.
    let (id, after_id) = if let Some(stripped) = rest.strip_prefix("[#") {
        if let Some(end) = stripped.find(']') {
            let id = stripped[..end].to_string();
            // Skip past `]` and any following whitespace.
            let after = &stripped[end + 1..];
            (Some(id), after.trim_start())
        } else {
            (None, rest)
        }
    } else {
        (None, rest)
    };

    // Optional `[idle-ok]` or `[idle-ok-suggest]` marker after the id.
    let (idle_ok, after_idle) = if let Some(rest) = after_id.strip_prefix("[idle-ok-suggest]") {
        (IdleOkState::Suggest, rest.trim_start())
    } else if let Some(rest) = after_id.strip_prefix("[idle-ok]") {
        (IdleOkState::Approved, rest.trim_start())
    } else {
        (IdleOkState::None, after_id)
    };

    // Optional `[parked]` / `[park-suggest]` marker after idle-ok (backlog tier).
    let (park, after_park) = if let Some(rest) = after_idle.strip_prefix("[park-suggest]") {
        (ParkState::Suggested, rest.trim_start())
    } else if let Some(rest) = after_idle.strip_prefix("[parked]") {
        (ParkState::Parked, rest.trim_start())
    } else {
        (ParkState::Active, after_idle)
    };

    // Optional `[released <iso8601>]` marker after park.
    let (released, released_ts, after_released) = if let Some(rest) = after_park.strip_prefix("[released ") {
        if let Some(end) = rest.find(']') {
            let ts = rest[..end].to_string();
            (true, Some(ts), rest[end + 1..].trim_start())
        } else {
            (false, None, after_park)
        }
    } else {
        (false, None, after_park)
    };

    // Optional `[created <iso8601>]` marker after released (M0).
    let (created_ts, after_created) = if let Some(rest) = after_released.strip_prefix("[created ") {
        if let Some(end) = rest.find(']') {
            (Some(rest[..end].to_string()), rest[end + 1..].trim_start())
        } else {
            (None, after_released)
        }
    } else {
        (None, after_released)
    };

    // Optional `[fseen <iso8601>]` marker after created (Phase-2a).
    let (first_seen_open, after_fseen) = if let Some(rest) = after_created.strip_prefix("[fseen ") {
        if let Some(end) = rest.find(']') {
            (Some(rest[..end].to_string()), rest[end + 1..].trim_start())
        } else {
            (None, after_created)
        }
    } else {
        (None, after_created)
    };

    // Optional `[real]` marker after fseen — escape-hatch for force-real closes.
    let (force_real, after_marker) = if let Some(rest) = after_fseen.strip_prefix("[real]") {
        (true, rest.trim_start())
    } else {
        (false, after_fseen)
    };

    // Zero-or-more `[ev <ref>]` markers after [real]. Consumed from the chain;
    // NOT peeked — each marker advances the cursor.
    let mut evidence = Vec::new();
    let mut after_ev = after_marker;
    loop {
        if let Some(rest) = after_ev.strip_prefix("[ev ") {
            if let Some(end) = rest.find(']') {
                evidence.push(rest[..end].to_string());
                after_ev = rest[end + 1..].trim_start();
            } else {
                break;
            }
        } else {
            break;
        }
    }

    // Peek-parse [lane:company/project/thread] from the start of remaining text.
    // The tag stays in `subject` (backward-compat: fleet-tui reads subject).
    // Peeking means we do NOT advance after_ev — lane is derived, not consumed.
    let lane = peek_lane(after_ev);

    // Strip trailing `(closed YYYY-MM-DD)` if present.
    let (subject, closed_on) = split_closed_annotation(after_ev);

    Some(Item {
        id: id.unwrap_or_default(), // empty → back-filled on write
        priority,
        subject: subject.trim().to_string(),
        open,
        line: line_no,
        closed_on,
        idle_ok,
        park,
        released,
        released_ts,
        created_ts,
        lane,
        first_seen_open,
        force_real,
        evidence,
    })
}

/// Peek at `s` for a `[lane:company/project/thread]` prefix and return a
/// parsed `Lane` if found. Does NOT advance `s` — the tag stays in the
/// subject text for backward-compat consumers (fleet-tui, grep).
///
/// Requires exactly three slash-delimited segments; two-part tags are
/// rejected (matching the canonical SPEC.md regex).
/// Thread `"_"` → `component: None`.
pub fn peek_lane(s: &str) -> Option<Lane> {
    let rest = s.strip_prefix("[lane:")?;
    let slash1 = rest.find('/')?;
    let company = &rest[..slash1];
    if company.is_empty() { return None; }
    let rest2 = &rest[slash1 + 1..];
    let slash2 = rest2.find('/')?;
    let project = &rest2[..slash2];
    if project.is_empty() { return None; }
    let rest3 = &rest2[slash2 + 1..];
    let close = rest3.find(']')?;
    let thread = &rest3[..close];
    let component = if thread == "_" { None } else { Some(thread.to_string()) };
    Some(Lane {
        company: company.to_string(),
        project: project.to_string(),
        component,
    })
}

/// Split a subject string into (subject, optional closed annotation).
/// Pattern: `... (closed YYYY-MM-DD)` at the END of the string.
fn split_closed_annotation(s: &str) -> (String, Option<String>) {
    let trimmed = s.trim_end();
    if !trimmed.ends_with(')') {
        return (trimmed.to_string(), None);
    }
    // Find last `(` and check the bracketed text starts with "closed ".
    if let Some(open_idx) = trimmed.rfind('(') {
        let bracketed = &trimmed[open_idx + 1..trimmed.len() - 1];
        if let Some(rest) = bracketed.strip_prefix("closed ") {
            let subject = trimmed[..open_idx].trim_end().to_string();
            return (subject, Some(rest.to_string()));
        }
    }
    (trimmed.to_string(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical_format() {
        let src = "# todos — global\n<!-- preamble -->\n\n## P0\n- [ ] [#abc1] first item\n- [x] [#def2] done (closed 2026-06-22)\n\n## P1\n- [ ] [#ghi3] later\n";
        let todos = parse(src, "global");
        assert_eq!(todos.company, "global");
        assert_eq!(todos.items.len(), 3);
        assert_eq!(todos.items[0].id, "abc1");
        assert_eq!(todos.items[0].priority, Priority(0));
        assert!(todos.items[0].open);
        assert_eq!(todos.items[1].id, "def2");
        assert!(!todos.items[1].open);
        assert_eq!(todos.items[1].closed_on.as_deref(), Some("2026-06-22"));
        assert_eq!(todos.items[2].priority, Priority(1));
    }

    #[test]
    fn missing_id_is_empty() {
        let src = "## P0\n- [ ] subject without id\n";
        let todos = parse(src, "global");
        assert_eq!(todos.items.len(), 1);
        assert_eq!(todos.items[0].id, "");
        assert_eq!(todos.items[0].subject, "subject without id");
    }

    #[test]
    fn priority_parse() {
        assert_eq!(Priority::parse("P0"), Some(Priority(0)));
        assert_eq!(Priority::parse("P42"), Some(Priority(42)));
        assert_eq!(Priority::parse("Px"), None);
    }

    #[test]
    fn peek_lane_parses_full_tag() {
        use crate::model::Lane;
        let lane = peek_lane("[lane:substrate/todo/parser]");
        assert_eq!(lane, Some(Lane {
            company: "substrate".into(),
            project: "todo".into(),
            component: Some("parser".into()),
        }));
    }

    #[test]
    fn peek_lane_underscore_thread_is_none() {
        let lane = peek_lane("[lane:acme/_/_]");
        assert!(lane.is_some());
        let l = lane.unwrap();
        assert_eq!(l.company, "acme");
        assert_eq!(l.project, "_");
        assert_eq!(l.component, None);
    }

    #[test]
    fn peek_lane_untagged_is_none() {
        assert_eq!(peek_lane("some plain subject"), None);
        assert_eq!(peek_lane("[created 2026-01-01T00:00:00Z] subject"), None);
    }

    #[test]
    fn parse_lane_tagged_item_lane_field() {
        let src = "## P1\n- [ ] [#abc1] [lane:outfit/app-android/_] fix crash\n";
        let todos = parse(src, "global");
        assert_eq!(todos.items.len(), 1);
        let item = &todos.items[0];
        let lane = item.lane.as_ref().expect("lane should be Some");
        assert_eq!(lane.company, "outfit");
        assert_eq!(lane.project, "app-android");
        assert_eq!(lane.component, None);
        // subject still contains the tag (backward-compat)
        assert!(item.subject.starts_with("[lane:outfit/app-android/_]"));
    }

    #[test]
    fn parse_untagged_item_lane_is_none() {
        let src = "## P0\n- [ ] [#xyz1] no lane tag here\n";
        let todos = parse(src, "global");
        assert_eq!(todos.items.len(), 1);
        assert!(todos.items[0].lane.is_none());
    }

    #[test]
    fn parses_fseen_marker() {
        let src = "## P1\n- [ ] [#aa01] [created 2026-07-02T00:01:00Z] [fseen 2026-07-02T00:05:00Z] the subject\n";
        let todos = parse(src, "global");
        let item = &todos.items[0];
        assert_eq!(item.first_seen_open.as_deref(), Some("2026-07-02T00:05:00Z"));
        assert!(!item.force_real);
        assert_eq!(item.subject, "the subject");
    }

    #[test]
    fn parses_real_marker() {
        let src = "## P1\n- [x] [#bb02] [created 2026-07-02T00:01:00Z] [real] quick task (closed 2026-07-02)\n";
        let todos = parse(src, "global");
        let item = &todos.items[0];
        assert!(item.force_real);
        assert!(item.first_seen_open.is_none());
        assert_eq!(item.subject, "quick task");
    }

    #[test]
    fn parses_fseen_and_real_together() {
        let src = "## P0\n- [ ] [#cc03] [created 2026-07-02T00:01:00Z] [fseen 2026-07-02T00:06:00Z] [real] dual subject\n";
        let todos = parse(src, "global");
        let item = &todos.items[0];
        assert_eq!(item.first_seen_open.as_deref(), Some("2026-07-02T00:06:00Z"));
        assert!(item.force_real);
        assert_eq!(item.subject, "dual subject");
    }

    #[test]
    fn absent_fseen_real_parses_to_defaults() {
        let src = "## P0\n- [ ] [#dd04] plain subject\n";
        let todos = parse(src, "global");
        let item = &todos.items[0];
        assert!(item.first_seen_open.is_none());
        assert!(!item.force_real);
    }

    // --- Evidence pool tests ---

    #[test]
    fn parses_single_evidence_marker() {
        let src = "## P0\n- [ ] [#ev01] [ev msg:abc123] subject text\n";
        let todos = parse(src, "global");
        let item = &todos.items[0];
        assert_eq!(item.evidence, vec!["msg:abc123"]);
        assert_eq!(item.subject, "subject text");
    }

    #[test]
    fn parses_multiple_evidence_markers_in_order() {
        let src = "## P0\n- [ ] [#ev02] [ev wire-a1b2] [ev file.rs:42] [ev https://example.com] the task\n";
        let todos = parse(src, "global");
        let item = &todos.items[0];
        assert_eq!(item.evidence, vec!["wire-a1b2", "file.rs:42", "https://example.com"]);
        assert_eq!(item.subject, "the task");
    }

    #[test]
    fn no_evidence_markers_gives_empty_vec() {
        let src = "## P0\n- [ ] [#ev03] plain subject\n";
        let todos = parse(src, "global");
        let item = &todos.items[0];
        assert!(item.evidence.is_empty());
    }

    #[test]
    fn evidence_markers_after_real_marker() {
        let src = "## P0\n- [ ] [#ev04] [created 2026-07-01T00:00:00Z] [real] [ev ref-xyz] subject\n";
        let todos = parse(src, "global");
        let item = &todos.items[0];
        assert!(item.force_real);
        assert_eq!(item.evidence, vec!["ref-xyz"]);
        assert_eq!(item.subject, "subject");
    }

    #[test]
    fn evidence_roundtrip_byte_stable() {
        use crate::writer;
        // Evidence markers roundtrip: parse -> write -> parse gives identical evidence.
        let src = "## P0\n- [ ] [#ev05] [ev ref-a] [ev ref-b] subject with evidence\n";
        let parsed = parse(src, "global");
        let rendered = writer::render(&parsed);
        let reparsed = parse(&rendered, "global");
        assert_eq!(reparsed.items[0].evidence, vec!["ref-a", "ref-b"]);
        assert_eq!(reparsed.items[0].subject, "subject with evidence");
    }

    #[test]
    fn existing_items_roundtrip_byte_stable_no_evidence_emitted() {
        use crate::writer;
        // Items with no evidence must not have [ev] markers added — byte-stable.
        let src = "# todos — global\n\n## P0\n- [ ] [#xx01] plain item\n- [x] [#xx02] done item (closed 2026-07-01)\n";
        let parsed = parse(src, "global");
        let rendered = writer::render(&parsed);
        // No [ev] in rendered output.
        assert!(!rendered.contains("[ev "));
        // md5 of rendered == md5 of a fresh render of the same data — content stable.
        let reparsed = parse(&rendered, "global");
        assert_eq!(reparsed.items[0].evidence, Vec::<String>::new());
        assert_eq!(reparsed.items[1].evidence, Vec::<String>::new());
    }

    #[test]
    fn evidence_with_lane_tag_roundtrip() {
        use crate::writer;
        // [ev] before lane-in-subject: both must survive roundtrip.
        let src = "## P1\n- [ ] [#ev06] [ev wire:abc] [lane:substrate/todo/_] the subject\n";
        let parsed = parse(src, "global");
        let item = &parsed.items[0];
        assert_eq!(item.evidence, vec!["wire:abc"]);
        assert!(item.subject.starts_with("[lane:substrate/todo/_]"));
        // Roundtrip preserves both.
        let rendered = writer::render(&parsed);
        let reparsed = parse(&rendered, "global");
        assert_eq!(reparsed.items[0].evidence, vec!["wire:abc"]);
        assert!(reparsed.items[0].subject.starts_with("[lane:substrate/todo/_]"));
    }
}
