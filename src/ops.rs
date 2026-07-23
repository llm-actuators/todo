//! CRUD operations. Each public op:
//! 1. Acquires a flock on `.todos.lock` (via OS file lock).
//! 2. Reads + parses `todos.md`.
//! 3. Mutates the in-memory model.
//! 4. Writes back to `todos.md` (atomic via tempfile + rename).
//! 5. Appends an audit event to `~/.local/state/todo/events.jsonl`.
//! 6. Releases lock.
//!
//! Reads (`list`, `show`, `path`) skip the lock — they're idempotent
//! and tolerate parallel writes (worst case: a transient view).

use crate::events;
use crate::model::{CountByProjectOutput, IdleOkState, Item, ParkState, Priority, ProjectCount, StatsOutput, Todos, WeightedItem, WeightOutput};
use crate::parser;
use crate::paths::{lock_path, todos_path};
use crate::writer;
use fs2::FileExt;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Load `todos.md` for the given company. Returns an empty `Todos`
/// if the file doesn't exist yet — `add` will create it on first
/// write.
pub fn load(company: &str) -> std::io::Result<Todos> {
    let path = todos_path(company);
    if !path.exists() {
        return Ok(Todos::empty(company));
    }
    let mut s = String::new();
    fs::File::open(&path)?.read_to_string(&mut s)?;
    Ok(parser::parse(&s, company))
}

/// Atomic write: render to a sibling tempfile, fsync, rename over.
/// Crash-safe: either the old file or the new file is present, never
/// a half-written one.
fn save(company: &str, todos: &Todos) -> std::io::Result<()> {
    let path = todos_path(company);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = path.with_extension("md.tmp");
    let rendered = writer::render(todos);
    {
        let mut f = fs::File::create(&tmp_path)?;
        f.write_all(rendered.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp_path, &path)?;
    Ok(())
}

/// Acquire an exclusive OS flock on `.todos.lock` for the duration of `f`.
/// Blocks until the lock is available. All mutation ops go through this
/// so concurrent agents (launchd sweep + interactive add/done) serialize.
fn with_lock<R, F: FnOnce() -> std::io::Result<R>>(
    company: &str,
    f: F,
) -> std::io::Result<R> {
    let lock = lock_path(company);
    if let Some(parent) = lock.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new().create(true).write(true).open(&lock)?;
    file.lock_exclusive()?;
    let result = f();
    let _ = file.unlock();
    result
}

/// Generate a stable opaque ID for a new item. 4-char base36 from a
/// PRNG. Collision-checked against existing items in caller.
fn generate_id() -> String {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0)
        ^ std::process::id() as u64;
    let alpha = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut n = nanos.wrapping_mul(2654435769).wrapping_add(1);
    let mut s = String::new();
    for _ in 0..4 {
        n = n.wrapping_mul(2654435769).wrapping_add(1);
        s.push(alpha[(n as usize) % alpha.len()] as char);
    }
    s
}

/// Bless an item: upgrade `Suggest` → `Approved`. Per SPEC v0.2 M1
/// this is the operator-only operation; binary stays identity-blind
/// (router/hook layer enforces who can run it).
pub fn bless(company: &str, id: &str) -> std::io::Result<()> {
    with_lock(company, || {
        let mut todos = load(company)?;
        let item = todos.find_mut(id).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("id `{}` not found", id))
        })?;
        item.idle_ok = IdleOkState::Approved;
        save(company, &todos)?;
        events::record("bless", id, company, None, None, None);
        Ok(())
    })
}

/// Mark an item released: sets `released = true` and stamps `released_ts`
/// with the current UTC ISO8601 timestamp. Reversible via `unrelease`.
pub fn release(company: &str, id: &str) -> std::io::Result<()> {
    with_lock(company, || {
        let mut todos = load(company)?;
        let ts = current_iso8601();
        let item = todos.find_mut(id).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("id `{}` not found", id))
        })?;
        item.released = true;
        item.released_ts = Some(ts.clone());
        save(company, &todos)?;
        events::record("release", id, company, None, None, Some(&ts));
        Ok(())
    })
}

