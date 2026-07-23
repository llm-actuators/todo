//! todo binary entry point.
//!
//! Subcommands per SPEC.md v1.1:
//!   todo add "<subject>" [--priority P0]
//!   todo list [--priority P0] [--include-done] [--pretty]
//!   todo reprioritize <id> --to P0
//!   todo done <id>
//!   todo reopen <id>
//!   todo show <id>
//!   todo touch
//!   todo path
//!   todo company [<name>]
//!
//! Output: JSON by default; --pretty for human-readable table.
//!
//! Exit codes: 0 success, 1 user error (bad args), 2 not found / runtime
//! error.

use std::process::ExitCode;
use todo::model::{IdleOkState, Item, ParkState, Priority, Todos};
use todo::{ops, paths};

/// Parse a `--lane c/p[/comp]` argument into the canonical lane tag string
/// `[lane:c/p/comp]`. Two-segment form (`c/p`) expands component to `_`.
fn parse_lane_arg(s: &str) -> Result<String, String> {
    let parts: Vec<&str> = s.splitn(4, '/').collect();
    match parts.len() {
        2 => {
            if parts[0].is_empty() || parts[1].is_empty() {
                return Err(format!("lane `{}` must be company/project[/component]", s));
            }
            Ok(format!("[lane:{}/{}/{}]", parts[0], parts[1], "_"))
        }
        3 => {
            if parts[0].is_empty() || parts[1].is_empty() {
                return Err(format!("lane `{}` must be company/project[/component]", s));
            }
            let comp = if parts[2].is_empty() { "_" } else { parts[2] };
            Ok(format!("[lane:{}/{}/{}]", parts[0], parts[1], comp))
        }
        _ => Err(format!("lane `{}` must be company/project[/component]", s)),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut iter = args.into_iter().peekable();

    let mut cli_company: Option<String> = None;
    // Peek for global flags before subcommand dispatch.
    while let Some(a) = iter.peek() {
        match a.as_str() {
            "--company" => {
                iter.next();
                cli_company = iter.next();
            }
            _ => break,
        }
    }

    let company = paths::resolve_company(cli_company.as_deref());

    let subcmd = match iter.next() {
        Some(s) => s,
        None => return run_list(&company, None, false, false, None, None, None, None, false),
    };

    match subcmd.as_str() {
        "add" => {
            let subject = match iter.next() {
                Some(s) => s,
                None => {
                    eprintln!("todo: `add` requires a subject");
                    return ExitCode::from(1);
                }
            };
            let mut priority = Priority(1); // default P1
            let mut idle_ok = IdleOkState::None;
            let mut lane_tag: Option<String> = None;
            let mut force_real = false;
            while let Some(a) = iter.next() {
                match a.as_str() {
                    "--priority" | "-p" => {
                        let p = match iter.next() {
                            Some(p) => p,
                            None => {
                                eprintln!("todo: `--priority` needs a value");
                                return ExitCode::from(1);
                            }
                        };
                        priority = match Priority::parse(&p) {
                            Some(p) => p,
                            None => {
                                eprintln!("todo: bad priority `{}` (expected P0/P1/...)", p);
                                return ExitCode::from(1);
                            }
                        };
                    }
                    "--idle-ok" => idle_ok = IdleOkState::Approved,
                    "--idle-ok-suggest" => idle_ok = IdleOkState::Suggest,
                    "--force-real" => force_real = true,
                    "--lane" => {
                        let v = match iter.next() {
                            Some(v) => v,
                            None => {
                                eprintln!("todo: `--lane` needs a value (company/project[/component])");
                                return ExitCode::from(1);
                            }
                        };
                        match parse_lane_arg(&v) {
                            Ok(tag) => lane_tag = Some(tag),
                            Err(e) => {
                                eprintln!("todo: add: {}", e);
                                return ExitCode::from(1);
                            }
                        }
                    }
                    other => {
                        eprintln!("todo: add: unknown flag `{}`", other);
                        return ExitCode::from(1);
                    }
                }
            }
            // Prepend lane tag to subject if provided.
            let final_subject = match &lane_tag {
                Some(tag) => format!("{} {}", tag, subject),
                None => subject,
            };
            match ops::add(&company, &final_subject, priority, idle_ok, force_real) {
                Ok(id) => {
                    println!("{}", id);
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("todo: add failed: {}", e);
                    ExitCode::from(2)
                }
            }
        }

        "list" => {
            let mut priority_filter: Option<Priority> = None;
            let mut include_done = false;
            let mut pretty = false;
            let mut idle_ok_filter: Option<IdleOkState> = None;
            let mut released_filter: Option<bool> = None;
            let mut lane_company_filter: Option<String> = None;
            let mut lane_project_filter: Option<String> = None;
            let mut include_parked = false;
            while let Some(a) = iter.next() {
                match a.as_str() {
                    "--priority" | "-p" => {
                        let p = match iter.next() {
                            Some(p) => p,
                            None => return ExitCode::from(1),
                        };
                        priority_filter = Priority::parse(&p);
                    }
                    "--include-done" => include_done = true,
                    "--pretty" => pretty = true,
                    "--json" => { /* JSON is the default; accept the flag as a no-op for spec compliance */ }
                    "--idle-ok" => idle_ok_filter = Some(IdleOkState::Approved),
                    "--idle-ok-suggest" => idle_ok_filter = Some(IdleOkState::Suggest),
                    "--released" => released_filter = Some(true),
                    "--unreleased" => released_filter = Some(false),
                    "--include-parked" => include_parked = true,
                    "--company" => {
                        lane_company_filter = iter.next();
                    }
                    "--project" => {
                        lane_project_filter = iter.next();
                    }
                    other => {
                        eprintln!("todo: list: unknown flag `{}`", other);
                        return ExitCode::from(1);
                    }
                }
            }
            run_list(
                &company,
                priority_filter,
                include_done,
                pretty,
                idle_ok_filter,
                released_filter,
                lane_company_filter.as_deref(),
                lane_project_filter.as_deref(),
                include_parked,
            )
        }

        "reprioritize" => {
            let id = match iter.next() {
                Some(s) => s,
                None => return ExitCode::from(1),
            };
            let mut target: Option<Priority> = None;
            while let Some(a) = iter.next() {
                if a == "--to" {
                    if let Some(p) = iter.next() {
                        target = Priority::parse(&p);
                    }
                }
            }
            let target = match target {
                Some(p) => p,
                None => {
                    eprintln!("todo: reprioritize requires --to P<N>");
                    return ExitCode::from(1);
                }
            };
            match ops::reprioritize(&company, &id, target) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("todo: reprioritize: {}", e);
                    ExitCode::from(2)
                }
            }
        }

        "done" => {
            let id = match iter.next() {
                Some(s) => s,
                None => return ExitCode::from(1),
            };
            let mut force_real = false;
            while let Some(a) = iter.next() {
                match a.as_str() {
                    "--force-real" => force_real = true,
                    other => {
                        eprintln!("todo: done: unknown flag `{}`", other);
                        return ExitCode::from(1);
                    }
                }
            }
            match ops::done(&company, &id, force_real) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("todo: done: {}", e);
                    ExitCode::from(2)
                }
            }
        }

        "reopen" => {
            let id = match iter.next() {
                Some(s) => s,
                None => return ExitCode::from(1),
            };
            match ops::reopen(&company, &id) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("todo: reopen: {}", e);
                    ExitCode::from(2)
                }
            }
        }

        "show" => {
            let id = match iter.next() {
                Some(s) => s,
                None => return ExitCode::from(1),
            };
            let todos = match ops::load(&company) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("todo: show: {}", e);
                    return ExitCode::from(2);
                }
            };
            match todos.find(&id) {
                Some(item) => match serde_json::to_string_pretty(item) {
                    Ok(s) => {
                        println!("{}", s);
                        ExitCode::SUCCESS
                    }
                    Err(_) => ExitCode::from(2),
                },
                None => {
                    eprintln!("todo: show: id `{}` not found", id);
                    ExitCode::from(2)
                }
            }
        }

        "bless" => {
            let id = match iter.next() {
                Some(s) => s,
                None => {
                    eprintln!("todo: bless requires <id>");
                    return ExitCode::from(1);
                }
            };
            match ops::bless(&company, &id) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("todo: bless: {}", e);
                    ExitCode::from(2)
                }
            }
        }

        "release" => {
            let id = match iter.next() {
                Some(s) => s,
                None => {
                    eprintln!("todo: release requires <id>");
                    return ExitCode::from(1);
                }
            };
            match ops::release(&company, &id) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("todo: release: {}", e);
                    ExitCode::from(2)
                }
            }
        }

        "unrelease" => {
            let id = match iter.next() {
                Some(s) => s,
                None => {
                    eprintln!("todo: unrelease requires <id>");
                    return ExitCode::from(1);
                }
            };
            match ops::unrelease(&company, &id) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("todo: unrelease: {}", e);
                    ExitCode::from(2)
                }
            }
        }

        "park" => {
            let id = match iter.next() {
                Some(s) => s,
                None => {
                    eprintln!("todo: park requires <id>");
                    return ExitCode::from(1);
                }
            };
            match ops::park(&company, &id) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("todo: park: {}", e);
                    ExitCode::from(2)
                }
            }
        }

        "unpark" => {
            let id = match iter.next() {
                Some(s) => s,
                None => {
                    eprintln!("todo: unpark requires <id>");
                    return ExitCode::from(1);
                }
            };
            match ops::unpark(&company, &id) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("todo: unpark: {}", e);
                    ExitCode::from(2)
                }
            }
        }

        "park-suggest" => {
            let id = match iter.next() {
                Some(s) => s,
                None => {
                    eprintln!("todo: park-suggest requires <id>");
                    return ExitCode::from(1);
                }
            };
            match ops::park_suggest(&company, &id) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("todo: park-suggest: {}", e);
                    ExitCode::from(2)
                }
            }
        }

        "evidence" => {
            let id = match iter.next() {
                Some(s) => s,
                None => {
                    eprintln!("todo: evidence requires <id>");
                    return ExitCode::from(1);
                }
            };
            let mut add_ref: Option<String> = None;
            let mut do_clear = false;
            let mut json = false;
            while let Some(a) = iter.next() {
                match a.as_str() {
                    "--add" => {
                        add_ref = match iter.next() {
                            Some(v) => Some(v),
                            None => {
                                eprintln!("todo: evidence: --add requires a value");
                                return ExitCode::from(1);
                            }
                        };
                    }
                    "--clear" => do_clear = true,
                    "--json" => json = true,
                    other => {
                        eprintln!("todo: evidence: unknown flag `{}`", other);
                        return ExitCode::from(1);
                    }
                }
            }
            if do_clear {
                match ops::evidence_clear(&company, &id) {
                    Ok(()) => return ExitCode::SUCCESS,
                    Err(e) => {
                        eprintln!("todo: evidence: {}", e);
                        return ExitCode::from(2);
                    }
                }
            }
            if let Some(reference) = add_ref {
                match ops::evidence_add(&company, &id, &reference) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(e) => {
                        eprintln!("todo: evidence: {}", e);
                        ExitCode::from(2)
                    }
                }
            } else {
                let todos = match ops::load(&company) {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("todo: evidence: {}", e);
                        return ExitCode::from(2);
                    }
                };
                match todos.find(&id) {
                    Some(item) => {
                        if json {
                            match serde_json::to_string_pretty(&item.evidence) {
                                Ok(s) => { println!("{}", s); ExitCode::SUCCESS }
                                Err(e) => { eprintln!("todo: evidence: {}", e); ExitCode::from(2) }
                            }
                        } else {
                            for r in &item.evidence { println!("{}", r); }
                            ExitCode::SUCCESS
                        }
                    }
                    None => {
                        eprintln!("todo: evidence: id `{}` not found", id);
                        ExitCode::from(2)
                    }
                }
            }
        }

        "backfill-created" => {
            let mut dry_run = false;
            while let Some(a) = iter.next() {
                match a.as_str() {
                    "--dry-run" => dry_run = true,
                    other => {
                        eprintln!("todo: backfill-created: unknown flag `{}`", other);
                        return ExitCode::from(1);
                    }
                }
            }
            match ops::backfill_created(&company, dry_run) {
                Ok((scanned, stamped, rejected_future)) => {
                    println!(
                        "{{\"scanned\":{},\"stamped\":{},\"rejected_future\":{},\"dry_run\":{}}}",
                        scanned, stamped, rejected_future, dry_run
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("todo: backfill-created: {}", e);
                    ExitCode::from(2)
                }
            }
        }

        "weight" => {
            let mut top_n = 5usize;
            while let Some(a) = iter.next() {
                match a.as_str() {
                    "--top" | "-n" => {
                        match iter.next().and_then(|v| v.parse::<usize>().ok()) {
                            Some(n) => top_n = n,
                            None => {
                                eprintln!("todo: weight: --top requires a positive integer");
                                return ExitCode::from(1);
                            }
                        }
                    }
                    other => {
                        eprintln!("todo: weight: unknown flag `{}`", other);
                        return ExitCode::from(1);
                    }
                }
            }
            let out = ops::weight(&company, top_n);
            match serde_json::to_string_pretty(&out) {
                Ok(s) => {
                    println!("{}", s);
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("todo: weight: serialize error: {}", e);
                    ExitCode::from(2)
                }
            }
        }

        "sweep" => {
            // Stamp first_seen_open on all open items that lack it.
            // Idempotent. Intended to be run via cron: */5 * * * * todo sweep
            // An optional --all-companies flag is not yet supported (run per-company).
            match ops::sweep(&company) {
                Ok((scanned, stamped)) => {
                    println!("{{\"scanned\":{},\"stamped\":{}}}", scanned, stamped);
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("todo: sweep: {}", e);
                    ExitCode::from(2)
                }
            }
        }

        "stats" => {
            let mut since: Option<String> = None;
            let mut json = false;
            while let Some(a) = iter.next() {
                match a.as_str() {
                    "--since" => {
                        since = iter.next();
                    }
                    "--json" => json = true,
                    other => {
                        eprintln!("todo: stats: unknown flag `{}`", other);
                        return ExitCode::from(1);
                    }
                }
            }
            let since = match since {
                Some(s) => s,
                None => {
                    eprintln!("todo: stats: requires --since <iso8601>");
                    return ExitCode::from(1);
                }
            };
            let out = ops::stats(&company, &since);
            if json {
                match serde_json::to_string_pretty(&out) {
                    Ok(s) => { println!("{}", s); ExitCode::SUCCESS }
                    Err(e) => { eprintln!("todo: stats: serialize error: {}", e); ExitCode::from(2) }
                }
            } else {
                println!("{}", out.summary);
                ExitCode::SUCCESS
            }
        }

        "touch" => match ops::touch(&company) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("todo: touch: {}", e);
                ExitCode::from(2)
            }
        },

        "path" => {
            println!("{}", ops::path_for(&company).display());
            ExitCode::SUCCESS
        }

        "company" => {
            if let Some(_new) = iter.next() {
                // v0.1 doesn't write the symlink — operator sets
                // $TODO_COMPANY or substrate's skill router does.
                eprintln!("todo: company-setting via flag not implemented in v0.1; set $TODO_COMPANY or pass --company");
                return ExitCode::from(1);
            }
            println!("{}", company);
            ExitCode::SUCCESS
        }

        "count" => {
            let mut by_project = false;
            while let Some(a) = iter.next() {
                match a.as_str() {
                    "--by-project" => by_project = true,
                    "--json" => { /* JSON is default */ }
                    other => {
                        eprintln!("todo: count: unknown flag `{}`", other);
                        return ExitCode::from(1);
                    }
                }
            }
            if !by_project {
                eprintln!("todo: count: requires --by-project");
                return ExitCode::from(1);
            }
            let out = ops::count_by_project(&company);
            match serde_json::to_string_pretty(&out) {
                Ok(s) => {
                    println!("{}", s);
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("todo: count: serialize error: {}", e);
                    ExitCode::from(2)
                }
            }
        }

        "--help" | "-h" | "help" => {
            print_help();
            ExitCode::SUCCESS
        }

        other => {
            eprintln!("todo: unknown subcommand `{}`. Try `todo --help`.", other);
            ExitCode::from(1)
        }
    }
}

