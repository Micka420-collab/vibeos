//! Agent supervisor core — the pure, testable logic behind `vibectl agent run`.
//!
//! Implements the decision/parsing pieces of ADR-012 (reasoning capture by
//! tapping the CLI stream) and ADR-013 (always-on autonomy with an async T2/T3
//! approval queue). The process orchestration (spawning the CLI, reading its
//! stdout, the kill-switch pidfile) lives in `bin/vibectl.rs`; everything here
//! is side-effect-light and unit-tested, so the tricky logic never depends on a
//! live CLI.
//!
//! What the supervisor does at runtime (bin/vibectl.rs):
//!   1. stamps a session id, writes an `autonomous_session` START journal event;
//!   2. spawns the CLI in structured mode (`claude -p --output-format stream-json`
//!      or the provider equivalent) with a wall-clock + tool-call budget;
//!   3. taps each streamed line: `extract_thinking` -> `reasoning::append_thinking`
//!      into `<memory>/reasoning/<session>.jsonl` (never the CLI's own transcript);
//!   4. enforces the budget (deadline kill) — the human kill-switch is
//!      `vibectl agent stop` (operator-only), never an MCP tool;
//!   5. writes an `autonomous_session` END journal event with the outcome.
//!
//! ADR-013: a task needing a not-yet-approved T2/T3 does NOT block. vibed already
//! answers `pending_approval` (non-blocking) and the operator approves out of
//! band; the agent, prompted by that response (see /etc/skel/.claude/CLAUDE.md),
//! moves on to other T0/T1 work. The supervisor's part is to keep running and to
//! record the pending item — it never lifts the T2/T3 floor.

use std::time::Duration;

use serde_json::{json, Value};

/// Parse a human budget like `8h`, `30m`, `45s`, `8h30m`, `90` (bare = seconds)
/// into whole seconds. Returns `None` on empty / malformed / zero input.
pub fn parse_duration(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Bare integer -> seconds. Restrict the fast path to pure ASCII digits so a
    // signed literal ("+8", "-8") — which u64::from_str otherwise accepts the
    // '+' form of — is rejected rather than silently read as a budget.
    if s.bytes().all(|b| b.is_ascii_digit()) {
        return s.parse::<u64>().ok().filter(|&n| n > 0);
    }
    let mut total: u64 = 0;
    let mut num: u64 = 0;
    let mut saw_digit = false;
    let mut saw_unit = false;
    // Rank of the last unit seen (h=3, m=2, s=1). Units must appear at most once
    // and in strictly descending order, so a typo-doubled or reordered flag
    // ("8h8h", "30m8h") is REJECTED rather than silently summed to a
    // longer-than-intended budget for an unattended run — matching the strict
    // intent already stated for "8h30".
    let mut last_rank: Option<u8> = None;
    for c in s.chars() {
        if let Some(d) = c.to_digit(10) {
            num = num.checked_mul(10)?.checked_add(d as u64)?;
            saw_digit = true;
        } else {
            let (mult, rank) = match c {
                'h' | 'H' => (3600u64, 3u8),
                'm' | 'M' => (60, 2),
                's' | 'S' => (1, 1),
                _ => return None,
            };
            if !saw_digit {
                return None; // a unit with no preceding number
            }
            if last_rank.is_some_and(|prev| rank >= prev) {
                return None; // duplicate or out-of-order unit
            }
            last_rank = Some(rank);
            total = total.checked_add(num.checked_mul(mult)?)?;
            num = 0;
            saw_digit = false;
            saw_unit = true;
        }
    }
    // Trailing digits with no unit are invalid in the compound form (avoid
    // silently reading "8h30" as 8h + 30s).
    if saw_digit || !saw_unit {
        return None;
    }
    (total > 0).then_some(total)
}

