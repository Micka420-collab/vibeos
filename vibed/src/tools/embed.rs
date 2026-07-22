//! Production d'embeddings locaux — le **contrat pur** d'appel au modèle
//! d'embedding local (ollama), pendant « production » de la couche
//! [`embeddings`](super::embeddings) (stockage/similarité) et du moteur
//! vectoriel de [`recall`](super::recall) (docs/MEMORY.md §3.6, ADR-030/ADR-028).
//!
//! **Pur et inerte** (même statut que [`embeddings`](super::embeddings)) : ce
//! module **construit la requête**, **parse et valide** la réponse, et
//! **fabrique** les [`EmbeddingRecord`](super::embeddings::EmbeddingRecord) — mais
//! il **ne fait AUCUN appel réseau**. L'appel HTTP à ollama (et le choix du
//! contenu à embarquer) est la seule pièce **différée** : elle exige une machine
//! bootée avec ollama local (Phase 3, jamais dans le cloud — la mémoire ne quitte
//! pas la machine, MEMORY.md §1). D'ici là ce module n'est pas appelé à
//! l'exécution.
//!
//! Ensemble, le sous-système d'embedding est complet et prêt à activer dès que
//! l'appel ollama sera câblé :
//! - **`embed.rs`** (ici) : PRODUIT les vecteurs (contrat requête/réponse) ;
//! - **`embeddings.rs`** : STOCKE l'index (`knowledge/embeddings/*.jsonl`) et
//!   calcule la similarité cosinus ;
//! - **`recall.rs`** : CLASSE avec le cosinus quand les vecteurs existent (repli
//!   lexical sinon).
//!
//! Fail-closed, dans l'esprit du reste de la mémoire : lot borné
//! ([`MAX_EMBED_BATCH`], [`MAX_EMBED_TEXT_BYTES`]), réponse dont chaque vecteur
//! est non vide, ≤ [`MAX_DIMS`](super::embeddings::MAX_DIMS), fini, et de
//! dimension **cohérente** — un index mélangeant des dimensions est inutilisable
//! pour le cosinus, donc refusé explicitement, jamais tronqué en silence.

// L'appel HTTP ollama (la production réelle) est l'incrément machine-gated
// suivant ; jusque-là ce contrat pur n'est pas encore appelé.
#![allow(dead_code)]

use serde_json::{json, Value};

use super::embeddings::{EmbeddingRecord, MAX_DIMS};

/// Nombre maximal de textes embarqués en une requête (borne l'empreinte de la
/// requête et la latence d'un lot ; les modèles locaux visés traitent aisément
/// des dizaines de textes par appel).
pub(crate) const MAX_EMBED_BATCH: usize = 64;
/// Taille maximale d'un texte à embarquer (un embedding résume ; au-delà, le
/// producteur doit découper — on refuse plutôt que d'envoyer un texte non borné).
pub(crate) const MAX_EMBED_TEXT_BYTES: usize = 8192;

/// Construit la charge utile de la requête d'embedding pour l'API locale
/// (ollama `/api/embed` : `{ "model", "input": [textes] }`). Refuse — fail-closed —
/// un modèle vide, un lot vide ou trop grand ([`MAX_EMBED_BATCH`]), un texte vide
/// ou trop long ([`MAX_EMBED_TEXT_BYTES`]) : jamais une requête non bornée.
pub(crate) fn build_request(model: &str, inputs: &[String]) -> Result<Value, String> {
    if model.trim().is_empty() {
        return Err("embed: empty model".to_string());
    }
    if inputs.is_empty() {
        return Err("embed: empty batch".to_string());
    }
    if inputs.len() > MAX_EMBED_BATCH {
        return Err(format!(
            "embed: batch of {} exceeds MAX_EMBED_BATCH ({MAX_EMBED_BATCH})",
            inputs.len()
        ));
    }
    for (i, t) in inputs.iter().enumerate() {
        if t.is_empty() {
            return Err(format!("embed: input {i} is empty"));
        }
        if t.len() > MAX_EMBED_TEXT_BYTES {
            return Err(format!(
                "embed: input {i} is {} bytes, exceeds MAX_EMBED_TEXT_BYTES ({MAX_EMBED_TEXT_BYTES})",
                t.len()
            ));
        }
    }
    Ok(json!({ "model": model, "input": inputs }))
}

