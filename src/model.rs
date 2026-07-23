//! Domain types. `Todos` is the in-memory mirror of one company's
//! `todos.md` file. Parser/writer roundtrips through this.

use serde::{Deserialize, Serialize, Serializer, Deserializer};

/// Priority bucket. Stored on the wire / in Markdown as the string
/// form (`P0`, `P1`, ...). Numeric `level` is for sort/comparison only.
///
/// Serialized as a string so the JSON output matches operator-visible
/// Markdown headings: `"priority":"P0"`, not `"priority":0`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Priority(pub u32);

impl Serialize for Priority {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.as_str())
    }
}

impl<'de> Deserialize<'de> for Priority {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let s = String::deserialize(d)?;
        Priority::parse(&s).ok_or_else(|| D::Error::custom("priority must be P<N>"))
    }
}

impl Priority {
    pub fn as_str(&self) -> String {
        format!("P{}", self.0)
    }
    /// Parse from a heading string ("P0" → Priority(0)). Returns None
    /// if the heading doesn't match the `P<N>` pattern.
    pub fn parse(s: &str) -> Option<Self> {
        s.strip_prefix('P')
            .and_then(|n| n.parse::<u32>().ok())
            .map(Priority)
    }
}

/// Idle-OK state for an item. Three values:
/// - `None` — default; item requires operator authorization to start.
/// - `Suggest` — overseer-set; a suggestion that operator hasn't blessed yet.
/// - `Approved` — operator-set or operator-blessed; idle-pool can pull autonomously.
///
/// Per SPEC-idle-pool-burn-governor-v0.2.md M1, only operator may set `Approved`
/// directly via `--idle-ok`. Overseers use `--idle-ok-suggest` to write `Suggest`;
/// `todo bless <id>` upgrades `Suggest` → `Approved`. Binary-layer enforcement of
/// the operator-only constraint is deferred to the router/hook layer (§I1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleOkState {
    None,
    Suggest,
    Approved,
}

impl Serialize for IdleOkState {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            Self::None => "none",
            Self::Suggest => "suggest",
            Self::Approved => "approved",
        })
    }
}

impl<'de> Deserialize<'de> for IdleOkState {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let s = String::deserialize(d)?;
        match s.as_str() {
            "none" => Ok(Self::None),
            "suggest" => Ok(Self::Suggest),
            "approved" => Ok(Self::Approved),
            other => Err(D::Error::custom(format!("unknown idle_ok state `{}`", other))),
        }
    }
}

impl Default for IdleOkState {
    fn default() -> Self { Self::None }
}

/// Park state for an item — separates STATUS (are we touching this) from PRIORITY
/// (how urgent). Three values:
/// - `Active` — default; a live item, shown in the normal list.
/// - `Suggested` — an agent flagged it as a park candidate (`park-suggest`); stays
///   VISIBLE (flagged) so the operator can confirm or ignore. Not hidden.
/// - `Parked` — operator confirmed `park`; hidden from the default list (backlog),
///   shown only with `--include-parked`. Not deleted — the item and its chain survive.
///
/// Operator-authorization (advocate guard, Right VII): only the operator may set
/// `Parked` / restore `Active`; agents may only `Suggested`. Like `bless`/`idle-ok`,
/// the binary stays identity-blind + audits `by=<handle>` (events.rs); the operator-only
/// enforcement is the router/hook layer, which sees the real tool call (a binary
/// env-check on SWITCHBOARD_NAME is bypassable by unsetting it — §I1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParkState {
    Active,
    Suggested,
    Parked,
}

impl Serialize for ParkState {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            Self::Active => "active",
            Self::Suggested => "suggested",
            Self::Parked => "parked",
        })
    }
}

impl<'de> Deserialize<'de> for ParkState {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let s = String::deserialize(d)?;
        match s.as_str() {
            "active" => Ok(Self::Active),
            "suggested" => Ok(Self::Suggested),
            "parked" => Ok(Self::Parked),
            other => Err(D::Error::custom(format!("unknown park state `{}`", other))),
        }
    }
}

