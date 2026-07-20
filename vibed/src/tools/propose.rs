//! `os.propose` — couche **pure** de validation des propositions d'auto-modification
//! (ADR-024).
//!
//! **Pure et inerte** : transforme les arguments de l'agent en une [`Proposal`] validée
//! **fail-closed**, ou refuse. Ne lit ni n'écrit aucun store, ne touche pas à l'image, ne
//! construit ni ne signe rien. Le store (`/var/lib/vibeos/proposals/`, gouverné comme la
//! mémoire), le quota anti-self-DoS, l'audit via `record()` et le câblage catalogue sont
//! des incréments suivants (après ratification d'ADR-024).
//!
//! **La validation EST la moitié sécurité** (revue Fable 5) — tout l'aval (pipeline de
//! build, UI de revue humaine) fait confiance à ce qui sort d'ici. Donc, fail-closed :
//! - **package** : nom `^[a-z0-9][a-z0-9+._-]*$` borné ; **version au charset EVR strict**
//!   (commence alphanumérique → pas de `-flag`, aucun espace/métacaractère : sinon
//!   `version` s'injecterait comme argument/flag de `dnf install <name> <version>`, p.ex.
//!   `--nogpgcheck` **défait le pin**) ; **sha256 (64 hex minuscules) obligatoire** ;
//! - **config** : chemin normalisé, sans `..`, et **sous une racine ALLOWLISTÉE**
//!   (default-deny — un denylist sur `/etc` est le mauvais modèle : presque tout `/etc`
//!   est TCB-adjacent et la liste des vecteurs grandit à chaque paquet). Un chemin qui
//!   passe ici n'est **PAS** « sûr » : l'enforcement COMPLET est l'analyseur d'image au
//!   build (ADR-024) ; cette couche est une première ligne ;
//! - **champs libres** (justification/rollback) bornés, non-blancs, sans caractère de
//!   contrôle ni bidi/format (anti-injection ANSI / anti-spoof Trojan-Source stocké) ;
//! - **kind `policy`** (narrower une politique) **refusé explicitement** : le juger sans
//!   faux-valider exige un diff sur politique EFFECTIVE (analyseur non conçu).
//!
//! Note : le `content` d'une config reste **opaque** (borné + refus du NUL seulement —
//! c'est un corps de fichier) ; tout rendu en aval DOIT l'échapper.

// Store + quota + audit + câblage catalogue/dispatch = incréments suivants ; jusque-là
// cette couche pure de validation n'est pas encore appelée.
#![allow(dead_code)]

use serde_json::Value;

/// Bornes des champs (audit/UI + stockage). Généreuses mais finies.
const MAX_FIELD: usize = 4096; // justification / rollback
const MAX_CONTENT: usize = 65536; // corps d'un fichier de config
const MAX_PKG_NAME: usize = 128;
const MAX_VERSION: usize = 128;
const SHA256_HEX_LEN: usize = 64;

/// Racines où une config PROPOSÉE peut atterrir. **Default-deny par allowlist** (Fable
/// 5) : un denylist sur `/etc` est le mauvais modèle (presque tout `/etc` est
/// TCB-adjacent — cron, modprobe.d, sysctl.d, sshd_config, pki, /root… — et la liste
/// grandit à chaque paquet, en plus d'être contournable par `//`/`.`). Seules ces racines
/// clairement gérables et hors-TCB sont permises ; tout le reste est refusé. L'enforcement
/// COMPLET reste l'analyseur d'image au build (ADR-024).
const ALLOWED_CONFIG_PREFIXES: &[&str] = &[
    "/etc/vibeos-managed/", // aire de config dédiée, non-TCB
    "/opt/",                // applications hors image de base
];

/// Une proposition validée, **agnostique du store**. Le kind `policy` (narrower une
/// politique) est absent par conception : le juger sans faux-valider exige un diff sur
/// politique EFFECTIVE (analyseur non conçu, ADR-024) — on le **refuse** explicitement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Proposal {
    /// Ajouter un paquet depuis un dépôt allowlisté, **épinglé + hashé**.
    Package {
        name: String,
        version: String,
        sha256: String,
        justification: String,
        rollback: String,
    },
    /// Écrire un fichier de config **sous une racine allowlistée** (hors TCB/image).
    Config {
        path: String,
        content: String,
        justification: String,
        rollback: String,
    },
}