/// Parse la réponse d'embedding (ollama `/api/embed` : `{ "embeddings": [[…], …] }`).
/// **Fail-closed** : le nombre de vecteurs doit égaler `expected` ; chaque vecteur
/// est non vide, ≤ [`MAX_DIMS`](super::embeddings::MAX_DIMS), entièrement fini, et
/// TOUS de **même dimension** (un index mélangeant des dimensions est inutilisable
/// pour le cosinus). Toute anomalie est une erreur explicite — jamais un vecteur
/// douteux propagé vers l'index.
pub(crate) fn parse_response(text: &str, expected: usize) -> Result<Vec<Vec<f32>>, String> {
    let v: Value =
        serde_json::from_str(text).map_err(|e| format!("embed: response is not JSON: {e}"))?;
    let arr = v
        .get("embeddings")
        .and_then(Value::as_array)
        .ok_or_else(|| "embed: response has no 'embeddings' array".to_string())?;
    if arr.len() != expected {
        return Err(format!(
            "embed: response has {} vectors, expected {expected}",
            arr.len()
        ));
    }
    let mut out: Vec<Vec<f32>> = Vec::with_capacity(arr.len());
    let mut dims: Option<usize> = None;
    for (i, ev) in arr.iter().enumerate() {
        let raw = ev
            .as_array()
            .ok_or_else(|| format!("embed: vector {i} is not an array"))?;
        if raw.is_empty() || raw.len() > MAX_DIMS {
            return Err(format!(
                "embed: vector {i} has {} dims (must be 1..={MAX_DIMS})",
                raw.len()
            ));
        }
        match dims {
            None => dims = Some(raw.len()),
            Some(d) if d != raw.len() => {
                return Err(format!(
                    "embed: vector {i} has {} dims, inconsistent with {d}",
                    raw.len()
                ));
            }
            Some(_) => {}
        }
        let mut vec = Vec::with_capacity(raw.len());
        for x in raw {
            let f = x
                .as_f64()
                .ok_or_else(|| format!("embed: vector {i} has a non-numeric component"))?;
            if !f.is_finite() {
                return Err(format!("embed: vector {i} has a non-finite component"));
            }
            vec.push(f as f32);
        }
        out.push(vec);
    }
    Ok(out)
}