/// Reverse a release: clears `released` and `released_ts`.
pub fn unrelease(company: &str, id: &str) -> std::io::Result<()> {
    with_lock(company, || {
        let mut todos = load(company)?;
        let item = todos.find_mut(id).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("id `{}` not found", id))
        })?;
        item.released = false;
        item.released_ts = None;
        save(company, &todos)?;
        events::record("unrelease", id, company, None, None, None);
        Ok(())
    })
}

/// Park an item: set `park = Parked`, hiding it from the default `list` (the
/// backlog tier). NOT deletion — the item + its chain survive verbatim; reverse
/// via `unpark`. Operator-authorized (advocate guard, Right VII): the binary
/// stays identity-blind + audits `by=<handle>` (events.rs); the operator-only
/// enforcement is the router/hook layer, which sees the real tool call — a binary
/// SWITCHBOARD_NAME check is bypassable by unsetting it, so it is NOT the
/// enforcement point (§I1). Agents use `park_suggest`.
pub fn park(company: &str, id: &str) -> std::io::Result<()> {
    with_lock(company, || {
        let mut todos = load(company)?;
        let item = todos.find_mut(id).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("id `{}` not found", id))
        })?;
        item.park = ParkState::Parked;
        save(company, &todos)?;
        events::record("park", id, company, None, None, None);
        Ok(())
    })
}

/// Restore a parked/suggested item to `Active` (back into the default list).
/// Operator-authorized (see `park`).
pub fn unpark(company: &str, id: &str) -> std::io::Result<()> {
    with_lock(company, || {
        let mut todos = load(company)?;
        let item = todos.find_mut(id).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("id `{}` not found", id))
        })?;
        item.park = ParkState::Active;
        save(company, &todos)?;
        events::record("unpark", id, company, None, None, None);
        Ok(())
    })
}

/// Suggest an item for parking: set `park = Suggested`. Stays VISIBLE (flagged) so
/// the operator can confirm (`park`) or ignore (`unpark`). This is the AGENT path —
/// it never hides an item, so it can't be a self-cleanup lever (Right VII).
pub fn park_suggest(company: &str, id: &str) -> std::io::Result<()> {
    with_lock(company, || {
        let mut todos = load(company)?;
        let item = todos.find_mut(id).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("id `{}` not found", id))
        })?;
        item.park = ParkState::Suggested;
        save(company, &todos)?;
        events::record("park-suggest", id, company, None, None, None);
        Ok(())
    })
}

/// Append an evidence reference to an item's pool. Each call adds one entry.
/// Refs may not contain `]` — that character would break the `[ev ...]` marker close.
pub fn evidence_add(company: &str, id: &str, reference: &str) -> std::io::Result<()> {
    if reference.contains(']') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "evidence ref must not contain ']'",
        ));
    }
    with_lock(company, || {
        let mut todos = load(company)?;
        let item = todos.find_mut(id).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("id `{}` not found", id))
        })?;
        item.evidence.push(reference.to_string());
        save(company, &todos)?;
        events::record("evidence_add", id, company, None, None, Some(reference));
        Ok(())
    })
}

/// Clear all evidence refs from an item's pool.
pub fn evidence_clear(company: &str, id: &str) -> std::io::Result<()> {
    with_lock(company, || {
        let mut todos = load(company)?;
        let item = todos.find_mut(id).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("id `{}` not found", id))
        })?;
        item.evidence.clear();
        save(company, &todos)?;
        events::record("evidence_clear", id, company, None, None, None);
        Ok(())
    })
}

fn current_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // Compute UTC YYYY-MM-DDTHH:MM:SSZ from epoch secs without chrono dep.
    // Days since 1970-01-01.
    let days = (secs / 86400) as i64;
    let time_of_day = secs % 86400;
    let hh = time_of_day / 3600;
    let mm = (time_of_day % 3600) / 60;
    let ss = time_of_day % 60;
    let (y, mo, d) = civil_from_days(days);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo, d, hh, mm, ss)
}

