//! `user.model` (T0) — the machine's DERIVED, TRANSPARENT understanding of the
//! human it serves, and a deterministic anticipation of their next moves
//! (ADR-028, "symbiose IA-citoyenne").
//!
//! WHAT IT IS. A read-only model of "how you work", folded from data VibeOS
//! ALREADY holds under governance — never from new surveillance:
//!   * your explicit preferences (the `user/` memory fold);
//!   * your patterns and rhythm, derived from the audit trail CONFINED to your
//!     own uid (the same per-uid view `agents.list`/`agent.activity` expose);
//!   * the friction you hit (actions that needed approval or were denied);
//!   * what the agent has observed about you (recent typed journal notes);
//!   * anticipations: your most likely next governed actions, ranked by a
//!     TRANSPARENT frequency×recency heuristic, each with its reason.
//!
//! WHAT IT IS NOT. Not a trained predictor, not mind-reading, not a hidden
//! profile. Every field is derived from records you can inspect, the store is
//! local and user-owned (`/var/lib/vibeos/memory/`, "belongs to its user and no
//! one else"), and an agent could obtain the same underlying data via
//! `memory.query` + `agent.activity` for its own uid — so there is NO NEW LEAK
//! (same reasoning as ADR-023/026). It also does NOT watch the human's raw
//! keystrokes or shell history: symbiosis is built on transparent, governed
//! signal, not on spying. Richer, opt-in signal capture is future work (ADR-028).
//!
//! PURE CORE. `derive_user_model` is a pure function of (audit records already
//! filtered to the caller's uid, the user-memory profile, recent journal notes,
//! now, window). The gather step (`user_model_for`) reads the stores and calls
//! it, so the derivation is deterministic and unit-testable in isolation — the
//! same discipline as `deploy::plan_command` and `propose::parse_proposal`.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};

use crate::audit::Caller;

/// How many entries each ranked list returns (patterns, anticipations, …).
const TOP_N: usize = 8;
/// Cap on the recent journal notes surfaced under `observed`.
const OBSERVED_CAP: usize = 12;
/// Longest window `user.model` accepts (its `window_seconds` clamp ceiling):
/// 30 days of governed history is plenty for a "how you work" model.
const MAX_WINDOW_SECS: u64 = 30 * 24 * 3600;
/// Default window when the caller does not specify one.
const DEFAULT_WINDOW_SECS: u64 = 7 * 24 * 3600;
/// Cap on journal lines scanned from the most-recent file (bounds a repeatable
/// T0 call — the model never needs more than the newest notes).
const JOURNAL_SCAN_LINES: usize = 500;

/// `user.model` (T0): the caller's OWN transparent understanding model, CONFINED
/// to their uid (SO_PEERCRED) exactly like `agents.list`/`agent.activity`. An
/// unidentified caller gets nothing (fail-closed). Gathers the governed inputs
/// (per-uid audit view, user-memory profile, recent journal notes) and folds
/// them with the pure [`derive_user_model`]. Read-only; no new leak.
pub(crate) fn user_model(args: &Value, caller: Caller, audit_dir: &Path) -> Result<String, String> {
    let window = args
        .get("window_seconds")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_WINDOW_SECS)
        .clamp(1, MAX_WINDOW_SECS);
    let now = now_epoch_secs();
    let cutoff = now.saturating_sub(window);

    let Some(uid) = caller.uid else {
        return Ok(json!({
            "note": "no caller uid (SO_PEERCRED); the user model is confined per-uid",
            "preferences": {}, "patterns": [], "anticipations": [], "friction": [],
            "rhythm": { "by_hour": vec![0u64; 24], "most_active_hours": [] },
            "observed": [], "executed_total": 0, "window_seconds": window,
        })
        .to_string());
    };

    // Audit view CONFINED to the caller's uid (the same discipline as agents.list).
    let records: Vec<Value> = crate::mcp::read_recent_audit(audit_dir, cutoff)
        .into_iter()
        .filter(|r| r.get("caller_uid").and_then(Value::as_u64) == Some(u64::from(uid)))
        .collect();

    let memory_dir = Path::new(crate::mcp::MEMORY_DIR);
    let profile = crate::vibectl::memory_profile_at(memory_dir);
    let journal = read_recent_journal(memory_dir);

    let mut model = derive_user_model(&records, &profile, &journal, now, window);
    model["uid"] = json!(uid);
    Ok(model.to_string())
}

