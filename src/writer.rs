//! Render `Todos` back to canonical Markdown. NOT bit-exact for
//! pre-parse content — operator notes interleaved between items are
//! dropped. Title + preamble HTML comments ARE preserved verbatim.
//!
//! Output convention:
//! ```text
//! <preamble lines verbatim>
//!
//! ## P0
//! - [ ] [#abc1] subject
//! - [x] [#def2] closed (closed 2026-06-22)
//!
//! ## P1
//! - [ ] [#ghi3] later
//! ```
//!
//! Empty buckets are omitted. Order within a bucket follows the
//! `Todos.items` Vec order — same as parsed.

use crate::model::Todos;
use std::collections::BTreeMap;

pub fn render(todos: &Todos) -> String {
    let mut out = String::new();

    // Preamble verbatim. If empty, synthesize a minimal title line.
    if todos.preamble.is_empty() {
        out.push_str(&format!("# todos — {}\n", todos.company));
        out.push('\n');
    } else {
        for line in &todos.preamble {
            out.push_str(line);
            out.push('\n');
        }
        if !out.ends_with("\n\n") {
            out.push('\n');
        }
    }

    // Bucket items by priority for grouped output.
    let mut buckets: BTreeMap<u32, Vec<&crate::model::Item>> = BTreeMap::new();
    for item in &todos.items {
        buckets.entry(item.priority.0).or_default().push(item);
    }

    use crate::model::{IdleOkState, ParkState};
    for (p, items) in buckets {
        out.push_str(&format!("## P{}\n", p));
        for item in items {
            let check = if item.open { "[ ]" } else { "[x]" };
            let id_part = if item.id.is_empty() {
                String::new()
            } else {
                format!("[#{}] ", item.id)
            };
            let idle_part = match item.idle_ok {
                IdleOkState::None => String::new(),
                IdleOkState::Suggest => "[idle-ok-suggest] ".to_string(),
                IdleOkState::Approved => "[idle-ok] ".to_string(),
            };
            let park_part = match item.park {
                ParkState::Active => String::new(),
                ParkState::Suggested => "[park-suggest] ".to_string(),
                ParkState::Parked => "[parked] ".to_string(),
            };
            let released_part = if item.released {
                match &item.released_ts {
                    Some(ts) => format!("[released {}] ", ts),
                    None => "[released] ".to_string(),
                }
            } else {
                String::new()
            };
            let created_part = match &item.created_ts {
                Some(ts) => format!("[created {}] ", ts),
                None => String::new(),
            };
            let fseen_part = match &item.first_seen_open {
                Some(ts) => format!("[fseen {}] ", ts),
                None => String::new(),
            };
            let real_part = if item.force_real { "[real] ".to_string() } else { String::new() };
            let evidence_part: String = item.evidence.iter()
                .map(|r| format!("[ev {}] ", r))
                .collect();
            let closed_part = match &item.closed_on {
                Some(d) => format!(" (closed {})", d),
                None => String::new(),
            };
            out.push_str(&format!(
                "- {} {}{}{}{}{}{}{}{}{}{}\n",
                check, id_part, idle_part, park_part, released_part, created_part, fseen_part, real_part, evidence_part, item.subject, closed_part
            ));
        }
        out.push('\n');
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Item, Priority, Todos};

    #[test]
    fn renders_canonical_format() {
        let todos = Todos {
            version: 1,
            company: "global".to_string(),
            items: vec![
                Item {
                    id: "abc1".into(),
                    priority: Priority(0),
                    subject: "first item".into(),
                    open: true,
                    line: 0,
                    closed_on: None,
                    idle_ok: crate::model::IdleOkState::None,
                    park: crate::model::ParkState::Active,
                    released: false,
                    released_ts: None,
                    created_ts: None,
                    lane: None,
                    first_seen_open: None,
                    force_real: false,
                    evidence: vec![],
                },
                Item {
                    id: "def2".into(),
                    priority: Priority(0),
                    subject: "done thing".into(),
                    open: false,
                    line: 0,
                    closed_on: Some("2026-06-22".into()),
                    idle_ok: crate::model::IdleOkState::None,
                    park: crate::model::ParkState::Active,
                    released: false,
                    released_ts: None,
                    created_ts: None,
                    lane: None,
                    first_seen_open: None,
                    force_real: false,
                    evidence: vec![],
                },
            ],
            preamble: vec!["# todos — global".into()],
        };
        let out = render(&todos);
        assert!(out.contains("## P0"));
        assert!(out.contains("- [ ] [#abc1] first item"));
        assert!(out.contains("- [x] [#def2] done thing (closed 2026-06-22)"));
    }

    #[test]
    fn roundtrips_through_parser() {
        let src = "# todos — global\n<!-- note -->\n\n## P0\n- [ ] [#abc1] first\n\n## P1\n- [x] [#def2] done (closed 2026-06-22)\n";
        let parsed = crate::parser::parse(src, "global");
        let rendered = render(&parsed);
        let reparsed = crate::parser::parse(&rendered, "global");
        assert_eq!(parsed.items.len(), reparsed.items.len());
        for (a, b) in parsed.items.iter().zip(reparsed.items.iter()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.priority, b.priority);
            assert_eq!(a.subject, b.subject);
            assert_eq!(a.open, b.open);
            assert_eq!(a.closed_on, b.closed_on);
        }
    }
}