impl Default for ParkState {
    fn default() -> Self { Self::Active }
}

/// Structured lane field extracted from `[lane:company/project/thread]` prefix.
/// The lane tag stays in `subject` text (backward-compat: fleet-tui reads subject);
/// this struct is a parsed copy for `count --by-project` and filter ops.
///
/// Per SPEC.md §Lane-tags: `thread = "_"` serializes as `null` (component is None).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lane {
    /// First path segment (e.g. "substrate", "acme", "outfit").
    pub company: String,
    /// Second path segment (e.g. "todo", "gate", "fleet-tui", "_").
    pub project: String,
    /// Third path segment; `_` in the tag becomes `None`.
    pub component: Option<String>,
}

/// One todo item.
///
/// `line` tracks the 1-indexed source line in `todos.md` — used by
/// editors and by `gate`'s external_check primitive for precise pattern
/// matching. Preserved across parse/write so an editor opening the
/// file lands exactly where the binary said the item lives.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    /// Opaque ID. Stable across reprioritization. 4-char hex by
    /// convention but the parser accepts any non-whitespace token.
    pub id: String,
    pub priority: Priority,
    pub subject: String,
    /// `true` if the checkbox is `[ ]`, `false` if `[x]`.
    pub open: bool,
    /// 1-indexed source line in the canonical writer output. Not
    /// authoritative across edits — re-read after any mutation.
    pub line: usize,
    /// Optional `(closed YYYY-MM-DD)` annotation appended on `done`.
    /// Parser preserves; writer re-emits.
    pub closed_on: Option<String>,
    /// Idle-OK state (M1, v0.2). Defaults to `None` for items that
    /// require operator authorization to start. Parser reads `[idle-ok]`
    /// / `[idle-ok-suggest]` markers; writer emits them.
    #[serde(default)]
    pub idle_ok: IdleOkState,
    /// Park state (backlog tier). Defaults to `Active`. `Parked` items are hidden
    /// from the default `list` (shown with `--include-parked`); `Suggested` stays
    /// visible (an agent's park candidate awaiting operator confirmation). Parser
    /// reads `[parked]` / `[park-suggest]` markers; writer emits them. Not deletion —
    /// parked items and their chains survive verbatim.
    #[serde(default)]
    pub park: ParkState,
    /// Released-to-client state (M1, v0.2). Default false. Operator flips
    /// via `todo release <id>`; tracked separately from `open` because a
    /// closed item may have been released, or not, or kept internal.
    /// Parser reads `[released <iso8601>]` marker; writer emits it.
    #[serde(default)]
    pub released: bool,
    /// ISO8601 UTC timestamp of the release (set when `released = true`).
    /// `None` when item is not released. Per SPEC §M3+M4 (demoted) +
    /// M5 egress-gate, this is the source-of-truth timestamp the
    /// fleet-progress digest and the egress-gate consult.
    #[serde(default)]
    pub released_ts: Option<String>,
    /// ISO8601 UTC timestamp of item creation (M0 per SPEC-time-embodiment).
    /// Auto-stamped on `todo add`. `None` for pre-M0 items (parser preserves
    /// absence; salience scoring treats absent as "unknown age, not zero").
    /// Backfill from wire / file mtime is a v0.2 candidate.
    #[serde(default)]
    pub created_ts: Option<String>,
    /// Structured parse of the `[lane:company/project/thread]` prefix in `subject`.
    /// The tag is also kept verbatim in `subject` for backward-compat consumers
    /// (fleet-tui, grep). This field is a derived fast-path for count/filter ops.
    /// `None` for untagged items.
    #[serde(default)]
    pub lane: Option<Lane>,
    /// ISO8601 UTC timestamp of first time item was observed open by a sweep.
    /// Stamped by `todo sweep` on every open item lacking it. `None` until first sweep.
    /// Used by the churn predicate: close with first_seen_open=None (and no force_real)
    /// is classified as churn (phantom burst).
    #[serde(default)]
    pub first_seen_open: Option<String>,
    /// Escape-hatch for legitimate closes faster than the sweep interval (5 min).
    /// Set via `todo add --force-real` or `todo done <id> --force-real`.
    /// When true, the item is NEVER classified as churn even if first_seen_open=None.
    #[serde(default)]
    pub force_real: bool,
    /// Evidence artifact pool — wire message-ids, file:line refs, URLs, or
    /// verification notes attached to this item. Parsed from `[ev <ref>]`
    /// markers in the chain; writer emits one marker per entry.
    #[serde(default)]
    pub evidence: Vec<String>,
}

