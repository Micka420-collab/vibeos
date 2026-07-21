//! Proxy CONNECT du navigateur (ADR-022) — le **seul** chemin d'egress de `chromium`.
//!
//! Le plancher réseau du sandbox (`IPAddressDeny=any` + seul `127.66.0.1/32`) fait que
//! `chromium` ne peut atteindre **que** ce proxy ; tout son trafic passe par des requêtes
//! HTTP **CONNECT**. Le proxy évalue `[rule.domains]` par requête et **mappe l'allowlist de
//! domaines sur un egress par-IP** — c'est la correction du « `IPAddressAllow` est par-adresse,
//! pas par-domaine » d'ADR-017, et l'endroit où l'enforcement domaine VIT réellement (le check
//! au niveau `navigate` dans `vibed` n'est qu'un lint : un `click` ou une redirection 302
//! changent de domaine sans repasser par lui — Fable 5).
//!
//! **Ce module est PUR** (sans réseau, testable isolément) : il **parse** la ligne de requête
//! CONNECT d'un client **hostile** ([`parse_connect_target`]) et rend le **verdict**
//! `[rule.domains]` par requête ([`connect_decision`] → tunnel / refus). Le **relais**
//! bidirectionnel (I/O), la **forme** du proxy (processus dédié vs thread du helper ; netns) et
//! **d'où vient l'allowlist approuvée** sont des incréments ultérieurs (décision de forme « à
//! trancher » d'ADR-022) — l'allowlist est prise en paramètre ici, form-agnostique.
//!
//! **Contrat CONNECT** (RFC 9110 §9.3.6) : la ligne de requête est `CONNECT authority HTTP/1.1`
//! où `authority = host:port`, le **port est obligatoire**, et il n'y a **jamais** de schéma
//! ni de chemin. Tout écart d'un `chromium` hostile est **fail-closed**.

/// Longueur maximale de la ligne de requête CONNECT (anti-DoS ; une autorité DNS légitime
/// tient largement dessous — un FQDN fait au plus 253 octets).
const MAX_REQUEST_LINE: usize = 512;