// Howard Hinnant's algorithm: civil date from days since 1970-01-01.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe/1460 + doe/36524 - doe/146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe/4 - yoe/100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2)/5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Add a new open item at the given priority. Returns the new ID.
/// `force_real` stamps `[real]` immediately — escape-hatch for items expected to
/// close faster than the 5-minute sweep interval.
pub fn add(company: &str, subject: &str, priority: Priority, idle_ok: IdleOkState, force_real: bool) -> std::io::Result<String> {
    with_lock(company, || {
        let mut todos = load(company)?;
        let mut id = generate_id();
        while todos.has_id(&id) {
            id = generate_id();
        }
        let created_ts = current_iso8601();
        let item = Item {
            idle_ok,
            park: ParkState::default(),
            released: false,
            released_ts: None,
            created_ts: Some(created_ts.clone()),
            id: id.clone(),
            priority: priority.clone(),
            subject: subject.to_string(),
            open: true,
            line: 0,
            closed_on: None,
            lane: None,
            first_seen_open: None,
            force_real,
            evidence: vec![],
        };
        todos.items.push(item);
        save(company, &todos)?;
        events::record("add", &id, company, None, Some(&priority.as_str()), Some(subject));
        Ok(id)
    })
}

/// Move an item to a new priority bucket. Reprioritization is the
/// load-bearing op for operator (R3 deliverable). Idempotent if the
/// target == current.
pub fn reprioritize(company: &str, id: &str, target: Priority) -> std::io::Result<()> {
    with_lock(company, || {
        let mut todos = load(company)?;
        let from_str;
        {
            let item = todos
                .find_mut(id)
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "id not found"))?;
            from_str = item.priority.as_str();
            item.priority = target.clone();
        }
        save(company, &todos)?;
        events::record(
            "reprioritize",
            id,
            company,
            Some(&from_str),
            Some(&target.as_str()),
            None,
        );
        Ok(())
    })
}

/// Mark closed. Records today's date in `closed_on`.
/// If `force_real` is true, also stamps the `[real]` marker on the item —
/// escape-hatch for legitimate closes faster than the 5-minute sweep interval.
pub fn done(company: &str, id: &str, force_real: bool) -> std::io::Result<()> {
    with_lock(company, || {
        let mut todos = load(company)?;
        let today = {
            let ts = current_iso8601();
            ts[..10].to_string() // "YYYY-MM-DD"
        };
        {
            let item = todos
                .find_mut(id)
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "id not found"))?;
            item.open = false;
            item.closed_on = Some(today);
            if force_real {
                item.force_real = true;
            }
        }
        save(company, &todos)?;
        events::record("done", id, company, None, None, None);
        Ok(())
    })
}

/// Re-open a closed item.
pub fn reopen(company: &str, id: &str) -> std::io::Result<()> {
    with_lock(company, || {
        let mut todos = load(company)?;
        {
            let item = todos
                .find_mut(id)
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "id not found"))?;
            item.open = true;
            item.closed_on = None;
        }
        save(company, &todos)?;
        events::record("reopen", id, company, None, None, None);
        Ok(())
    })
}

