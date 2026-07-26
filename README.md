# todo — priority-aware Markdown task ledger for agent cohorts

A single-binary CLI that stores a shared, per-project todo list as human-readable Markdown, with priority buckets, a parked backlog tier, salience scoring, and an append-only audit log. Built so a cohort of autonomous agents (and a human operator) can coordinate cross-session work over a file that stays greppable and hand-editable.

Part of the [llm-actuators](https://github.com/llm-actuators) toolchain — single-purpose CLIs an LLM agent uses to act on real systems (files, processes, devices, other agents). Each tool is sterile: it exposes primitives and records facts; domain knowledge and policy live in the caller, not the binary.

## What it does

`todo` is the write-and-query layer over a `todos.md` file. The Markdown *is* the database — the binary parses it, mutates an in-memory model, and re-renders canonical Markdown on every write. That keeps the ledger diffable, greppable, editable by hand, and survivable across process/session boundaries, while the binary supplies the structure (stable IDs, priority buckets, state markers) a plain text file can't enforce on its own.

Design stance: the binary is a **primitive**, not a policy engine. It is identity-blind — it records *who* mutated an item (from an env var) into an audit log, but it does not decide *whether* a given actor is allowed to. Authorization is layered above by whatever invokes the binary. This keeps the tool sterile and composable.

Core concepts:

- **Priority buckets** — items live under `## P0`, `## P1`, `## P2`, … headings. Priority is orthogonal to status.
- **Stable IDs** — each item carries an opaque 4-char id (`[#abc1]`) that survives reprioritization, so references stay valid when an item moves buckets.
- **Open/done** — a Markdown checkbox (`- [ ]` / `- [x]`); `done` stamps a `(closed YYYY-MM-DD)` annotation.
- **Park tier (backlog)** — separates *status* (are we touching this) from *priority* (how urgent). `Parked` items are hidden from the default list but not deleted; `Suggested` items stay visible as park candidates awaiting confirmation.
- **Idle-ok state** (`none` / `suggest` / `approved`) — flags whether an item may be picked up autonomously vs. requiring explicit authorization; `bless` upgrades a suggestion to approved.
- **Released state** — tracks whether a closed item was delivered externally, timestamped separately from `open`.
- **Lane tags** — an optional `[lane:company/project/component]` subject prefix, parsed into a structured field for `count`/filter ops while remaining verbatim in the subject text.
- **Churn accounting** — a `sweep` stamps `first_seen_open` on live items; items added and closed without ever being observed open (a "phantom burst") are classified as churn and excluded from genuine-close stats.
- **Salience weighting** — `weight` scores open items by `importance × ln(age_hours + 1)` so old-and-important items surface.
- **Evidence pool** — arbitrary refs (`file:line`, URLs, notes) attachable to an item.

## Build

Rust, no non-crates.io dependencies.

```sh
cargo build --release        # binary at target/release/todo
cargo test                   # unit + integration tests (parser, writer, weight, park, churn, attribution)
cargo clippy                 # all lints denied (see Cargo.toml)
```

## Usage

```
todo [--company <name>] <subcommand> [args...]
```

Output is **compact JSON by default** (jq-friendly); `--pretty` renders a human table where supported. Company selection: `--company <name>` flag → `$TODO_COMPANY` env var → default `_global`. Exit codes: `0` success, `1` user error, `2` not found / runtime error.

### Reading

```sh
todo list                              # open, non-parked items (JSON)
todo list --pretty                     # id / prio / open / subject table
todo list --priority P0                # filter to one bucket
todo list --include-done               # include closed items
todo list --include-parked             # include the backlog tier
todo show <id>                         # one item as pretty JSON
todo path                              # absolute path to the active todos.md
```

### Mutating

```sh
todo add "fix login crash"                       # new open P1 item; prints the new id
todo add "urgent" --priority P0                  # add into a specific bucket
todo add "task" --lane acme/app-android          # prepend a lane tag (company/project[/component])
todo reprioritize <id> --to P0                   # move between buckets (id stays stable)
todo done <id>                                   # close; stamps (closed YYYY-MM-DD)
todo reopen <id>                                 # reopen a closed item
```

### Backlog / park tier

```sh
todo park-suggest <id>   # flag a park candidate — STAYS VISIBLE
todo park <id>           # move to backlog — hidden from default list, NOT deleted
todo unpark <id>         # restore a parked/suggested item to the active list
```

### Authorization & delivery markers

```sh
todo bless <id>          # idle-ok: suggest -> approved
todo release <id>        # mark delivered externally; stamps released_ts (UTC ISO8601)
```

### Evidence

```sh
todo evidence <id>                    # print attached refs
todo evidence <id> --add src/foo.rs:42
todo evidence <id> --clear            # drop all refs
```

### Reporting & maintenance

```sh
todo weight --top 5                   # top-N open items by salience (importance x ln(age+1))
todo count --by-project               # open-item counts grouped by lane company/project
todo stats --since 2026-01-01         # genuine closes since T, excluding churn
todo sweep                            # stamp first_seen_open on open items lacking it (idempotent)
todo touch                            # back-fill missing IDs + re-canonicalize after hand-edits
```

`sweep` is idempotent and cron-friendly — it is the mechanism that makes churn accounting work: an item never seen open by a sweep before it closes is treated as a phantom.

## Storage

The ledger is a Markdown file per company:

```
~/.config/<app>/<company>/todos.md
```

Canonical format:

```markdown
# todos — <company>
<!-- operator preamble notes preserved verbatim -->

## P0
- [ ] [#abc1] fix login crash
- [x] [#def2] ship release notes (closed 2026-01-14)

## P1
- [ ] [#ghi3] [created 2026-01-10T09:00:00Z] [lane:acme/app/_] later item
```

Structured state is encoded as inline bracket markers between the id and the subject, in a fixed order. Absent state emits **no marker** (byte-stable round-trips: parse → render → parse is idempotent for known content). Items missing an id are back-filled on the next write / `touch`.

Persistence properties:

- **Atomic writes** — render to a sibling tempfile, `fsync`, then `rename` over the target. A crash leaves either the old or the new file, never a half-written one.
- **Cross-process locking** — every mutation takes an exclusive OS `flock` on a sibling `.todos.lock`, so concurrent writers serialize. Reads skip the lock.
- **Canonicalizing** — the writer preserves the title/preamble verbatim but re-renders items to canonical form; the round-trip is canonicalizing, not bit-exact.

An append-only audit log records every mutation as JSONL at `~/.local/state/todo/events.jsonl`. Each line: `{ts, op, id, by, from?, to?, subject?, company}`. The actor (`by`) is read from `$SWITCHBOARD_NAME`; if unset, the mutation still proceeds but is stamped `by=UNATTRIBUTED` (a silent unknown actor is never allowed). The audit write is best-effort. Downstream tooling greps this log for forensics ("who demoted P0 X", "who closed Y").

## Where it fits

In a toolchain of small, single-purpose CLIs an LLM agent uses to act on real systems, `todo` is the durable, cross-session task ledger:

- **Survives session boundaries** — the file persists; an agent whose context is wiped (compaction, restart) re-reads the ledger instead of losing in-flight work.
- **Coordinates a cohort** — multiple agents and a human share one file per project, serialized by the file lock; the audit log attributes every change.
- **Feeds higher layers** — JSON output pipes into digests, dashboards, and TUIs; `weight`/`count`/`stats` are the aggregation surface. Authorization (who may park, bless, or release) is enforced above the binary, which stays identity-blind by design.
