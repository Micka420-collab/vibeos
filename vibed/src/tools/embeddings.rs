//! Embeddings locaux — couche **pure** de la sur-couche « similarité de tâches »
//! d'`user.model` (ADR-028, opt-in, Phase 3).
//!
//! **Pure et inerte** (même statut que `propose.rs`) : formats, similarité et
//! agrégation d'un index vectoriel local sous `knowledge/embeddings/` — le
//! répertoire réservé par docs/MEMORY.md §3.6. Ne lit ni n'écrit aucun store,
//! n'appelle AUCUN modèle ni réseau : la **production** des vecteurs (ollama,
//! local) et le câblage opt-in sont des incréments suivants. D'ici là cette
//! couche n'est pas appelée.
//!
//! Contrats fail-closed, dans l'esprit du reste de la mémoire :
//! - **JSONL append-only** : une ligne malformée est ignorée et COMPTÉE, jamais
//!   un panic ni un abandon du fichier entier (même contrat que les folds) ;
//! - **dédup last-write-wins par `id`** (ts le plus récent gagne), comme les
//!   vues `user/`/`projects/` ;
//! - **bornes explicites** ([`MAX_EMBED_RECORDS`], [`MAX_DIMS`]) — au-delà,
//!   refus explicite, pas de troncature silencieuse ;
//! - **similarité déterministe** : cosinus (norme nulle ⇒ `None`, jamais un
//!   NaN), top-k à tri STABLE (score décroissant, puis `id` croissant) pour
//!   qu'une égalité de score ne rende pas le classement dépendant de l'ordre
//!   d'arrivée.

// Production des vecteurs (ollama) + câblage opt-in = incréments suivants ;
// jusque-là cette couche pure n'est pas encore appelée.
#![allow(dead_code)]

use serde_json::{json, Value};

/// Nombre maximal d'enregistrements chargés d'un index (anti-DoS mémoire :
/// l'index vit sous la mémoire utilisateur, qu'un agent alimente ligne à ligne).
pub(crate) const MAX_EMBED_RECORDS: usize = 4096;
/// Dimension maximale d'un vecteur (les modèles locaux visés produisent
/// 384–1024 ; 4096 laisse de la marge sans accepter l'absurde).
pub(crate) const MAX_DIMS: usize = 4096;

/// Un enregistrement d'embedding tel que sérialisé en JSONL sous
/// `knowledge/embeddings/*.jsonl` : `{id, ts, source, text_digest, vector}`.
/// `text_digest` identifie le texte embarqué (fnv/sha côté producteur) sans le
/// stocker — l'index ne duplique pas le contenu, il pointe vers la mémoire.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EmbeddingRecord {
    pub(crate) id: String,
    pub(crate) ts: u64,
    pub(crate) source: String,
    pub(crate) text_digest: String,
    pub(crate) vector: Vec<f32>,
}

impl EmbeddingRecord {
    /// Parse une ligne JSONL. `None` = ligne malformée (à compter, pas à
    /// propager) : champ manquant, type faux, vecteur vide ou > [`MAX_DIMS`],
    /// valeur non finie (NaN/inf casserait le cosinus en aval).
    pub(crate) fn from_json_line(line: &str) -> Option<Self> {
        let v: Value = serde_json::from_str(line).ok()?;
        let id = v.get("id")?.as_str()?.to_string();
        let ts = v.get("ts")?.as_u64()?;
        let source = v.get("source")?.as_str()?.to_string();
        let text_digest = v.get("text_digest")?.as_str()?.to_string();
        let raw = v.get("vector")?.as_array()?;
        if id.is_empty() || raw.is_empty() || raw.len() > MAX_DIMS {
            return None;
        }
        let mut vector = Vec::with_capacity(raw.len());
        for x in raw {
            let f = x.as_f64()?;
            if !f.is_finite() {
                return None;
            }
            vector.push(f as f32);
        }
        Some(Self {
            id,
            ts,
            source,
            text_digest,
            vector,
        })
    }

    /// Sérialise en une ligne JSONL (sans retour à la ligne final).
    pub(crate) fn to_json_line(&self) -> String {
        json!({
            "id": self.id,
            "ts": self.ts,
            "source": self.source,
            "text_digest": self.text_digest,
            "vector": self.vector,
        })
        .to_string()
    }
}