/// A running session's budget: an optional wall-clock allowance, an optional
/// tool-call cap and an optional token cap.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    /// Wall-clock allowance. Enforced against a MONOTONIC elapsed time (see
    /// `wall_expired`), NOT the system clock — a clock step (NTP correction, VM
    /// snapshot resume, boot-with-wrong-RTC then sync) must never extend a
    /// runaway agent's run past its budget.
    pub wall: Option<Duration>,
    pub max_tool_calls: Option<u64>,
    /// Total-token allowance for the run (input + output + cache-write +
    /// cache-read, i.e. [`TokenLedger::total_tokens`]). `None` = unbounded on
    /// tokens. BEST-EFFORT, same caveat as `max_tool_calls`: it is driven only
    /// by the `usage` blocks the CLI reports on its `stream-json` stream (a
    /// non-contractual schema, see [`extract_usage`]). A CLI that under-reports
    /// usage is under-counted, so this deadline may not fire — the WALL-CLOCK
    /// budget remains the hard runtime cap regardless of what the stream says.
    pub max_tokens: Option<u64>,
}

impl Budget {
    /// Build from optional wall-clock seconds, tool-call cap and token cap.
    /// `None` wall = effectively unbounded on time.
    pub fn new(
        wall_secs: Option<u64>,
        max_tool_calls: Option<u64>,
        max_tokens: Option<u64>,
    ) -> Self {
        Self {
            wall: wall_secs.map(Duration::from_secs),
            max_tool_calls,
            max_tokens,
        }
    }
    /// Wall-clock budget exhausted after `elapsed`? `elapsed` MUST come from a
    /// monotonic clock (`Instant::elapsed`) so the check cannot be defeated by a
    /// backward system-clock step.
    pub fn wall_expired(&self, elapsed: Duration) -> bool {
        self.wall.is_some_and(|w| elapsed >= w)
    }
    /// Tool-call budget exhausted at `count`?
    pub fn tool_calls_exhausted(&self, count: u64) -> bool {
        self.max_tool_calls.is_some_and(|m| count >= m)
    }
    /// Token budget exhausted at `total`? `total` is the running
    /// [`TokenLedger::total_tokens`].
    pub fn tokens_exhausted(&self, total: u64) -> bool {
        self.max_tokens.is_some_and(|m| total >= m)
    }
}

/// How aggressively a supervised run should spend tokens. A *strategy*, not a
/// model choice — the model (Opus/Sonnet/Haiku) is selected in the CLI command;
/// this maps a human intent ("be frugal") to a concrete [`Budget`] and to the
/// caching/context guidance an operator (and the citizen itself, ADR-028) should
/// follow. See docs/TOKENS.md.
///
/// The presets are DEFAULTS an operator overrides with explicit `--budget` /
/// `--calls` / `--tokens`; they exist so `--mode frugale` alone yields a sane,
/// bounded run instead of the unbounded default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsumptionMode {
    /// Minimise spend: short leash, tight token cap. For routine/again-and-again
    /// work where a wrong turn is cheap to redo. Lean context, cache everything.
    Frugale,
    /// The default balance of reach and cost.
    Equilibree,
    /// Spend for depth: long leash, generous caps. For hard one-shot reasoning
    /// where re-running costs more than the tokens.
    Performance,
}

impl ConsumptionMode {
    /// Parse a mode name (case-insensitive; French or English spellings, and
    /// their ASCII fold). `None` on anything unrecognised — the caller decides
    /// whether that is an error or a fall-through to the unbounded default.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "frugale" | "frugal" | "eco" | "éco" => Some(Self::Frugale),
            "equilibree" | "équilibrée" | "equilibre" | "équilibré" | "balanced" | "balance" => {
                Some(Self::Equilibree)
            }
            "performance" | "perf" | "max" => Some(Self::Performance),
            _ => None,
        }
    }
    /// The canonical (French) name, for journaling and display.
    pub fn name(self) -> &'static str {
        match self {
            Self::Frugale => "frugale",
            Self::Equilibree => "équilibrée",
            Self::Performance => "performance",
        }
    }
    /// The preset budget for this mode. These are deliberately round, defensive
    /// numbers — a floor of safety for an unattended run — not tuned SLAs.
    pub fn budget(self) -> Budget {
        match self {
            //                       wall           calls        tokens
            Self::Frugale => Budget::new(Some(1800), Some(60), Some(300_000)),
            Self::Equilibree => Budget::new(Some(14400), Some(400), Some(3_000_000)),
            Self::Performance => Budget::new(Some(28800), Some(1200), Some(12_000_000)),
        }
    }
}

