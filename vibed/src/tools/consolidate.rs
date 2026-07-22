//! Consolidation du savoir — la **vue courante dédupliquée** de
//! `knowledge/facts.jsonl` (docs/MEMORY.md §3.6/§4.2/§7, ADR-030).
//!
//! **Pure** (aucune E/S, aucun modèle, aucun réseau) : le *fold* des faits
//! append-only en une vue consolidée, comme `memory_profile_at`/`memory_projects_at`
//! le font pour `user/`/`projects/`, mais avec le schéma et les invariants
//! propres au savoir. Le câblage (`vibectl memory knowledge`, `memory.query`
//! `fold:true` scope `knowledge`) vit dans `vibectl`/`memory.rs` et appelle ce
//! cœur.
//!
//! **Pourquoi.** `facts.jsonl` est **append-only** : un même fait ré-affirmé
//! s'y accumule en autant de lignes (chacune son `id` généré par `vibed`).
//! Sans consolidation, `memory.query scope=knowledge` rend chaque ré-assertion
//! comme une ligne distincte — le savoir enfle sans se distiller. Le fold
//! **déduplique par `(subject, fact)`**, dernière écriture gagnant (le `ts` le
//! plus récent), et rend une vue stable et ordonnée.
//!
//! **Invariant de sécurité (THREAT-MODEL §9, non négociable).** `source`,
//! `fact` et `confidence` sont **auto-déclarés par l'agent** — un *insider non
//! fiable*. La consolidation **n'élève JAMAIS** la confiance d'un fait :
//! - pas d'agrégation de confiance par nombre d'assertions (N assertions d'un
//!   agent menteur ne sont pas N corroborations) ;
//! - pas de bonus par `source` (un `source` forgé n'est pas une preuve) ;
//! - la confiance rendue est **exactement** celle stockée par l'entrée gagnante
//!   (la plus récente), jamais recalculée à la hausse.
//!
//! La consolidation **compacte** (dédup), elle ne **juge** pas.

// Le cœur est appelé par le fold `vibectl`/`memory.rs` ; les helpers de parse
// exposés servent aussi aux tests. `dead_code` ciblé sur ce qui n'est pas encore
// consommé hors tests.
use serde_json::{json, Value};

/// Borne dure du nombre de faits dédupliqués rendus (anti-DoS mémoire : le store
/// vit sous la mémoire, qu'un agent alimente ligne à ligne). Au-delà, la vue est
/// tronquée (drapeau `truncated`), jamais une allocation sans borne.
pub(crate) const MAX_CONSOLIDATED_FACTS: usize = 8192;

/// Un fait tel que sérialisé dans `knowledge/facts.jsonl`
/// (`{id, ts, subject, fact, confidence?, source}`), réduit aux champs dont la
/// consolidation a besoin. `ts` est gardé en **chaîne** : `vibed` l'écrit en
/// ISO-8601 UTC (`utc_iso8601`, suffixe `Z`), donc l'ordre lexicographique = l'ordre
/// chronologique (à la seconde) — suffisant et robuste pour un dernier-écrit-gagne.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct KnowledgeFact {
    pub(crate) subject: String,
    pub(crate) fact: String,
    pub(crate) confidence: Option<f64>,
    pub(crate) ts: String,
    pub(crate) source: String,
}

impl KnowledgeFact {
    /// Extrait un fait d'une valeur JSON déjà parsée. `None` = entrée
    /// inexploitable (pas un objet, `subject`/`fact` absents ou vides) : à
    /// compter comme ignorée, jamais fatale. Une `confidence` hors `[0, 1]` ou
    /// non finie est **écartée** (ramenée à `None`) plutôt que propagée — on ne
    /// laisse pas une valeur absurde teinter la vue.
    pub(crate) fn from_value(v: &Value) -> Option<Self> {
        if !v.is_object() {
            return None;
        }
        let subject = v.get("subject")?.as_str()?.trim().to_string();
        let fact = v.get("fact")?.as_str()?.trim().to_string();
        if subject.is_empty() || fact.is_empty() {
            return None;
        }
        let confidence = v
            .get("confidence")
            .and_then(Value::as_f64)
            .filter(|c| c.is_finite() && (0.0..=1.0).contains(c));
        let ts = v
            .get("ts")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let source = v
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        Some(Self {
            subject,
            fact,
            confidence,
            ts,
            source,
        })
    }
}

/// Résultat du fold : les faits dédupliqués (une entrée par `(subject, fact)`),
/// le nombre de lignes ignorées, et si la borne a tronqué la vue.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Consolidation {
    pub(crate) facts: Vec<KnowledgeFact>,
    pub(crate) skipped: usize,
    pub(crate) truncated: bool,
}

