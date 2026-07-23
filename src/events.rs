//! Append-only audit log for every mutation. JSONL at
//! `~/.local/state/todo/events.jsonl` per SPEC.md.
//!
//! Read by gate's external_check primitive when wired (Flow B,
//! deferred). For v0.1 the file is write-only from todo's side; later
//! consumers grep it for "who demoted P0 X" forensics.

use crate::paths::events_path;
use chrono::Utc;
use serde::Serialize;
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;

#[derive(Debug, Serialize)]
pub struct Event<'a> {
    pub ts: String,
    pub op: &'a str,
    pub id: &'a str,
    pub by: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<&'a str>,
    pub company: &'a str,
}

/// Append a single event line. Failures are silent — the file is
/// audit, not a hard dependency. If it can't be written, the mutation
/// still happened; we just lose the trail line for this call.
pub fn append(event: Event<'_>) {
    let path = events_path();
    if let Some(parent) = path.parent() {
        let _ = create_dir_all(parent);
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        if let Ok(line) = serde_json::to_string(&event) {
            let _ = writeln!(f, "{}", line);
        }
    }
}

/// Convenience constructor. `by` is resolved from the environment:
/// $SWITCHBOARD_NAME → "UNATTRIBUTED" (with stderr warning).
/// A silent by=unknown is impossible: every close is either attributed
/// or loudly flagged as UNATTRIBUTED.
pub fn record(
    op: &str,
    id: &str,
    company: &str,
    from: Option<&str>,
    to: Option<&str>,
    subject: Option<&str>,
) {
    let by_env = std::env::var("SWITCHBOARD_NAME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            eprintln!(
                "todo: audit: closer unattributable — SWITCHBOARD_NAME unset; stamping by=UNATTRIBUTED"
            );
            "UNATTRIBUTED".to_string()
        });
    append(Event {
        ts: Utc::now().to_rfc3339(),
        op,
        id,
        by: &by_env,
        from,
        to,
        subject,
        company,
    });
}
