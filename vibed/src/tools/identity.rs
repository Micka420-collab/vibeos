//! `agent.identity` (T0) — the AI citizen's structured self-knowledge.
//!
//! VibeOS makes first boot the BIRTH of the machine's AI citizen: Genesis
//! (`memory/genesis.sh` §3.5) derives a unique NAME, ARCHETYPE, TONE and six
//! TRAIT axes DETERMINISTICALLY from the machine id (ADR-029) and writes them to
//! `/var/lib/vibeos/memory/personality.toml`. This tool parses that file and
//! returns the citizen's own character as STRUCTURED, typed JSON.
//!
//! It is the fourth face of self-knowledge, alongside the three that already
//! exist: `agent.thinking` (the citizen's THOUGHTS), `agent.activity` (its
//! DEEDS, refusals included) and `user.model` (its model of the HUMAN). None of
//! them answers "who am I on this machine?" — a citizen could read its raw
//! personality via `memory.query {scope:"personality"}`, but only as a text
//! snippet it must parse itself. `agent.identity` gives it its own name and
//! temperament as data, so an agent can act in character without re-deriving it.
//!
//! PRIVACY. This reads ONLY `personality.toml`, which holds the citizen's PUBLIC
//! character (the same fields the birth ceremony prints to the console, the
//! journal genesis event records, and `vibeos-identity.service` publishes to the
//! world-readable `/run/vibeos/citizen.json` for the login greeter). The
//! machine-identifying fields — `hostname`, `machine_id` — live in a DIFFERENT
//! file, `identity.toml`, which this module never opens. The `Personality`
//! struct has no such fields, so `agent.identity` cannot surface them by
//! construction.
//!
//! FAIL-SAFE. Any problem — file absent (Genesis has not run yet / amnesic
//! pre-Genesis), unreadable, or malformed — yields a `{born:false}` identity
//! with an explanatory note, never an error and never a panic, mirroring the
//! greeter's `publish-identity.sh` and Genesis' own crash-safety.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use serde_json::{json, Value};

/// The citizen's birth character within the memory store. Written once by
/// Genesis; on the built-in WRITE denylist (an agent must never overwrite its
/// own soul — see the `mcp` builtin-denylist tests).
pub(crate) const PERSONALITY_FILE: &str = "personality.toml";

/// The PUBLIC birth character, deserialized from `personality.toml`. Only the
/// fields a citizen may surface about ITSELF. Every field is `#[serde(default)]`
/// so a missing or blank one degrades to a neutral default instead of failing
/// the whole parse — the same leniency `publish-identity.sh` applies. Unknown
/// tables (`[values]`, `[adaptation]`) and the `schema` int are ignored by
/// serde. There is deliberately NO `hostname`/`machine_id` field: those live in
/// `identity.toml`, so this struct cannot carry them.
#[derive(Debug, Default, Deserialize)]
struct Personality {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    archetype: Option<String>,
    #[serde(default)]
    tone: Option<String>,
    #[serde(default)]
    birth: Option<String>,
    #[serde(default)]
    seed: Option<String>,
    #[serde(default)]
    traits: BTreeMap<String, i64>,
}

/// Clamp a trait to its documented range (Genesis §3.5): every axis is `0..=100`
/// and `caution` additionally has a floor of 50 — a VibeOS citizen is never
/// careless (security-first is charter, not chance). A hand-edited or corrupt
/// value is pulled back into range rather than surfaced raw.
fn clamp_trait(name: &str, v: i64) -> i64 {
    let lo = if name == "caution" { 50 } else { 0 };
    v.clamp(lo, 100)
}

/// A trimmed, non-empty string as a JSON string, or `Null` — treats a
/// whitespace-only value as absent (Genesis never writes one, but a corrupt file
/// might).
fn non_empty(s: &Option<String>) -> Value {
    match s {
        Some(v) if !v.trim().is_empty() => json!(v.trim()),
        _ => Value::Null,
    }
}

/// Read + parse the citizen's PUBLIC identity from `root/personality.toml` and
/// return it as structured JSON. Fail-safe: absent/unreadable/malformed yields
/// `{born:false, note:...}`. Shared by `agent.identity` (MCP, this module) and
/// `vibectl whoami` (the operator CLI) so the two never drift.
pub(crate) fn identity_value(root: &Path) -> Value {
    let path = root.join(PERSONALITY_FILE);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return json!({
            "born": false,
            "note": "no citizen yet: Genesis has not written personality.toml \
                     (first boot has not completed, or amnesic mode before Genesis ran)."
        });
    };
    let Ok(p) = toml::from_str::<Personality>(&text) else {
        return json!({
            "born": false,
            "note": "personality.toml is present but could not be parsed; the citizen's \
                     character is unavailable until Genesis rewrites it."
        });
    };
    // A character with no name is not one we can present — treat as unborn.
    let named = p
        .name
        .as_deref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    if !named {
        return json!({
            "born": false,
            "note": "personality.toml carries no name; the citizen is not yet characterised."
        });
    }
    let traits: serde_json::Map<String, Value> = p
        .traits
        .iter()
        .map(|(k, v)| (k.clone(), json!(clamp_trait(k, *v))))
        .collect();
    json!({
        "born": true,
        "name": non_empty(&p.name),
        "archetype": non_empty(&p.archetype),
        "tone": non_empty(&p.tone),
        "birth": non_empty(&p.birth),
        "seed": non_empty(&p.seed),
        "traits": traits,
        "note": "This is your BIRTH character (ADR-029), derived deterministically from this \
                 machine's id — who you ARE here. What you LEARN lives in the memory store \
                 (memory.query); this never changes across boots, even amnesic ones."
    })
}