/// Déduplique des faits déjà parsés par `(subject, fact)`, **dernière écriture
/// gagnant** (le `ts` le plus grand ; à `ts` égal, la dernière ligne du fichier,
/// ordre append, gagne). Ordre de sortie **déterministe** : `subject` croissant
/// puis `fact` croissant — jamais dépendant de l'ordre d'arrivée. Tronqué à
/// [`MAX_CONSOLIDATED_FACTS`] entrées **après** dédup (la borne porte sur la vue
/// utile, pas sur l'historique append-only).
///
/// N'élève jamais la confiance (voir l'invariant du module) : la confiance et la
/// source rendues sont celles de l'entrée gagnante, point.
pub(crate) fn consolidate(records: &[Value]) -> Consolidation {
    use std::collections::BTreeMap;
    // Clé stable et ordonnée : (subject, fact). BTreeMap => sortie triée gratuite.
    let mut by_key: BTreeMap<(String, String), KnowledgeFact> = BTreeMap::new();
    let mut skipped = 0usize;
    for rec in records {
        let Some(f) = KnowledgeFact::from_value(rec) else {
            skipped += 1;
            continue;
        };
        let key = (f.subject.clone(), f.fact.clone());
        match by_key.get(&key) {
            // Une entrée strictement plus récente gagne ; à ts égal, la dernière
            // vue (ordre d'itération = ordre append) gagne — d'où le `>` (on
            // remplace tant que le nouveau n'est pas plus ancien).
            Some(prev) if prev.ts > f.ts => {}
            _ => {
                by_key.insert(key, f);
            }
        }
    }
    let total = by_key.len();
    let truncated = total > MAX_CONSOLIDATED_FACTS;
    let facts: Vec<KnowledgeFact> = by_key.into_values().take(MAX_CONSOLIDATED_FACTS).collect();
    Consolidation {
        facts,
        skipped,
        truncated,
    }
}