/// M0.5 backfill: for every item with `created_ts == None`, attempt to
/// stamp a real filing date. Strictly conservative per advocate's
/// HOLD on the v0.1 draft (triad qz6w4xzlvhnhsi1g): the prior version
/// grabbed mid-subject incidental dates (commit-context, blessed-at,
/// even FUTURE-tagged dates) and would have poisoned 12 of 13 items.
///
/// Re-draft rules:
///   1. LEADING position only. The regex matches only at the start of
///      the subject (modulo a single bracket). Mid-subject dates are
///      incidental, not filing dates.
///   2. FUTURE GUARD. Any candidate date > today (UTC) is rejected.
///      Negative ages break the salience score.
///   3. mtime + wire-source-ref-ts fallbacks are M0.6 (mtime requires
///      design — first-create vs last-modified — and wire walk is a
///      separate codepath).
///
/// Returns (scanned, stamped, rejected_future). `dry_run = true` walks
/// the same code path but skips save+audit.
pub fn backfill_created(company: &str, dry_run: bool) -> std::io::Result<(usize, usize, usize)> {
    with_lock(company, || {
        let mut todos = load(company)?;
        let today = current_iso8601();
        let today_date = &today[..10]; // "YYYY-MM-DD"
        let mut scanned = 0usize;
        let mut stamped = 0usize;
        let mut rejected_future = 0usize;
        for item in todos.items.iter_mut() {
            if item.created_ts.is_some() {
                continue;
            }
            scanned += 1;
            let Some(date) = extract_leading_iso_date(&item.subject) else { continue };
            if date.as_str() > today_date {
                rejected_future += 1;
                continue;
            }
            item.created_ts = Some(format!("{}T00:00:00Z", date));
            stamped += 1;
        }
        if !dry_run && stamped > 0 {
            save(company, &todos)?;
            events::record(
                "backfill-created",
                &format!("stamped-{}", stamped),
                company,
                None,
                None,
                Some(&format!(
                    "scanned={},rejected_future={}",
                    scanned, rejected_future
                )),
            );
        }
        Ok((scanned, stamped, rejected_future))
    })
}

/// Match YYYY-MM-DD only if it leads the subject (optionally inside a
/// single leading `[` bracket). Mid-subject dates are not filing dates.
fn extract_leading_iso_date(s: &str) -> Option<String> {
    let trimmed = s.trim_start();
    let candidate = trimmed.strip_prefix('[').unwrap_or(trimmed);
    let bytes = candidate.as_bytes();
    if bytes.len() < 10 {
        return None;
    }
    let w = &bytes[..10];
    if w[0].is_ascii_digit()
        && w[1].is_ascii_digit()
        && w[2].is_ascii_digit()
        && w[3].is_ascii_digit()
        && w[4] == b'-'
        && w[5].is_ascii_digit()
        && w[6].is_ascii_digit()
        && w[7] == b'-'
        && w[8].is_ascii_digit()
        && w[9].is_ascii_digit()
    {
        // Reject if the 11th byte is a digit (would be a longer numeric run).
        if bytes.get(10).map(|b| b.is_ascii_digit()).unwrap_or(false) {
            return None;
        }
        return Some(std::str::from_utf8(w).ok()?.to_string());
    }
    None
}

/// Parse an ISO8601 UTC timestamp string ("YYYY-MM-DDTHH:MM:SSZ") into
/// seconds since UNIX epoch. Returns None on malformed input.
pub fn parse_iso8601_secs(s: &str) -> Option<u64> {
    if s.len() < 19 { return None; }
    let year: i64 = s[..4].parse().ok()?;
    let month: i64 = s[5..7].parse().ok()?;
    let day: i64 = s[8..10].parse().ok()?;
    let hour: u64 = s[11..13].parse().ok()?;
    let min: u64 = s[14..16].parse().ok()?;
    let sec: u64 = s[17..19].parse().ok()?;
    let days = days_from_civil(year, month, day);
    Some((days as u64) * 86400 + hour * 3600 + min * 60 + sec)
}

