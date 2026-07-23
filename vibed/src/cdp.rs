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
//! Le NUL brut ne peut apparaître dans une chaîne JSON valide (RFC 8259 interdit les
//! contrôles bruts ; serde les échappe en `\u0000`), donc découper sur le premier NUL
//! est sûr : un NUL brut injecté par un pair hostile ne fait que produire des
//! fragments que serde rejette → erreur fail-closed, jamais une désynchronisation.
//!
//! Le contenu d'un message venant du navigateur est une **entrée hostile** (le pair
//! CDP parle au nom d'une page potentiellement piégée) :
//! - **décodage borné** (anti-OOM) — [`CdpCodec::push_bytes`] refuse tout ce qui
//!   ferait dépasser [`MAX_FRAME`] octets non consommés, ce qui borne à la fois une
//!   trame géante ET un pair qui inonde (y compris une rafale de NUL) sans que le
//!   transport draine ;
//! - **fail-closed** — une trame malformée, ou ni-réponse-ni-événement, est une
//!   erreur, jamais une supposition ; le transport DOIT **avorter la session** sur
//!   `Err` (la garantie fail-closed en dépend : après une erreur le curseur a déjà
//!   dépassé la trame fautive et se resynchronise sur la suivante, ce qui n'est sûr
//!   que si le transport ne « logue et continue » pas).
//!
//! **Invariant de corrélation, appliqué par [`CdpSession`]** au-dessus du codec (qui,
//! lui, rapporte l'`id` verbatim sans corréler — et les `id` émis sont séquentiels donc
//! prévisibles) : n'accepter une réponse que pour un `id` **actuellement en attente**,
//! ne l'apparier **qu'une fois**, et **jamais** accepter une réponse pour un `id` pas
//! encore émis (un pair hostile peut pré-envoyer une fausse réponse pour un `id` futur
//! prévisible). Le `result`/`params` renvoyés restent du JSON attaquant **opaque** :
//! tout consommateur en aval doit le traiter comme non fiable.

use serde_json::{json, Value};

/// Taille maximale des octets non consommés dans le tampon (une trame ne peut donc
/// pas dépasser cette taille). Une réponse CDP (snapshot DOM, capture base64…) peut
/// être grosse mais pas illimitée ; le transport borne aussi la capture globale.
/// 16 MiB laisse passer une capture d'écran raisonnable.
const MAX_FRAME: usize = 16 * 1024 * 1024;