/// Parse la **ligne de requête** HTTP CONNECT d'un client (`chromium`) et rend `(host, port)`.
/// L'entrée est **hostile** : fail-closed sur tout écart (méthode ≠ CONNECT, autorité absente,
/// port absent/invalide, ligne trop longue, non-UTF8, tokens en trop, host non canonique).
///
/// **Le host passe par [`crate::domain::is_valid_host`]** — la MÊME validation que `host_of`,
/// donc la **précondition de `domain_match`** (Fable 5) : source de vérité UNIQUE, sinon le proxy
/// et le lint `navigate` valideraient différemment le même domaine sur un couple de gardes egress.
/// Sont donc refusés (en plus du charset `[a-z0-9-]`) : point de tête/queue (`example.com.`),
/// label vide/> 63, tiret de bord, host > 253, non-ASCII, IPv6 `[::1]`, userinfo `@`. Le host est
/// rendu en **minuscules ASCII** (forme canonique pour le matching). Les **littéraux IPv4** (que
/// `is_valid_host` accepte structurellement) sont refusés en plus (modèle par-DOMAINE) ; `chromium`
/// derrière un proxy envoie `CONNECT domaine:443`, c'est le proxy qui résout le DNS.
pub fn parse_connect_target(request: &[u8]) -> Result<(String, u16), String> {
    // On ne lit QUE la première ligne (jusqu'au premier CRLF) — les en-têtes ne nous concernent
    // pas pour la cible.
    // Cherche le CRLF UNIQUEMENT dans les premiers `MAX_REQUEST_LINE + 2` octets — sinon un
    // `chromium` hostile qui n'envoie jamais de CRLF ferait scanner tout le tampon (DoS de scan,
    // Fable 5 F3). Auto-défensif quelle que soit la taille passée par le relais ; borne aussi la
    // ligne à `MAX_REQUEST_LINE`.
    let window = request.get(..MAX_REQUEST_LINE + 2).unwrap_or(request);
    let crlf = window
        .windows(2)
        .position(|w| w == b"\r\n")
        .ok_or_else(|| {
            format!(
                "proxy: CRLF absent dans les premiers {} octets — refusé",
                MAX_REQUEST_LINE + 2
            )
        })?;
    let line = std::str::from_utf8(&request[..crlf])
        .map_err(|_| "proxy: ligne CONNECT non-UTF8 — refusé".to_string())?;

    // "CONNECT <authority> HTTP/1.x" — exactement 3 tokens séparés par un seul espace.
    let mut parts = line.split(' ');
    let method = parts.next().unwrap_or("");
    if method != "CONNECT" {
        return Err(format!("proxy: méthode {method:?} != CONNECT — refusé"));
    }
    let authority = parts
        .next()
        .filter(|a| !a.is_empty())
        .ok_or_else(|| "proxy: CONNECT sans autorité — refusé".to_string())?;
    let version = parts.next().unwrap_or("");
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return Err(format!("proxy: version {version:?} inattendue — refusé"));
    }
    if parts.next().is_some() {
        return Err("proxy: ligne CONNECT malformée (token en trop) — refusé".to_string());
    }

    // authority = host:port ; le port est OBLIGATOIRE en CONNECT. `rsplit_once` sépare sur le
    // DERNIER ':' — mais un host de type domaine n'en contient pas, donc pour du non-IPv6 c'est
    // sans ambiguïté (les littéraux IPv6 `[::1]:443` sont de toute façon refusés par le charset).
    let (host, port_str) = authority
        .rsplit_once(':')
        .ok_or_else(|| "proxy: autorité CONNECT sans port — refusé".to_string())?;
    // Source de vérité UNIQUE : la MÊME validation que `host_of`/`domain_match` (Fable 5 F1).
    // Sinon deux parseurs valident différemment le même domaine sur un couple de gardes egress,
    // et le host que le proxy connecte diverge de celui que la politique croit avoir autorisé.
    // `is_valid_host` rejette : vide, > 253, non-ASCII, point de TÊTE/QUEUE (`example.com.`),
    // label vide (`a..b`) / > 63, tiret de bord (`-x`), charset hors `[a-z0-9-]` (donc IPv6
    // `[::1]`, userinfo `@`, ctrl chars). On lowercase D'ABORD (elle suppose la minuscule).
    let host = host.to_ascii_lowercase();
    if !crate::domain::is_valid_host(&host) {
        return Err(format!(
            "proxy: host CONNECT {host:?} non canonique (cf. domain::is_valid_host) — refusé"
        ));
    }
    // `is_valid_host` accepte `10.0.0.1` STRUCTURELLEMENT (labels numériques valides) : le modèle
    // étant par-DOMAINE, on refuse EN PLUS les littéraux IPv4 (TLD tout-numérique). Le point de
    // queue étant déjà exclu par `is_valid_host`, le TLD est non vide (ce qui referme le trou
    // « `127.0.0.1.` » — Fable 5 F2). ⚠️ Le VRAI gate reste l'allowlist deny-par-défaut ; cette
    // heuristique n'attrape que le décimal-pointé classique — ne JAMAIS la traiter comme LE gate
    // IP (formes hex/octales passeraient — Fable 5 F5).
    if host
        .rsplit('.')
        .next()
        .is_some_and(|tld| tld.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(format!(
            "proxy: host CONNECT {host:?} ressemble à une IP littérale — refusé (modèle par-domaine)"
        ));
    }
    // Port : exiger tout-digit AVANT le parse (aligné sur `host_of` — refuse `+443`, espaces… que
    // `u16::from_str` accepterait sinon, Fable 5 F4).
    if port_str.is_empty() || !port_str.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!(
            "proxy: port CONNECT {port_str:?} invalide — refusé"
        ));
    }
    let port: u16 = port_str
        .parse()
        .map_err(|_| format!("proxy: port CONNECT {port_str:?} hors bornes u16 — refusé"))?;
    if port == 0 {
        return Err("proxy: port CONNECT 0 — refusé".to_string());
    }

    Ok((host, port))
}

/// Réponse HTTP renvoyée au client sur un refus (parse raté). Corps vide (`Connection: close`) :
/// un `chromium` hostile connaît déjà la validité de SA requête, rien à lui apprendre.
const RESPONSE_BAD_REQUEST: &[u8] = b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n";
/// Réponse HTTP sur un refus de politique (host hors-allowlist). Gouvernance transparente :
/// l'allowlist n'est pas un secret vis-à-vis de l'agent.
const RESPONSE_FORBIDDEN: &[u8] = b"HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n";
/// Réponse écrite au client quand le tunnel est **autorisé** (RFC 9110 §9.3.6). À partir de cette
/// ligne, le corps de l'échange CONNECT EST le tunnel brut : plus aucun en-tête, les octets sont
/// relayés tels quels dans les deux sens. Le status 2xx DOIT précéder tout octet applicatif.
const RESPONSE_TUNNEL_ESTABLISHED: &[u8] = b"HTTP/1.1 200 Connection Established\r\n\r\n";

