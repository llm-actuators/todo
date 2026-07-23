//! Resolve `~/.config/substrate/<company>/todos.md` paths.
//!
//! Per SPEC.md v1.1 Q3: company resolution is NOT in the binary.
//! Skill router (or operator's shell) sets `$TODO_COMPANY` based on
//! the active project dir; binary accepts `--company <name>` to
//! override; defaults to `global` when unset.

use std::env;
use std::path::PathBuf;

/// Resolve the company name from (in priority order) the `--company`
/// CLI flag, the `$TODO_COMPANY` env var, then default `_global`.
/// Canonical cross-company dir is `_global` (§38 doctrine). The old
/// default `global` was a bug — writes landed in the wrong path.
pub fn resolve_company(cli_company: Option<&str>) -> String {
    if let Some(c) = cli_company {
        return c.to_string();
    }
    env::var("TODO_COMPANY").unwrap_or_else(|_| "_global".to_string())
}

/// Absolute path to the `todos.md` for a given company.
pub fn todos_path(company: &str) -> PathBuf {
    let home = env::var("HOME").unwrap_or_default();
    PathBuf::from(home)
        .join(".config/substrate")
        .join(company)
        .join("todos.md")
}

/// Lock-file path. flock'd during any mutation so concurrent agents
/// serialize on writes; reads don't lock.
pub fn lock_path(company: &str) -> PathBuf {
    todos_path(company).with_file_name(".todos.lock")
}

/// Events log path. Append-only audit JSONL of every mutation.
pub fn events_path() -> PathBuf {
    let home = env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".local/state/todo/events.jsonl")
}