/// Valide les arguments d'une proposition d'auto-modification. Fail-closed.
pub(crate) fn parse_proposal(args: &Value) -> Result<Proposal, String> {
    let kind = req_str(args, "kind")?;
    let justification = req_free_text(args, "justification")?;
    let rollback = req_free_text(args, "rollback")?;
    match kind {
        "package" => Ok(Proposal::Package {
            name: validate_pkg_name(req_str(args, "name")?)?,
            version: validate_version(req_str(args, "version")?)?,
            sha256: validate_sha256(req_str(args, "sha256")?)?,
            justification,
            rollback,
        }),
        "config" => Ok(Proposal::Config {
            path: validate_config_path(req_str(args, "path")?)?,
            content: validate_content(req_str(args, "content")?)?,
            justification,
            rollback,
        }),
        "policy" => Err(
            "os.propose: le kind 'policy' (narrower une politique) exige un \
                         diff sur politique EFFECTIVE — analyseur non encore conçu \
                         (ADR-024), refusé"
                .to_string(),
        ),
        other => Err(format!(
            "os.propose: kind inconnu {other:?} (attendus : package, config)"
        )),
    }
}

/// Argument chaîne requis, non vide.
fn req_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    match args.get(key).and_then(Value::as_str) {
        Some(s) if !s.is_empty() => Ok(s),
        Some(_) => Err(format!("os.propose: '{key}' est vide")),
        None => Err(format!("os.propose: '{key}' manquant")),
    }
}

/// Caractères bidirectionnels / de format (Unicode Cf) : jamais légitimes dans un champ
/// de proposition, et vecteur de spoof Trojan-Source dans l'UI de revue humaine.
fn is_bidi_or_format(c: char) -> bool {
    matches!(c,
        '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2060}'
        | '\u{2066}'..='\u{2069}'
        | '\u{FEFF}')
}

/// Champ de texte libre (justification/rollback) : requis, non-blanc, borné, SANS
/// caractère de contrôle (sauf `\n`/`\t`) ni bidi/format — sinon des échappements ANSI ou
/// des marques bidi stockés injecteraient/spooferaient la ligne d'audit ou l'UI de revue
/// (Fable 5).
fn req_free_text(args: &Value, key: &str) -> Result<String, String> {
    let s = bounded(req_str(args, key)?, key, MAX_FIELD)?;
    if s.trim().is_empty() {
        return Err(format!("os.propose: '{key}' est vide (espaces seuls)"));
    }
    if let Some(c) = s
        .chars()
        .find(|c| (c.is_control() && *c != '\n' && *c != '\t') || is_bidi_or_format(*c))
    {
        return Err(format!(
            "os.propose: '{key}' contient un caractère de contrôle/format ({:#06x}) — refusé",
            c as u32
        ));
    }
    Ok(s)
}

/// Borne la longueur d'une chaîne (déjà non vide).
fn bounded(s: &str, key: &str, max: usize) -> Result<String, String> {
    if s.len() > max {
        return Err(format!(
            "os.propose: '{key}' trop long ({} > {max} octets)",
            s.len()
        ));
    }
    Ok(s.to_string())
}

/// Nom de paquet : `^[a-z0-9][a-z0-9+._-]*$`, borné.
fn validate_pkg_name(name: &str) -> Result<String, String> {
    if name.len() > MAX_PKG_NAME {
        return Err(format!(
            "os.propose: nom de paquet trop long ({} > {MAX_PKG_NAME})",
            name.len()
        ));
    }
    let first_ok = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let rest_ok = name.chars().all(|c| {
        c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '+' | '.' | '_' | '-')
    });
    if !first_ok || !rest_ok {
        return Err(format!(
            "os.propose: nom de paquet invalide {name:?} (attendu ^[a-z0-9][a-z0-9+._-]*$)"
        ));
    }
    Ok(name.to_string())
}

/// Version RPM (EVR) : commence par un **alphanumérique** (bloque un `-flag`), puis charset
/// EVR strict — AUCUN espace ni métacaractère shell/flag. Sans ça, `version` s'injecterait
/// comme argument/flag de `dnf install <name> <version>` : `--nogpgcheck` désactive la
/// vérif GPG (défait le pin), `1 evil` installe un 2e paquet non validé (Fable 5).
fn validate_version(v: &str) -> Result<String, String> {
    if v.len() > MAX_VERSION {
        return Err(format!(
            "os.propose: version trop longue ({} > {MAX_VERSION})",
            v.len()
        ));
    }
    let first_ok = v.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
    let rest_ok = v
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '+' | '~' | '^' | '_' | '-'));
    if !first_ok || !rest_ok {
        return Err(format!(
            "os.propose: version invalide {v:?} (attendu ^[0-9A-Za-z][0-9A-Za-z.:+~^_-]*$ — \
             pas de flag/espace/métacaractère)"
        ));
    }
    Ok(v.to_string())
}

