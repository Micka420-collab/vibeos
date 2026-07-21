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
//! **Cœur logique pur** (testable isolément) : parse la ligne CONNECT d'un client **hostile**
//! ([`parse_connect_target`]), rend le **verdict** `[rule.domains]` ([`connect_decision`] →
//! tunnel / refus), et filtre les IP internes ([`is_internal_ip`], anti-SSRF/rebinding). Le
//! **relais I/O** ([`serve_connection`] : dial-sûr avec épinglage d'IP + splice bidirectionnel)
//! sert UNE connexion. Le **listener**, l'entrée `run_proxy` (mode helper) et la **forme** du proxy
//! (processus/netns : `chromium` restreint à l'IP du proxy, proxy avec egress Internet) — la
//! décision de forme « à trancher » d'ADR-022 — sont l'incrément suivant. **D'où vient l'allowlist
//! approuvée** est pris en paramètre, form-agnostique.
//!
//! **Contrat CONNECT** (RFC 9110 §9.3.6) : la ligne de requête est `CONNECT authority HTTP/1.1`
//! où `authority = host:port`, le **port est obligatoire**, et il n'y a **jamais** de schéma
//! ni de chemin. Tout écart d'un `chromium` hostile est **fail-closed**.

use std::io::{self, Read, Write};
use std::net::{
    IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

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
/// Réponse écrite au client quand le **dial de la cible échoue** (injoignable ou IP interne
/// refusée). RFC 9110 §9.3.6 : jamais de `200` optimiste — la cible n'a pas répondu.
const RESPONSE_BAD_GATEWAY: &[u8] = b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n";

/// Taille max de l'en-tête CONNECT lu avant le tunnel (ligne de requête + en-têtes + ligne vide).
/// Un `chromium` légitime envoie ~200 octets ; 8 KiB borne un pair hostile qui n'enverrait jamais
/// le `\r\n\r\n` terminal.
const MAX_REQUEST_HEAD: usize = 8 * 1024;
/// Timeout de connexion à la cible ET de résolution DNS — borne un dial/résolveur qui pend (cible
/// qui n'accepte jamais, résolveur hostile/lent).
const DIAL_TIMEOUT: Duration = Duration::from_secs(10);
/// Timeout de lecture/écriture pendant la **phase d'en-tête** : un `chromium` hostile qui ouvre la
/// connexion puis n'envoie jamais le `\r\n\r\n` (slowloris) — ou ne lit jamais la réponse — est
/// coupé au lieu de tenir un thread indéfiniment (Fable 5, BLOQUANT anti-DoS).
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
/// Timeout d'**inactivité** du tunnel (par read/write) : un tunnel muet (slowloris / slow-read
/// hostile) est coupé ; un tunnel **actif** (≥ 1 octet par fenêtre) survit. Généreux — l'automation
/// gouvernée fait des échanges courts, pas des flux longuement inactifs.
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

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

/// Un IP est-il **interne** — donc INTERDIT comme cible de dial du relais ? Défense
/// anti-SSRF / anti-DNS-rebinding (browser audit, forward-looking) : `parse_connect_target`
/// valide le **nom**, mais c'est le relais qui **résout le DNS et dial** — un domaine allowlisté
/// dont le DNS résout (ou *rebind* entre deux requêtes) vers `169.254.169.254` / `127.0.0.1` / une
/// IP RFC1918 défait l'allowlist par-domaine et atteint l'infra interne. Le relais DOIT rejeter
/// toute IP résolue interne **et** épingler l'IP entre la décision et le dial. Le plancher réseau
/// du sandbox ne protège que `chromium`, pas le processus proxy qui, lui, atteint l'Internet.
///
/// **Fail-closed** (on préfère refuser trop) — couvre :
/// - **IPv4** : `0/8` (this host), `10/8`·`172.16/12`·`192.168/16` (privé), `127/8` (loopback),
///   `100.64/10` (CGNAT / tailnet), `169.254/16` (link-local, dont la métadonnée cloud
///   `169.254.169.254`), `192.0.0/24` (IETF), `198.18/15` (benchmarking), `224/4`+`240/4`
///   (multicast/réservé/broadcast).
/// - **IPv6** : `::`/`::1` et tout préfixe tout-zéro, `ff00::/8` (multicast), `fe80::/10`
///   (link-local), `fc00::/7` (ULA), `2001:db8::/32` (doc), et les formes **IPv4-mapped/compat**
///   (`::ffff:a.b.c.d`, `::a.b.c.d`) **ré-évaluées sur le v4 embarqué** — sinon un rebind vers
///   `::ffff:127.0.0.1` passerait.
pub fn is_internal_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_internal_v4(v4),
        IpAddr::V6(v6) => is_internal_v6(v6),
    }
}

