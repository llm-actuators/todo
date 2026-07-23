//! §46 b-test: closer attribution on `todo done`.
//!
//! Proves that NO close is silently unattributable:
//!   (a) WITH $SWITCHBOARD_NAME → by=<handle> in audit
//!   (b) WITHOUT $SWITCHBOARD_NAME → by=UNATTRIBUTED in audit + stderr warning
//!
//! $USER is intentionally NOT a fallback: all agents run as the same OS
//! user, so USER-fallback would stamp by=the maintainer on any handle-less close —
//! masking the agent and falsely implicating the operator.
//!
//! "Enforcement is real or it's theater" (§46): these tests prove the
//! audit cannot produce a silent by=unknown.

use std::fs;
use std::process::Command;

fn todo_bin() -> String {
    env!("CARGO_BIN_EXE_todo").to_string()
}

fn tempdir(tag: &str) -> String {
    use std::time::SystemTime;
    let n = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = format!("/tmp/todo-closer-test-{}-{}", tag, n);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn add_item(tmpdir: &str, company: &str, extra_env: &[(&str, &str)]) -> String {
    let mut cmd = Command::new(todo_bin());
    cmd.args(["--company", company, "add", "closer-attribution test item", "--priority", "P1"])
        .env("HOME", tmpdir)
        .env("XDG_CONFIG_HOME", &format!("{}/.config", tmpdir))
        .env("XDG_STATE_HOME", &format!("{}/.local/state", tmpdir));
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("add failed");
    assert!(out.status.success(), "add stderr: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn events_jsonl(tmpdir: &str) -> String {
    let path = format!("{}/.local/state/todo/events.jsonl", tmpdir);
    fs::read_to_string(&path).unwrap_or_default()
}

/// (a) Close WITH $SWITCHBOARD_NAME set → audit record carries by=<handle>.
#[test]
fn done_with_switchboard_name_stamps_handle() {
    let tmpdir = tempdir("a");
    let company = "_test_closer_a";
    let handle = "builder-7623";

    let id = add_item(&tmpdir, company, &[("SWITCHBOARD_NAME", handle)]);
    assert!(!id.is_empty());

    let out = Command::new(todo_bin())
        .args(["--company", company, "done", &id])
        .env("HOME", &tmpdir)
        .env("XDG_CONFIG_HOME", &format!("{}/.config", tmpdir))
        .env("XDG_STATE_HOME", &format!("{}/.local/state", tmpdir))
        .env("SWITCHBOARD_NAME", handle)
        .output()
        .expect("done failed");
    assert!(out.status.success(), "done stderr: {}", String::from_utf8_lossy(&out.stderr));

    let events = events_jsonl(&tmpdir);
    assert!(
        events.contains(&format!("\"by\":\"{}\"", handle)),
        "audit does not carry by={}: {}",
        handle,
        events
    );
    assert!(
        events.contains("\"op\":\"done\""),
        "no done event in audit: {}",
        events
    );
}

/// (b) Close WITHOUT $SWITCHBOARD_NAME and WITHOUT $USER →
///     by=UNATTRIBUTED in audit + stderr warning emitted.
/// Proves the by=unknown silent hole is closed: unattributable closes
/// are loudly flagged, not silently swallowed.
#[test]
fn done_without_any_handle_stamps_unattributed() {
    let tmpdir = tempdir("b");
    let company = "_test_closer_b";

    let id = add_item(&tmpdir, company, &[]);
    assert!(!id.is_empty());

    let out = Command::new(todo_bin())
        .args(["--company", company, "done", &id])
        .env("HOME", &tmpdir)
        .env("XDG_CONFIG_HOME", &format!("{}/.config", tmpdir))
        .env("XDG_STATE_HOME", &format!("{}/.local/state", tmpdir))
        // explicitly clear attribution env vars so we hit the UNATTRIBUTED branch
        .env_remove("SWITCHBOARD_NAME")
        .env_remove("USER")
        .output()
        .expect("done failed");
    assert!(out.status.success(), "done stderr: {}", String::from_utf8_lossy(&out.stderr));

    let events = events_jsonl(&tmpdir);
    assert!(
        events.contains("\"by\":\"UNATTRIBUTED\""),
        "audit does not carry by=UNATTRIBUTED: {}",
        events
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("UNATTRIBUTED"),
        "stderr does not warn about unattributable close: {}",
        stderr
    );
}