/// Current unix time in whole seconds (0 on a clock error) — the model is a
/// heuristic, so a stuck clock degrades it gracefully rather than failing.
fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Read recent typed journal notes the agent recorded about the human. Bounded:
/// scans the tail of the MOST RECENT `journal/*.jsonl` file only, keeps the
/// agent-appendable event types (`observation`/`decision`/`preference`/…), and
/// returns them newest-last (as `derive_user_model` expects). Best-effort: an
/// absent/unreadable store yields an empty Vec, never an error.
fn read_recent_journal(memory_dir: &Path) -> Vec<Value> {
    let dir = memory_dir.join("journal");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    // The journal is one file per UTC day named `YYYY-MM-DD.jsonl`, so the
    // lexicographically-greatest name is the most recent.
    let mut files: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("jsonl"))
        .collect();
    files.sort();
    let Some(newest) = files.into_iter().next_back() else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(&newest) else {
        return Vec::new();
    };
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(JOURNAL_SCAN_LINES);
    lines[start..]
        .iter()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| {
            v.get("type")
                .and_then(Value::as_str)
                .is_some_and(|t| crate::mcp::JOURNAL_AGENT_TYPES.contains(&t))
        })
        .collect()
}

/// Extract `(tool, target, ts_secs, decision)` from one audit record, skipping
/// records missing a tool name.
fn fields(rec: &Value) -> Option<(String, Option<String>, u64, String)> {
    let tool = rec.get("tool").and_then(Value::as_str)?.to_string();
    if tool.is_empty() {
        return None;
    }
    let target = rec
        .get("target")
        .and_then(Value::as_str)
        .map(str::to_string);
    let ts = rec
        .get("ts_unix_ms")
        .and_then(Value::as_u64)
        .map(|ms| ms / 1000)
        .unwrap_or(0);
    let decision = rec
        .get("decision")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Some((tool, target, ts, decision))
}

/// Running aggregate for one grouping key (a tool, or a (tool,target) action).
#[derive(Default, Clone)]
struct Agg {
    count: u64,
    last_ts: u64,
    /// Most frequent target seen for this tool (patterns only).
    targets: BTreeMap<String, u64>,
}

impl Agg {
    fn observe(&mut self, ts: u64, target: Option<&str>) {
        self.count += 1;
        self.last_ts = self.last_ts.max(ts);
        if let Some(t) = target {
            *self.targets.entry(t.to_string()).or_insert(0) += 1;
        }
    }
    fn top_target(&self) -> Option<String> {
        self.targets
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
            .map(|(t, _)| t.clone())
    }
}

/// Rank an aggregate map into a Vec, most frequent first, ties broken by
/// recency (newer first), then by key for determinism. Truncated to `TOP_N`.
fn ranked(map: &BTreeMap<String, Agg>) -> Vec<(String, Agg)> {
    let mut v: Vec<(String, Agg)> = map.iter().map(|(k, a)| (k.clone(), a.clone())).collect();
    v.sort_by(|a, b| {
        b.1.count
            .cmp(&a.1.count)
            .then_with(|| b.1.last_ts.cmp(&a.1.last_ts))
            .then_with(|| a.0.cmp(&b.0))
    });
    v.truncate(TOP_N);
    v
}

