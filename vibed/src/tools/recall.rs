//! Rappel mémoire — couche **pure** du classement de pertinence de `memory.query`
//! (docs/MEMORY.md §3.5/§3.6, ADR-030).
//!
//! **Pure** (aucune E/S, aucun modèle, aucun réseau) — mais **désormais câblée** :
//! `memory.query` en mode `rank` (opt-in, scopes `journal`/`knowledge`) classe
//! les événements par ce score. Le **moteur lexical** est donc appelé à
//! l'exécution ; seul le **moteur vectoriel** ([`relevance_cosine`], cosinus de
//! [`embeddings`](super::embeddings)) reste **inerte** tant que la production des
//! vecteurs (ollama local, ADR-028) n'existe pas — c'est l'incrément suivant.
//!
//! **Pourquoi.** Une mémoire qui grossit sans classement se dégrade : tout
//! remonter noie le signal, filtrer par sous-chaîne rate le pertinent mal
//! nommé. Le modèle éprouvé (« memory stream » des *Generative Agents*,
//! Park et al. 2023) score chaque souvenir sur **trois axes** combinés
//! linéairement — et c'est reproductible, explicable, testable, à la portée d'un
//! OS hors-ligne :
//!
//!   score = w_récence · récence + w_importance · importance + w_pertinence · pertinence
//!
//! - **récence** : décroissance exponentielle par demi-vie (un souvenir de ce
//!   matin pèse plus qu'un de l'an dernier), dans `[0, 1]` ;
//! - **importance** : saillance intrinsèque `[0, 1]` — dérivée du *type*
//!   d'événement journal (une `decision` pèse plus qu'une `observation`), jamais
//!   d'un champ auto-déclaré par l'agent (THREAT-MODEL §1 : `source`/`data` sont
//!   NON FIABLES) ;
//! - **pertinence** : proximité à la requête. Deux moteurs, même échelle `[0, 1]`
//!   et même contrat : **lexical** (recouvrement de jetons, dispo tout de suite,
//!   hors-ligne, zéro dépendance) et **vectoriel** (cosinus de l'index
//!   [`embeddings`](super::embeddings), quand les vecteurs existent) — c'est le
//!   premier consommateur réel de cette couche inerte.
//!
//! Fail-closed, déterministe : poids clampés dans `[0, 1]`, tri STABLE (score
//! décroissant puis `id` croissant — une égalité ne dépend jamais de l'ordre
//! d'arrivée), aucune valeur non finie propagée.

// Le moteur lexical est câblé (memory.query `rank`) ; le moteur VECTORIEL
// (`relevance_cosine` + `embeddings`) reste inerte tant que la production des
// vecteurs n'existe pas — d'où l'allow ciblé sur cette seule fonction.
use super::embeddings::cosine_similarity;

/// Poids des trois axes du score de rappel. Les valeurs par défaut suivent la
/// littérature (les trois axes comptent, la pertinence un peu plus) ; un
/// opérateur pourra les régler par politique (Phase 3).
#[derive(Debug, Clone, Copy)]
pub(crate) struct RecallWeights {
    pub(crate) recency: f32,
    pub(crate) importance: f32,
    pub(crate) relevance: f32,
}

impl Default for RecallWeights {
    fn default() -> Self {
        Self {
            recency: 1.0,
            importance: 1.0,
            relevance: 1.5,
        }
    }
}

/// Un souvenir candidat au rappel : l'unité que `memory.query` classera. `text`
/// est le contenu déjà extrait (le `snippet` borné de la spec §9), `ts_unix`
/// l'horodatage posé par `vibed`, `importance` la saillance dérivée du type.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MemoryItem {
    pub(crate) id: String,
    pub(crate) ts_unix: u64,
    pub(crate) importance: f32,
    pub(crate) text: String,
}

/// Récence dans `[0, 1]` par décroissance exponentielle de demi-vie : un
/// souvenir d'âge `half_life_secs` vaut 0.5, deux demi-vies 0.25, etc. Un
/// horodatage futur (dérive d'horloge) est clampé à « maintenant » ⇒ 1.0, jamais
/// un score > 1. `half_life_secs == 0` ⇒ 1.0 (récence désactivée, pas un NaN).
pub(crate) fn recency(now_unix: u64, ts_unix: u64, half_life_secs: u64) -> f32 {
    if half_life_secs == 0 {
        return 1.0;
    }
    let age = now_unix.saturating_sub(ts_unix) as f64;
    let hl = half_life_secs as f64;
    // 0.5^(age / hl) = 2^(-age/hl).
    2f64.powf(-age / hl) as f32
}

