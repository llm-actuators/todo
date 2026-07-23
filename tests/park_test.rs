//! §46 (b)-tests for the park backlog tier (operator-blessed tledg7h4a,
//! advocate §46-verify). Proves park isn't decorative:
//! - `park` HIDES an item from the default `list` (the backlog tier).
//! - `--include-parked` shows it (hidden, NOT deleted).
//! - `unpark` restores it to the default list.
//! - `park-suggest` keeps the item VISIBLE (an agent suggestion the operator
//!   confirms) and carries `park:suggested` — so it can't be a self-cleanup lever.
//! - the `[parked]` marker survives the parse/write roundtrip (persisted).

use std::process::Command;

fn todo_bin() -> String {
    env!("CARGO_BIN_EXE_todo").to_string()
}

fn tempdir() -> String {
    let base = std::env::temp_dir().join(format!(
        "todo_park_test_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&base).expect("mkdir tempdir");
    base.to_string_lossy().to_string()
}
static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Helper: run `todo` with the given args in an isolated HOME, return (stdout, ok).
fn run(tmp: &str, args: &[&str]) -> (String, bool) {
    let env = [
        ("HOME", tmp),
        ("XDG_CONFIG_HOME", &format!("{}/.config", tmp)),
    ];
    let out = Command::new(todo_bin())
        .args(args)
        .envs(env.iter().copied())
        .output()
        .expect("todo invocation failed");
    (String::from_utf8_lossy(&out.stdout).to_string(), out.status.success())
}

fn add(tmp: &str, company: &str, subject: &str) -> String {
    let (stdout, ok) = run(tmp, &["--company", company, "add", subject, "--priority", "P1"]);
    assert!(ok, "add failed");
    stdout.trim().to_string()
}

#[test]
fn park_hides_from_default_list_but_include_parked_shows_it() {
    let tmp = tempdir();
    let co = "_test_park";
    let id = add(&tmp, co, "parkable item");

    // Before park: in the default list.
    let (before, _) = run(&tmp, &["--company", co, "list"]);
    assert!(before.contains(&format!("\"id\":\"{}\"", id)), "item should be listed before park: {}", before);

    // Park it.
    let (_, ok) = run(&tmp, &["--company", co, "park", &id]);
    assert!(ok, "park failed");

    // Default list HIDES it (backlog tier).
    let (after, _) = run(&tmp, &["--company", co, "list"]);
    assert!(!after.contains(&format!("\"id\":\"{}\"", id)),
        "parked item MUST be hidden from default list — it wasn't: {}", after);

    // --include-parked SHOWS it (hidden, not deleted) with park:parked.
    let (incl, _) = run(&tmp, &["--company", co, "list", "--include-parked"]);
    assert!(incl.contains(&format!("\"id\":\"{}\"", id)),
        "--include-parked MUST show the parked item: {}", incl);
    assert!(incl.contains("\"park\":\"parked\""),
        "parked item MUST carry park:parked: {}", incl);
}

#[test]
fn unpark_restores_to_default_list() {
    let tmp = tempdir();
    let co = "_test_unpark";
    let id = add(&tmp, co, "roundtrip item");
    run(&tmp, &["--company", co, "park", &id]);
    let (_, ok) = run(&tmp, &["--company", co, "unpark", &id]);
    assert!(ok, "unpark failed");
    let (after, _) = run(&tmp, &["--company", co, "list"]);
    assert!(after.contains(&format!("\"id\":\"{}\"", id)),
        "unparked item MUST be back in the default list: {}", after);
    assert!(after.contains("\"park\":\"active\""), "unparked item MUST be active: {}", after);
}

#[test]
fn park_suggest_stays_visible_not_hidden() {
    let tmp = tempdir();
    let co = "_test_suggest";
    let id = add(&tmp, co, "suggested item");
    let (_, ok) = run(&tmp, &["--company", co, "park-suggest", &id]);
    assert!(ok, "park-suggest failed");
    // Suggested items are NOT hidden — the operator must still see them to confirm.
    let (list, _) = run(&tmp, &["--company", co, "list"]);
    assert!(list.contains(&format!("\"id\":\"{}\"", id)),
        "park-suggest MUST keep the item visible (not a self-cleanup lever): {}", list);
    assert!(list.contains("\"park\":\"suggested\""), "item MUST carry park:suggested: {}", list);
}

#[test]
fn parked_marker_survives_roundtrip() {
    let tmp = tempdir();
    let co = "_test_roundtrip";
    let id = add(&tmp, co, "persisted park");
    run(&tmp, &["--company", co, "park", &id]);
    // The ledger file must carry the [parked] marker (parse<->write roundtrip).
    let path = format!("{}/.config/substrate/{}/todos.md", tmp, co);
    let contents = std::fs::read_to_string(&path).expect("read ledger");
    assert!(contents.contains("[parked]"), "ledger MUST persist the [parked] marker: {}", contents);
    // And a re-read (list --include-parked) still resolves it as parked.
    let (incl, _) = run(&tmp, &["--company", co, "list", "--include-parked"]);
    assert!(incl.contains("\"park\":\"parked\""), "re-read MUST resolve park:parked: {}", incl);
}