/// Nombre max de trames VIDES (`\0` sans contenu) tolérées sur la vie du codec. Le
/// protocole CDP n'en émet jamais ; une rafale de `\0` est donc un flux hostile. Sans
/// ce plafond, les trames vides sont sautées sans jamais toucher ni le plafond
/// d'octets (le tampon draine) ni le compteur d'événements (elles n'en sont pas) →
/// boucle infinie côté session (Fable 5).
const MAX_EMPTY_FRAMES: usize = 4096;

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
    /// Curseur de consommation : `inbuf[pos..]` est la partie non encore consommée.
    /// Consommer avance le curseur (O(1)) au lieu de re-décaler le tampon à chaque
    /// trame ; le préfixe consommé est réclamé en amorti par [`Self::compact`].
    pos: usize,
    /// Total de trames vides sautées, borné par [`MAX_EMPTY_FRAMES`] (anti-flood).
    empty_skipped: usize,
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
            pos: 0,
            empty_skipped: 0,
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
        // serde_json ne peut échouer à sérialiser un Value déjà construit (clés
        // toujours String, ni NaN ni Inf, écriture vers un Vec infaillible). Un NUL
        // dans une chaîne serait émis en `\u0000`, jamais en octet brut.
        let mut bytes = serde_json::to_vec(&obj).expect("un Value CDP se sérialise");
        bytes.push(0); // terminateur de trame NUL
        (id, bytes)
    }

    /// Nombre d'octets non encore consommés.
    fn unconsumed(&self) -> usize {
        self.inbuf.len() - self.pos
    }

    /// Réclame le préfixe consommé quand ça vaut le coup (amorti O(1) : on ne
    /// memmove que si au moins la moitié du tampon est déjà consommée, donc le coût
    /// du décalage est imputable aux octets consommés).
    fn compact(&mut self) {
        if self.pos > 0 && self.pos >= self.unconsumed() {
            self.inbuf.drain(..self.pos);
            self.pos = 0;
        }
    }

    /// Ajoute des octets bruts lus du pipe. **Fail-closed** : refuse si le total non
    /// consommé dépasserait [`MAX_FRAME`] — ce qui borne à la fois une trame géante,
    /// une rafale de NUL, et un pair qui inonde plus vite que le transport ne draine.
    pub fn push_bytes(&mut self, data: &[u8]) -> Result<(), String> {
        self.compact();
        if self.unconsumed() + data.len() > MAX_FRAME {
            return Err(format!(
                "tampon CDP dépasserait {MAX_FRAME} octets non consommés — refusé \
                 (trame géante, rafale, ou transport qui ne draine pas)"
            ));
        }
        self.inbuf.extend_from_slice(data);
        Ok(())
    }

    /// Extrait le prochain message complet, ou `Ok(None)` si aucune trame entière
    /// n'est encore bufferisée. `Err` sur du JSON malformé (fail-closed — le
    /// transport avorte la session). La borne de taille est garantie en amont par
    /// [`Self::push_bytes`], donc toute trame ici mesure au plus [`MAX_FRAME`].
    pub fn next_message(&mut self) -> Result<Option<CdpMessage>, String> {
        loop {
            // Scanne UNIQUEMENT la partie non consommée depuis le curseur — pas
            // depuis 0 — pour éviter un coût super-linéaire sous cadrage adversarial.
            let rel = match self.inbuf[self.pos..].iter().position(|&b| b == 0) {
                Some(i) => i,
                None => return Ok(None),
            };
            let start = self.pos;
            let nul = start + rel;
            self.pos = nul + 1; // consomme la trame + son NUL (curseur, pas de drain)
            if rel == 0 {
                // Trame vide (`\0` sans contenu). Le protocole n'en émet pas ; on les
                // borne pour qu'un flux de `\0` ne boucle pas à l'infini côté session.
                self.empty_skipped += 1;
                if self.empty_skipped > MAX_EMPTY_FRAMES {
                    return Err(format!(
                        "trop de trames CDP vides (> {MAX_EMPTY_FRAMES}) — flux hostile, refusé"
                    ));
                }
                continue;
            }
            let v: Value = serde_json::from_slice(&self.inbuf[start..nul])
                .map_err(|e| format!("trame CDP malformée : {e}"))?;
            return parse_message(&v).map(Some);
        }
    }
}

/// Longueur max d'une chaîne issue du pair reprise dans un message d'erreur `vibed`.
const MAX_PEER_TEXT: usize = 512;

/// Assainit une chaîne **issue du pair CDP** (message d'erreur `error.message`, `errorText`…)
/// AVANT qu'elle ne rejoigne une chaîne d'erreur `vibed` que l'audit rend comme texte **de
/// confiance**. Un `chromium` hostile (modèle de menace explicite d'ADR-022) peut y glisser des
/// caractères de contrôle (ESC, CR/LF, BEL…) ou des marques bidi/format (RTL override U+202E,
/// isolats, ZWSP…) qui **corrompent ou spoofent la ligne d'audit** — or l'audit EST le mécanisme
/// de gouvernance sur un insider non fiable. On remplace tout Cc/Cf par `?` et on borne la
/// longueur (mêmes plages que `tools::browser::is_bidi_or_format` ; duplication assumée pour
/// éviter que `cdp`, couche basse, ne dépende de `tools`).
pub(crate) fn sanitize_peer_text(s: &str) -> String {
    let mut out: String = s
        .chars()
        .take(MAX_PEER_TEXT)
        .map(|c| {
            if c.is_control() || is_bidi_or_format(c) {
                '?'
            } else {
                c
            }
        })
        .collect();
    if s.chars().nth(MAX_PEER_TEXT).is_some() {
        out.push('…'); // tronqué : signale-le plutôt que de mentir sur la longueur
    }
    out
}