// Inverse of civil_from_days: signed days since 1970-01-01 from a civil date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Compute salience weights for M0-dated open items.
///
/// Formula: `weight = importance × ln(age_hours + 1)`
/// Importance: P0→3.0, P1→2.0, P2→1.0, P3+→1.0.
/// Only items where `open == true` AND `created_ts != None` are scored.
/// Pre-M0 items (null created_ts) are counted in `unscored_count` so the
/// caller can communicate the scope boundary to the agent.
///
/// Returns top `top_n` items by weight descending.
pub fn weight(company: &str, top_n: usize) -> WeightOutput {
    let todos = load(company).unwrap_or_else(|_| Todos::empty(company));
    let now_ts = current_iso8601();
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let mut scored_count = 0usize;
    let mut unscored_count = 0usize;
    let mut scored: Vec<WeightedItem> = Vec::new();

    for item in &todos.items {
        if !item.open { continue; }
        let Some(ref cts) = item.created_ts else {
            unscored_count += 1;
            continue;
        };
        scored_count += 1;
        let created_secs = parse_iso8601_secs(cts).unwrap_or(now_secs);
        let age_secs = now_secs.saturating_sub(created_secs);
        let age_hours = age_secs as f64 / 3600.0;
        let importance: f64 = match item.priority.0 {
            0 => 3.0,
            1 => 2.0,
            _ => 1.0,
        };
        let w = importance * (age_hours + 1.0).ln();
        scored.push(WeightedItem {
            id: item.id.clone(),
            subject: item.subject.clone(),
            priority: item.priority.as_str(),
            created_ts: cts.clone(),
            age_hours: (age_hours * 10.0).round() / 10.0,
            weight: (w * 100.0).round() / 100.0,
        });
    }

    scored.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(top_n);

    WeightOutput {
        ts: now_ts,
        company: company.to_string(),
        top_n,
        scored_count,
        unscored_count,
        items: scored,
    }
}

/// Count open items grouped by `lane.company/lane.project`.
///
/// Untagged items (lane=None) bucket under the key `"untagged"` — this is
/// the coverage-gap signal; they must NOT be silently dropped or mis-grouped.
/// Only open items are counted.
pub fn count_by_project(company: &str) -> CountByProjectOutput {
    let todos = load(company).unwrap_or_else(|_| Todos::empty(company));
    let mut projects: std::collections::BTreeMap<String, ProjectCount> =
        std::collections::BTreeMap::new();
    for item in &todos.items {
        if !item.open { continue; }
        let key = match &item.lane {
            Some(lane) => format!("{}/{}", lane.company, lane.project),
            None => "untagged".to_string(),
        };
        let entry = projects.entry(key).or_insert(ProjectCount {
            p0: 0, p1: 0, p2: 0, other: 0, total: 0,
        });
        match item.priority.0 {
            0 => entry.p0 += 1,
            1 => entry.p1 += 1,
            2 => entry.p2 += 1,
            _ => entry.other += 1,
        }
        entry.total += 1;
    }
    CountByProjectOutput {
        company: company.to_string(),
        projects,
    }
}

/// Touch: parse + back-fill any missing IDs + re-canonicalize the
/// file. Useful after operator hand-edits.
pub fn touch(company: &str) -> std::io::Result<()> {
    with_lock(company, || {
        let mut todos = load(company)?;
        // Pre-collect taken IDs to avoid borrow conflict with iter_mut.
        let mut taken: std::collections::HashSet<String> =
            todos.items.iter().map(|i| i.id.clone()).collect();
        let mut backfilled = 0;
        for item in todos.items.iter_mut() {
            if item.id.is_empty() {
                let mut new_id = generate_id();
                while taken.contains(&new_id) {
                    new_id = generate_id();
                }
                taken.insert(new_id.clone());
                item.id = new_id;
                backfilled += 1;
            }
        }
        save(company, &todos)?;
        if backfilled > 0 {
            events::record("touch", &format!("backfilled-{}", backfilled), company, None, None, None);
        }
        Ok(())
    })
}

/// Stamp `first_seen_open = <now>` on every OPEN item that lacks it.
/// Idempotent: already-stamped items are skipped (None → ts only).
/// The file is saved only if at least one item was stamped.
/// Returns (scanned_open, stamped).
pub fn sweep(company: &str) -> std::io::Result<(usize, usize)> {
    with_lock(company, || {
        let mut todos = load(company)?;
        let now = current_iso8601();
        let mut scanned = 0usize;
        let mut stamped = 0usize;
        for item in todos.items.iter_mut() {
            if !item.open { continue; }
            scanned += 1;
            if item.first_seen_open.is_none() {
                item.first_seen_open = Some(now.clone());
                stamped += 1;
            }
        }
        if stamped > 0 {
            save(company, &todos)?;
            events::record(
                "sweep",
                &format!("stamped-{}", stamped),
                company,
                None,
                None,
                Some(&format!("scanned={}", scanned)),
            );
        }
        Ok((scanned, stamped))
    })
}