/// Le motif d'un refus CONNECT. Porte la **cause** (400 syntaxe / 403 politique), pas les octets :
/// le rendu octets vit dans [`tunnel_handshake`], de sorte qu'un refus **ne peut structurellement
/// pas** porter une réponse `2xx` (le trou du `Reject { response: <200> }` constructible est fermé
/// — Fable 5). Fail-closed par construction du type, plus par convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectKind {
    /// Syntaxe invalide (parse raté) → `400 Bad Request`. Client hostile : rien à lui apprendre.
    BadRequest,
    /// Host hors-allowlist `[rule.domains]` → `403 Forbidden`. Gouvernance transparente.
    Forbidden,
}

/// La décision du proxy pour une requête CONNECT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectDecision {
    /// Autorisé : le relais (incrément suivant) DOIT ouvrir un tunnel vers `(host, port)`, écrire
    /// `HTTP/1.1 200 Connection Established\r\n\r\n` au client **une fois le dial réussi**, puis
    /// relayer les octets (cf. [`tunnel_handshake`] pour l'ordre exact).
    Tunnel { host: String, port: u16 },
    /// Refusé : le relais DOIT écrire la réponse (déterminée par `kind` dans [`tunnel_handshake`])
    /// au client puis **fermer** — jamais de tunnel.
    Reject { kind: RejectKind },
}

/// Décide, pour la requête CONNECT d'un `chromium` **hostile** et l'allowlist **approuvée** de la
/// session (les patterns `[rule.domains]` autorisés pour CETTE session), si le tunnel est permis.
/// **Fail-closed** : parse raté → `Reject(400)` ; host hors-allowlist → `Reject(403)` ; sinon
/// `Tunnel`. C'est ICI que l'enforcement domaine par-requête vit (le check `navigate` amont n'est
/// qu'un lint : un `click`/302 change de domaine sans repasser par lui — Fable 5).
///
/// Le host est déjà validé/canonique (via [`parse_connect_target`] → `is_valid_host`), donc
/// [`crate::domain::domain_matches_any`] voit la **même** forme que le lint `navigate` — pas de
/// différentiel. **D'où vient `allowed`** (comment `vibed` descend l'allowlist approuvée au proxy)
/// est la décision de forme « à trancher » d'ADR-022 — pris en paramètre ici, form-agnostique.
pub fn connect_decision(request: &[u8], allowed: &[String]) -> ConnectDecision {
    let (host, port) = match parse_connect_target(request) {
        Ok(hp) => hp,
        Err(_) => {
            return ConnectDecision::Reject {
                kind: RejectKind::BadRequest,
            }
        }
    };
    if crate::domain::domain_matches_any(allowed, &host) {
        ConnectDecision::Tunnel { host, port }
    } else {
        ConnectDecision::Reject {
            kind: RejectKind::Forbidden,
        }
    }
}