/// Epoch **secondes** à minuit UTC de la date portée par le début `AAAA-MM-JJ`
/// d'un horodatage ISO-8601. **Granularité au jour** — délibérée : robuste, sans
/// arithmétique de fuseau (le journal est déjà bucketé au jour, §3.5), suffisant
/// pour l'axe récence. `None` si les 10 premiers caractères ne sont pas une date
/// valide. Inverse (au jour près) de `mcp::utc_civil` — algorithme
/// `days_from_civil` de Howard Hinnant, correct pour les années négatives via
/// division euclidienne.
pub(crate) fn epoch_day_from_iso(ts: &str) -> Option<u64> {
    let b = ts.as_bytes();
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let y: i64 = ts.get(0..4)?.parse().ok()?;
    let m: i64 = ts.get(5..7)?.parse().ok()?;
    let d: i64 = ts.get(8..10)?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // days_from_civil : y décalé si jan/fév, ère de 400 ans, jour-de-l'ère.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400; // [0, 399]
    let mp = if m > 2 { m - 3 } else { m + 9 }; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    let days = era * 146_097 + doe - 719_468;
    // Une date antérieure à l'époque (improbable dans le journal) ⇒ 0, jamais
    // un u64 qui déborde par le bas.
    Some((days.max(0) as u64) * 86_400)
}

/// Saillance `[0, 1]` d'un événement journal, dérivée de son **type** (§3.5) —
/// jamais d'un champ auto-déclaré. Un type inconnu tombe sur un plancher neutre
/// (0.3) : on n'écarte pas un souvenir parce qu'on ne connaît pas son étiquette.
pub(crate) fn importance_of_type(event_type: &str) -> f32 {
    match event_type {
        "decision" => 0.9,
        "preference" => 0.8,
        "error" => 0.7,
        "project_seen" => 0.6,
        "observation" => 0.5,
        // Bruit méta / événements système : présents mais peu saillants.
        "boot" | "tool_call" => 0.2,
        "genesis" => 0.4,
        _ => 0.3,
    }
}

/// Jetons normalisés d'un texte pour le recouvrement lexical : minuscules
/// ASCII, découpés sur tout ce qui n'est ni alphanumérique ni `_`, vides écartés.
fn tokenize(text: &str) -> std::collections::BTreeSet<String> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

/// Pertinence LEXICALE `[0, 1]` : indice de Jaccard des jetons (|∩| / |∪|).
/// Reproductible, hors-ligne, sans dépendance — le moteur par défaut tant que
/// l'index vectoriel n'est pas produit. Requête ou texte vide ⇒ 0.0.
pub(crate) fn relevance_lexical(query: &str, text: &str) -> f32 {
    let (q, t) = (tokenize(query), tokenize(text));
    if q.is_empty() || t.is_empty() {
        return 0.0;
    }
    let inter = q.intersection(&t).count();
    if inter == 0 {
        return 0.0;
    }
    let union = q.len() + t.len() - inter;
    inter as f32 / union as f32
}

/// Pertinence VECTORIELLE `[0, 1]` : cosinus de l'index [`embeddings`], replié de
/// `[-1, 1]` vers `[0, 1]` par `(c + 1) / 2`. `None` (dimensions différentes,
/// norme nulle) est traité par l'appelant comme « pas de signal vectoriel »
/// (repli lexical), jamais comme un score de 0 imposé. **Inerte** tant que la
/// production des vecteurs n'existe pas (d'où l'`allow`) — l'incrément suivant.
#[allow(dead_code)]
pub(crate) fn relevance_cosine(query_vec: &[f32], item_vec: &[f32]) -> Option<f32> {
    cosine_similarity(query_vec, item_vec).map(|c| (c + 1.0) / 2.0)
}

/// Score de rappel combiné = Σ poids·axe. `relevance` est fourni par l'appelant
/// (lexical ou vectoriel, même échelle) pour que la combinaison ne dépende pas du
/// moteur choisi. Les trois axes sont supposés dans `[0, 1]` ; le résultat n'est
/// pas renormalisé (seul l'ORDRE compte pour un top-k).
pub(crate) fn recall_score(
    w: &RecallWeights,
    recency: f32,
    importance: f32,
    relevance: f32,
) -> f32 {
    w.recency * recency + w.importance * importance + w.relevance * relevance
}