/// Marques bidi/format (catégorie Cf) qui spoofent le rendu texte.
fn is_bidi_or_format(c: char) -> bool {
    matches!(c,
        '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2060}'
        | '\u{2066}'..='\u{2069}'
        | '\u{FEFF}')
}

/// Classe un objet JSON CDP en réponse (a un `id`) ou événement (a un `method`).
/// L'`id` précède `method` : une trame hostile portant les deux est classée réponse.
/// Tout `id` non-`u64` (négatif, flottant, chaîne) échoue à `as_u64` → pas d'`id` →
/// on tente `method`, sinon erreur fail-closed.
fn parse_message(v: &Value) -> Result<CdpMessage, String> {
    if let Some(id) = v.get("id").and_then(Value::as_u64) {
        if let Some(err) = v.get("error") {
            // `message` vient du pair (hostile) et finit dans une chaîne d'erreur vibed → assaini.
            let message = sanitize_peer_text(
                err.get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("erreur CDP sans message"),
            );
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
    // La trame vient du PAIR (hostile) : `{v}` brut échappe les Cc mais PAS les bidi (U+202E…)
    // ni les C1, et n'est PAS borné (jusqu'à MAX_FRAME = 16 Mio). Repris tel quel dans une
    // erreur vibed, il spooferait la ligne d'audit ET gonflerait le body d'erreur. Assaini +
    // borné (Fable 5, résidu F3).
    Err(format!(
        "trame CDP ni réponse ni événement : {}",
        sanitize_peer_text(&v.to_string())
    ))
}

/// Un canal d'octets bidirectionnel vers le pair CDP. Le transport le branche sur les
/// descripteurs du pipe `--remote-debugging-pipe` ; les tests fournissent un faux pair
/// en mémoire.
///
/// **Contrat que l'implémentation sur fds DOIT respecter** (le pilote en dépend) :
/// - `send` écrit **intégralement** la trame (sémantique `write_all`) — jamais
///   d'écriture partielle silencieuse, qui désynchroniserait le pair ;
/// - `recv` est **bloquant** avec une **limite de temps** (le `RuntimeMaxSec` de l'unité
///   transitoire est le garde-fou ultime) et ne renvoie `Ok(vide)` que sur un **vrai
///   EOF** ; un `EAGAIN`/timeout doit être une `Err`, **jamais** `Ok(vide)` (que
///   [`CdpSession`] traite en fermeture fail-closed).
pub trait CdpChannel {
    /// Écrit intégralement une trame encodée (sémantique `write_all`).
    fn send(&mut self, bytes: &[u8]) -> std::io::Result<()>;
    /// Lit les octets disponibles ; `Ok(vec vide)` = **vrai** EOF, jamais un timeout.
    fn recv(&mut self) -> std::io::Result<Vec<u8>>;
}

/// Nombre max d'événements CDP tolérés entre l'émission d'une commande et sa réponse :
/// une page piégée peut inonder d'événements ; au-delà, on abandonne. Les trames vides
/// sont bornées séparément dans le codec ([`MAX_EMPTY_FRAMES`]).
const MAX_EVENTS_PER_CALL: usize = 1024;

/// Pilote une session CDP au-dessus d'un [`CdpChannel`], modèle **synchrone** (une
/// commande à la fois). Il **applique l'invariant de sécurité** que le codec seul ne peut
/// pas enforcer :
/// - **purge avant émission** : au début de chaque appel, aucune commande n'est en
///   attente, donc **aucune RÉPONSE ne doit exister** dans le flux ; une réponse trouvée
///   à ce moment est une trame **pré-staged** pour un `id` futur prévisible (un pair
///   hostile connaît la séquence d'`id`) → violation, on avorte. Seuls des **événements**
///   sont tolérés entre deux commandes (bornés) ;
/// - **corrélation stricte** : après émission de l'`id` N, on n'accepte que la réponse
///   d'`id` N arrivée **après** l'émission ; tout autre `id` → violation ;
/// - **abort-on-erreur (poison)** : toute erreur de **protocole/flux** (trame malformée,
///   `id` inattendu, flood, canal fermé, I/O) **empoisonne** la session — les appels
///   suivants sont refusés, le transport doit repartir d'une session neuve (le flux
///   restant peut contenir des trames pré-staged). Une **erreur CDP applicative**
///   (réponse `error` pour NOTRE `id`) **n'empoisonne pas** : la commande a échoué, pas
///   la session ; l'appelant peut continuer.
pub struct CdpSession<C: CdpChannel> {
    codec: CdpCodec,
    chan: C,
    poisoned: bool,
}

/// Distingue une erreur CDP **applicative** (session saine) d'une erreur de
/// **protocole/flux** (session cassée → à empoisonner).
enum CallErr {
    /// Réponse `error` du pair pour NOTRE commande : la commande a échoué, pas la session.
    Cdp(String),
    /// Trame malformée, `id` inattendu, flood, canal fermé, I/O : le flux est cassé.
    Proto(String),
}

impl<C: CdpChannel> CdpSession<C> {
    pub fn new(chan: C) -> Self {
        Self {
            codec: CdpCodec::new(),
            chan,
            poisoned: false,
        }
    }

    /// Consomme la session et rend le canal (fermeture par le transport).
    pub fn into_channel(self) -> C {
        self.chan
    }

    /// La session est-elle **empoisonnée** (une erreur de PROTOCOLE/flux l'a avortée) ? Permet à
    /// l'appelant de distinguer, après un [`Self::call`] en `Err`, une erreur **protocole** (la
    /// commande est PARTIE mais le flux s'est cassé — poison) d'une erreur **applicative CDP** (le
    /// pair a répondu `error` pour NOTRE id — pas de poison). Critique pour `browser.submit` : un
    /// `callFunctionOn` de submit qui échoue APRÈS émission (poison) = POST peut-être parti =
    /// `Indeterminate`, jamais `Failed` (sinon double-POST au retry — Fable 5).
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Émet `method(params)` (optionnellement ciblé sur `session_id`) et renvoie le
    /// `result` de la réponse corrélée. Voir l'invariant sur [`CdpSession`].
    pub fn call(
        &mut self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<Value, String> {
        if self.poisoned {
            return Err("session CDP avortée par une erreur de protocole antérieure".to_string());
        }
        match self.call_inner(method, params, session_id) {
            Ok(v) => Ok(v),
            // Échec applicatif : la session reste utilisable.
            Err(CallErr::Cdp(m)) => Err(format!("erreur CDP : {m}")),
            // Flux cassé : on empoisonne (le transport repartira d'une session neuve).
            Err(CallErr::Proto(m)) => {
                self.poisoned = true;
                Err(m)
            }
        }
    }

    fn call_inner(
        &mut self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<Value, CallErr> {
        let mut events = 0usize;
        // 1) Purge le tampon AVANT d'émettre : aucune commande en attente ⇒ toute réponse
        //    déjà présente est un pré-stage hostile (id futur prévisible).
        loop {
            match self.codec.next_message().map_err(CallErr::Proto)? {
                Some(CdpMessage::Result { id, .. }) | Some(CdpMessage::Error { id, .. }) => {
                    return Err(CallErr::Proto(format!(
                        "réponse CDP (id {id}) sans commande en attente — pré-stage hostile, refusé"
                    )));
                }
                Some(CdpMessage::Event { .. }) => bump_events(&mut events)?,
                None => break,
            }
        }
        // 2) Émet la commande.
        let (id, bytes) = self.codec.encode(method, params, session_id);
        self.chan
            .send(&bytes)
            .map_err(|e| CallErr::Proto(format!("CDP send : {e}")))?;
        // 3) Attend SA réponse, arrivée APRÈS l'émission ; tout autre id → violation.
        loop {
            match self.codec.next_message().map_err(CallErr::Proto)? {
                Some(CdpMessage::Result { id: rid, result }) => {
                    if rid != id {
                        return Err(CallErr::Proto(format!(
                            "réponse CDP pour un id inattendu ({rid}, attendu {id})"
                        )));
                    }
                    return Ok(result);
                }
                Some(CdpMessage::Error { id: rid, message }) => {
                    if rid != id {
                        return Err(CallErr::Proto(format!(
                            "erreur CDP pour un id inattendu ({rid}, attendu {id})"
                        )));
                    }
                    return Err(CallErr::Cdp(message));
                }
                Some(CdpMessage::Event { .. }) => bump_events(&mut events)?,
                None => {
                    let more = self
                        .chan
                        .recv()
                        .map_err(|e| CallErr::Proto(format!("CDP recv : {e}")))?;
                    if more.is_empty() {
                        return Err(CallErr::Proto(
                            "canal CDP fermé avant la réponse".to_string(),
                        ));
                    }
                    self.codec.push_bytes(&more).map_err(CallErr::Proto)?;
                }
            }
        }
    }
}

/// Incrémente le compteur d'événements et échoue (protocole) au-delà de la borne.
fn bump_events(events: &mut usize) -> Result<(), CallErr> {
    *events += 1;
    if *events > MAX_EVENTS_PER_CALL {
        return Err(CallErr::Proto(
            "trop d'événements CDP sans réponse — session abandonnée".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Faux pair CDP en mémoire : enregistre les commandes émises, restitue des
    /// morceaux d'octets pré-chargés (un `recv` vide = EOF).
    struct FakeChan {
        sent: Vec<Vec<u8>>,
        inbox: std::collections::VecDeque<Vec<u8>>,
    }
    impl FakeChan {
        fn new(chunks: Vec<Vec<u8>>) -> Self {
            Self {
                sent: Vec::new(),
                inbox: chunks.into(),
            }
        }
    }
    impl CdpChannel for FakeChan {
        fn send(&mut self, bytes: &[u8]) -> std::io::Result<()> {
            self.sent.push(bytes.to_vec());
            Ok(())
        }
        fn recv(&mut self) -> std::io::Result<Vec<u8>> {
            Ok(self.inbox.pop_front().unwrap_or_default())
        }
    }

    #[test]
    fn call_sends_a_command_and_returns_the_correlated_result() {
        let chan = FakeChan::new(vec![frame(json!({"id": 1, "result": {"ok": true}}))]);
        let mut s = CdpSession::new(chan);
        let r = s
            .call("Page.navigate", json!({"url": "https://x"}), None)
            .unwrap();
        assert_eq!(r, json!({"ok": true}));
        // La commande émise porte l'id 1 et le bon method.
        let sent = &s.into_channel().sent[0];
        let v: Value = serde_json::from_slice(&sent[..sent.len() - 1]).unwrap();
        assert_eq!(v["id"], 1);
        assert_eq!(v["method"], "Page.navigate");
    }

    #[test]
    fn a_cdp_error_response_is_propagated() {
        let chan = FakeChan::new(vec![frame(json!({"id": 1, "error": {"message": "boom"}}))]);
        let mut s = CdpSession::new(chan);
        assert!(s.call("X", json!({}), None).unwrap_err().contains("boom"));
    }

    #[test]
    fn a_response_with_the_wrong_id_is_refused() {
        // La réponse arrive après notre commande (id 1) mais porte un autre id (99).
        let chan = FakeChan::new(vec![frame(json!({"id": 99, "result": {}}))]);
        let mut s = CdpSession::new(chan);
        assert!(s
            .call("X", json!({}), None)
            .unwrap_err()
            .contains("inattendu"));
    }

    #[test]
    fn a_pre_staged_future_id_response_is_caught_and_poisons_the_session() {
        // Le pair répond à la commande 1 ET pré-stage une réponse pour l'id 2
        // (séquence prévisible) dans le MÊME morceau — donc bufferisée avant la
        // commande 2.
        let mut chunk = frame(json!({"id": 1, "result": {"ok": 1}}));
        chunk.extend_from_slice(&frame(json!({"id": 2, "result": {"forged": true}})));
        let chan = FakeChan::new(vec![chunk]);
        let mut s = CdpSession::new(chan);
        // Appel 1 : OK (sa réponse est consommée, le pré-stage id=2 reste bufferisé).
        assert_eq!(s.call("A", json!({}), None).unwrap(), json!({"ok": 1}));
        // Appel 2 : la purge d'avant-émission trouve la réponse id=2 sans commande en
        // attente → pré-stage détecté, refusé.
        assert!(s
            .call("B", json!({}), None)
            .unwrap_err()
            .contains("pré-stage"));
        // La session est empoisonnée : tout appel ultérieur est refusé d'emblée.
        assert!(s
            .call("C", json!({}), None)
            .unwrap_err()
            .contains("avortée"));
    }

    #[test]
    fn a_cdp_application_error_does_not_poison_the_session() {
        // Erreur CDP applicative sur l'appel 1 (réponse `error` pour NOTRE id), puis un
        // appel 2 légitime (sa réponse arrive APRÈS son émission, morceau séparé).
        let chan = FakeChan::new(vec![
            frame(json!({"id": 1, "error": {"message": "no such node"}})),
            frame(json!({"id": 2, "result": {"ok": 1}})),
        ]);
        let mut s = CdpSession::new(chan);
        assert!(s
            .call("A", json!({}), None)
            .unwrap_err()
            .contains("no such node"));
        // Pas empoisonnée : l'appel 2 réussit.
        assert_eq!(s.call("B", json!({}), None).unwrap(), json!({"ok": 1}));
    }

    #[test]
    fn an_empty_frame_flood_is_bounded_not_looped_forever() {
        // Un pair qui n'envoie que des `\0` : borné par le codec (MAX_EMPTY_FRAMES),
        // pas de boucle infinie — c'était le DoS relevé par Fable 5.
        let chunks: Vec<Vec<u8>> = (0..10).map(|_| vec![0u8; 1000]).collect(); // 10000 > 4096
        let chan = FakeChan::new(chunks);
        let mut s = CdpSession::new(chan);
        assert!(s.call("A", json!({}), None).unwrap_err().contains("vides"));
    }

    #[test]
    fn events_before_the_response_are_skipped() {
        let chan = FakeChan::new(vec![
            frame(json!({"method": "Page.frameStartedLoading", "params": {}})),
            frame(json!({"method": "Network.requestWillBeSent", "params": {}})),
            frame(json!({"id": 1, "result": {"done": 1}})),
        ]);
        let mut s = CdpSession::new(chan);
        assert_eq!(s.call("X", json!({}), None).unwrap(), json!({"done": 1}));
    }

    #[test]
    fn a_closed_channel_before_the_response_fails_closed() {
        let chan = FakeChan::new(vec![]); // recv renvoie vide = EOF
        let mut s = CdpSession::new(chan);
        assert!(s.call("X", json!({}), None).unwrap_err().contains("fermé"));
    }

    #[test]
    fn an_event_flood_without_a_response_is_aborted() {
        let chunks: Vec<Vec<u8>> = (0..=MAX_EVENTS_PER_CALL)
            .map(|_| frame(json!({"method": "E", "params": {}})))
            .collect();
        let chan = FakeChan::new(chunks);
        let mut s = CdpSession::new(chan);
        assert!(s
            .call("X", json!({}), None)
            .unwrap_err()
            .contains("trop d'événements"));
    }

    #[test]
    fn a_response_reassembled_across_recvs_correlates() {
        let f = frame(json!({"id": 1, "result": {"x": 1}}));
        let (a, b) = f.split_at(4);
        let chan = FakeChan::new(vec![a.to_vec(), b.to_vec()]);
        let mut s = CdpSession::new(chan);
        assert_eq!(s.call("X", json!({}), None).unwrap(), json!({"x": 1}));
    }

    #[test]
    fn encode_allocates_incrementing_ids_and_nul_terminates() {
        let mut c = CdpCodec::new();
        let (id1, b1) = c.encode("Page.navigate", json!({"url": "https://x"}), None);
        let (id2, _b2) = c.encode("Page.captureScreenshot", json!({}), Some("SID"));
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(*b1.last().unwrap(), 0, "trame terminée par NUL");
        let v1: Value = serde_json::from_slice(&b1[..b1.len() - 1]).unwrap();
        assert_eq!(v1["id"], 1);
        assert_eq!(v1["method"], "Page.navigate");
        assert_eq!(v1["params"]["url"], "https://x");
        assert!(v1.get("sessionId").is_none());
        let (_id, b2) = c.encode("X", json!({}), Some("SID"));
        let v2: Value = serde_json::from_slice(&b2[..b2.len() - 1]).unwrap();
        assert_eq!(v2["sessionId"], "SID");
    }

    #[test]
    fn a_string_param_with_a_nul_is_escaped_never_raw() {
        // Le NUL dans une chaîne est émis en `\u0000` (6 octets), pas en octet brut :
        // le SEUL 0x00 de la sortie est le terminateur final. Sinon le cadrage du pair
        // serait cassé.
        let mut c = CdpCodec::new();
        let (_id, bytes) = c.encode("X", json!({"v": "a\u{0}b"}), None);
        assert_eq!(
            bytes.iter().filter(|&&b| b == 0).count(),
            1,
            "un seul NUL: le terminateur"
        );
    }

    fn frame(v: Value) -> Vec<u8> {
        let mut b = serde_json::to_vec(&v).unwrap();
        b.push(0);
        b
    }

    #[test]
    fn decodes_result_error_and_event() {
        let mut c = CdpCodec::new();
        c.push_bytes(&frame(json!({"id": 7, "result": {"frameId": "F"}})))
            .unwrap();
        c.push_bytes(&frame(
            json!({"id": 8, "error": {"code": -32000, "message": "boom"}}),
        ))
        .unwrap();
        c.push_bytes(&frame(
            json!({"method": "Page.loadEventFired", "params": {"t": 1}}),
        ))
        .unwrap();
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
    fn sanitize_peer_text_strips_control_and_bidi_and_bounds() {
        // Cc (ESC/CR/LF/BEL) → '?' ; texte propre (accents) inchangé.
        assert_eq!(sanitize_peer_text("a\u{1b}b\r\n\u{07}c"), "a?b???c");
        assert_eq!(sanitize_peer_text("x\u{202e}y\u{200b}z"), "x?y?z"); // RLO + ZWSP
        assert_eq!(sanitize_peer_text("échec réseau"), "échec réseau");
        // Borné : au-delà de MAX_PEER_TEXT, tronqué + marqueur '…'.
        let out = sanitize_peer_text(&"z".repeat(MAX_PEER_TEXT + 50));
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), MAX_PEER_TEXT + 1);
    }

    #[test]
    fn parse_message_sanitizes_a_hostile_error_message() {
        // Un chromium hostile glisse des ctrl/bidi dans error.message ; parsé, il est assaini
        // AVANT de rejoindre une erreur vibed que l'audit affiche.
        let mut c = CdpCodec::new();
        c.push_bytes(&frame(
            json!({"id": 1, "error": {"message": "boom\u{1b}[2K\u{202e}evil"}}),
        ))
        .unwrap();
        match c.next_message().unwrap().unwrap() {
            CdpMessage::Error { id, message } => {
                assert_eq!(id, 1);
                assert!(!message.contains('\u{1b}'), "ESC retiré : {message:?}");
                assert!(!message.contains('\u{202e}'), "RTL override retiré");
                assert!(message.starts_with("boom"));
            }
            other => panic!("attendu Error, obtenu {other:?}"),
        }
    }

    #[test]
    fn reassembles_a_frame_split_across_pushes() {
        let mut c = CdpCodec::new();
        let f = frame(json!({"id": 1, "result": {}}));
        let (head, tail) = f.split_at(5);
        c.push_bytes(head).unwrap();
        assert_eq!(c.next_message().unwrap(), None, "trame incomplète => None");
        c.push_bytes(tail).unwrap();
        assert_eq!(
            c.next_message().unwrap(),
            Some(CdpMessage::Result {
                id: 1,
                result: json!({})
            })
        );
    }

    #[test]
    fn many_small_frames_decode_in_order_exercising_the_cursor() {
        // Exerce le curseur + la compaction amortie sur un flux de nombreuses trames.
        let mut c = CdpCodec::new();
        for i in 0..500u64 {
            c.push_bytes(&frame(json!({"id": i, "result": {"n": i}})))
                .unwrap();
        }
        for i in 0..500u64 {
            assert_eq!(
                c.next_message().unwrap(),
                Some(CdpMessage::Result {
                    id: i,
                    result: json!({"n": i})
                })
            );
        }
        assert_eq!(c.next_message().unwrap(), None);
    }

    #[test]
    fn empty_frames_are_skipped() {
        let mut c = CdpCodec::new();
        c.push_bytes(b"\0\0").unwrap();
        c.push_bytes(&frame(json!({"id": 3, "result": {}})))
            .unwrap();
        assert_eq!(
            c.next_message().unwrap(),
            Some(CdpMessage::Result {
                id: 3,
                result: json!({})
            })
        );
    }

    #[test]
    fn a_raw_nul_inside_a_string_fails_closed_never_desyncs() {
        // Un pair hostile insère un NUL brut au milieu d'une "chaîne" : le découpage
        // sur le premier NUL donne un fragment que serde rejette → erreur, pas de
        // désync silencieuse.
        let mut c = CdpCodec::new();
        c.push_bytes(b"{\"id\":1,\"result\":\"a\0b\"}\0").unwrap();
        assert!(c.next_message().unwrap_err().contains("malform"));
    }

    #[test]
    fn malformed_json_is_a_fail_closed_error() {
        let mut c = CdpCodec::new();
        c.push_bytes(b"{not json}\0").unwrap();
        assert!(c.next_message().unwrap_err().contains("malform"));
    }

    #[test]
    fn a_frame_that_is_neither_response_nor_event_is_refused() {
        let mut c = CdpCodec::new();
        c.push_bytes(&frame(json!({"foo": "bar"}))).unwrap();
        assert!(c
            .next_message()
            .unwrap_err()
            .contains("ni réponse ni événement"));
    }

    #[test]
    fn a_hostile_neither_frame_error_is_sanitized_and_bounded() {
        // (F3 résidu, Fable 5) Une trame « ni réponse ni événement » d'un chromium hostile :
        // son contenu (bidi + très long) rejoignait l'erreur vibed BRUT → spoof d'audit + body
        // d'erreur géant (jusqu'à MAX_FRAME) poussé sans cap par le bras Err de run_batch. Doit
        // être assaini ET borné.
        let mut c = CdpCodec::new();
        let evil = format!("\u{202e}{}", "A".repeat(MAX_PEER_TEXT + 5000));
        c.push_bytes(&frame(json!({ "foo": evil }))).unwrap();
        let err = c.next_message().unwrap_err();
        assert!(err.contains("ni réponse ni événement"));
        assert!(!err.contains('\u{202e}'), "RTL override retiré : {err:?}");
        assert!(
            err.chars().count() < MAX_PEER_TEXT + 100,
            "l'extrait pair est borné, pas la trame entière : {} chars",
            err.chars().count()
        );
    }

    #[test]
    fn a_non_u64_id_fails_closed() {
        let mut c = CdpCodec::new();
        // id flottant / négatif / chaîne : pas de as_u64, pas de method => refus.
        for bad in [json!({"id": 2.5}), json!({"id": -1}), json!({"id": "x"})] {
            let mut cc = CdpCodec::new();
            cc.push_bytes(&frame(bad)).unwrap();
            assert!(cc.next_message().is_err());
        }
        c.push_bytes(&frame(json!({"id": 5, "result": {}})))
            .unwrap();
        assert!(matches!(
            c.next_message().unwrap(),
            Some(CdpMessage::Result { id: 5, .. })
        ));
    }

    #[test]
    fn push_over_the_bound_is_refused_including_a_nul_run() {
        // Une seule poussée trop grosse : refusée (borne la trame géante).
        let mut c = CdpCodec::new();
        assert!(c.push_bytes(&vec![b'x'; MAX_FRAME + 1]).is_err());
        // Une rafale de NUL trop grosse : refusée aussi (ne contourne plus la borne).
        let mut c2 = CdpCodec::new();
        assert!(c2.push_bytes(&vec![0u8; MAX_FRAME + 1]).is_err());
    }
}