/// Résultat du chargement d'un index : les enregistrements valides (dédupliqués
/// last-write-wins par `id`) + le nombre de lignes ignorées (malformées ou en
/// dimension incohérente avec le premier vecteur accepté).
#[derive(Debug, Default)]
pub(crate) struct EmbeddingIndex {
    pub(crate) records: Vec<EmbeddingRecord>,
    pub(crate) skipped: usize,
}

/// Charge un index depuis un texte JSONL. Fail-closed ligne à ligne :
/// malformée ⇒ comptée dans `skipped` ; dimension différente du premier vecteur
/// accepté ⇒ comptée aussi (un index mélangeant des modèles est inutilisable
/// pour le cosinus). Refus explicite au-delà de [`MAX_EMBED_RECORDS`] APRÈS
/// dédup — la borne porte sur l'index utile, pas sur l'historique append-only.
pub(crate) fn load_index(text: &str) -> Result<EmbeddingIndex, String> {
    let mut by_id: std::collections::BTreeMap<String, EmbeddingRecord> =
        std::collections::BTreeMap::new();
    let mut skipped = 0usize;
    let mut dims: Option<usize> = None;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Some(rec) = EmbeddingRecord::from_json_line(line) else {
            skipped += 1;
            continue;
        };
        match dims {
            None => dims = Some(rec.vector.len()),
            Some(d) if d != rec.vector.len() => {
                skipped += 1;
                continue;
            }
            Some(_) => {}
        }
        // Last-write-wins par id : le ts le plus récent gagne ; à ts égal, la
        // dernière ligne du fichier (ordre append) gagne.
        match by_id.get(&rec.id) {
            Some(prev) if prev.ts > rec.ts => {}
            _ => {
                by_id.insert(rec.id.clone(), rec);
            }
        }
    }
    if by_id.len() > MAX_EMBED_RECORDS {
        return Err(format!(
            "embeddings: index over capacity ({} > {MAX_EMBED_RECORDS} records after dedup)",
            by_id.len()
        ));
    }
    Ok(EmbeddingIndex {
        records: by_id.into_values().collect(),
        skipped,
    })
}

/// Similarité cosinus. `None` si les dimensions diffèrent ou si l'une des
/// normes est nulle — jamais un NaN silencieux.
pub(crate) fn cosine_similarity(a: &[f32], b: &[f32]) -> Option<f32> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let (mut dot, mut na, mut nb) = (0f64, 0f64, 0f64);
    for (x, y) in a.iter().zip(b) {
        dot += f64::from(*x) * f64::from(*y);
        na += f64::from(*x) * f64::from(*x);
        nb += f64::from(*y) * f64::from(*y);
    }
    if na == 0.0 || nb == 0.0 {
        return None;
    }
    Some((dot / (na.sqrt() * nb.sqrt())) as f32)
}