/// Fabrique des [`EmbeddingRecord`](super::embeddings::EmbeddingRecord)
/// append-ables à l'index à partir de `texts` et `vectors` **alignés** (même
/// longueur, même ordre). `text_digest` = SHA-256 hex du texte : il **identifie**
/// le texte embarqué sans le **stocker** (l'index pointe vers la mémoire, il ne la
/// duplique pas) ; `id` = `"<source>#<digest[..16]>"`, stable et déterministe.
/// Erreur si les longueurs divergent — on ne fabrique jamais un enregistrement
/// dont le vecteur ne correspond pas au texte.
pub(crate) fn build_records(
    source: &str,
    ts: u64,
    texts: &[String],
    vectors: &[Vec<f32>],
) -> Result<Vec<EmbeddingRecord>, String> {
    if texts.len() != vectors.len() {
        return Err(format!(
            "embed: {} texts vs {} vectors — cannot align",
            texts.len(),
            vectors.len()
        ));
    }
    let mut records = Vec::with_capacity(texts.len());
    for (t, v) in texts.iter().zip(vectors) {
        let digest = crate::sha256::sha256_hex(t.as_bytes());
        let id = format!("{source}#{}", &digest[..16]);
        records.push(EmbeddingRecord {
            id,
            ts,
            source: source.to_string(),
            text_digest: digest,
            vector: v.clone(),
        });
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_request_shapes_the_payload_and_bounds_it() {
        let req = build_request(
            "nomic-embed-text",
            &["hello".to_string(), "world".to_string()],
        )
        .expect("valid request");
        assert_eq!(req["model"], "nomic-embed-text");
        assert_eq!(req["input"][0], "hello");
        assert_eq!(req["input"][1], "world");
        // Fail-closed refusals.
        assert!(
            build_request("", &["x".to_string()]).is_err(),
            "empty model"
        );
        assert!(build_request("m", &[]).is_err(), "empty batch");
        assert!(build_request("m", &["".to_string()]).is_err(), "empty text");
        let too_many: Vec<String> = (0..=MAX_EMBED_BATCH).map(|i| format!("t{i}")).collect();
        assert!(build_request("m", &too_many).is_err(), "batch over cap");
        let too_long = "x".repeat(MAX_EMBED_TEXT_BYTES + 1);
        assert!(build_request("m", &[too_long]).is_err(), "text over cap");
    }

    #[test]
    fn parse_response_reads_valid_vectors() {
        let text = r#"{"model":"m","embeddings":[[0.1,0.2,0.3],[0.4,0.5,0.6]]}"#;
        let out = parse_response(text, 2).expect("valid");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], vec![0.1_f32, 0.2, 0.3]);
        assert_eq!(out[1].len(), 3);
    }

    #[test]
    fn parse_response_is_fail_closed() {
        // Wrong count.
        assert!(
            parse_response(r#"{"embeddings":[[1.0]]}"#, 2).is_err(),
            "count mismatch"
        );
        // Missing array.
        assert!(
            parse_response(r#"{"model":"m"}"#, 1).is_err(),
            "no embeddings"
        );
        // Not JSON.
        assert!(parse_response("not json", 1).is_err());
        // A vector that is not an array.
        assert!(
            parse_response(r#"{"embeddings":[42]}"#, 1).is_err(),
            "scalar vector"
        );
        // Empty vector.
        assert!(
            parse_response(r#"{"embeddings":[[]]}"#, 1).is_err(),
            "empty vector"
        );
        // Inconsistent dims across vectors (unusable for cosine).
        assert!(
            parse_response(r#"{"embeddings":[[1.0,2.0],[3.0]]}"#, 2).is_err(),
            "mixed dims refused"
        );
        // Non-finite / non-numeric components.
        assert!(
            parse_response(r#"{"embeddings":[[null]]}"#, 1).is_err(),
            "non-numeric"
        );
        assert!(
            parse_response(r#"{"embeddings":[["x"]]}"#, 1).is_err(),
            "string component"
        );
        // Over MAX_DIMS.
        let wide = format!(
            "{{\"embeddings\":[[{}]]}}",
            vec!["0.0"; MAX_DIMS + 1].join(",")
        );
        assert!(parse_response(&wide, 1).is_err(), "over MAX_DIMS");
    }

    #[test]
    fn build_records_aligns_text_and_vector_with_stable_digest() {
        let texts = vec![
            "le projet utilise pnpm".to_string(),
            "cible Fedora 44".to_string(),
        ];
        let vectors = vec![vec![0.1_f32, 0.2], vec![0.3_f32, 0.4]];
        let recs = build_records("knowledge", 1_783_000_000, &texts, &vectors).expect("aligned");
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].source, "knowledge");
        assert_eq!(recs[0].ts, 1_783_000_000);
        assert_eq!(recs[0].vector, vec![0.1_f32, 0.2]);
        // id derives from the digest and is stable + text-sensitive.
        assert!(recs[0].id.starts_with("knowledge#"));
        assert_ne!(recs[0].id, recs[1].id, "different texts -> different ids");
        // Same text -> same digest (deterministic).
        let again = build_records("knowledge", 1, &[texts[0].clone()], &[vectors[0].clone()])
            .expect("valid");
        assert_eq!(again[0].text_digest, recs[0].text_digest);
        // A record round-trips through the embeddings index format.
        let line = recs[0].to_json_line();
        assert_eq!(
            EmbeddingRecord::from_json_line(&line).as_ref(),
            Some(&recs[0])
        );
        // Length mismatch is refused (never a mispaired record).
        assert!(
            build_records("s", 1, &texts, &vectors[..1]).is_err(),
            "misaligned"
        );
    }

    #[test]
    fn produced_records_load_as_a_coherent_index() {
        // End-to-end (still pure): parse a response, build records, and confirm
        // they load as a valid embeddings index (same dims, dedup by id).
        let resp = r#"{"embeddings":[[1.0,0.0],[0.0,1.0]]}"#;
        let vectors = parse_response(resp, 2).unwrap();
        let texts = vec!["a".to_string(), "b".to_string()];
        let recs = build_records("knowledge", 5, &texts, &vectors).unwrap();
        let jsonl = recs
            .iter()
            .map(EmbeddingRecord::to_json_line)
            .collect::<Vec<_>>()
            .join("\n");
        let idx = super::super::embeddings::load_index(&jsonl).expect("loads");
        assert_eq!(idx.records.len(), 2);
        assert_eq!(idx.skipped, 0);
    }
}