/// Empreinte : exactement 64 hexadécimaux minuscules.
fn validate_sha256(h: &str) -> Result<String, String> {
    if h.len() != SHA256_HEX_LEN
        || !h
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return Err(format!(
            "os.propose: sha256 invalide (attendu {SHA256_HEX_LEN} hex minuscules)"
        ));
    }
    Ok(h.to_string())
}

/// Normalisation lexicale d'un chemin absolu : collapse `//`, retire `.`, **REFUSE** `..`
/// (aucune remontée). Renvoie le chemin canonique — sans ça, `/etc//x` ou `/etc/./x`
/// échapperaient à la comparaison de préfixe (Fable 5).
fn normalize_abs(path: &str) -> Result<String, String> {
    let mut out: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        match comp {
            "" | "." => continue,
            ".." => return Err(format!("os.propose: chemin de config avec '..' : {path:?}")),
            c => out.push(c),
        }
    }
    Ok(format!("/{}", out.join("/")))
}

/// Chemin de config : absolu, normalisé, et **sous une racine allowlistée** (default-deny).
fn validate_config_path(path: &str) -> Result<String, String> {
    if !path.starts_with('/') {
        return Err(format!(
            "os.propose: chemin de config non absolu : {path:?}"
        ));
    }
    if let Some(c) = path.chars().find(|c| c.is_control()) {
        return Err(format!(
            "os.propose: chemin de config avec caractère de contrôle ({:#04x})",
            c as u32
        ));
    }
    // Normalise AVANT de comparer (anti-`//`/`.` bypass), puis exige une racine allowlistée.
    let norm = normalize_abs(path)?;
    if !ALLOWED_CONFIG_PREFIXES.iter().any(|p| norm.starts_with(p)) {
        return Err(format!(
            "os.propose: chemin de config {norm:?} hors des racines gérées \
             {ALLOWED_CONFIG_PREFIXES:?} — refusé (default-deny : le TCB et le reste de /etc \
             ne sont pas proposables par config)"
        ));
    }
    Ok(norm)
}

