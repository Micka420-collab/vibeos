//! Codec **pur** CDP-sur-pipe (Chrome DevTools Protocol, cadrage JSON du mode
//! `--remote-debugging-pipe`).
//!
//! **Aucune I/O** : le transport (`run_browser`, incrément ultérieur) possède les
//! descripteurs et le process ; ce module ne possède que la correspondance
//! octets ↔ messages et l'allocation des `id`. Il est donc entièrement testable
//! **sans chromium**, ce qui isole la partie où se cachent les bugs de cadrage.
//!
//! Format de fil (mode JSON, le défaut du pipe) : chaque message est un objet JSON
//! suivi d'**un octet NUL** (`\0`). Une *commande* porte un `id` numérique et un
//! `method` (+ `params`/`sessionId` optionnels) ; une *réponse* renvoie le même `id`
//! (avec `result` ou `error`) ; un *événement* porte un `method` et **pas** d'`id`.
//!
//! Le contenu d'un message venant du navigateur est une **entrée hostile** (le pair
//! CDP parle au nom d'une page potentiellement piégée) : le décodage est **borné**
//! (anti-OOM) et **fail-closed** (une trame malformée ou trop longue est une erreur,
//! jamais une supposition).

use serde_json::{json, Value};

/// Taille maximale d'une trame décodée. Une réponse CDP (snapshot DOM, capture
/// base64…) peut être grosse mais pas illimitée ; le transport borne aussi la
/// capture globale. 16 MiB laisse passer une capture d'écran raisonnable.
const MAX_FRAME: usize = 16 * 1024 * 1024;

/// Un message CDP décodé depuis le pair.
#[derive(Debug, Clone, PartialEq)]
pub enum CdpMessage {
    /// Réponse réussie à la commande portant cet `id`.
    Result { id: u64, result: Value },
    /// Réponse d'erreur à la commande portant cet `id`.
    Error { id: u64, message: String },
    /// Événement non sollicité (pas d'`id`).
    Event { method: String, params: Value },
}

/// Encodeur/décodeur avec état pour le pipe JSON CDP. Sans I/O : le transport fournit
/// les octets lus et écrit les octets encodés.
#[derive(Debug)]
pub struct CdpCodec {
    next_id: u64,
    inbuf: Vec<u8>,
}

impl Default for CdpCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl CdpCodec {
    pub fn new() -> Self {
        // Les `id` commencent à 1 : `0` est réservé comme sentinelle « aucune
        // commande » côté transport.
        Self {
            next_id: 1,
            inbuf: Vec::new(),
        }
    }