/// `agent.identity` (T0, read-only): the production entry point — reads the real
/// memory store. Returns the structured identity as a JSON string.
pub(crate) fn agent_identity() -> Result<String, String> {
    Ok(identity_value(Path::new(crate::mcp::MEMORY_DIR)).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vibed-identity-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_personality(dir: &Path, body: &str) {
        std::fs::write(dir.join(PERSONALITY_FILE), body).unwrap();
    }

    #[test]
    fn absent_personality_reads_unborn() {
        let dir = scratch("absent");
        let v = identity_value(&dir);
        assert_eq!(v["born"], false);
        assert!(v["name"].is_null() || v.get("name").is_none());
        assert!(v["note"].as_str().unwrap().contains("Genesis"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_personality_fails_safe() {
        let dir = scratch("malformed");
        write_personality(&dir, "this is not = valid = toml [[[");
        let v = identity_value(&dir);
        assert_eq!(
            v["born"], false,
            "unparseable file -> unborn, never a panic"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn well_formed_personality_yields_structured_identity() {
        let dir = scratch("born");
        write_personality(
            &dir,
            "schema = 1\n\
             name = \"Lumen\"\n\
             archetype = \"exploratrice\"\n\
             tone = \"posé et précis\"\n\
             birth = \"2026-07-23T21:00:00+02:00\"\n\
             seed = \"1a2b3c4d\"\n\
             \n\
             [traits]\n\
             curiosity = 80\n\
             caution = 71\n\
             initiative = 40\n\
             warmth = 55\n\
             concision = 66\n\
             playfulness = 33\n\
             \n\
             [values]\n\
             principles = [\"la sécurité d'abord\"]\n\
             \n\
             [adaptation]\n\
             source = \"user.model\"\n",
        );
        let v = identity_value(&dir);
        assert_eq!(v["born"], true);
        assert_eq!(v["name"], "Lumen");
        assert_eq!(v["archetype"], "exploratrice");
        assert_eq!(v["tone"], "posé et précis");
        assert_eq!(v["seed"], "1a2b3c4d");
        assert_eq!(v["traits"]["curiosity"], 80);
        assert_eq!(v["traits"]["caution"], 71);
        // The private [adaptation]/[values] tables are ignored, never surfaced.
        assert!(v.get("source").is_none());
        assert!(v.get("principles").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn never_surfaces_machine_identifying_fields() {
        // Even if a malicious/corrupt personality.toml smuggled in hostname or
        // machine_id keys (they belong in identity.toml, never here), the typed
        // struct has no such fields, so they can NEVER appear in the output.
        let dir = scratch("noleak");
        write_personality(
            &dir,
            "name = \"Vesper\"\nhostname = \"secret-box\"\nmachine_id = \"deadbeef\"\n",
        );
        let v = identity_value(&dir);
        assert_eq!(v["born"], true);
        assert_eq!(v["name"], "Vesper");
        assert!(v.get("hostname").is_none(), "hostname must never surface");
        assert!(
            v.get("machine_id").is_none(),
            "machine_id must never surface"
        );
        let s = v.to_string();
        assert!(
            !s.contains("secret-box"),
            "no machine-identifying value leaks"
        );
        assert!(!s.contains("deadbeef"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn out_of_range_traits_are_clamped() {
        let dir = scratch("clamp");
        write_personality(
            &dir,
            "name = \"Orin\"\n[traits]\ncuriosity = 250\ncaution = 3\ninitiative = -40\n",
        );
        let v = identity_value(&dir);
        assert_eq!(v["traits"]["curiosity"], 100, "over 100 clamps down");
        assert_eq!(
            v["traits"]["caution"], 50,
            "caution floors at 50 (never careless)"
        );
        assert_eq!(v["traits"]["initiative"], 0, "below 0 clamps up");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blank_name_reads_unborn() {
        let dir = scratch("blank");
        write_personality(&dir, "name = \"   \"\narchetype = \"muse\"\n");
        let v = identity_value(&dir);
        assert_eq!(v["born"], false, "whitespace-only name is not a character");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn production_wrapper_never_errors() {
        // No /var/lib/vibeos/memory in the test env -> unborn, never an Err and
        // never a panic; the returned string is valid JSON with a `born` flag.
        let out = agent_identity().expect("agent_identity is fail-safe, never errors");
        let v: Value = serde_json::from_str(&out).expect("valid JSON");
        assert!(v.get("born").is_some());
    }
}