fn run_list(
    company: &str,
    priority_filter: Option<Priority>,
    include_done: bool,
    pretty: bool,
    idle_ok_filter: Option<IdleOkState>,
    released_filter: Option<bool>,
    lane_company: Option<&str>,
    lane_project: Option<&str>,
    include_parked: bool,
) -> ExitCode {
    let todos = match ops::load(company) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("todo: list: {}", e);
            return ExitCode::from(2);
        }
    };
    let items: Vec<&Item> = todos
        .items
        .iter()
        .filter(|i| {
            (include_done || i.open)
                // Parked items are the backlog tier — hidden by default, shown with
                // --include-parked. Suggested + Active always show (a suggested park
                // stays visible for the operator to confirm).
                && (include_parked || i.park != ParkState::Parked)
                && priority_filter
                    .as_ref()
                    .map(|p| &i.priority == p)
                    .unwrap_or(true)
                && idle_ok_filter
                    .map(|f| i.idle_ok == f)
                    .unwrap_or(true)
                && released_filter
                    .map(|f| i.released == f)
                    .unwrap_or(true)
                && lane_company
                    .map(|c| i.lane.as_ref().map(|l| l.company == c).unwrap_or(false))
                    .unwrap_or(true)
                && lane_project
                    .map(|p| i.lane.as_ref().map(|l| l.project == p).unwrap_or(false))
                    .unwrap_or(true)
        })
        .collect();

    if pretty {
        print_pretty(company, &todos, &items);
    } else {
        print_json(company, &items);
    }
    ExitCode::SUCCESS
}