fn is_internal_v4(v4: Ipv4Addr) -> bool {
    let o = v4.octets();
    let (a, b) = (o[0], o[1]);
    a == 0                                       // 0.0.0.0/8 « this host »
        || a == 10                               // 10.0.0.0/8
        || a == 127                              // 127.0.0.0/8 loopback
        || (a == 100 && (64..=127).contains(&b)) // 100.64.0.0/10 CGNAT
        || (a == 169 && b == 254)                // 169.254.0.0/16 link-local (métadonnée cloud)
        || (a == 172 && (16..=31).contains(&b))  // 172.16.0.0/12
        || (a == 192 && b == 168)                // 192.168.0.0/16
        || (a == 192 && b == 0 && o[2] == 0)     // 192.0.0.0/24 IETF
        || (a == 192 && b == 88 && o[2] == 99)   // 192.88.99.0/24 relais 6to4 anycast (déprécié)
        || (a == 198 && (b == 18 || b == 19))    // 198.18.0.0/15 benchmarking
        || a >= 224 // 224/4 multicast + 240/4 réservé + 255.255.255.255 broadcast
}

fn is_internal_v6(v6: Ipv6Addr) -> bool {
    // IPv4-mapped/compat (`::ffff:a.b.c.d`, `::a.b.c.d`, dont `::`/`::1`) → ré-évalue sur le v4
    // embarqué : un rebind vers `::ffff:127.0.0.1` doit être attrapé par le check v4.
    if let Some(v4) = v6.to_ipv4() {
        return is_internal_v4(v4);
    }
    let s = v6.segments();
    // Préfixes de TRANSITION qui embarquent une IPv4 de destination que `to_ipv4()` NE convertit
    // PAS (Fable 5, faux négatifs) — sur un hôte à transit NAT64 (cloud IPv6-only : AWS/GCP,
    // 464XLAT) le dial atteint RÉELLEMENT l'IPv4. On ré-évalue l'IPv4 embarquée (un NAT64/6to4 vers
    // une IPv4 publique reste externe — zéro faux positif).
    // NAT64 well-known 64:ff9b::/96 (RFC 6052) : IPv4 dans les 32 bits bas.
    if s[0] == 0x0064 && s[1] == 0xff9b {
        if s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0 {
            let v4 = Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8);
            return is_internal_v4(v4);
        }
        // local-use 64:ff9b:1::/48 (RFC 8215) & autres sous-préfixes : offset IPv4 piégeux
        // (octet « u ») → tout le /32 réservé NAT64 est bloqué fail-closed.
        return true;
    }
    // 6to4 2002::/16 (RFC 3056, déprécié RFC 7526) : IPv4 de site en s[1]s[2].
    if s[0] == 0x2002 {
        let v4 = Ipv4Addr::new((s[1] >> 8) as u8, s[1] as u8, (s[2] >> 8) as u8, s[2] as u8);
        return is_internal_v4(v4);
    }
    let first = s[0];
    first == 0                                    // ::/16 et préfixes tout-zéro non mappés
        || (first & 0xff00) == 0xff00             // ff00::/8 multicast
        || (first & 0xffc0) == 0xfe80             // fe80::/10 link-local
        || (first & 0xfe00) == 0xfc00             // fc00::/7 ULA
        || (first == 0x2001 && s[1] == 0x0db8) // 2001:db8::/32 documentation
}

