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

use serde_json::{json, Value};

/// Parse a human budget like `8h`, `30m`, `45s`, `8h30m`, `90` (bare = seconds)
/// into whole seconds. Returns `None` on empty / malformed / zero input.
pub fn parse_duration(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Bare integer -> seconds.
    if let Ok(n) = s.parse::<u64>() {
        return (n > 0).then_some(n);
    }
    let mut total: u64 = 0;
    let mut num: u64 = 0;
    let mut saw_digit = false;
    let mut saw_unit = false;
    for c in s.chars() {
        if let Some(d) = c.to_digit(10) {
            num = num.checked_mul(10)?.checked_add(d as u64)?;
            saw_digit = true;
        } else {
            let mult = match c {
                'h' | 'H' => 3600,
                'm' | 'M' => 60,
                's' | 'S' => 1,
                _ => return None,
            };
            if !saw_digit {
                return None; // a unit with no preceding number
            }
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

/// A running session's budget. `deadline_unix` is `start + wall_secs`.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    pub deadline_unix: u64,
    pub max_tool_calls: Option<u64>,
}

impl Budget {
    /// Build from a start time and optional wall-clock seconds / tool-call cap.
    /// With no wall budget the deadline is `u64::MAX` (effectively unbounded).
    pub fn new(start_unix: u64, wall_secs: Option<u64>, max_tool_calls: Option<u64>) -> Self {
        Self {
            deadline_unix: wall_secs.map_or(u64::MAX, |w| start_unix.saturating_add(w)),
            max_tool_calls,
        }
    }
    /// Wall-clock budget exhausted at `now`?
    pub fn wall_expired(&self, now: u64) -> bool {
        now >= self.deadline_unix
    }
    /// Tool-call budget exhausted at `count`?
    pub fn tool_calls_exhausted(&self, count: u64) -> bool {
        self.max_tool_calls.is_some_and(|m| count >= m)
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
    }

    #[test]
    fn budget_wall_and_tool_limits() {
        let b = Budget::new(1_000, Some(3600), Some(10));
        assert_eq!(b.deadline_unix, 4_600);
        assert!(!b.wall_expired(4_599));
        assert!(b.wall_expired(4_600));
        assert!(!b.tool_calls_exhausted(9));
        assert!(b.tool_calls_exhausted(10));
        // Unbounded wall.
        let u = Budget::new(1_000, None, None);
        assert_eq!(u.deadline_unix, u64::MAX);
        assert!(!u.wall_expired(u64::MAX - 1));
        assert!(!u.tool_calls_exhausted(1_000_000));
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
}