/// Les `k` enregistrements les plus proches de `query`, tri STABLE :
/// score décroissant puis `id` croissant (une égalité de score ne dépend
/// jamais de l'ordre d'arrivée). Les enregistrements sans score (dimension
/// différente, norme nulle) sont écartés.
pub(crate) fn top_k<'a>(
    query: &[f32],
    records: &'a [EmbeddingRecord],
    k: usize,
) -> Vec<(&'a EmbeddingRecord, f32)> {
    let mut scored: Vec<(&EmbeddingRecord, f32)> = records
        .iter()
        .filter_map(|r| cosine_similarity(query, &r.vector).map(|s| (r, s)))
        .collect();
    scored.sort_by(|(ra, sa), (rb, sb)| {
        sb.partial_cmp(sa)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| ra.id.cmp(&rb.id))
    });
    scored.truncate(k);
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, ts: u64, vector: Vec<f32>) -> EmbeddingRecord {
        EmbeddingRecord {
            id: id.to_string(),
            ts,
            source: "test".to_string(),
            text_digest: "d".to_string(),
            vector,
        }
    }

    #[test]
    fn cosine_known_values() {
        // Orthogonaux -> 0 ; colinéaires -> 1 ; opposés -> -1.
        let s = cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]).unwrap();
        assert!(s.abs() < 1e-6, "orthogonal must be ~0, got {s}");
        let s = cosine_similarity(&[2.0, 0.0], &[5.0, 0.0]).unwrap();
        assert!((s - 1.0).abs() < 1e-6, "collinear must be ~1, got {s}");
        let s = cosine_similarity(&[1.0, 1.0], &[-1.0, -1.0]).unwrap();
        assert!((s + 1.0).abs() < 1e-6, "opposite must be ~-1, got {s}");
        // Norme nulle et dimensions différentes -> None, jamais NaN.
        assert!(cosine_similarity(&[0.0, 0.0], &[1.0, 0.0]).is_none());
        assert!(cosine_similarity(&[1.0], &[1.0, 0.0]).is_none());
        assert!(cosine_similarity(&[], &[]).is_none());
    }

    #[test]
    fn top_k_is_stable_and_bounded() {
        // b et c ont le MÊME score que la query : l'égalité se départage par
        // id croissant, quel que soit l'ordre du tableau d'entrée.
        let records = vec![
            rec("c", 1, vec![1.0, 0.0]),
            rec("a", 1, vec![0.0, 1.0]),
            rec("b", 1, vec![1.0, 0.0]),
        ];
        let hits = top_k(&[1.0, 0.0], &records, 2);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].0.id, "b", "tie broken by ascending id");
        assert_eq!(hits[1].0.id, "c");
        // k > disponibles : tout rend, sans panic.
        assert_eq!(top_k(&[1.0, 0.0], &records, 10).len(), 3);
        // Un enregistrement en dimension incompatible est écarté, pas une erreur.
        let mixed = vec![rec("a", 1, vec![1.0, 0.0]), rec("z", 1, vec![1.0])];
        assert_eq!(top_k(&[1.0, 0.0], &mixed, 10).len(), 1);
    }

    #[test]
    fn jsonl_round_trip_and_corrupt_lines() {
        let r = rec("s1", 42, vec![0.25, -0.5]);
        let line = r.to_json_line();
        assert_eq!(EmbeddingRecord::from_json_line(&line).unwrap(), r);
        // Lignes corrompues : jamais un panic, toujours None.
        for bad in [
            "not json",
            "{}",
            r#"{"id":"x","ts":1,"source":"s","text_digest":"d","vector":[]}"#,
            r#"{"id":"","ts":1,"source":"s","text_digest":"d","vector":[1.0]}"#,
            r#"{"id":"x","ts":1,"source":"s","text_digest":"d","vector":["a"]}"#,
        ] {
            assert!(EmbeddingRecord::from_json_line(bad).is_none(), "{bad}");
        }
        // NaN/inf refusés à l'entrée (casseraient le cosinus en aval).
        let nan = r#"{"id":"x","ts":1,"source":"s","text_digest":"d","vector":[null]}"#;
        assert!(EmbeddingRecord::from_json_line(nan).is_none());
    }

    #[test]
    fn load_index_dedups_last_write_wins_and_counts_skips() {
        let text = format!(
            "{}\n{}\ngarbage line\n{}\n{}\n",
            rec("a", 1, vec![1.0, 0.0]).to_json_line(),
            rec("a", 5, vec![0.0, 1.0]).to_json_line(), // ts plus récent : gagne
            rec("a", 3, vec![0.5, 0.5]).to_json_line(), // plus vieux que 5 : perd
            rec("mix", 1, vec![1.0]).to_json_line(),    // dimension incohérente
        );
        let idx = load_index(&text).expect("index loads");
        assert_eq!(idx.records.len(), 1);
        assert_eq!(idx.records[0].ts, 5, "last write (highest ts) wins");
        assert_eq!(idx.records[0].vector, vec![0.0, 1.0]);
        assert_eq!(
            idx.skipped, 2,
            "garbage + mixed-dims are counted, not fatal"
        );
        // Un texte vide charge un index vide, sans erreur.
        let empty = load_index("").expect("empty ok");
        assert!(empty.records.is_empty() && empty.skipped == 0);
    }

    #[test]
    fn load_index_refuses_over_capacity() {
        // MAX_DIMS refusé à la ligne (from_json_line), capacité à l'index.
        let mut text = String::new();
        for i in 0..=MAX_EMBED_RECORDS {
            text.push_str(&rec(&format!("id{i:05}"), 1, vec![1.0]).to_json_line());
            text.push('\n');
        }
        let err = load_index(&text).unwrap_err();
        assert!(err.contains("over capacity"), "unexpected error: {err}");
        let too_wide = rec("w", 1, vec![0.5; MAX_DIMS + 1]).to_json_line();
        assert!(EmbeddingRecord::from_json_line(&too_wide).is_none());
    }
}