    /// Construit une trame de commande. Renvoie l'`id` alloué (pour corréler la
    /// réponse) et les octets de fil (JSON + NUL final). `session_id` cible une
    /// page/session (protocole « plat ») ; `None` pour une commande au niveau
    /// navigateur.
    pub fn encode(
        &mut self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> (u64, Vec<u8>) {
        let id = self.next_id;
        self.next_id += 1;
        let mut obj = json!({ "id": id, "method": method, "params": params });
        if let Some(sid) = session_id {
            obj["sessionId"] = Value::String(sid.to_string());
        }
        // serde_json ne peut échouer à sérialiser un Value déjà construit.
        let mut bytes = serde_json::to_vec(&obj).expect("un Value CDP se sérialise");
        bytes.push(0); // terminateur de trame NUL
        (id, bytes)
    }

    /// Ajoute des octets bruts lus depuis le pipe.
    pub fn push_bytes(&mut self, data: &[u8]) {
        self.inbuf.extend_from_slice(data);
    }

    /// Extrait le prochain message complet, ou `Ok(None)` si aucune trame entière
    /// n'est encore bufferisée. `Err` sur une trame trop longue ou du JSON malformé
    /// (fail-closed — le transport avorte la session).
    pub fn next_message(&mut self) -> Result<Option<CdpMessage>, String> {
        loop {
            let nul = match self.inbuf.iter().position(|&b| b == 0) {
                Some(i) => i,
                None => {
                    // Pas de trame complète ; refuse un buffer qui gonfle sans fin.
                    if self.inbuf.len() > MAX_FRAME {
                        return Err(format!(
                            "trame CDP dépasse {MAX_FRAME} octets sans terminateur — refusé"
                        ));
                    }
                    return Ok(None);
                }
            };
            if nul > MAX_FRAME {
                return Err(format!("trame CDP dépasse {MAX_FRAME} octets — refusé"));
            }
            // Détache la trame [0, nul] (NUL inclus) du buffer.
            let frame: Vec<u8> = self.inbuf.drain(..=nul).collect();
            let json_bytes = &frame[..frame.len() - 1]; // sans le NUL final
            if json_bytes.is_empty() {
                // Trame vide (`\0` sans contenu) : ignore, passe à la suivante.
                continue;
            }
            let v: Value = serde_json::from_slice(json_bytes)
                .map_err(|e| format!("trame CDP malformée : {e}"))?;
            return parse_message(&v).map(Some);
        }
    }
}

/// Classe un objet JSON CDP en réponse (a un `id`) ou événement (a un `method`).
fn parse_message(v: &Value) -> Result<CdpMessage, String> {
    if let Some(id) = v.get("id").and_then(Value::as_u64) {
        if let Some(err) = v.get("error") {
            let message = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("erreur CDP sans message")
                .to_string();
            return Ok(CdpMessage::Error { id, message });
        }
        let result = v.get("result").cloned().unwrap_or(Value::Null);
        return Ok(CdpMessage::Result { id, result });
    }
    if let Some(method) = v.get("method").and_then(Value::as_str) {
        let params = v.get("params").cloned().unwrap_or(Value::Null);
        return Ok(CdpMessage::Event {
            method: method.to_string(),
            params,
        });
    }
    Err(format!("trame CDP ni réponse ni événement : {v}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_allocates_incrementing_ids_and_nul_terminates() {
        let mut c = CdpCodec::new();
        let (id1, b1) = c.encode("Page.navigate", json!({"url": "https://x"}), None);
        let (id2, _b2) = c.encode("Page.captureScreenshot", json!({}), Some("SID"));
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(*b1.last().unwrap(), 0, "trame terminée par NUL");
        // Le JSON encodé porte id/method/params, et sessionId quand fourni.
        let v1: Value = serde_json::from_slice(&b1[..b1.len() - 1]).unwrap();
        assert_eq!(v1["id"], 1);
        assert_eq!(v1["method"], "Page.navigate");
        assert_eq!(v1["params"]["url"], "https://x");
        assert!(v1.get("sessionId").is_none());
        let (_id, b2) = c.encode("X", json!({}), Some("SID"));
        let v2: Value = serde_json::from_slice(&b2[..b2.len() - 1]).unwrap();
        assert_eq!(v2["sessionId"], "SID");
    }

    fn frame(v: Value) -> Vec<u8> {
        let mut b = serde_json::to_vec(&v).unwrap();
        b.push(0);
        b
    }

    #[test]
    fn decodes_result_error_and_event() {
        let mut c = CdpCodec::new();
        c.push_bytes(&frame(json!({"id": 7, "result": {"frameId": "F"}})));
        c.push_bytes(&frame(
            json!({"id": 8, "error": {"code": -32000, "message": "boom"}}),
        ));
        c.push_bytes(&frame(
            json!({"method": "Page.loadEventFired", "params": {"t": 1}}),
        ));
        assert_eq!(
            c.next_message().unwrap(),
            Some(CdpMessage::Result {
                id: 7,
                result: json!({"frameId": "F"})
            })
        );
        assert_eq!(
            c.next_message().unwrap(),
            Some(CdpMessage::Error {
                id: 8,
                message: "boom".to_string()
            })
        );
        assert_eq!(
            c.next_message().unwrap(),
            Some(CdpMessage::Event {
                method: "Page.loadEventFired".to_string(),
                params: json!({"t": 1})
            })
        );
        assert_eq!(c.next_message().unwrap(), None, "buffer vidé");
    }

    #[test]
    fn reassembles_a_frame_split_across_pushes() {
        let mut c = CdpCodec::new();
        let f = frame(json!({"id": 1, "result": {}}));
        let (head, tail) = f.split_at(5);
        c.push_bytes(head);
        assert_eq!(c.next_message().unwrap(), None, "trame incomplète => None");
        c.push_bytes(tail);
        assert_eq!(
            c.next_message().unwrap(),
            Some(CdpMessage::Result {
                id: 1,
                result: json!({})
            })
        );
    }

    #[test]
    fn two_frames_in_one_push_both_decode() {
        let mut c = CdpCodec::new();
        let mut both = frame(json!({"id": 1, "result": {}}));
        both.extend_from_slice(&frame(json!({"method": "E", "params": {}})));
        c.push_bytes(&both);
        assert!(matches!(
            c.next_message().unwrap(),
            Some(CdpMessage::Result { id: 1, .. })
        ));
        assert!(matches!(
            c.next_message().unwrap(),
            Some(CdpMessage::Event { .. })
        ));
        assert_eq!(c.next_message().unwrap(), None);
    }

    #[test]
    fn empty_frames_are_skipped() {
        let mut c = CdpCodec::new();
        c.push_bytes(b"\0\0");
        c.push_bytes(&frame(json!({"id": 3, "result": {}})));
        assert_eq!(
            c.next_message().unwrap(),
            Some(CdpMessage::Result {
                id: 3,
                result: json!({})
            })
        );
    }

    #[test]
    fn malformed_json_is_a_fail_closed_error() {
        let mut c = CdpCodec::new();
        c.push_bytes(b"{not json}\0");
        assert!(c.next_message().unwrap_err().contains("malform"));
    }

    #[test]
    fn a_frame_that_is_neither_response_nor_event_is_refused() {
        let mut c = CdpCodec::new();
        c.push_bytes(&frame(json!({"foo": "bar"})));
        assert!(c
            .next_message()
            .unwrap_err()
            .contains("ni réponse ni événement"));
    }

    #[test]
    fn an_unterminated_oversize_buffer_is_refused() {
        let mut c = CdpCodec::new();
        // Pousse plus que MAX_FRAME sans NUL : doit être refusé, pas bufferisé sans fin.
        c.push_bytes(&vec![b'x'; MAX_FRAME + 1]);
        assert!(c.next_message().unwrap_err().contains("sans terminateur"));
    }
}