/// Corps d'un fichier de config : borné, sans NUL (le reste est du contenu de fichier,
/// gardé opaque ; tout rendu en aval doit l'échapper).
fn validate_content(content: &str) -> Result<String, String> {
    if content.len() > MAX_CONTENT {
        return Err(format!(
            "os.propose: contenu trop long ({} > {MAX_CONTENT} octets)",
            content.len()
        ));
    }
    if content.contains('\0') {
        return Err("os.propose: contenu contient un octet NUL — refusé".to_string());
    }
    Ok(content.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const GOOD_SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn pkg(name: &str) -> Value {
        json!({
            "kind": "package", "name": name, "version": "1.2.3", "sha256": GOOD_SHA,
            "justification": "outil de dev", "rollback": "retirer le paquet"
        })
    }
    fn with(base: &Value, key: &str, val: Value) -> Value {
        let mut v = base.clone();
        v[key] = val;
        v
    }

    #[test]
    fn a_valid_package_proposal_parses() {
        assert_eq!(
            parse_proposal(&pkg("ripgrep")).unwrap(),
            Proposal::Package {
                name: "ripgrep".into(),
                version: "1.2.3".into(),
                sha256: GOOD_SHA.into(),
                justification: "outil de dev".into(),
                rollback: "retirer le paquet".into(),
            }
        );
    }

    #[test]
    fn a_package_requires_a_pin_and_a_valid_hash() {
        let mut v = pkg("ripgrep");
        v.as_object_mut().unwrap().remove("sha256");
        assert!(parse_proposal(&v).unwrap_err().contains("sha256"));
        for bad in ["deadbeef", &"A".repeat(64), &"g".repeat(64)] {
            assert!(parse_proposal(&with(&pkg("ripgrep"), "sha256", json!(bad)))
                .unwrap_err()
                .contains("sha256"));
        }
        let mut v2 = pkg("ripgrep");
        v2.as_object_mut().unwrap().remove("version");
        assert!(parse_proposal(&v2).unwrap_err().contains("version"));
    }

    #[test]
    fn a_bad_package_name_is_refused_and_good_ones_pass() {
        for bad in ["Ripgrep", "-rg", "rg;rm", "rg name", "rg/../x", ""] {
            assert!(parse_proposal(&pkg(bad)).is_err(), "doit refuser {bad:?}");
        }
        for ok in ["ripgrep", "gcc-c++", "python3.12", "lib_foo", "a"] {
            assert!(parse_proposal(&pkg(ok)).is_ok(), "doit accepter {ok:?}");
        }
    }

    #[test]
    fn a_version_cannot_inject_a_dnf_flag_or_second_arg() {
        // Le cas critique (Fable 5) : version ne doit pas devenir un flag/argument dnf.
        for bad in [
            "--nogpgcheck",
            "--allowerasing",
            "--releasever=rawhide",
            "1 evil-pkg",
            "1;reboot",
            "$(reboot)",
            "-1",
            "1\n2",
        ] {
            assert!(
                parse_proposal(&with(&pkg("ripgrep"), "version", json!(bad))).is_err(),
                "version dangereuse doit être refusée : {bad:?}"
            );
        }
        // Les versions RPM légitimes passent.
        for ok in ["1.2.3", "2:1.0.0-1.fc44", "1.0~rc1", "20240101"] {
            assert!(
                parse_proposal(&with(&pkg("ripgrep"), "version", json!(ok))).is_ok(),
                "version légitime doit passer : {ok:?}"
            );
        }
    }

    fn cfg(path: &str) -> Value {
        json!({
            "kind": "config", "path": path, "content": "clé = valeur\n",
            "justification": "réglage", "rollback": "supprimer le fichier"
        })
    }

    #[test]
    fn config_paths_default_deny_outside_the_allowlist_and_resist_normalization_bypass() {
        // Hors allowlist (TCB, /etc divers, /usr, /boot) => refusés.
        for bad in [
            "/etc/systemd/system/x.service",
            "/etc/cron.d/x",
            "/etc/sysctl.d/x.conf",
            "/etc/ssh/sshd_config",
            "/root/.ssh/authorized_keys",
            "/etc/vibeos/policy.d/00-x.toml",
            "/usr/bin/x",
            "/boot/x",
            "/etc/anything",
        ] {
            assert!(
                parse_proposal(&cfg(bad))
                    .unwrap_err()
                    .contains("hors des racines"),
                "doit refuser (hors allowlist) {bad:?}"
            );
        }
        // Bypass par // ou /./ vers une racine interdite => refusé (normalisation).
        for evil in [
            "/opt/../etc/systemd/x",
            "/etc//systemd/system/x",
            "/etc/./cron.d/x",
        ] {
            assert!(parse_proposal(&cfg(evil)).is_err(), "doit refuser {evil:?}");
        }
        // Non absolu => refusé.
        assert!(parse_proposal(&cfg("etc/x")).is_err());
        // Racines allowlistées => acceptées (et normalisées).
        assert_eq!(
            parse_proposal(&cfg("/opt//app/./config.toml")).unwrap(),
            Proposal::Config {
                path: "/opt/app/config.toml".into(),
                content: "clé = valeur\n".into(),
                justification: "réglage".into(),
                rollback: "supprimer le fichier".into(),
            }
        );
        assert!(parse_proposal(&cfg("/etc/vibeos-managed/app.conf")).is_ok());
    }

    #[test]
    fn the_policy_kind_is_refused_pending_the_effective_policy_analyzer() {
        let v = json!({"kind": "policy", "justification": "x", "rollback": "y"});
        assert!(parse_proposal(&v).unwrap_err().contains("analyseur"));
    }

    #[test]
    fn free_text_refuses_controls_and_bidi_but_keeps_newlines() {
        for bad in [
            "avant\u{1b}[2Japrès",
            "spoof\u{202e}gnp.exe",
            "zero\u{200b}width",
        ] {
            assert!(
                parse_proposal(&with(&pkg("ripgrep"), "justification", json!(bad)))
                    .unwrap_err()
                    .contains("contrôle/format"),
                "doit refuser {bad:?}"
            );
        }
        assert!(parse_proposal(&with(&pkg("ripgrep"), "justification", json!("l1\nl2"))).is_ok());
        // Blanc seul => refusé.
        assert!(parse_proposal(&with(&pkg("ripgrep"), "rollback", json!("   "))).is_err());
    }

    #[test]
    fn oversize_fields_and_nul_content_are_refused() {
        assert!(parse_proposal(&with(
            &pkg("ripgrep"),
            "justification",
            json!("x".repeat(MAX_FIELD + 1))
        ))
        .unwrap_err()
        .contains("trop long"));
        assert!(
            parse_proposal(&with(&cfg("/opt/x.conf"), "content", json!("a\0b")))
                .unwrap_err()
                .contains("NUL")
        );
    }
}