/// One scored item returned by `todo weight`.
#[derive(Debug, Clone, Serialize)]
pub struct WeightedItem {
    pub id: String,
    pub subject: String,
    pub priority: String,
    pub created_ts: String,
    /// Age in hours, rounded to 1 decimal place.
    pub age_hours: f64,
    /// Salience weight = importance × ln(age_hours + 1), rounded to 2 dp.
    pub weight: f64,
}

/// Envelope returned by `ops::weight`.
#[derive(Debug, Clone, Serialize)]
pub struct WeightOutput {
    /// UTC timestamp of this computation.
    pub ts: String,
    pub company: String,
    pub top_n: usize,
    /// Open items that have a created_ts (scored pool).
    pub scored_count: usize,
    /// Open items without created_ts — pre-M0, excluded from scoring.
    pub unscored_count: usize,
    pub items: Vec<WeightedItem>,
}

/// Per-project open item counts, returned by `ops::count_by_project`.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectCount {
    pub p0: usize,
    pub p1: usize,
    pub p2: usize,
    pub other: usize,
    pub total: usize,
}

/// Envelope returned by `ops::count_by_project`.
#[derive(Debug, Clone, Serialize)]
pub struct CountByProjectOutput {
    pub company: String,
    /// Keys are `"company/project"` for tagged items, `"untagged"` for items
    /// with no lane tag. BTreeMap so output order is deterministic.
    pub projects: std::collections::BTreeMap<String, ProjectCount>,
}

/// Output of `todo stats --since <ts>`.
#[derive(Debug, Clone, Serialize)]
pub struct StatsOutput {
    pub company: String,
    /// The `--since` timestamp passed by the caller.
    pub since: String,
    /// Items closed on or after `since` that have first_seen_open set (or force_real).
    pub closed_real: usize,
    /// Items closed on or after `since` where first_seen_open=None AND force_real=false.
    pub excluded_churn: usize,
    /// Items closed on or after `since` where force_real=true (counted in closed_real).
    pub forced_real: usize,
    /// Human-readable summary line.
    pub summary: String,
}

/// One company's complete todo list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Todos {
    pub version: u32,
    pub company: String,
    pub items: Vec<Item>,
    /// Pre-`## P0` preamble (title line + HTML comments). Parser
    /// preserves verbatim; writer re-emits unchanged so operator
    /// notes survive roundtrips.
    #[serde(skip)]
    pub preamble: Vec<String>,
}

impl Todos {
    pub fn empty(company: &str) -> Self {
        Self {
            version: 1,
            company: company.to_string(),
            items: Vec::new(),
            preamble: Vec::new(),
        }
    }

    /// Lookup by stable ID.
    pub fn find(&self, id: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn find_mut(&mut self, id: &str) -> Option<&mut Item> {
        self.items.iter_mut().find(|i| i.id == id)
    }

    /// All items at one priority bucket, in source order. Closed items
    /// included unless `open_only` is true.
    pub fn by_priority(&self, priority: &Priority, open_only: bool) -> Vec<&Item> {
        self.items
            .iter()
            .filter(|i| &i.priority == priority && (!open_only || i.open))
            .collect()
    }

    /// Determine whether an ID is already present. Used by `add` to
    /// avoid stable-ID collisions when the generator unluckily picks
    /// an existing token.
    pub fn has_id(&self, id: &str) -> bool {
        self.items.iter().any(|i| i.id == id)
    }
}