/// Lit l'en-tête de la requête CONNECT — jusqu'au `\r\n\r\n` terminal, borné à `max` octets.
/// Lecture **octet par octet** (pas de `BufReader`) pour NE PAS aspirer les octets du tunnel qui
/// suivent l'en-tête : après le `200`, le corps de l'échange EST le tunnel brut, il ne doit pas
/// être pré-lu. Fail-closed : EOF avant la fin, ou dépassement de `max` (pair qui n'envoie jamais
/// la ligne vide) → `Err`.
fn read_request_head<R: Read>(r: &mut R, max: usize) -> io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    loop {
        if buf.len() >= max {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "proxy: en-tête CONNECT trop long — refusé",
            ));
        }
        if r.read(&mut byte)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "proxy: EOF avant la fin de l'en-tête CONNECT",
            ));
        }
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") {
            return Ok(buf);
        }
    }
}

/// Vrai si TOUTES les adresses résolues sont externes (aucune interne). **Fail-closed** : la
/// présence d'UNE SEULE adresse interne fait rejeter tout le dial — un domaine public légitime ne
/// résout jamais vers de l'interne ; un mélange `[publique, interne]` est un signal de
/// rebinding/SSRF (Fable 5), l'attaquant ne doit pas espérer que le dial tombe sur l'interne. Liste
/// vide → faux (rien à dialer).
fn all_addrs_external(addrs: &[SocketAddr]) -> bool {
    !addrs.is_empty() && addrs.iter().all(|a| !is_internal_ip(a.ip()))
}

/// Résout `host:port` avec un timeout **borné** (le `to_socket_addrs`/getaddrinfo de std n'en a
/// PAS — modèle de menace : résolveur hostile/lent qui pendrait le thread ; Fable 5). Résolution
/// dans un thread dédié, `recv_timeout` borné ; si le délai expire → `Err` (le thread résolveur se
/// termine seul dans le timeout du résolveur OS, sans bloquer le thread de service).
fn resolve_bounded(host: &str, port: u16, timeout: Duration) -> io::Result<Vec<SocketAddr>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let host_owned = host.to_string();
    std::thread::spawn(move || {
        let res = (host_owned.as_str(), port)
            .to_socket_addrs()
            .map(|it| it.collect::<Vec<_>>());
        let _ = tx.send(res); // le récepteur peut être parti (timeout) → on ignore
    });
    match rx.recv_timeout(timeout) {
        Ok(res) => res,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "proxy: résolution DNS expirée — refusé",
        )),
    }
}

/// Résout `host:port` (borné, [`resolve_bounded`]), REFUSE si une seule adresse est interne
/// ([`all_addrs_external`]), puis connecte l'une des adresses **validées** (timeout borné). Les IP
/// connectées sont EXACTEMENT celles vérifiées — **aucune re-résolution** entre le check et le dial,
/// ce qui ferme la fenêtre de DNS-rebinding. Toutes étant externes, itérer sur les adresses (hôte
/// multi-homé) est sûr et améliore la disponibilité (Fable 5, MINEUR).
fn dial_safe(host: &str, port: u16) -> io::Result<TcpStream> {
    let addrs = resolve_bounded(host, port, DIAL_TIMEOUT)?;
    if !all_addrs_external(&addrs) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("proxy: {host} résout vers une IP interne — refusé (anti-rebinding)"),
        ));
    }
    // `addrs` non vide (garanti par all_addrs_external). Essayer chaque adresse validée.
    let mut last_err = None;
    for addr in &addrs {
        match TcpStream::connect_timeout(addr, DIAL_TIMEOUT) {
            Ok(s) => return Ok(s),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "proxy: aucune adresse joignable",
        )
    }))
}