fn print_json(company: &str, items: &[&Item]) {
    #[derive(serde::Serialize)]
    struct Out<'a> {
        version: u32,
        company: &'a str,
        items: Vec<&'a Item>,
    }
    let out = Out {
        version: 1,
        company,
        items: items.iter().copied().collect(),
    };
    // Use compact JSON by default for jq downstream; --pretty path
    // uses a different code path entirely (table).
    match serde_json::to_string(&out) {
        Ok(s) => println!("{}", s),
        Err(_) => eprintln!("todo: JSON encode failed"),
    }
}

fn print_pretty(_company: &str, _todos: &Todos, items: &[&Item]) {
    if items.is_empty() {
        println!("(no items)");
        return;
    }
    println!("{:<8}  {:<5}  {:<6}  {}", "id", "prio", "open", "subject");
    println!("{}", "─".repeat(80));
    for item in items {
        let open_str = if item.open { "open" } else { "done" };
        let subject = if item.subject.len() > 60 {
            format!("{}...", &item.subject[..57])
        } else {
            item.subject.clone()
        };
        println!(
            "{:<8}  {:<5}  {:<6}  {}",
            item.id,
            item.priority.as_str(),
            open_str,
            subject
        );
    }
}

fn print_help() {
    println!(
        "todo — priority-aware Markdown todo list

usage:
    todo [--company <name>] <subcommand> [args...]

subcommands:
    add \"<subject>\" [--priority P0]      add new open item (default P1)
    list [--priority P0] [--include-done] [--include-parked] [--pretty]
                                          list items as JSON (default) or table
                                          (parked items hidden unless --include-parked)
    reprioritize <id> --to P0             move item between buckets
    done <id>                             mark closed
    reopen <id>                           re-open closed item
    park <id>                             move to backlog (hidden from default list;
                                          operator-authorized, not deleted)
    unpark <id>                           restore a parked item to the active list
    park-suggest <id>                     flag a park candidate (stays visible; the
                                          agent path — operator confirms via park)
    show <id>                             print one item as JSON
    touch                                 back-fill missing IDs + reformat
    path                                  print path to active company's todos.md
    company                               print active company name

global flags (before subcommand):
    --company <name>     override $TODO_COMPANY (default: global)

storage:
    ~/.config/substrate/<company>/todos.md
    ~/.local/state/todo/events.jsonl      audit log of mutations

JSON output schema (compact, jq-friendly):
    {{\"version\":1,\"company\":\"...\",\"items\":[
        {{\"id\":\"...\",\"priority\":\"P0\",\"subject\":\"...\",\"open\":true,\"line\":N}}
    ]}}"
    );
}
