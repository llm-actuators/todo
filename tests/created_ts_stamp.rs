//! M0 (b)-test per SPEC-time-embodiment + advocate dispatch
//! (triad msg voqsfju1byl1fyz4): a fresh `todo add` produces an
//! item carrying `created_ts`, and downstream consumers (salience
//! score, JSON output) can read it.
//!
//! Per §46 "enforcement is real or it's theater" doctrine: this is
//! the paired test that proves M0 isn't decorative. No (b)-test, no
//! promote-to-hard.

use std::process::Command;

fn todo_bin() -> String {
    env!("CARGO_BIN_EXE_todo").to_string()
}

#[test]
fn fresh_add_stamps_created_ts() {
    let tmpdir = tempdir();
    let company = "_test_m0";
    let env = [
        ("HOME", tmpdir.as_str()),
        ("XDG_CONFIG_HOME", &format!("{}/.config", tmpdir)),
    ];

    let add_out = Command::new(todo_bin())
        .args(["--company", company, "add", "M0 (b)-test subject", "--priority", "P1"])
        .envs(env.iter().copied())
        .output()
        .expect("add failed");
    assert!(add_out.status.success(), "add stderr: {}",
        String::from_utf8_lossy(&add_out.stderr));
    let id = String::from_utf8_lossy(&add_out.stdout).trim().to_string();
    assert!(!id.is_empty(), "add did not return an id");

    let list_out = Command::new(todo_bin())
        .args(["--company", company, "list"])
        .envs(env.iter().copied())
        .output()
        .expect("list failed");
    let json = String::from_utf8_lossy(&list_out.stdout);

    // (b)-test assertion: created_ts present, ISO8601 UTC format.
    assert!(
        json.contains("\"created_ts\""),
        "list JSON lacks created_ts field: {}",
        json
    );
    assert!(
        json.contains(&format!("\"id\":\"{}\"", id)),
        "list JSON lacks new id {}: {}",
        id,
        json
    );

    // Cheap ISO8601 sanity: contains a T..Z window in the new item's record.
    let v: serde_json::Value = serde_json::from_str(&json).expect("list JSON parse");
    let items = v["items"].as_array().expect("items array");
    let new_item = items
        .iter()
        .find(|it| it["id"].as_str() == Some(&id))
        .expect("new item present");
    let ts = new_item["created_ts"]
        .as_str()
        .expect("created_ts string");
    assert!(
        ts.len() >= 20 && ts.contains('T') && ts.ends_with('Z'),
        "created_ts not ISO8601 UTC: {}",
        ts
    );
}

fn tempdir() -> String {
    use std::time::SystemTime;
    let n = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = format!("/tmp/todo-m0-test-{}", n);
    std::fs::create_dir_all(&p).unwrap();
    p
}