/// Relaie les octets dans les DEUX sens entre le client (`chromium`) et l'upstream jusqu'à
/// fermeture. Un thread par sens ; sur EOF d'un sens, `shutdown(Write)` de l'autre pair
/// (**half-close**) pour qu'il draine sa réponse sans couper le sens inverse. Tampon borné
/// (`io::copy`). Tunnel **BRUT** — aucun octet interprété (le proxy ne voit jamais le clair TLS,
/// ADR-022). Le `RuntimeMaxSec` de l'unité borne un pair qui ne fermerait jamais.
fn splice(client: TcpStream, upstream: TcpStream) -> io::Result<()> {
    let mut client_read = client.try_clone()?;
    let mut upstream_write = upstream.try_clone()?;
    // Sens 1 (thread) : client → upstream.
    let t = std::thread::spawn(move || {
        let _ = io::copy(&mut client_read, &mut upstream_write);
        let _ = upstream_write.shutdown(Shutdown::Write);
    });
    // Sens 2 (thread courant) : upstream → client.
    let mut upstream_read = upstream;
    let mut client_write = client;
    let _ = io::copy(&mut upstream_read, &mut client_write);
    let _ = client_write.shutdown(Shutdown::Write);
    let _ = t.join();
    Ok(())
}

/// Sert UNE connexion cliente (`chromium` **hostile**) : lit l'en-tête CONNECT (borné), rend le
/// verdict `[rule.domains]`, et soit ouvre le tunnel (**dial-sûr → `200` → splice**), soit écrit
/// le refus (400/403) et ferme. **Dial D'ABORD** (RFC 9110 §9.3.6 : le `200` signale que
/// l'établissement A réussi ; un dial raté → `502`, jamais de `200` optimiste). Fail-closed
/// partout — toute erreur ferme la connexion sans tunnel.
pub fn serve_connection(mut client: TcpStream, allowed: &[String]) -> io::Result<()> {
    // Phase d'en-tête : borne read ET write (slowloris d'en-tête + client slow-read qui ne lirait
    // jamais la réponse) — sinon un thread pend indéfiniment (Fable 5, BLOQUANT anti-DoS).
    client.set_read_timeout(Some(HEAD_TIMEOUT))?;
    client.set_write_timeout(Some(HEAD_TIMEOUT))?;
    let head = read_request_head(&mut client, MAX_REQUEST_HEAD)?;
    let (response, target) = tunnel_handshake(connect_decision(&head, allowed));
    let Some((host, port)) = target else {
        // Refus (400/403) : écrire la réponse puis fermer. Jamais de tunnel.
        let _ = client.write_all(response);
        let _ = client.flush();
        return Ok(());
    };
    match dial_safe(&host, port) {
        Ok(upstream) => {
            // `response` est ici `RESPONSE_TUNNEL_ESTABLISHED` (bras Tunnel) — écrit APRÈS le dial.
            client.write_all(response)?;
            client.flush()?;
            // Phase tunnel : timeout d'INACTIVITÉ sur les deux sens (un tunnel muet hostile est
            // coupé, un tunnel actif survit) → le splice ne peut plus pendre indéfiniment.
            set_idle_timeouts(&client, IDLE_TIMEOUT)?;
            set_idle_timeouts(&upstream, IDLE_TIMEOUT)?;
            splice(client, upstream)
        }
        Err(_) => {
            // Cible injoignable OU IP interne refusée (anti-rebinding) → 502, jamais de 200.
            let _ = client.write_all(RESPONSE_BAD_GATEWAY);
            let _ = client.flush();
            Ok(())
        }
    }
}

/// Pose un timeout d'**inactivité** (read + write) sur un socket du tunnel — un `io::copy` qui
/// n'avance plus dans la fenêtre retourne alors `Err`, ce qui termine le thread de splice et libère
/// le socket (anti-pendaison, Fable 5).
fn set_idle_timeouts(s: &TcpStream, d: Duration) -> io::Result<()> {
    s.set_read_timeout(Some(d))?;
    s.set_write_timeout(Some(d))?;
    Ok(())
}