/// Extract a normalized reasoning block from one `stream-json` event, or `None`
/// if the event carries no reasoning. Returns `{ kind, text }` where kind is
/// `thinking` (full block), `thinking_delta` (streamed increment) or
/// `redacted_thinking` (provider-encrypted — text is empty).
///
/// NOTE: the exact `stream-json` schema is provider- and version-specific and
/// NOT contractual (ADR-012). This handles the documented Claude shapes
/// defensively; anything unrecognized yields `None` (we never guess). Verify
/// against the packaged CLI at integration time.
pub fn extract_thinking(event: &Value) -> Option<Value> {
    match event.get("type").and_then(Value::as_str)? {
        // Streamed increment: {"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"..."}}
        "content_block_delta" => {
            let delta = event.get("delta")?;
            match delta.get("type").and_then(Value::as_str)? {
                "thinking_delta" => {
                    let text = delta.get("thinking").and_then(Value::as_str)?;
                    Some(json!({ "kind": "thinking_delta", "text": text }))
                }
                _ => None,
            }
        }
        // Full assistant message: {"type":"assistant","message":{"content":[{"type":"thinking",...}]}}
        "assistant" => {
            let content = event.get("message")?.get("content")?.as_array()?;
            let mut texts: Vec<String> = Vec::new();
            let mut redacted = false;
            for item in content {
                match item.get("type").and_then(Value::as_str) {
                    Some("thinking") => {
                        if let Some(t) = item.get("thinking").and_then(Value::as_str) {
                            texts.push(t.to_string());
                        }
                    }
                    Some("redacted_thinking") => redacted = true,
                    _ => {}
                }
            }
            if !texts.is_empty() {
                Some(json!({ "kind": "thinking", "text": texts.join("") }))
            } else if redacted {
                Some(json!({ "kind": "redacted_thinking", "text": "" }))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Does this `stream-json` event represent the agent invoking a tool? Defensive,
/// same schema caveat as above.
pub fn is_tool_use(event: &Value) -> bool {
    count_tool_use(event) > 0
}

/// Count the `tool_use` blocks in this event. An assistant turn can carry SEVERAL
/// parallel tool calls in one message; counting them (not just "any") keeps the
/// `--calls` budget honest against batching. Defensive, same schema caveat.
///
/// BEST-EFFORT (not a security boundary): the `--calls` budget is driven ONLY by
/// the tool_use blocks the CLI reports on its `stream-json` stream (a
/// non-contractual schema). A CLI that performs tool calls without emitting them
/// there — or with a schema we do not recognize — is undercounted, so the
/// call-count deadline may never fire. This bounds *cost/verbosity*, not the
/// security envelope: every real tool call still passes vibed's audit, per-uid
/// rate limit and approval gate independently, and the WALL-CLOCK budget still
/// caps total runtime regardless of what the stream reports.
pub fn count_tool_use(event: &Value) -> usize {
    match event.get("type").and_then(Value::as_str) {
        Some("assistant") => event
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter(|i| i.get("type").and_then(Value::as_str) == Some("tool_use"))
                    .count()
            })
            .unwrap_or(0),
        _ => 0,
    }
}

/// One `stream-json` `usage` block: the four token counts a Claude API call
/// reports. Split out because the four are billed very differently — the whole
/// point of measuring them separately is to make the cache lever *visible*:
/// `cache_read` costs ~10% of a fresh input token and `cache_creation` ~125%
/// (Anthropic's documented, model-independent ratios). See [`TokenLedger`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    /// Fresh (uncached) input tokens read this turn.
    pub input: u64,
    /// Output (generated) tokens this turn.
    pub output: u64,
    /// Input tokens WRITTEN into the prompt cache this turn (a 5-min-TTL write
    /// bills at ~1.25× a fresh input token; it pays for itself on the first
    /// re-read).
    pub cache_creation: u64,
    /// Input tokens served FROM the prompt cache this turn (bills at ~0.10× a
    /// fresh input token — the saving caching buys).
    pub cache_read: u64,
}

impl TokenUsage {
    fn saturating_add(self, o: Self) -> Self {
        Self {
            input: self.input.saturating_add(o.input),
            output: self.output.saturating_add(o.output),
            cache_creation: self.cache_creation.saturating_add(o.cache_creation),
            cache_read: self.cache_read.saturating_add(o.cache_read),
        }
    }
}

/// Pull the `usage` counts out of ONE `stream-json` event, or `None` if the
/// event carries none. Reads the per-turn `usage` on an `assistant` message —
/// each assistant turn is one API call, so SUMMING these across a run
/// ([`TokenLedger::add_event`]) yields the run's true token consumption.
///
/// A `result` event ALSO carries a `usage` (last turn, cumulative, or absent —
/// version-dependent); to avoid double-counting we deliberately do NOT read it
/// here (its authoritative figure is the cost, see [`extract_final_cost`]).
///
/// SCHEMA CAVEAT (identical to [`extract_thinking`]): the `stream-json` schema
/// is provider- and version-specific and NOT contractual (ADR-012). Missing
/// count fields default to 0; an event with no `usage` object yields `None`. We
/// never guess. This measures cost/consumption, it is NOT a security boundary.
pub fn extract_usage(event: &Value) -> Option<TokenUsage> {
    if event.get("type").and_then(Value::as_str)? != "assistant" {
        return None;
    }
    let usage = event.get("message")?.get("usage")?;
    let field = |k: &str| usage.get(k).and_then(Value::as_u64).unwrap_or(0);
    let u = TokenUsage {
        input: field("input_tokens"),
        output: field("output_tokens"),
        cache_creation: field("cache_creation_input_tokens"),
        cache_read: field("cache_read_input_tokens"),
    };
    // An all-zero usage object is real (a cache-only turn can be 0/0/0/N) — only
    // a totally absent object is `None`, handled by the `?` on `usage` above.
    Some(u)
}

/// The authoritative run cost in USD from a terminal `result` event
/// (`total_cost_usd`), or `None`. The CLI computes this with its own
/// always-current price table, so we prefer it over any estimate. `result` is
/// cumulative for the run, so the LAST one wins (the caller stores, not sums).
/// Same schema caveat as [`extract_usage`].
pub fn extract_final_cost(event: &Value) -> Option<f64> {
    if event.get("type").and_then(Value::as_str)? != "result" {
        return None;
    }
    event
        .get("total_cost_usd")
        .and_then(Value::as_f64)
        .filter(|c| c.is_finite() && *c >= 0.0)
}

/// Running total of a supervised run's token consumption, plus the metrics that
/// make token spend *manageable* rather than merely counted. Pure and additive
/// — feed it every `stream-json` event ([`add_event`]); it ignores everything
/// that is not a per-turn `assistant` usage block.
///
/// [`add_event`]: TokenLedger::add_event
#[derive(Debug, Clone, Copy, Default)]
pub struct TokenLedger {
    pub totals: TokenUsage,
    /// Number of usage-bearing turns folded in (denominator for per-turn means).
    pub turns: u64,
}

impl TokenLedger {
    /// Fold one event's usage into the running total; a no-op for events that
    /// carry none. Returns whether anything was added (useful for tests/metrics).
    pub fn add_event(&mut self, event: &Value) -> bool {
        match extract_usage(event) {
            Some(u) => {
                self.totals = self.totals.saturating_add(u);
                self.turns = self.turns.saturating_add(1);
                true
            }
            None => false,
        }
    }
    /// Every token that crossed the wire: input + output + cache write + cache
    /// read. This is what [`Budget::tokens_exhausted`] caps.
    pub fn total_tokens(&self) -> u64 {
        self.totals
            .input
            .saturating_add(self.totals.output)
            .saturating_add(self.totals.cache_creation)
            .saturating_add(self.totals.cache_read)
    }
    /// Fraction of prompt-side tokens served from cache, in `[0, 1]`, or `None`
    /// when there was no read-eligible input yet. THE lever for token
    /// management: a high ratio means the expensive context is being re-read
    /// from cache (~0.10×) instead of re-billed as fresh input (1.0×).
    pub fn cache_hit_ratio(&self) -> Option<f32> {
        let denom = self.totals.input.saturating_add(self.totals.cache_read);
        (denom > 0).then(|| self.totals.cache_read as f32 / denom as f32)
    }
    /// Input-side spend expressed in *fresh-input-token equivalents*, using
    /// Anthropic's documented, model-independent multipliers (fresh 1.0×, cache
    /// read 0.10×, cache write 1.25×). A provider-stable relative cost that does
    /// NOT hardcode volatile dollar prices — pair it with [`extract_final_cost`]
    /// for the authoritative USD.
    pub fn input_equiv_tokens(&self) -> f64 {
        self.totals.input as f64
            + self.totals.cache_read as f64 * 0.10
            + self.totals.cache_creation as f64 * 1.25
    }
    /// Fresh-input-token equivalents SAVED by cache hits this run (what the
    /// cached reads would have cost at the full input price minus what they did
    /// cost). Zero when nothing was cached.
    pub fn cache_savings_tokens(&self) -> f64 {
        self.totals.cache_read as f64 * 0.90
    }
}

/// Build the reserved `autonomous_session` journal record (a system type an
/// agent can never forge via memory.append). `phase` is `start` | `end`.
pub fn session_journal_event(
    session_id: &str,
    phase: &str,
    provider: &str,
    ts_iso: &str,
    extra: Value,
) -> Value {
    json!({
        "ts": ts_iso,
        "type": "autonomous_session",
        "source": "vibectl-supervisor",
        "data": {
            "session_id": session_id,
            "phase": phase,
            "provider": provider,
            "detail": extra,
        }
    })
}

/// Deterministic session id from a timestamp and pid (no RNG). Charset is a
/// subset of `safe_session_id`, so it always round-trips through the store.
pub fn new_session_id(ts_unix: u64, pid: u32) -> String {
    format!("auto-{ts_unix}-{pid}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_forms() {
        assert_eq!(parse_duration("90"), Some(90));
        assert_eq!(parse_duration("45s"), Some(45));
        assert_eq!(parse_duration("30m"), Some(1800));
        assert_eq!(parse_duration("8h"), Some(28800));
        assert_eq!(parse_duration("8h30m"), Some(28800 + 1800));
        assert_eq!(parse_duration("1h1m1s"), Some(3661));
        // Invalid / zero.
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("0"), None);
        assert_eq!(parse_duration("abc"), None);
        assert_eq!(parse_duration("8x"), None);
        assert_eq!(parse_duration("h"), None);
        assert_eq!(
            parse_duration("8h30"),
            None,
            "trailing unitless digits rejected"
        );
        // Duplicate or out-of-order units are a typo, not a sum: reject them so an
        // unattended run never gets a longer-than-intended budget by accident.
        assert_eq!(parse_duration("8h8h"), None, "duplicate unit rejected");
        assert_eq!(parse_duration("30m8h"), None, "out-of-order units rejected");
        assert_eq!(parse_duration("1s1h"), None, "ascending units rejected");
        assert_eq!(
            parse_duration("8h30m45s"),
            Some(28800 + 1800 + 45),
            "descending units still parse"
        );
        // A signed bare integer is not a valid budget (u64 parse would accept +8).
        assert_eq!(parse_duration("+8"), None, "leading + rejected");
        assert_eq!(parse_duration("-8"), None, "leading - rejected");
    }

    #[test]
    fn budget_wall_tool_and_token_limits() {
        let b = Budget::new(Some(3600), Some(10), Some(1000));
        // Wall budget is checked against MONOTONIC elapsed time (a Duration).
        assert!(!b.wall_expired(Duration::from_secs(3599)));
        assert!(b.wall_expired(Duration::from_secs(3600)));
        assert!(!b.tool_calls_exhausted(9));
        assert!(b.tool_calls_exhausted(10));
        assert!(!b.tokens_exhausted(999));
        assert!(b.tokens_exhausted(1000));
        // Unbounded: never expires however much elapses / is consumed.
        let u = Budget::new(None, None, None);
        assert!(!u.wall_expired(Duration::from_secs(u64::MAX / 1_000)));
        assert!(!u.tool_calls_exhausted(1_000_000));
        assert!(!u.tokens_exhausted(u64::MAX));
    }

    #[test]
    fn consumption_mode_parse_and_budgets() {
        // Aliases (FR/EN + ASCII fold) all resolve; junk is None.
        assert_eq!(
            ConsumptionMode::parse("Frugale"),
            Some(ConsumptionMode::Frugale)
        );
        assert_eq!(
            ConsumptionMode::parse("eco"),
            Some(ConsumptionMode::Frugale)
        );
        assert_eq!(
            ConsumptionMode::parse("équilibrée"),
            Some(ConsumptionMode::Equilibree)
        );
        assert_eq!(
            ConsumptionMode::parse("balanced"),
            Some(ConsumptionMode::Equilibree)
        );
        assert_eq!(
            ConsumptionMode::parse("PERF"),
            Some(ConsumptionMode::Performance)
        );
        assert_eq!(ConsumptionMode::parse("turbo"), None);
        assert_eq!(ConsumptionMode::Equilibree.name(), "équilibrée");
        // Presets are ordered: frugale is the tightest leash on every axis.
        let (f, e, p) = (
            ConsumptionMode::Frugale.budget(),
            ConsumptionMode::Equilibree.budget(),
            ConsumptionMode::Performance.budget(),
        );
        assert!(f.wall.unwrap() < e.wall.unwrap() && e.wall.unwrap() < p.wall.unwrap());
        assert!(f.max_tool_calls < e.max_tool_calls && e.max_tool_calls < p.max_tool_calls);
        assert!(f.max_tokens < e.max_tokens && e.max_tokens < p.max_tokens);
    }

    #[test]
    fn extract_usage_from_assistant_turn() {
        let ev = json!({"type":"assistant","message":{"usage":{
            "input_tokens": 12,
            "output_tokens": 34,
            "cache_creation_input_tokens": 100,
            "cache_read_input_tokens": 5000
        }}});
        let u = extract_usage(&ev).unwrap();
        assert_eq!(u.input, 12);
        assert_eq!(u.output, 34);
        assert_eq!(u.cache_creation, 100);
        assert_eq!(u.cache_read, 5000);
        // Missing fields default to 0 (a cache-only turn is legitimately 0/0/0/N).
        let sparse = json!({"type":"assistant","message":{"usage":{"cache_read_input_tokens":9}}});
        let u2 = extract_usage(&sparse).unwrap();
        assert_eq!(
            u2,
            TokenUsage {
                cache_read: 9,
                ..Default::default()
            }
        );
    }

    #[test]
    fn extract_usage_ignores_non_usage_events() {
        // No usage object, wrong type, and a `result` event (its usage is not
        // summed — that path is cost-only) all yield None.
        assert!(extract_usage(&json!({"type":"assistant","message":{"content":[]}})).is_none());
        assert!(extract_usage(&json!({"type":"content_block_delta","delta":{}})).is_none());
        assert!(extract_usage(
            &json!({"type":"result","usage":{"input_tokens":9},"total_cost_usd":1.0})
        )
        .is_none());
    }

    #[test]
    fn extract_final_cost_from_result() {
        let ev = json!({"type":"result","subtype":"success","total_cost_usd":0.4237});
        assert_eq!(extract_final_cost(&ev), Some(0.4237));
        // Absent / negative / non-finite / wrong-type -> None (never a bogus cost).
        assert!(extract_final_cost(&json!({"type":"result"})).is_none());
        assert!(extract_final_cost(&json!({"type":"result","total_cost_usd":-1.0})).is_none());
        assert!(extract_final_cost(&json!({"type":"assistant","total_cost_usd":1.0})).is_none());
    }

    #[test]
    fn ledger_accumulates_and_derives_metrics() {
        let mut led = TokenLedger::default();
        // Two turns: a cache-priming turn then a cache-hit turn.
        let t1 = json!({"type":"assistant","message":{"usage":{
            "input_tokens": 1000, "output_tokens": 200, "cache_creation_input_tokens": 8000
        }}});
        let t2 = json!({"type":"assistant","message":{"usage":{
            "input_tokens": 50, "output_tokens": 150, "cache_read_input_tokens": 8000
        }}});
        assert!(led.add_event(&t1));
        assert!(led.add_event(&t2));
        // A non-usage event is folded as a no-op.
        assert!(!led.add_event(&json!({"type":"result","total_cost_usd":1.0})));
        assert_eq!(led.turns, 2);
        assert_eq!(led.totals.input, 1050);
        assert_eq!(led.totals.output, 350);
        assert_eq!(led.totals.cache_creation, 8000);
        assert_eq!(led.totals.cache_read, 8000);
        assert_eq!(led.total_tokens(), 1050 + 350 + 8000 + 8000);
        // Cache hit ratio = cache_read / (input + cache_read) = 8000 / 9050.
        let ratio = led.cache_hit_ratio().unwrap();
        assert!((ratio - 8000.0 / 9050.0).abs() < 1e-6, "ratio {ratio}");
        // Input-equiv = 1050*1 + 8000*0.10 (read) + 8000*1.25 (write) = 11850.
        assert!((led.input_equiv_tokens() - 11850.0).abs() < 1e-6);
        // Savings = 8000 * 0.90 = 7200 fresh-input-token equivalents.
        assert!((led.cache_savings_tokens() - 7200.0).abs() < 1e-6);
        // No input yet -> ratio is None, never a divide-by-zero.
        assert!(TokenLedger::default().cache_hit_ratio().is_none());
    }

    #[test]
    fn extract_thinking_delta() {
        let ev = json!({"type":"content_block_delta",
                        "delta":{"type":"thinking_delta","thinking":"Let me check"}});
        let out = extract_thinking(&ev).unwrap();
        assert_eq!(out["kind"], "thinking_delta");
        assert_eq!(out["text"], "Let me check");
    }

    #[test]
    fn extract_thinking_full_message_joins_blocks() {
        let ev = json!({"type":"assistant","message":{"content":[
            {"type":"thinking","thinking":"step 1 "},
            {"type":"text","text":"visible answer"},
            {"type":"thinking","thinking":"step 2"}
        ]}});
        let out = extract_thinking(&ev).unwrap();
        assert_eq!(out["kind"], "thinking");
        assert_eq!(out["text"], "step 1 step 2");
    }

    #[test]
    fn extract_redacted_thinking() {
        let ev = json!({"type":"assistant","message":{"content":[
            {"type":"redacted_thinking","data":"encrypted"}
        ]}});
        let out = extract_thinking(&ev).unwrap();
        assert_eq!(out["kind"], "redacted_thinking");
        assert_eq!(out["text"], "");
    }

    #[test]
    fn extract_thinking_ignores_non_reasoning() {
        assert!(extract_thinking(&json!({"type":"result","subtype":"success"})).is_none());
        assert!(
            extract_thinking(&json!({"type":"assistant","message":{"content":[
                {"type":"text","text":"just an answer"}
            ]}}))
            .is_none()
        );
        assert!(extract_thinking(&json!({"foo":"bar"})).is_none());
    }

    #[test]
    fn is_tool_use_detection() {
        assert!(is_tool_use(
            &json!({"type":"assistant","message":{"content":[
                {"type":"tool_use","name":"fs.read","input":{}}
            ]}})
        ));
        assert!(!is_tool_use(
            &json!({"type":"assistant","message":{"content":[
                {"type":"text","text":"hi"}
            ]}})
        ));
        assert!(!is_tool_use(
            &json!({"type":"content_block_delta","delta":{}})
        ));
    }

    #[test]
    fn count_tool_use_counts_parallel_calls() {
        // Several tool_use blocks in ONE assistant turn all count (budget honest).
        let ev = json!({"type":"assistant","message":{"content":[
            {"type":"tool_use","name":"fs.read","input":{}},
            {"type":"text","text":"and"},
            {"type":"tool_use","name":"fs.list","input":{}},
            {"type":"tool_use","name":"os.status","input":{}}
        ]}});
        assert_eq!(count_tool_use(&ev), 3);
        assert_eq!(count_tool_use(&json!({"type":"result"})), 0);
    }

    #[test]
    fn session_event_is_reserved_type() {
        let e = session_journal_event(
            "auto-1-2",
            "start",
            "claude-code",
            "2026-07-13T10:00:00Z",
            json!({"budget_secs": 28800}),
        );
        assert_eq!(e["type"], "autonomous_session");
        assert_eq!(e["source"], "vibectl-supervisor");
        assert_eq!(e["data"]["phase"], "start");
        assert_eq!(e["data"]["session_id"], "auto-1-2");
        assert_eq!(e["data"]["detail"]["budget_secs"], 28800);
    }

    #[test]
    fn session_id_roundtrips_through_store_validation() {
        let id = new_session_id(1_783_468_800, 4242);
        assert_eq!(id, "auto-1783468800-4242");
        // Must satisfy the reasoning store's filename validator.
        assert!(crate::reasoning::safe_session_id(&id).is_some());
    }

    #[test]
    fn ledger_accounting_invariants_over_a_sequence() {
        // INVARIANTS de comptabilité, sur une séquence arbitraire d'usages :
        //  (1) total_tokens == somme exacte des quatre compteurs ;
        //  (2) l'accumulation est MONOTONE (ajouter un tour ne baisse jamais le
        //      total) — le budget de tokens ne peut donc pas être « défait » ;
        //  (3) cache_hit_ratio ∈ [0, 1] dès qu'il est défini.
        let mut led = TokenLedger::default();
        let mut prev_total = 0u64;
        let seq = [
            (10u64, 5u64, 0u64, 0u64),
            (0, 3, 100, 0),
            (2, 1, 0, 5000),
            (0, 0, 0, 0), // un tour à zéro (cache-only dégénéré) reste valide
            (7, 9, 20, 300),
        ];
        for (i, o, cc, cr) in seq {
            let ev = json!({"type":"assistant","message":{"usage":{
                "input_tokens": i, "output_tokens": o,
                "cache_creation_input_tokens": cc, "cache_read_input_tokens": cr}}});
            led.add_event(&ev);
            let total = led.total_tokens();
            assert_eq!(
                total,
                led.totals.input
                    + led.totals.output
                    + led.totals.cache_creation
                    + led.totals.cache_read,
                "total = somme des parts"
            );
            assert!(total >= prev_total, "accumulation monotone");
            if let Some(ratio) = led.cache_hit_ratio() {
                assert!(
                    (0.0..=1.0).contains(&ratio),
                    "ratio de cache hors [0,1] : {ratio}"
                );
            }
            // input_equiv et savings ne sont jamais négatifs ni non finis.
            assert!(led.input_equiv_tokens().is_finite() && led.input_equiv_tokens() >= 0.0);
            assert!(led.cache_savings_tokens() >= 0.0);
            prev_total = total;
        }
        assert_eq!(led.turns, seq.len() as u64);
    }
}