/// The UTC timestamp constant marking the start of Phase-2a churn tracking.
/// Items created before this epoch have first_seen_open=None because the field
/// didn't exist yet — they are EXEMPT from churn classification.
pub const PHASE_2A_EPOCH: &str = "2026-07-02T00:00:00Z";

/// Returns true if an item's close should be classified as CHURN.
///
/// Churn predicate (Phase-2a simple):
///   - closed item (verified by caller)
///   - first_seen_open is None (never swept while open)
///   - NOT force_real (no escape-hatch stamp)
///   - created_ts >= PHASE_2A_EPOCH (exempt pre-epoch items to avoid retroactive mislabeling)
pub fn is_churn(item: &Item) -> bool {
    if item.force_real { return false; }
    if item.first_seen_open.is_some() { return false; }
    // Exempt items created before phase-2a tracking started.
    match &item.created_ts {
        Some(ts) => ts.as_str() >= PHASE_2A_EPOCH,
        None => false, // no created_ts = pre-phase-2a era, exempt
    }
}

/// Compute drain-delta stats since a given ISO8601 timestamp.
///
/// drain-delta = genuine closes EXCLUDING churn.
/// Closes are items where `closed_on >= since_date` (date portion of since).
/// Output names exclusions: "closed N real, excluded M churn, K forced-real".
pub fn stats(company: &str, since: &str) -> StatsOutput {
    let todos = load(company).unwrap_or_else(|_| Todos::empty(company));
    // Use only the date portion for comparison with closed_on (YYYY-MM-DD).
    let since_date = &since[..since.len().min(10)];

    let mut closed_real = 0usize;
    let mut excluded_churn = 0usize;
    let mut forced_real = 0usize;

    for item in &todos.items {
        if item.open { continue; }
        let Some(ref closed_date) = item.closed_on else { continue };
        if closed_date.as_str() < since_date { continue; }
        // Item was closed on or after since_date.
        if is_churn(item) {
            excluded_churn += 1;
        } else {
            closed_real += 1;
            if item.force_real {
                forced_real += 1;
            }
        }
    }

    let summary = format!(
        "closed {} real, excluded {} churn, {} forced-real",
        closed_real, excluded_churn, forced_real
    );

    StatsOutput {
        company: company.to_string(),
        since: since.to_string(),
        closed_real,
        excluded_churn,
        forced_real,
        summary,
    }
}

/// Path lookup for `--path` flag.
pub fn path_for(company: &str) -> std::path::PathBuf {
    todos_path(company)
}

/// Verify a file exists at the given path. Used by `which` / tests.
pub fn exists(p: &Path) -> bool {
    p.exists()
}

#[cfg(test)]
mod churn_tests {
    use super::*;
    use crate::model::{Item, Priority};

    fn make_item(id: &str, open: bool, first_seen_open: Option<&str>, force_real: bool, created_ts: Option<&str>, closed_on: Option<&str>) -> Item {
        Item {
            id: id.to_string(),
            priority: Priority(1),
            subject: "test item".to_string(),
            open,
            line: 0,
            closed_on: closed_on.map(|s| s.to_string()),
            idle_ok: crate::model::IdleOkState::None,
            park: crate::model::ParkState::Active,
            released: false,
            released_ts: None,
            created_ts: created_ts.map(|s| s.to_string()),
            lane: None,
            first_seen_open: first_seen_open.map(|s| s.to_string()),
            force_real,
            evidence: vec![],
        }
    }