/// La suite de la boucle CONNECT une fois la décision prise : traduit un [`ConnectDecision`] en
/// **(octets à écrire au client, cible du tunnel s'il y a lieu)**. C'est la **frontière protocolaire
/// pure** entre la décision `[rule.domains]` et l'I/O — l'ultime brique testable avant le relais, et
/// **le seul endroit** où la cause d'un refus ([`RejectKind`]) devient des octets.
///
/// - `Tunnel{host,port}` → `(RESPONSE_TUNNEL_ESTABLISHED, Some((host, port)))` : le relais ouvre
///   D'ABORD le tunnel vers `(host, port)` ; **si et seulement si** le dial réussit, il écrit ce
///   `200`, puis relaie bidirectionnellement ; sinon il **ferme** (le `502 Bad Gateway` est l'affaire
///   de l'incrément d'I/O — jamais de `200` optimiste avant que la cible réponde, cf. squid / RFC
///   9110 §9.3.6 : le `2xx` signale que l'établissement A réussi). L'ordre dial-puis-`200` est le
///   même que celui gravé sur le bras `Tunnel` de l'enum — cette fonction ne le contredit pas.
/// - `Reject{kind}` → `(octets 400/403, None)` : le relais écrit ces octets puis **ferme** — `None`
///   rend la cible **inatteignable par construction**, jamais de tunnel sur un refus.
///
/// **Fail-closed structurel (par le type, pas par convention)** : seul le bras `Tunnel` produit une
/// cible `Some` ET des octets `2xx` ; un `Reject` ne porte qu'un [`RejectKind`] (400/403), il ne peut
/// donc ni produire de cible ni **annoncer** un tunnel — un `Reject { response: <200> }` est
/// irreprésentable. **Forme-agnostique** : ne décrit QUE les octets protocolaires échangés avec le
/// client — *comment* le tunnel s'ouvre (processus dédié dans le netns du navigateur, cf. plancher
/// egress `127.66.0.1`, vs thread) est la décision de forme « à trancher » d'ADR-022, laissée à
/// l'incrément d'I/O suivant.
#[must_use]
pub fn tunnel_handshake(decision: ConnectDecision) -> (&'static [u8], Option<(String, u16)>) {
    match decision {
        ConnectDecision::Tunnel { host, port } => (RESPONSE_TUNNEL_ESTABLISHED, Some((host, port))),
        ConnectDecision::Reject { kind } => {
            let response = match kind {
                RejectKind::BadRequest => RESPONSE_BAD_REQUEST,
                RejectKind::Forbidden => RESPONSE_FORBIDDEN,
            };
            (response, None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_valid_connect_and_lowercases_the_host() {
        assert_eq!(
            parse_connect_target(b"CONNECT Example.COM:443 HTTP/1.1\r\n\r\n").unwrap(),
            ("example.com".to_string(), 443)
        );
        // HTTP/1.0 accepté aussi.
        assert_eq!(
            parse_connect_target(b"CONNECT api.github.com:443 HTTP/1.0\r\nHost: x\r\n\r\n")
                .unwrap(),
            ("api.github.com".to_string(), 443)
        );
        // Un port non-443 est parsé tel quel (le verdict est un incrément suivant).
        assert_eq!(
            parse_connect_target(b"CONNECT sub.domain.example:8443 HTTP/1.1\r\n").unwrap(),
            ("sub.domain.example".to_string(), 8443)
        );
    }

    #[test]
    fn rejects_a_non_connect_method() {
        for bad in [
            &b"GET http://x/ HTTP/1.1\r\n"[..],
            &b"POST evil:443 HTTP/1.1\r\n"[..],
            &b"connect x:443 HTTP/1.1\r\n"[..], // casse : la méthode est sensible à la casse
        ] {
            assert!(parse_connect_target(bad).unwrap_err().contains("CONNECT"));
        }
    }

    #[test]
    fn requires_a_valid_port() {
        assert!(parse_connect_target(b"CONNECT example.com HTTP/1.1\r\n")
            .unwrap_err()
            .contains("sans port"));
        // Port vide.
        assert!(parse_connect_target(b"CONNECT example.com: HTTP/1.1\r\n")
            .unwrap_err()
            .contains("invalide"));
        // > u16::MAX : tout-digit donc passe le charset, rejeté au parse.
        assert!(
            parse_connect_target(b"CONNECT example.com:99999 HTTP/1.1\r\n")
                .unwrap_err()
                .contains("hors bornes")
        );
        // Port 0.
        assert!(parse_connect_target(b"CONNECT example.com:0 HTTP/1.1\r\n")
            .unwrap_err()
            .contains("port CONNECT 0"));
        // (F4) `+443` : `u16::from_str` l'accepterait — on exige tout-digit AVANT le parse.
        assert!(
            parse_connect_target(b"CONNECT example.com:+443 HTTP/1.1\r\n")
                .unwrap_err()
                .contains("invalide")
        );
    }

    #[test]
    fn rejects_non_domain_authorities() {
        // Littéral IPv4 pur (contournement d'allowlist par-domaine) — is_valid_host l'accepte
        // structurellement, l'heuristique IP le refuse.
        assert!(parse_connect_target(b"CONNECT 10.0.0.1:443 HTTP/1.1\r\n")
            .unwrap_err()
            .contains("IP littérale"));
        // IPv6, userinfo, host vide, ctrl char : rejetés par is_valid_host (« non canonique »).
        for bad in [
            &b"CONNECT [::1]:443 HTTP/1.1\r\n"[..],
            &b"CONNECT user@evil.com:443 HTTP/1.1\r\n"[..],
            &b"CONNECT :443 HTTP/1.1\r\n"[..],
            &b"CONNECT ex\x07ample.com:443 HTTP/1.1\r\n"[..],
        ] {
            assert!(
                parse_connect_target(bad)
                    .unwrap_err()
                    .contains("non canonique"),
                "doit refuser {:?}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn rejects_non_canonical_hosts_the_domain_match_precondition() {
        // (F1/F2, Fable 5) Toute forme que `domain::is_valid_host` rejette — la PRÉCONDITION de
        // `domain_match` — DOIT être refusée ici, sinon le proxy et le lint `navigate` valident
        // différemment le même domaine (différentiel de parseurs sur un couple de gardes egress).
        for bad in [
            "example.com.", // point de QUEUE (défait aussi l'anti-IP : cf. 127.0.0.1.)
            ".example.com", // point de TÊTE
            "a..b.com",     // label vide
            "-example.com", // tiret de bord
            "example-.com", // tiret de bord
            "127.0.0.1.",   // IPv4 + point final (F2 : ne doit PAS passer comme domaine)
        ] {
            let req = format!("CONNECT {bad}:443 HTTP/1.1\r\n");
            assert!(
                parse_connect_target(req.as_bytes())
                    .unwrap_err()
                    .contains("non canonique"),
                "doit refuser {bad:?}"
            );
        }
        // Label > 63 octets rejeté (borné par is_valid_host, pas seulement par MAX_REQUEST_LINE).
        let long_label = "a".repeat(64);
        let req = format!("CONNECT {long_label}.com:443 HTTP/1.1\r\n");
        assert!(parse_connect_target(req.as_bytes())
            .unwrap_err()
            .contains("non canonique"));
    }

    #[test]
    fn is_fail_closed_on_malformed_lines() {
        // Pas de CRLF (dans la fenêtre bornée).
        assert!(parse_connect_target(b"CONNECT x.com:443 HTTP/1.1")
            .unwrap_err()
            .contains("CRLF absent"));
        // Version inattendue.
        assert!(parse_connect_target(b"CONNECT x.com:443 HTTP/2\r\n")
            .unwrap_err()
            .contains("version"));
        // Token en trop.
        assert!(
            parse_connect_target(b"CONNECT x.com:443 HTTP/1.1 extra\r\n")
                .unwrap_err()
                .contains("token en trop")
        );
        // Autorité absente (2 espaces).
        assert!(parse_connect_target(b"CONNECT  HTTP/1.1\r\n").is_err());
        // (F3) Pas de CRLF dans les premiers MAX_REQUEST_LINE+2 octets → refus AVANT de scanner
        // tout le tampon (DoS de scan). Le CRLF est repoussé au-delà de la fenêtre.
        let mut giant = b"CONNECT ".to_vec();
        giant.extend(std::iter::repeat(b'a').take(MAX_REQUEST_LINE));
        giant.extend_from_slice(b".com:443 HTTP/1.1\r\n");
        assert!(parse_connect_target(&giant)
            .unwrap_err()
            .contains("CRLF absent"));
    }

    // ----- connect_decision : verdict [rule.domains] par requête -----

    fn req(authority: &str) -> Vec<u8> {
        format!("CONNECT {authority} HTTP/1.1\r\n\r\n").into_bytes()
    }

    #[test]
    fn connect_decision_tunnels_an_allowlisted_exact_host() {
        let allowed = vec!["github.com".to_string()];
        assert_eq!(
            connect_decision(&req("github.com:443"), &allowed),
            ConnectDecision::Tunnel {
                host: "github.com".to_string(),
                port: 443
            }
        );
        // Casse insensible : le host est canonicalisé en minuscules avant le match.
        assert_eq!(
            connect_decision(&req("GitHub.com:443"), &allowed),
            ConnectDecision::Tunnel {
                host: "github.com".to_string(),
                port: 443
            }
        );
        // Un sous-domaine n'est PAS couvert par un pattern exact.
        assert!(matches!(
            connect_decision(&req("api.github.com:443"), &allowed),
            ConnectDecision::Reject {
                kind: RejectKind::Forbidden
            }
        ));
    }

    #[test]
    fn connect_decision_respects_anchored_subdomain_wildcards() {
        let allowed = vec!["*.github.com".to_string()];
        // Sous-domaine : autorisé.
        assert!(matches!(
            connect_decision(&req("api.github.com:443"), &allowed),
            ConnectDecision::Tunnel { .. }
        ));
        // Apex NON couvert par `*.` (il faut lister les deux — invariant domain_match).
        assert!(matches!(
            connect_decision(&req("github.com:443"), &allowed),
            ConnectDecision::Reject { .. }
        ));
        // Ancrage à droite : `evil-github.com` finit par `-github.com`, pas `.github.com`.
        assert!(matches!(
            connect_decision(&req("evil-github.com:443"), &allowed),
            ConnectDecision::Reject { .. }
        ));
    }

    #[test]
    fn connect_decision_is_fail_closed_on_deny_and_on_malformed() {
        // Host hors-allowlist → 403.
        assert_eq!(
            connect_decision(&req("evil.example:443"), &["github.com".to_string()]),
            ConnectDecision::Reject {
                kind: RejectKind::Forbidden
            }
        );
        // Allowlist VIDE → tout refusé (deny-par-défaut).
        assert!(matches!(
            connect_decision(&req("github.com:443"), &[]),
            ConnectDecision::Reject {
                kind: RejectKind::Forbidden
            }
        ));
        // Requête malformée (host non canonique / parse raté) → 400, jamais un tunnel.
        assert_eq!(
            connect_decision(
                b"CONNECT 10.0.0.1:443 HTTP/1.1\r\n",
                &["10.0.0.1".to_string()]
            ),
            ConnectDecision::Reject {
                kind: RejectKind::BadRequest
            }
        );
        // Même si le pattern « matcherait », un parse raté ferme AVANT le verdict (400).
        assert!(matches!(
            connect_decision(b"GET / HTTP/1.1\r\n", &["*.x.com".to_string()]),
            ConnectDecision::Reject {
                kind: RejectKind::BadRequest
            }
        ));
    }

    // ----- tunnel_handshake : décision → octets client + cible -----

    #[test]
    fn tunnel_handshake_maps_a_tunnel_to_the_200_and_the_target() {
        let (response, target) = tunnel_handshake(ConnectDecision::Tunnel {
            host: "github.com".to_string(),
            port: 443,
        });
        // Câblage : le bras Tunnel rend bien la constante 200.
        assert_eq!(response, RESPONSE_TUNNEL_ESTABLISHED);
        // Octets EXACTS figés (littéral indépendant de la constante) : un `2xx` à CONNECT ne doit
        // porter NI Content-Length NI Transfer-Encoding (RFC 9112) — la nudité de la ligne EST
        // l'invariant, qu'un futur « enrichissement » de la constante casserait sans ce verrou.
        assert_eq!(
            response,
            b"HTTP/1.1 200 Connection Established\r\n\r\n".as_slice()
        );
        assert_eq!(target, Some(("github.com".to_string(), 443)));
    }

    #[test]
    fn tunnel_handshake_maps_a_reject_to_its_response_and_no_target() {
        // La cause du refus devient des octets ICI (le seul endroit) — aucune cible, jamais de
        // tunnel. 403 (politique).
        assert_eq!(
            tunnel_handshake(ConnectDecision::Reject {
                kind: RejectKind::Forbidden
            }),
            (RESPONSE_FORBIDDEN, None)
        );
        // 400 (parse).
        assert_eq!(
            tunnel_handshake(ConnectDecision::Reject {
                kind: RejectKind::BadRequest
            }),
            (RESPONSE_BAD_REQUEST, None)
        );
    }

    #[test]
    fn the_pure_pipeline_parse_decide_handshake_is_coherent() {
        // Le pipeline pur complet : requête hostile → verdict → octets/cible, bout en bout.
        let allowed = vec!["*.github.com".to_string()];

        // Autorisé : le handshake livre le 200 ET une cible relayable = celle validée/canonique.
        let (resp, target) =
            tunnel_handshake(connect_decision(&req("Api.GitHub.com:443"), &allowed));
        assert_eq!(resp, RESPONSE_TUNNEL_ESTABLISHED);
        assert_eq!(target, Some(("api.github.com".to_string(), 443)));

        // Hors-allowlist : 403, aucune cible — impossible de relayer un refus (garanti par le type).
        let (resp, target) = tunnel_handshake(connect_decision(&req("evil.example:443"), &allowed));
        assert_eq!(resp, RESPONSE_FORBIDDEN);
        assert_eq!(target, None);

        // Malformé (host IP littérale) : 400, aucune cible — ferme AVANT toute idée de tunnel.
        let (resp, target) = tunnel_handshake(connect_decision(
            b"CONNECT 10.0.0.1:443 HTTP/1.1\r\n",
            &allowed,
        ));
        assert_eq!(resp, RESPONSE_BAD_REQUEST);
        assert_eq!(target, None);
    }
}