/// Nombre max de connexions concurrentes servies. Même avec les timeouts par-connexion, on borne
/// le nombre de threads/FD qu'un `chromium` hostile peut ouvrir d'un coup (Fable 5, défense en
/// profondeur anti-DoS). Au-delà, la nouvelle connexion est **fermée immédiatement** (fail-closed).
const MAX_CONCURRENT_CONNS: usize = 64;
/// Taille max de la config lue sur stdin (bind + allowlist). Une allowlist légitime tient largement
/// dessous ; borne un `vibed` bogué ou un futur chemin non fiable.
const MAX_CONFIG_BYTES: u64 = 64 * 1024;

/// Parse la **config** que `vibed` descend au proxy sur stdin : `{ "bind": "127.66.0.1:8888",
/// "allowed": ["github.com", "*.github.com"] }`. `bind` DOIT parser en `SocketAddr` **loopback**
/// (défense en profondeur : le proxy ne se lie JAMAIS à une adresse routable, même sur une config
/// bogue). `allowed` = les patterns `[rule.domains]` **déjà approuvés** pour la session (le proxy
/// est form-agnostique : il ne décide pas de l'allowlist, il l'applique). Fail-closed sur tout écart.
fn parse_proxy_request(payload: &Value) -> Result<(SocketAddr, Vec<String>), String> {
    let bind = payload
        .get("bind")
        .and_then(Value::as_str)
        .ok_or_else(|| "proxy: champ 'bind' manquant ou non-chaîne — refusé".to_string())?;
    let addr: SocketAddr = bind.parse().map_err(|_| {
        format!("proxy: 'bind' {bind:?} n'est pas une adresse:port valide — refusé")
    })?;
    if !addr.ip().is_loopback() {
        return Err(format!(
            "proxy: 'bind' {bind:?} doit être une adresse loopback (jamais routable) — refusé"
        ));
    }
    let allowed = payload
        .get("allowed")
        .and_then(Value::as_array)
        .ok_or_else(|| "proxy: champ 'allowed' manquant ou non-tableau — refusé".to_string())?
        .iter()
        .map(|v| {
            v.as_str().map(str::to_string).ok_or_else(|| {
                "proxy: 'allowed' contient une entrée non-chaîne — refusé".to_string()
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((addr, allowed))
}

/// Boucle d'`accept` : chaque connexion cliente est servie dans son propre thread par
/// [`serve_connection`], avec un **plafond de connexions concurrentes** ([`MAX_CONCURRENT_CONNS`]).
/// Ne retourne **jamais** (le proxy vit tant que `vibed` ne tue pas l'unité transitoire — ADR-019).
/// Une erreur d'`accept` transitoire est ignorée (on continue à servir). Le compteur `active` n'est
/// incrémenté que par cette boucle (unique accepteur) et décrémenté par les threads de service.
fn serve_listener(listener: TcpListener, allowed: Vec<String>) {
    let allowed = Arc::new(allowed);
    let active = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let Ok(client) = stream else {
            continue; // erreur d'accept transitoire → on continue
        };
        if active.load(Ordering::Relaxed) >= MAX_CONCURRENT_CONNS {
            drop(client); // trop de connexions ouvertes → fermer immédiatement (fail-closed)
            continue;
        }
        active.fetch_add(1, Ordering::Relaxed);
        let allowed = Arc::clone(&allowed);
        let active = Arc::clone(&active);
        std::thread::spawn(move || {
            let _ = serve_connection(client, &allowed);
            active.fetch_sub(1, Ordering::Relaxed);
        });
    }
}

/// Entrée du **mode helper `proxy`** (ADR-022) : lit la config `{bind, allowed}` sur stdin (bornée),
/// bind le listener, et sert jusqu'à ce que `vibed` tue l'unité. Ne retourne `Ok` jamais en
/// fonctionnement normal (la boucle d'accept est infinie) ; `Err` sur config malformée ou bind raté
/// (fail-closed). **Forme** (netns : `chromium` restreint à l'IP du proxy, proxy avec egress
/// Internet) et **orchestration** (spawn concurrent au navigateur, teardown) = incrément suivant.
pub fn run_proxy() -> Result<String, String> {
    let mut payload = String::new();
    io::stdin()
        .take(MAX_CONFIG_BYTES)
        .read_to_string(&mut payload)
        .map_err(|e| format!("proxy: lecture de la config : {e}"))?;
    let payload: Value = serde_json::from_str(payload.trim())
        .map_err(|e| format!("proxy: config malformée : {e}"))?;
    let (bind, allowed) = parse_proxy_request(&payload)?;
    let listener = TcpListener::bind(bind).map_err(|e| format!("proxy: bind {bind} : {e}"))?;
    serve_listener(listener, allowed); // ne retourne pas (accept infini)
    Ok(String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_proxy_request_validates_bind_loopback_and_allowlist() {
        // Config valide.
        let (addr, allowed) = parse_proxy_request(&serde_json::json!({
            "bind": "127.66.0.1:8888",
            "allowed": ["github.com", "*.github.com"]
        }))
        .unwrap();
        assert_eq!(addr.to_string(), "127.66.0.1:8888");
        assert_eq!(
            allowed,
            vec!["github.com".to_string(), "*.github.com".to_string()]
        );
        // bind NON loopback → refus (défense en profondeur : jamais routable).
        assert!(parse_proxy_request(&serde_json::json!({
            "bind": "0.0.0.0:8888", "allowed": []
        }))
        .unwrap_err()
        .contains("loopback"));
        assert!(parse_proxy_request(&serde_json::json!({
            "bind": "8.8.8.8:443", "allowed": []
        }))
        .unwrap_err()
        .contains("loopback"));
        // bind malformé, champs manquants, allowed non-tableau, entrée non-chaîne → refus.
        for bad in [
            serde_json::json!({ "bind": "pas une adresse", "allowed": [] }),
            serde_json::json!({ "allowed": [] }),
            serde_json::json!({ "bind": "127.0.0.1:1" }),
            serde_json::json!({ "bind": "127.0.0.1:1", "allowed": "x" }),
            serde_json::json!({ "bind": "127.0.0.1:1", "allowed": [1, 2] }),
        ] {
            assert!(parse_proxy_request(&bad).is_err(), "doit refuser {bad}");
        }
    }

    #[test]
    fn read_request_head_stops_at_the_blank_line_without_eating_the_tunnel() {
        use std::io::Cursor;
        // S'arrête EXACTEMENT au `\r\n\r\n` — les octets du tunnel qui suivent ne sont PAS consommés.
        let mut c =
            Cursor::new(b"CONNECT x.com:443 HTTP/1.1\r\nHost: x.com\r\n\r\nTUNNEL".to_vec());
        let head = read_request_head(&mut c, 8192).unwrap();
        assert_eq!(
            &head[..],
            b"CONNECT x.com:443 HTTP/1.1\r\nHost: x.com\r\n\r\n"
        );
        let mut rest = Vec::new();
        Read::read_to_end(&mut c, &mut rest).unwrap();
        assert_eq!(&rest[..], b"TUNNEL", "le tunnel ne doit pas être pré-lu");
        // EOF avant la fin → Err.
        let mut c2 = Cursor::new(b"CONNECT x.com:443 HTTP/1.1\r\n".to_vec());
        assert!(read_request_head(&mut c2, 8192).is_err());
        // Dépassement de la borne (pair qui n'envoie jamais la ligne vide) → Err.
        let mut c3 = Cursor::new(vec![b'a'; 100]);
        assert!(read_request_head(&mut c3, 32)
            .unwrap_err()
            .to_string()
            .contains("trop long"));
    }

    #[test]
    fn all_addrs_external_is_fail_closed_on_any_internal_addr() {
        let sa = |s: &str| s.parse::<SocketAddr>().unwrap();
        // Toutes externes → OK.
        assert!(all_addrs_external(&[sa("8.8.8.8:443"), sa("1.1.1.1:443")]));
        // Une SEULE interne → tout refusé (anti-rebinding, l'attaquant ne choisit pas l'IP dialée).
        assert!(!all_addrs_external(&[
            sa("8.8.8.8:443"),
            sa("127.0.0.1:443")
        ]));
        assert!(!all_addrs_external(&[sa("10.0.0.1:443")]));
        assert!(!all_addrs_external(&[sa("[64:ff9b::7f00:1]:443")])); // NAT64 → loopback
                                                                      // Vide → faux (rien à dialer).
        assert!(!all_addrs_external(&[]));
    }

    #[test]
    fn is_internal_ip_rejects_every_ssrf_and_rebinding_target() {
        use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
        let v4 = |s: &str| IpAddr::V4(s.parse::<Ipv4Addr>().unwrap());
        // INTERNES — DOIVENT être refusés comme cible de dial.
        for ip in [
            "0.0.0.0",
            "0.1.2.3", // 0/8
            "10.0.0.1",
            "10.255.255.255", // 10/8
            "127.0.0.1",
            "127.1.2.3", // loopback
            "100.64.0.1",
            "100.100.0.1",
            "100.127.255.255", // CGNAT 100.64/10
            "169.254.169.254",
            "169.254.0.1", // link-local + métadonnée cloud
            "172.16.0.1",
            "172.31.255.255", // 172.16/12
            "192.168.0.1",
            "192.168.255.1", // 192.168/16
            "192.0.0.1",     // 192.0.0/24
            "198.18.0.1",
            "198.19.255.255", // 198.18/15
            "224.0.0.1",
            "239.1.2.3", // multicast
            "240.0.0.1",
            "255.255.255.255", // réservé / broadcast
        ] {
            assert!(is_internal_ip(v4(ip)), "{ip} doit être interne");
        }
        // EXTERNES — DOIVENT être autorisés (bornes JUSTE hors des plages internes).
        for ip in [
            "8.8.8.8",
            "1.1.1.1",
            "140.82.121.4", // publics
            "100.63.255.255",
            "100.128.0.1", // hors CGNAT 100.64/10
            "172.15.255.255",
            "172.32.0.1", // hors 172.16/12
            "192.167.255.255",
            "192.169.0.1", // hors 192.168/16
            "198.17.255.255",
            "198.20.0.1", // hors 198.18/15
            "192.0.1.1",  // hors 192.0.0/24
        ] {
            assert!(!is_internal_ip(v4(ip)), "{ip} doit être externe");
        }
        // IPv6.
        let v6 = |s: &str| IpAddr::V6(s.parse::<Ipv6Addr>().unwrap());
        for ip in [
            "::1",
            "::", // loopback / unspecified
            "fe80::1",
            "fe80::abcd", // link-local
            "fc00::1",
            "fd12:3456::1", // ULA
            "ff02::1",      // multicast
            "2001:db8::1",  // documentation
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",        // IPv4-mapped privé/loopback (rebinding !)
            "::ffff:169.254.169.254", // IPv4-mapped métadonnée
            "::127.0.0.1",            // IPv4-compat (forme ::a.b.c.d)
            "64:ff9b::7f00:1",        // NAT64 well-known → 127.0.0.1 (rebinding, Fable 5)
            "64:ff9b::a9fe:a9fe",     // NAT64 → 169.254.169.254 métadonnée
            "64:ff9b:1::1",           // NAT64 local-use /48 → bloc fail-closed
            "2002:7f00:1::",          // 6to4 → 127.0.0.1
            "2002:a9fe:a9fe::",       // 6to4 → 169.254.169.254 métadonnée
            "2002:0a00:1::",          // 6to4 → 10.0.0.1 privé
            "febf::1",                // borne haute fe80::/10
        ] {
            assert!(is_internal_ip(v6(ip)), "{ip} doit être interne");
        }
        for ip in [
            "2606:4700:4700::1111",
            "2001:4860:4860::8888",
            "::ffff:8.8.8.8",   // IPv4-mapped PUBLIC → autorisé
            "64:ff9b::808:808", // NAT64 → 8.8.8.8 PUBLIC → externe (pas de sur-blocage)
            "fec0::1",          // ex site-local déprécié → externe (borne fe80::/10)
            "2001:db9::1",      // borne 2001:db8::/32
        ] {
            assert!(!is_internal_ip(v6(ip)), "{ip} doit être externe");
        }
    }

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