    // §42 guard 1: byte-stable writer — None/false emit nothing.
    #[test]
    fn writer_emits_nothing_for_absent_fseen_real() {
        use crate::model::Todos;
        use crate::writer;
        let mut todos = Todos::empty("test");
        todos.items.push(make_item("a1b2", true, None, false, None, None));
        let out = writer::render(&todos);
        assert!(!out.contains("[fseen"), "fseen must not appear when first_seen_open=None");
        assert!(!out.contains("[real]"), "[real] must not appear when force_real=false");
    }

    // §42 guard 1 continued: fseen and real ARE emitted when present.
    #[test]
    fn writer_emits_fseen_and_real_when_set() {
        use crate::model::Todos;
        use crate::writer;
        let mut todos = Todos::empty("test");
        todos.items.push(make_item("a1b2", true, Some("2026-07-02T01:00:00Z"), true, None, None));
        let out = writer::render(&todos);
        assert!(out.contains("[fseen 2026-07-02T01:00:00Z]"), "fseen must appear");
        assert!(out.contains("[real]"), "[real] must appear");
    }

    // Roundtrip: fseen + real survive parse→write→parse.
    #[test]
    fn roundtrip_fseen_real_markers() {
        let src = "## P1\n- [ ] [#a1b2] [created 2026-07-02T00:10:00Z] [fseen 2026-07-02T01:00:00Z] [real] the subject\n";
        let parsed = crate::parser::parse(src, "test");
        let rendered = crate::writer::render(&parsed);
        let reparsed = crate::parser::parse(&rendered, "test");
        let item = &reparsed.items[0];
        assert_eq!(item.first_seen_open.as_deref(), Some("2026-07-02T01:00:00Z"));
        assert!(item.force_real);
        assert_eq!(item.subject, "the subject");
    }

    // §42 guard 2: pre-epoch items are EXEMPT from churn.
    #[test]
    fn pre_epoch_item_not_churn() {
        let item = make_item("b1c2", false, None, false, Some("2026-06-30T12:00:00Z"), Some("2026-07-02"));
        assert!(!is_churn(&item), "pre-epoch item must not be flagged churn");
    }

    // No created_ts = pre-epoch by convention, also exempt.
    #[test]
    fn no_created_ts_not_churn() {
        let item = make_item("c2d3", false, None, false, None, Some("2026-07-02"));
        assert!(!is_churn(&item), "item with no created_ts must not be flagged churn");
    }

    // SHAPE (a): phantom burst — add+done 10x, NO sweep between → EXCLUDE all 10.
    #[test]
    fn shape_a_phantom_burst_all_excluded() {
        // 10 items: created_ts >= epoch, first_seen_open=None, force_real=false, closed today.
        let items: Vec<Item> = (0..10).map(|i| {
            make_item(&format!("ph{:02}", i), false, None, false,
                      Some("2026-07-02T00:30:00Z"), Some("2026-07-02"))
        }).collect();
        let churn_count = items.iter().filter(|i| is_churn(i)).count();
        assert_eq!(churn_count, 10, "all 10 phantom items must be churn");

        let mut todos = crate::model::Todos::empty("test");
        todos.items = items;
        let out = stats_from_todos(&todos, "2026-07-02T00:00:00Z");
        assert_eq!(out.excluded_churn, 10);
        assert_eq!(out.closed_real, 0);
    }

    // SHAPE (b): legit — add → sweep → done → COUNTED.
    #[test]
    fn shape_b_legit_close_counted() {
        let item = make_item("lg01", false,
                             Some("2026-07-02T00:05:00Z"), // swept
                             false,
                             Some("2026-07-02T00:01:00Z"),
                             Some("2026-07-02"));
        assert!(!is_churn(&item));
        let mut todos = crate::model::Todos::empty("test");
        todos.items = vec![item];
        let out = stats_from_todos(&todos, "2026-07-02T00:00:00Z");
        assert_eq!(out.closed_real, 1);
        assert_eq!(out.excluded_churn, 0);
    }

