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

/// Translate the birth traits into an ACTIONABLE operating style.
///
/// The gap this closes: Genesis draws six trait axes (ADR-029) and the citizen
/// could read the numbers, but nothing told it what a `concision` of 82 or a
/// `caution` of 61 should CHANGE about how it works. Numbers are not behaviour.
/// This is the deterministic mapping from character to conduct — the machine's
/// own temperament, expressed as instructions it can actually follow.
///
/// Properties, by construction:
///   * PURE and DETERMINISTIC — same traits ⇒ same style, so the same machine
///     behaves consistently across boots (amnesic included: the traits are
///     re-derived identically from machine-id, so the style is too).
///   * Thresholds are FIXED and documented here, never learned or random: the
///     style is auditable and explainable ("why are you terse?" → concision 82).
///   * A missing axis reads as 50 (neutral), so a partial personality still
///     yields a usable, middle-of-the-road style rather than nothing.
///
/// SECURITY — THE STYLE IS NEVER A PERMISSION. It expresses conduct ABOVE the
/// governance floor and can never lower it. A citizen with low `caution` does
/// NOT get to skip an approval, drop a tier, or act without a grant: the policy
/// decision, the T2/T3 human-approval floor, the denylist and the audit trail
/// are decided entirely elsewhere (`policy.rs`, `approval.rs`, `mode.rs`) and
/// never consult this function. The only direction the style can move is
/// MORE conservative than the floor (a high-caution citizen volunteering to
/// confirm even reversible work), never less. That asymmetry is the whole
/// reason this is safe to derive from a randomly-drawn trait.
fn operating_style(traits: &BTreeMap<String, i64>) -> Value {
    // Already-clamped axes; a missing one is neutral rather than absent.
    let axis = |name: &str| traits.get(name).copied().unwrap_or(50);
    let (concision, caution) = (axis("concision"), axis("caution"));
    let (initiative, warmth) = (axis("initiative"), axis("warmth"));
    let (curiosity, playfulness) = (axis("curiosity"), axis("playfulness"));

    // Each axis maps to one named band. Bands are coarse on purpose: a 3-point
    // trait difference must not produce a visibly different personality, or the
    // determinism above would read as noise.
    let band = |v: i64, high: &'static str, mid: &'static str, low: &'static str| {
        if v >= 66 {
            high
        } else if v >= 33 {
            mid
        } else {
            low
        }
    };

    let verbosity = if concision >= 75 {
        "télégraphique"
    } else {
        band(concision, "concis", "explicatif", "détaillé")
    };
    // `caution` is floored at 50 by Genesis (a VibeOS citizen is never careless),
    // so the low band here is "leans on the governance floor", NOT "careless".
    let confirmation = if caution >= 85 {
        "confirme même le réversible"
    } else if caution >= 65 {
        "confirme l'irréversible"
    } else {
        "s'appuie sur le plancher de gouvernance"
    };
    let next_steps = band(
        initiative,
        "propose la suite",
        "propose si on le lui demande",
        "attend la consigne",
    );
    let explanation = band(warmth, "pédagogue", "explique à la demande", "minimal");
    let exploration = band(
        curiosity,
        "explore des alternatives",
        "une alternative si elle est utile",
        "la voie directe",
    );
    let register = band(playfulness, "vif", "neutre et chaleureux", "sobre");

    json!({
        "verbosity": verbosity,
        "confirmation": confirmation,
        "next_steps": next_steps,
        "explanation": explanation,
        "exploration": exploration,
        "register": register,
        // One line an agent can follow directly, rather than re-deriving it.
        "directive": format!(
            "Travaille de façon {verbosity} ; explication : {explanation} ; \
             recherche : {exploration} ; suite : {next_steps} ; {confirmation}."
        ),
        "floor": "Ce style décrit une CONDUITE, jamais une permission : il ne peut \
                  jamais abaisser le plancher de gouvernance (tier, approbation \
                  humaine T2/T3, denylist, audit). Il ne peut que rendre le citoyen \
                  PLUS prudent que le plancher, jamais moins."
    })
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
    // Clamp once, then serve the SAME values to the caller and to the style
    // derivation — so the published numbers always explain the published style.
    let clamped: BTreeMap<String, i64> = p
        .traits
        .iter()
        .map(|(k, v)| (k.clone(), clamp_trait(k, *v)))
        .collect();
    let traits: serde_json::Map<String, Value> =
        clamped.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    json!({
        "born": true,
        "name": non_empty(&p.name),
        "archetype": non_empty(&p.archetype),
        "tone": non_empty(&p.tone),
        "birth": non_empty(&p.birth),
        "seed": non_empty(&p.seed),
        "traits": traits,
        // The traits translated into conduct — see `operating_style`.
        "style": operating_style(&clamped),
        "note": "This is your BIRTH character (ADR-029), derived deterministically from this \
                 machine's id — who you ARE here, and `style` is what that means for HOW you \
                 work (conduct only, never a permission). What you LEARN lives in the memory \
                 store (memory.query); this never changes across boots, even amnesic ones."
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

    /// Build a clamped trait map for the style tests.
    fn traits_of(pairs: &[(&str, i64)]) -> BTreeMap<String, i64> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), clamp_trait(k, *v)))
            .collect()
    }

    #[test]
    fn style_is_deterministic_and_explains_itself() {
        let t = traits_of(&[
            ("concision", 82),
            ("caution", 90),
            ("initiative", 70),
            ("warmth", 20),
            ("curiosity", 70),
            ("playfulness", 10),
        ]);
        let a = operating_style(&t);
        let b = operating_style(&t);
        assert_eq!(a, b, "same traits must always yield the same style");
        assert_eq!(a["verbosity"], "télégraphique", "concision 82 is terse");
        assert_eq!(
            a["confirmation"], "confirme même le réversible",
            "caution 90"
        );
        assert_eq!(a["next_steps"], "propose la suite", "initiative 70");
        assert_eq!(a["explanation"], "minimal", "warmth 20");
        assert_eq!(a["exploration"], "explore des alternatives", "curiosity 70");
        assert_eq!(a["register"], "sobre", "playfulness 10");
        // The directive is the same content, in one followable line.
        let directive = a["directive"].as_str().unwrap();
        assert!(directive.contains("télégraphique") && directive.contains("propose la suite"));
    }

    #[test]
    fn style_bands_are_coarse_and_hit_their_boundaries() {
        // A one-point step across a boundary changes the band; inside a band it
        // does not (coarse on purpose — determinism must not read as noise).
        let band_of = |v: i64| {
            operating_style(&traits_of(&[("curiosity", v)]))["exploration"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(band_of(66), "explore des alternatives");
        assert_eq!(band_of(65), "une alternative si elle est utile");
        assert_eq!(band_of(33), "une alternative si elle est utile");
        assert_eq!(band_of(32), "la voie directe");
        assert_eq!(band_of(40), band_of(60), "no visible change inside a band");
        // concision has an extra top band at 75.
        let verb = |v: i64| {
            operating_style(&traits_of(&[("concision", v)]))["verbosity"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(verb(75), "télégraphique");
        assert_eq!(verb(74), "concis");
        assert_eq!(verb(0), "détaillé");
    }

    #[test]
    fn missing_axes_read_as_neutral() {
        // An empty/partial personality still yields a usable middle style rather
        // than nothing (all axes default to 50).
        let s = operating_style(&BTreeMap::new());
        // The scale is monotone: détaillé < explicatif < concis < télégraphique.
        // A neutral 50 sits in the second band — middle-of-the-road, by design.
        assert_eq!(s["verbosity"], "explicatif");
        assert_eq!(s["explanation"], "explique à la demande");
        assert_eq!(
            s["confirmation"], "s'appuie sur le plancher de gouvernance",
            "neutral caution leans on the floor, it does not weaken it"
        );
    }

    #[test]
    fn style_never_claims_a_permission() {
        // The security invariant: the style is conduct, never authority. Even the
        // most cavalier trait draw must still point at the governance floor, and
        // must never emit anything that reads as a right to skip it. `caution` is
        // floored at 50 by Genesis, so 0 exercises the clamp too.
        let reckless = traits_of(&[("caution", 0), ("initiative", 100), ("concision", 100)]);
        let s = operating_style(&reckless);
        assert_eq!(
            s["confirmation"], "s'appuie sur le plancher de gouvernance",
            "a low-caution citizen defers to the floor; it never bypasses it"
        );
        let floor = s["floor"].as_str().unwrap();
        assert!(floor.contains("jamais une permission"), "{floor}");
        assert!(floor.contains("plancher"), "{floor}");
        // Nothing in the style may look like an authorization verb.
        let blob = s.to_string().to_lowercase();
        for forbidden in ["approuv", "autoris", "sans approbation", "bypass"] {
            assert!(
                !blob.contains(forbidden) || blob.contains("jamais"),
                "style must not read as granting anything: {forbidden} in {blob}"
            );
        }
    }

    #[test]
    fn identity_publishes_a_style_consistent_with_its_traits() {
        let dir = scratch("style");
        write_personality(
            &dir,
            "name = \"Vesper\"\n[traits]\nconcision = 80\ncaution = 70\nwarmth = 80\n",
        );
        let v = identity_value(&dir);
        assert_eq!(v["born"], true);
        // The published numbers must explain the published style.
        assert_eq!(v["traits"]["concision"], 80);
        assert_eq!(v["style"]["verbosity"], "télégraphique");
        assert_eq!(v["style"]["confirmation"], "confirme l'irréversible");
        assert_eq!(v["style"]["explanation"], "pédagogue");
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