/// The pure derivation. `records` are audit records ALREADY confined to the
/// caller's uid and time window; `profile` is `{ "profile": {k: v} }` from the
/// user-memory fold; `journal` are recent typed journal notes (newest last).
pub(crate) fn derive_user_model(
    records: &[Value],
    profile: &Value,
    journal: &[Value],
    now: u64,
    window: u64,
) -> Value {
    // Split executed actions (what actually ran for the human) from friction
    // (needed approval / was denied) — both are signal, with opposite meaning.
    let mut by_tool: BTreeMap<String, Agg> = BTreeMap::new(); // executed, per tool
    let mut by_action: BTreeMap<String, Agg> = BTreeMap::new(); // executed, per (tool,target)
    let mut friction: BTreeMap<String, Agg> = BTreeMap::new(); // deny/require_approval, per tool
    let mut by_hour = [0u64; 24];
    let mut executed_total = 0u64;

    for rec in records {
        let Some((tool, target, ts, decision)) = fields(rec) else {
            continue;
        };
        match decision.as_str() {
            "allow" => {
                by_tool
                    .entry(tool.clone())
                    .or_default()
                    .observe(ts, target.as_deref());
                let action_key = match &target {
                    Some(t) => format!("{tool} {t}"),
                    None => tool.clone(),
                };
                by_action
                    .entry(action_key)
                    .or_default()
                    .observe(ts, target.as_deref());
                // Rhythm is about WHEN the human is active — executed actions only.
                by_hour[((ts % 86_400) / 3_600) as usize] += 1;
                executed_total += 1;
            }
            "deny" | "require_approval" => {
                friction
                    .entry(tool)
                    .or_default()
                    .observe(ts, target.as_deref());
            }
            _ => {}
        }
    }

    let patterns: Vec<Value> = ranked(&by_tool)
        .into_iter()
        .map(|(tool, a)| {
            json!({
                "tool": tool,
                "count": a.count,
                "last_unix": a.last_ts,
                "top_target": a.top_target(),
            })
        })
        .collect();

    // Anticipations: the most likely next governed actions. Frequency is the
    // primary signal, recency the tie-break; each carries its reason so the
    // human sees WHY it was surfaced (never an opaque score).
    let anticipations: Vec<Value> = ranked(&by_action)
        .into_iter()
        .map(|(action, a)| {
            let ago = now.saturating_sub(a.last_ts);
            json!({
                "action": action,
                "count": a.count,
                "last_seen_unix": a.last_ts,
                "reason": format!("done {} time(s), last {}s ago", a.count, ago),
            })
        })
        .collect();

    let friction_list: Vec<Value> = ranked(&friction)
        .into_iter()
        .map(|(tool, a)| {
            json!({
                "tool": tool,
                "count": a.count,
                "last_unix": a.last_ts,
                "note": "often needs human approval or is denied — a candidate to pre-arrange",
            })
        })
        .collect();

    let most_active_hours = {
        let mut hours: Vec<(usize, u64)> = by_hour.iter().copied().enumerate().collect();
        hours.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        hours
            .into_iter()
            .filter(|(_, c)| *c > 0)
            .take(3)
            .map(|(h, _)| h as u64)
            .collect::<Vec<_>>()
    };

    // Recent journal notes the agent recorded ABOUT the human (typed events).
    // Newest last in input; surface the newest, capped, with no raw payload
    // dump (keys only) to keep the model small and the content non-secret.
    let observed: Vec<Value> = journal
        .iter()
        .rev()
        .take(OBSERVED_CAP)
        .map(|e| {
            json!({
                "ts": e.get("ts").cloned().unwrap_or(Value::Null),
                "type": e.get("type").cloned().unwrap_or(Value::Null),
                "source": e.get("source").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();

    let preferences = profile.get("profile").cloned().unwrap_or_else(|| json!({}));

    json!({
        "preferences": preferences,
        "patterns": patterns,
        "anticipations": anticipations,
        "friction": friction_list,
        "rhythm": { "by_hour": by_hour, "most_active_hours": most_active_hours },
        "observed": observed,
        "executed_total": executed_total,
        "window_seconds": window,
        "note": "Dérivé de données DÉJÀ gouvernées (mémoire user + trace d'audit confinée \
                 à votre uid) — pas de nouvelle surveillance. Anticipations = heuristique \
                 déterministe fréquence×récence, PAS un prédicteur entraîné. Local, \
                 transparent, effaçable (la mémoire vous appartient).",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(tool: &str, target: Option<&str>, ts: u64, decision: &str) -> Value {
        let mut r = json!({ "tool": tool, "ts_unix_ms": ts * 1000, "decision": decision });
        if let Some(t) = target {
            r["target"] = json!(t);
        }
        r
    }

    #[test]
    fn frequency_and_recency_shape_patterns_and_anticipations() {
        let now = 100_000;
        let records = vec![
            rec("svc.status", Some("nginx"), now - 10, "allow"),
            rec("svc.status", Some("nginx"), now - 20, "allow"),
            rec("svc.status", Some("sshd"), now - 30, "allow"),
            rec("fs.read", Some("/etc/hosts"), now - 5, "allow"),
        ];
        let model = derive_user_model(&records, &json!({}), &[], now, 3600);

        // svc.status (3×) ranks above fs.read (1×) in patterns.
        let patterns = model["patterns"].as_array().unwrap();
        assert_eq!(patterns[0]["tool"], "svc.status");
        assert_eq!(patterns[0]["count"], 3);
        assert_eq!(
            patterns[0]["top_target"], "nginx",
            "the most frequent target is surfaced"
        );

        // The single most likely next action is the most frequent (tool,target).
        let ant = model["anticipations"].as_array().unwrap();
        assert_eq!(ant[0]["action"], "svc.status nginx");
        assert_eq!(ant[0]["count"], 2);
        assert!(
            ant[0]["reason"]
                .as_str()
                .unwrap()
                .contains("done 2 time(s)"),
            "the reason is explicit, never an opaque score: {}",
            ant[0]["reason"]
        );
    }

    #[test]
    fn friction_captures_refusals_not_executions() {
        let now = 200_000;
        let records = vec![
            rec(
                "svc.restart",
                Some("nginx.service"),
                now - 5,
                "require_approval",
            ),
            rec(
                "svc.restart",
                Some("nginx.service"),
                now - 6,
                "require_approval",
            ),
            rec("pkg.install", Some("htop"), now - 7, "deny"),
            rec("os.status", None, now - 8, "allow"),
        ];
        let model = derive_user_model(&records, &json!({}), &[], now, 3600);

        let friction = model["friction"].as_array().unwrap();
        assert_eq!(
            friction[0]["tool"], "svc.restart",
            "most-refused tool first"
        );
        assert_eq!(friction[0]["count"], 2);
        // Executed patterns must NOT include the refused tools.
        let pattern_tools: Vec<&str> = model["patterns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["tool"].as_str().unwrap())
            .collect();
        assert!(pattern_tools.contains(&"os.status"));
        assert!(
            !pattern_tools.contains(&"svc.restart"),
            "a refused action never counts as an executed pattern"
        );
    }

    #[test]
    fn rhythm_buckets_executed_actions_by_hour() {
        // Two actions at 02:00 UTC, one at 14:00 UTC (epoch-day math).
        let day = 1_800_000u64 * 86_400 / 86_400; // arbitrary day start multiple
        let h2 = day * 86_400 + 2 * 3_600 + 100;
        let h14 = day * 86_400 + 14 * 3_600 + 100;
        let records = vec![
            rec("os.status", None, h2, "allow"),
            rec("os.status", None, h2 + 60, "allow"),
            rec("fs.read", Some("/x"), h14, "allow"),
        ];
        let model = derive_user_model(&records, &json!({}), &[], h14 + 10, 86_400);
        let by_hour = model["rhythm"]["by_hour"].as_array().unwrap();
        assert_eq!(by_hour[2], 2, "two actions in the 02:00 bucket");
        assert_eq!(by_hour[14], 1, "one action in the 14:00 bucket");
        assert_eq!(
            model["rhythm"]["most_active_hours"][0], 2,
            "02:00 is the most active hour"
        );
    }

    #[test]
    fn preferences_pass_through_and_observed_is_capped_newest_first() {
        let profile = json!({ "profile": { "editor": "neovim", "lang": "fr" } });
        let journal: Vec<Value> = (0..20)
            .map(
                |i| json!({ "ts": format!("t{i}"), "type": "preference", "source": "claude-code" }),
            )
            .collect();
        let model = derive_user_model(&[], &profile, &journal, 1000, 3600);

        assert_eq!(model["preferences"]["editor"], "neovim");
        let observed = model["observed"].as_array().unwrap();
        assert_eq!(observed.len(), OBSERVED_CAP, "observed is capped");
        assert_eq!(
            observed[0]["ts"], "t19",
            "newest journal note first (input newest-last, reversed)"
        );
    }

    #[test]
    fn empty_inputs_yield_a_well_formed_empty_model() {
        let model = derive_user_model(&[], &json!({}), &[], 0, 3600);
        assert_eq!(model["patterns"].as_array().unwrap().len(), 0);
        assert_eq!(model["anticipations"].as_array().unwrap().len(), 0);
        assert_eq!(model["executed_total"], 0);
        assert!(model["preferences"].is_object());
        assert!(model["note"].as_str().unwrap().contains("transparent"));
    }
}