    // SHAPE (c): cross-day — add "day1", done "day2", NO sweep → EXCLUDED.
    #[test]
    fn shape_c_cross_day_no_sweep_excluded() {
        let item = make_item("cd01", false,
                             None, // no sweep
                             false,
                             Some("2026-07-02T00:01:00Z"), // created day1 (after epoch)
                             Some("2026-07-03")); // closed day2
        assert!(is_churn(&item));
        let mut todos = crate::model::Todos::empty("test");
        todos.items = vec![item];
        let out = stats_from_todos(&todos, "2026-07-02T00:00:00Z");
        assert_eq!(out.excluded_churn, 1);
    }

    // SHAPE (d): force-real — add --force-real → immediate done → COUNTED despite no fseen.
    #[test]
    fn shape_d_force_real_counted() {
        let item = make_item("fr01", false,
                             None, // no sweep
                             true, // force_real
                             Some("2026-07-02T00:01:00Z"),
                             Some("2026-07-02"));
        assert!(!is_churn(&item));
        let mut todos = crate::model::Todos::empty("test");
        todos.items = vec![item];
        let out = stats_from_todos(&todos, "2026-07-02T00:00:00Z");
        assert_eq!(out.closed_real, 1);
        assert_eq!(out.forced_real, 1);
        assert_eq!(out.excluded_churn, 0);
    }

    // SHAPE (e): sweep-reliability — item open across >=1 cron-sweep WITHOUT fleet-digest
    // read → gets stamped → then closed → COUNTED.
    #[test]
    fn shape_e_sweep_reliability_counted() {
        // Simulates: item created, sweep fires (stamps fseen), then closed.
        let item = make_item("sr01", false,
                             Some("2026-07-02T00:05:00Z"), // stamped by cron sweep
                             false,
                             Some("2026-07-02T00:01:00Z"),
                             Some("2026-07-02"));
        assert!(!is_churn(&item), "swept item must not be churn");
        let mut todos = crate::model::Todos::empty("test");
        todos.items = vec![item];
        let out = stats_from_todos(&todos, "2026-07-02T00:00:00Z");
        assert_eq!(out.closed_real, 1);
        assert_eq!(out.excluded_churn, 0);
    }

    // --- Evidence guard tests ---

    #[test]
    fn evidence_add_rejects_bracket_in_ref() {
        // A ref containing ']' must return Err — callers/scripts detect failure via non-zero exit.
        let result = super::evidence_add("__test_nonexistent__", "id", "bad]ref");
        assert!(result.is_err(), "expected Err for ref containing ']'");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("']'"), "error should name the forbidden char: {}", msg);
    }

    #[test]
    fn evidence_add_accepts_clean_ref() {
        // Verify the guard does NOT fire on a clean ref (error is only from missing file, not the guard).
        let result = super::evidence_add("__test_nonexistent__", "id", "wire:abc123");
        // Should reach the filesystem and fail with NotFound, not InvalidInput.
        match result {
            Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {
                panic!("guard fired on a clean ref: {}", e);
            }
            _ => {} // NotFound or any other IO error is expected (company doesn't exist)
        }
    }

    // Helper: run stats logic against an in-memory Todos (avoids filesystem).
    fn stats_from_todos(todos: &crate::model::Todos, since: &str) -> StatsOutput {
        let since_date = &since[..since.len().min(10)];
        let mut closed_real = 0usize;
        let mut excluded_churn = 0usize;
        let mut forced_real = 0usize;
        for item in &todos.items {
            if item.open { continue; }
            let Some(ref closed_date) = item.closed_on else { continue };
            if closed_date.as_str() < since_date { continue; }
            if is_churn(item) {
                excluded_churn += 1;
            } else {
                closed_real += 1;
                if item.force_real { forced_real += 1; }
            }
        }
        let summary = format!("closed {} real, excluded {} churn, {} forced-real",
                              closed_real, excluded_churn, forced_real);
        StatsOutput {
            company: todos.company.clone(),
            since: since.to_string(),
            closed_real, excluded_churn, forced_real, summary,
        }
    }
}