/// Les `k` souvenirs les plus pertinents pour `query`, classés par score de
/// rappel décroissant (récence + importance + pertinence LEXICALE). Tri STABLE :
/// à score égal, `id` croissant — le classement ne dépend jamais de l'ordre du
/// tableau d'entrée. `k == 0` ⇒ vide ; `k` > disponible ⇒ tout, sans panic.
pub(crate) fn top_k_lexical<'a>(
    query: &str,
    items: &'a [MemoryItem],
    now_unix: u64,
    half_life_secs: u64,
    weights: &RecallWeights,
    k: usize,
) -> Vec<(&'a MemoryItem, f32)> {
    let mut scored: Vec<(&MemoryItem, f32)> = items
        .iter()
        .map(|it| {
            let s = recall_score(
                weights,
                recency(now_unix, it.ts_unix, half_life_secs),
                it.importance.clamp(0.0, 1.0),
                relevance_lexical(query, &it.text),
            );
            (it, s)
        })
        .collect();
    scored.sort_by(|(ia, sa), (ib, sb)| {
        sb.partial_cmp(sa)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| ia.id.cmp(&ib.id))
    });
    scored.truncate(k);
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, ts: u64, importance: f32, text: &str) -> MemoryItem {
        MemoryItem {
            id: id.to_string(),
            ts_unix: ts,
            importance,
            text: text.to_string(),
        }
    }

    #[test]
    fn recency_halves_per_half_life() {
        let now = 1_000_000;
        let hl = 86_400; // un jour
        assert!((recency(now, now, hl) - 1.0).abs() < 1e-6, "âge 0 -> 1.0");
        assert!(
            (recency(now, now - hl, hl) - 0.5).abs() < 1e-6,
            "1 demi-vie -> 0.5"
        );
        assert!(
            (recency(now, now - 2 * hl, hl) - 0.25).abs() < 1e-6,
            "2 -> 0.25"
        );
        // Horodatage futur (dérive d'horloge) clampé à 1.0, jamais > 1.
        assert!((recency(now, now + hl, hl) - 1.0).abs() < 1e-6);
        // Demi-vie nulle : récence désactivée, pas un NaN.
        assert_eq!(recency(now, now - 999, 0), 1.0);
    }

    #[test]
    fn epoch_day_parses_iso_date_at_day_granularity() {
        // Points d'ancrage connus (minuit UTC).
        assert_eq!(epoch_day_from_iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_day_from_iso("1970-01-02T12:34:56Z"), Some(86_400));
        assert_eq!(
            epoch_day_from_iso("2000-01-01T00:00:00Z"),
            Some(946_684_800)
        );
        // Le temps intra-journée est ignoré (granularité au jour).
        assert_eq!(
            epoch_day_from_iso("2026-07-22T09:14:22+02:00"),
            epoch_day_from_iso("2026-07-22T23:59:59Z")
        );
        // Un jour de plus = +86 400 s, pile.
        let d1 = epoch_day_from_iso("2026-07-22T00:00:00Z").unwrap();
        let d2 = epoch_day_from_iso("2026-07-23T00:00:00Z").unwrap();
        assert_eq!(d2 - d1, 86_400);
        // Formes invalides ⇒ None (jamais un panic ni une date bidon).
        for bad in [
            "",
            "2026",
            "2026/07/22",
            "not-a-date",
            "2026-13-01",
            "2026-07-00",
        ] {
            assert!(epoch_day_from_iso(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn importance_ordered_by_type_with_neutral_floor() {
        assert!(importance_of_type("decision") > importance_of_type("observation"));
        assert!(importance_of_type("observation") > importance_of_type("tool_call"));
        // Un type inconnu ne tombe pas à 0 : plancher neutre.
        assert_eq!(importance_of_type("inconnu"), 0.3);
    }

    #[test]
    fn lexical_relevance_is_jaccard() {
        // {rust, async} ∩ {rust, tokio, async} = 2 ; ∪ = 3 -> 2/3.
        let s = relevance_lexical("rust async", "rust tokio async");
        assert!((s - 2.0 / 3.0).abs() < 1e-6, "got {s}");
        // Insensible à la casse et à la ponctuation.
        assert!(relevance_lexical("Rust!", "rust") > 0.99);
        // Aucun jeton commun -> 0 ; requête/texte vide -> 0 (jamais NaN).
        assert_eq!(relevance_lexical("go", "rust"), 0.0);
        assert_eq!(relevance_lexical("", "rust"), 0.0);
        assert_eq!(relevance_lexical("rust", ""), 0.0);
    }

    #[test]
    fn cosine_relevance_maps_to_unit_interval() {
        // Colinéaires -> cos 1 -> 1.0 ; opposés -> cos -1 -> 0.0 ; orthogonaux -> 0.5.
        assert!((relevance_cosine(&[1.0, 0.0], &[2.0, 0.0]).unwrap() - 1.0).abs() < 1e-6);
        assert!((relevance_cosine(&[1.0, 0.0], &[-1.0, 0.0]).unwrap() - 0.0).abs() < 1e-6);
        assert!((relevance_cosine(&[1.0, 0.0], &[0.0, 1.0]).unwrap() - 0.5).abs() < 1e-6);
        // Norme nulle / dimensions différentes -> None (repli à l'appelant).
        assert!(relevance_cosine(&[0.0, 0.0], &[1.0, 0.0]).is_none());
        assert!(relevance_cosine(&[1.0], &[1.0, 0.0]).is_none());
    }

    #[test]
    fn top_k_ranks_by_combined_score_and_is_stable() {
        let now = 1_000_000;
        let hl = 86_400;
        // "a" : vieux mais pertinent + décision (importance haute).
        // "b" : récent, pertinent, observation.
        // "c" : récent mais hors-sujet.
        let items = vec![
            item(
                "a",
                now - 3 * hl,
                importance_of_type("decision"),
                "déploiement rust gouverné",
            ),
            item("b", now, importance_of_type("observation"), "rust tokio"),
            item(
                "c",
                now,
                importance_of_type("observation"),
                "recette de cuisine",
            ),
        ];
        let hits = top_k_lexical("rust", &items, now, hl, &RecallWeights::default(), 3);
        assert_eq!(hits.len(), 3);
        // "c" (hors-sujet) est dernier malgré sa récence maximale.
        assert_eq!(hits[2].0.id, "c");
        // Les deux premiers sont "a"/"b" (pertinents), dans un ordre déterministe.
        assert!(hits[0].0.id == "a" || hits[0].0.id == "b");

        // Stabilité : deux items strictement équivalents se départagent par id.
        let tie = vec![item("z", now, 0.5, "rust"), item("y", now, 0.5, "rust")];
        let h = top_k_lexical("rust", &tie, now, hl, &RecallWeights::default(), 2);
        assert_eq!(h[0].0.id, "y", "égalité départagée par id croissant");
        assert_eq!(h[1].0.id, "z");
        // k > disponible et k == 0 : bornes sûres.
        assert_eq!(
            top_k_lexical("rust", &tie, now, hl, &RecallWeights::default(), 99).len(),
            2
        );
        assert!(top_k_lexical("rust", &tie, now, hl, &RecallWeights::default(), 0).is_empty());
    }

    #[test]
    fn recency_is_monotonic_and_bounded() {
        // INVARIANT : à demi-vie fixe, la récence ne CROÎT jamais avec l'âge et
        // reste dans [0, 1] — la propriété de base du classement (un souvenir plus
        // vieux ne peut pas paraître plus frais). Balayage déterministe de 0 à
        // 400 demi-vies (sans RNG).
        let now = 10_000_000u64;
        let hl = 86_400u64;
        let mut prev = f32::INFINITY;
        for k in 0..=400 {
            let ts = now.saturating_sub(k * hl);
            let r = recency(now, ts, hl);
            assert!((0.0..=1.0).contains(&r), "récence hors [0,1] à k={k} : {r}");
            assert!(
                r <= prev + 1e-6,
                "la récence ne doit pas croître avec l'âge (k={k})"
            );
            prev = r;
        }
        // Et le classement est invariant par permutation de l'entrée (au-delà des
        // égalités déjà couvertes) : mêmes items, ordre d'entrée inversé, même top-k.
        let items = vec![
            item("a", now - 3 * hl, 0.9, "rust tokio"),
            item("b", now, 0.5, "rust async"),
            item("c", now, 0.5, "cuisine"),
        ];
        let mut reversed = items.clone();
        reversed.reverse();
        let w = RecallWeights::default();
        let h1 = top_k_lexical("rust", &items, now, hl, &w, 3);
        let h2 = top_k_lexical("rust", &reversed, now, hl, &w, 3);
        let ids1: Vec<&str> = h1.iter().map(|(it, _)| it.id.as_str()).collect();
        let ids2: Vec<&str> = h2.iter().map(|(it, _)| it.id.as_str()).collect();
        assert_eq!(ids1, ids2, "classement invariant par permutation d'entrée");
    }
}
