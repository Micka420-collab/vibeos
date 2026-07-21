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
//! **Ce module (premier brick) est PUR** : il ne fait que **parser** la ligne de requête
//! CONNECT d'un client **hostile** (`chromium` est traité comme hostile par ADR-022). Le
//! **verdict** `[rule.domains]`, le **relais** bidirectionnel, et la **forme** du proxy
//! (processus dédié vs thread du helper ; netns) sont des incréments ultérieurs (décision de
//! forme « à trancher » d'ADR-022) — isolés ici pour tester le parsing sans réseau.
//!
//! **Contrat CONNECT** (RFC 9110 §9.3.6) : la ligne de requête est `CONNECT authority HTTP/1.1`
//! où `authority = host:port`, le **port est obligatoire**, et il n'y a **jamais** de schéma
//! ni de chemin. Tout écart d'un `chromium` hostile est **fail-closed**.

/// Longueur maximale de la ligne de requête CONNECT (anti-DoS ; une autorité DNS légitime
/// tient largement dessous — un FQDN fait au plus 253 octets).
const MAX_REQUEST_LINE: usize = 512;

/// Parse la **ligne de requête** HTTP CONNECT d'un client (`chromium`) et rend `(host, port)`.
/// L'entrée est **hostile** : fail-closed sur tout écart (méthode ≠ CONNECT, autorité absente,
/// port absent/invalide, host au charset non-hostname, ligne trop longue, non-UTF8, tokens en
/// trop). Le `host` est renvoyé en **minuscules ASCII** (les noms d'hôte sont insensibles à la
/// casse — le matching `[rule.domains]` en aval doit voir une forme canonique).
///
/// N'accepte **que** des hosts de type nom-de-domaine (`[a-z0-9.-]`) : les littéraux IP (v4/v6)
/// et l'userinfo (`@`) sont **refusés** — le modèle `[rule.domains]` est par-DOMAINE, et
/// `chromium` derrière un proxy envoie `CONNECT domaine:443` (c'est le proxy qui résout le DNS).
/// Un littéral IP en autorité serait donc soit une tentative de contournement de l'allowlist,
/// soit hors-modèle : refusé.
pub fn parse_connect_target(request: &[u8]) -> Result<(String, u16), String> {
    // On ne lit QUE la première ligne (jusqu'au premier CRLF) — les en-têtes ne nous concernent
    // pas pour la cible.
    let crlf = request
        .windows(2)
        .position(|w| w == b"\r\n")
        .ok_or_else(|| "proxy: requête CONNECT sans CRLF — refusé".to_string())?;
    if crlf > MAX_REQUEST_LINE {
        return Err(format!(
            "proxy: ligne de requête CONNECT trop longue ({crlf} > {MAX_REQUEST_LINE}) — refusé"
        ));
    }
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
    if host.is_empty() {
        return Err("proxy: host CONNECT vide — refusé".to_string());
    }
    // Charset host = nom de domaine strict : lettres/chiffres ASCII, `-`, `.`. Refuse userinfo
    // (`@`), les crochets IPv6, les espaces, les caractères de contrôle — tout ce qui n'est pas
    // un FQDN. (Le matching de sous-domaine ancré vit dans `domain`/`policy` ; ici on garantit
    // juste une forme propre pour ce matching.)
    if !host
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
    {
        return Err(format!(
            "proxy: host CONNECT {host:?} non-hostname (IP littérale/userinfo/charset) — refusé"
        ));
    }
    // Refuse un host qui serait un littéral IPv4 pur (ex `10.0.0.1`) : le modèle est par-domaine,
    // et le charset seul le laisserait passer (chiffres + points). Heuristique : tous les labels
    // numériques ⇒ ressemble à une IPv4 ⇒ refusé (un vrai FQDN a un TLD alphabétique).
    if host
        .rsplit('.')
        .next()
        .is_some_and(|tld| !tld.is_empty() && tld.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(format!(
            "proxy: host CONNECT {host:?} ressemble à une IP littérale — refusé (modèle par-domaine)"
        ));
    }
    let port: u16 = port_str
        .parse()
        .map_err(|_| format!("proxy: port CONNECT {port_str:?} invalide — refusé"))?;
    if port == 0 {
        return Err("proxy: port CONNECT 0 — refusé".to_string());
    }

    Ok((host.to_ascii_lowercase(), port))
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
    fn requires_a_port() {
        assert!(parse_connect_target(b"CONNECT example.com HTTP/1.1\r\n")
            .unwrap_err()
            .contains("sans port"));
        assert!(parse_connect_target(b"CONNECT example.com: HTTP/1.1\r\n")
            .unwrap_err()
            .contains("invalide")); // port vide
        assert!(
            parse_connect_target(b"CONNECT example.com:99999 HTTP/1.1\r\n")
                .unwrap_err()
                .contains("invalide")
        ); // > u16::MAX
        assert!(parse_connect_target(b"CONNECT example.com:0 HTTP/1.1\r\n")
            .unwrap_err()
            .contains("port CONNECT 0"));
    }

    #[test]
    fn rejects_non_domain_authorities() {
        // Littéral IPv4 pur (contournement d'allowlist par-domaine).
        assert!(parse_connect_target(b"CONNECT 10.0.0.1:443 HTTP/1.1\r\n")
            .unwrap_err()
            .contains("IP littérale"));
        // Littéral IPv6 (crochets hors charset).
        assert!(parse_connect_target(b"CONNECT [::1]:443 HTTP/1.1\r\n")
            .unwrap_err()
            .contains("non-hostname"));
        // Userinfo.
        assert!(
            parse_connect_target(b"CONNECT user@evil.com:443 HTTP/1.1\r\n")
                .unwrap_err()
                .contains("non-hostname")
        );
        // Host vide.
        assert!(parse_connect_target(b"CONNECT :443 HTTP/1.1\r\n")
            .unwrap_err()
            .contains("host CONNECT vide"));
    }

    #[test]
    fn is_fail_closed_on_malformed_lines() {
        // Pas de CRLF.
        assert!(parse_connect_target(b"CONNECT x:443 HTTP/1.1")
            .unwrap_err()
            .contains("sans CRLF"));
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
        // Autorité absente.
        assert!(parse_connect_target(b"CONNECT  HTTP/1.1\r\n").is_err());
        // Ligne géante (anti-DoS).
        let mut giant = b"CONNECT ".to_vec();
        giant.extend(std::iter::repeat(b'a').take(MAX_REQUEST_LINE));
        giant.extend_from_slice(b".com:443 HTTP/1.1\r\n");
        assert!(parse_connect_target(&giant)
            .unwrap_err()
            .contains("trop longue"));
        // Caractère de contrôle dans le host (anti-spoof).
        assert!(
            parse_connect_target(b"CONNECT ex\x07ample.com:443 HTTP/1.1\r\n")
                .unwrap_err()
                .contains("non-hostname")
        );
    }
}