/// Le fold consommé par `vibectl memory knowledge` et `memory.query fold:true`
/// (scope `knowledge`) : signature `fn(Vec<Value>) -> Value` pour se brancher sur
/// `fold_updates_cached` comme `user`/`projects`. Rend
/// `{ "knowledge": [ {subject, fact, confidence, ts, source} … ], "count", "truncated" }`.
pub(crate) fn consolidate_fold(records: Vec<Value>) -> Value {
    let c = consolidate(&records);
    let facts: Vec<Value> = c
        .facts
        .into_iter()
        .map(|f| {
            json!({
                "subject": f.subject,
                "fact": f.fact,
                // `null` si non fournie/écartée — jamais une valeur inventée.
                "confidence": f.confidence,
                "ts": f.ts,
                "source": f.source,
            })
        })
        .collect();
    json!({
        "knowledge": facts,
        "count": facts.len(),
        "truncated": c.truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(subject: &str, fact: &str, confidence: Option<f64>, ts: &str, source: &str) -> Value {
        let mut o = serde_json::Map::new();
        o.insert("subject".into(), json!(subject));
        o.insert("fact".into(), json!(fact));
        if let Some(c) = confidence {
            o.insert("confidence".into(), json!(c));
        }
        o.insert("ts".into(), json!(ts));
        o.insert("source".into(), json!(source));
        Value::Object(o)
    }

    #[test]
    fn dedups_by_subject_and_fact_last_write_wins() {
        let records = vec![
            fact(
                "proj.vibeos",
                "utilise pnpm",
                Some(0.6),
                "2026-07-01T00:00:00Z",
                "a",
            ),
            // Même (subject, fact) ré-affirmé plus tard : gagne (ts plus récent).
            fact(
                "proj.vibeos",
                "utilise pnpm",
                Some(0.9),
                "2026-07-20T00:00:00Z",
                "b",
            ),
            // Fait DIFFÉRENT pour le même subject : entrée distincte (pas un doublon).
            fact(
                "proj.vibeos",
                "cible Fedora 44",
                Some(0.8),
                "2026-07-10T00:00:00Z",
                "a",
            ),
        ];
        let c = consolidate(&records);
        assert_eq!(c.facts.len(), 2, "deux (subject,fact) distincts");
        assert_eq!(c.skipped, 0);
        // L'entrée gagnante de « utilise pnpm » est la plus récente (0.9, source b).
        let pnpm = c.facts.iter().find(|f| f.fact == "utilise pnpm").unwrap();
        assert_eq!(pnpm.confidence, Some(0.9));
        assert_eq!(pnpm.ts, "2026-07-20T00:00:00Z");
        assert_eq!(pnpm.source, "b");
        // Ordre déterministe : subject puis fact croissant.
        assert_eq!(c.facts[0].fact, "cible Fedora 44");
        assert_eq!(c.facts[1].fact, "utilise pnpm");
    }

    #[test]
    fn confidence_is_never_raised_by_repetition_or_source() {
        // Le MÊME fait affirmé trois fois, confiance faible, avec des `source`
        // variés : la vue rend UNE entrée, à la confiance stockée de la dernière
        // — jamais gonflée par la répétition ni par la source (THREAT-MODEL §9).
        let records = vec![
            fact("x", "y", Some(0.2), "2026-07-01T00:00:00Z", "genesis.sh"),
            fact("x", "y", Some(0.2), "2026-07-02T00:00:00Z", "vibed"),
            fact("x", "y", Some(0.2), "2026-07-03T00:00:00Z", "claude-code"),
        ];
        let c = consolidate(&records);
        assert_eq!(c.facts.len(), 1);
        assert_eq!(
            c.facts[0].confidence,
            Some(0.2),
            "la confiance reste celle stockée, jamais agrégée à la hausse"
        );
    }

    #[test]
    fn malformed_and_out_of_range_are_handled_fail_closed() {
        let records = vec![
            fact("s", "f", Some(0.5), "2026-07-01T00:00:00Z", "a"),
            json!("not an object"),              // ignoré
            json!({"subject": "s2"}),            // pas de fact -> ignoré
            json!({"fact": "f2"}),               // pas de subject -> ignoré
            json!({"subject": "", "fact": "f"}), // subject vide -> ignoré
            // confidence hors [0,1] -> écartée (None), fait quand même gardé.
            fact("s3", "f3", Some(1.5), "2026-07-01T00:00:00Z", "a"),
        ];
        let c = consolidate(&records);
        assert_eq!(
            c.skipped, 4,
            "4 lignes inexploitables comptées, pas fatales"
        );
        let f3 = c.facts.iter().find(|f| f.subject == "s3").unwrap();
        assert_eq!(
            f3.confidence, None,
            "confiance absurde écartée, pas propagée"
        );
    }

    #[test]
    fn tie_on_ts_prefers_last_appended() {
        // À ts égal, la dernière ligne (ordre append) gagne : correction tardive.
        let records = vec![
            fact("s", "f", Some(0.3), "2026-07-01T00:00:00Z", "old"),
            fact("s", "f", Some(0.7), "2026-07-01T00:00:00Z", "new"),
        ];
        let c = consolidate(&records);
        assert_eq!(c.facts.len(), 1);
        assert_eq!(c.facts[0].confidence, Some(0.7));
        assert_eq!(c.facts[0].source, "new");
    }

    #[test]
    fn fold_shape_and_empty_input() {
        let out = consolidate_fold(vec![fact("s", "f", None, "2026-07-01T00:00:00Z", "a")]);
        assert_eq!(out["count"], 1);
        assert_eq!(out["truncated"], false);
        assert_eq!(out["knowledge"][0]["subject"], "s");
        // confidence absente -> null explicite, jamais inventée.
        assert_eq!(out["knowledge"][0]["confidence"], Value::Null);
        // Entrée vide : vue vide, jamais une erreur.
        let empty = consolidate_fold(vec![]);
        assert_eq!(empty["count"], 0);
        assert!(empty["knowledge"].as_array().unwrap().is_empty());
    }

    #[test]
    fn truncates_over_capacity_after_dedup() {
        let mut records = Vec::new();
        for i in 0..=MAX_CONSOLIDATED_FACTS {
            records.push(fact(
                &format!("s{i:05}"),
                "f",
                Some(0.5),
                "2026-07-01T00:00:00Z",
                "a",
            ));
        }
        let c = consolidate(&records);
        assert!(c.truncated, "au-delà de la borne, vue tronquée");
        assert_eq!(c.facts.len(), MAX_CONSOLIDATED_FACTS);
    }

    #[test]
    fn consolidation_is_idempotent() {
        // INVARIANT : consolider une vue déjà consolidée ne change rien. Re-passer
        // la sortie dans `consolidate` (via re-sérialisation) doit rendre les MÊMES
        // faits, dans le même ordre — le fold est un point fixe (verrouille la
        // stabilité du dédup + du tri, revue adverse).
        let records = vec![
            fact("s1", "fa", Some(0.5), "2026-07-01T00:00:00Z", "a"),
            fact("s1", "fa", Some(0.9), "2026-07-20T00:00:00Z", "b"), // gagne
            fact("s1", "fb", Some(0.3), "2026-07-05T00:00:00Z", "a"),
            fact("s2", "fa", None, "2026-07-10T00:00:00Z", "c"),
        ];
        let first = consolidate(&records);
        // Re-sérialise les faits consolidés en lignes JSON et re-consolide.
        let reserialized: Vec<Value> = first
            .facts
            .iter()
            .map(|f| fact(&f.subject, &f.fact, f.confidence, &f.ts, &f.source))
            .collect();
        let second = consolidate(&reserialized);
        assert_eq!(second.facts, first.facts, "consolidation idempotente");
        assert_eq!(
            second.skipped, 0,
            "aucune ligne re-sérialisée n'est ignorée"
        );
        assert!(!second.truncated);
    }
}
