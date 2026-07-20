//! `os.propose` — couche **pure** de validation des propositions d'auto-modification
//! (ADR-024).
//!
//! **Pure et inerte** : transforme les arguments de l'agent en une [`Proposal`] validée
//! **fail-closed**, ou refuse. Ne lit ni n'écrit aucun store, ne touche pas à l'image, ne
//! construit ni ne signe rien. Le store (`/var/lib/vibeos/proposals/`, gouverné comme la
//! mémoire), le quota anti-self-DoS, l'audit via `record()` et le câblage catalogue sont
//! des incréments suivants (après ratification d'ADR-024).
//!
//! **La validation EST la moitié sécurité** (revue Fable 5) — une proposition mal formée
//! est une injection stockée dans le futur pipeline de build ou l'UI de revue humaine.
//! Donc : nom de paquet borné, **pin + hash obligatoires**, chemins de config refusés
//! s'ils s'approchent du TCB (première ligne — l'enforcement COMPLET exige l'analyseur
//! d'image entière au build, ADR-024, pas ce validateur), champs libres bornés + sans
//! caractères de contrôle. Les kinds qui exigeraient l'analyseur (narrower une politique)
//! sont **refusés explicitement** plutôt que faux-validés.

// Store + quota + audit + câblage catalogue/dispatch = incréments suivants ; jusque-là
// cette couche pure de validation n'est pas encore appelée.
#![allow(dead_code)]

use serde_json::Value;

/// Bornes des champs (audit/UI + stockage). Généreuses mais finies.
const MAX_FIELD: usize = 4096; // justification / rollback / version
const MAX_CONTENT: usize = 65536; // corps d'un fichier de config
const MAX_PKG_NAME: usize = 128;
const SHA256_HEX_LEN: usize = 64;

/// Préfixes de chemins dont une config PROPOSÉE ne doit JAMAIS s'approcher : y écrire
/// changerait le comportement **runtime du TCB** sans toucher la source de vibed (Fable
/// 5), ou muterait l'image/boot. **Première ligne fail-closed** ; l'enforcement complet
/// (clôture de dépendances, sortie des scriptlets) est l'analyseur d'image entière au
/// build (ADR-024), pas ce validateur.
const FORBIDDEN_PATH_PREFIXES: &[&str] = &[
    "/usr/lib/systemd/",
    "/etc/systemd/",
    "/lib/systemd/", // unités + drop-ins
    "/etc/ld.so.preload",
    "/etc/ld.so.conf", // linker dynamique
    "/etc/pam.d/",
    "/lib/security/",
    "/usr/lib64/security/", // PAM
    "/etc/selinux/",        // SELinux
    "/etc/vibeos/",         // vibed + policy.d (le moteur qui juge)
    "/etc/profile",
    "/etc/bashrc",
    "/etc/profile.d/", // hooks shell
    "/etc/sudoers",    // sudo
    "/etc/udev/",      // règles udev
    "/etc/environment",
    "/etc/dnf/",
    "/etc/yum.repos.d/", // sources de paquets
    "/usr/",
    "/boot/",
    "/proc/",
    "/sys/",
    "/dev/", // image, boot, pseudo-fs
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
    /// Écrire un fichier de config **hors** des chemins TCB/image.
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
            version: bounded(req_str(args, "version")?, "version", MAX_FIELD)?,
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

/// Champ de texte libre (justification/rollback) : requis, borné, SANS caractère de
/// contrôle sauf `\n`/`\t` — sinon des échappements ANSI stockés injecteraient dans la
/// ligne d'audit ou l'UI de revue (Fable 5).
fn req_free_text(args: &Value, key: &str) -> Result<String, String> {
    let s = bounded(req_str(args, key)?, key, MAX_FIELD)?;
    if let Some(c) = s
        .chars()
        .find(|c| c.is_control() && *c != '\n' && *c != '\t')
    {
        return Err(format!(
            "os.propose: '{key}' contient un caractère de contrôle ({:#04x}) — refusé",
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
    let mut chars = name.chars();
    let first_ok = chars
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

/// Chemin de config : absolu, sans `..`, hors des préfixes TCB/image.
fn validate_config_path(path: &str) -> Result<String, String> {
    if !path.starts_with('/') {
        return Err(format!(
            "os.propose: chemin de config non absolu : {path:?}"
        ));
    }
    if path.contains("..") {
        return Err(format!("os.propose: chemin de config avec '..' : {path:?}"));
    }
    if let Some(c) = path.chars().find(|c| c.is_control()) {
        return Err(format!(
            "os.propose: chemin de config avec caractère de contrôle ({:#04x})",
            c as u32
        ));
    }
    if let Some(pfx) = FORBIDDEN_PATH_PREFIXES
        .iter()
        .find(|p| path.starts_with(**p))
    {
        return Err(format!(
            "os.propose: chemin de config interdit (préfixe TCB/image {pfx:?}) : {path:?} \
             — refusé (première ligne ; l'analyseur d'image au build est l'enforcement complet)"
        ));
    }
    Ok(path.to_string())
}

/// Corps d'un fichier de config : borné, sans NUL (le reste est du contenu de fichier).
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

    fn pkg(name: &str) -> Value {
        json!({
            "kind": "package", "name": name, "version": "1.2.3",
            "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "justification": "outil de dev", "rollback": "retirer le paquet"
        })
    }

    #[test]
    fn a_valid_package_proposal_parses() {
        let p = parse_proposal(&pkg("ripgrep")).unwrap();
        assert_eq!(
            p,
            Proposal::Package {
                name: "ripgrep".into(),
                version: "1.2.3".into(),
                sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
                justification: "outil de dev".into(),
                rollback: "retirer le paquet".into(),
            }
        );
    }

    #[test]
    fn a_package_requires_a_pin_and_a_valid_hash() {
        // sha256 absent.
        let mut v = pkg("ripgrep");
        v.as_object_mut().unwrap().remove("sha256");
        assert!(parse_proposal(&v).unwrap_err().contains("sha256"));
        // sha256 mal formé (trop court / majuscules).
        for bad in ["deadbeef", &"A".repeat(64)] {
            assert!(parse_proposal(&pkg_with(&pkg("ripgrep"), "sha256", bad))
                .unwrap_err()
                .contains("sha256"));
        }
        // version absente = pas de pin.
        let mut v2 = pkg("ripgrep");
        v2.as_object_mut().unwrap().remove("version");
        assert!(parse_proposal(&v2).unwrap_err().contains("version"));
    }

    #[test]
    fn a_bad_package_name_is_refused() {
        for bad in ["Ripgrep", "-rg", "rg;rm", "rg name", "rg/../x", ""] {
            assert!(parse_proposal(&pkg(bad)).is_err(), "doit refuser {bad:?}");
        }
        for ok in ["ripgrep", "gcc-c++", "python3.12", "lib_foo", "a"] {
            assert!(parse_proposal(&pkg(ok)).is_ok(), "doit accepter {ok:?}");
        }
    }

    fn cfg(path: &str) -> Value {
        json!({
            "kind": "config", "path": path, "content": "clé = valeur\n",
            "justification": "réglage", "rollback": "supprimer le fichier"
        })
    }

    #[test]
    fn a_config_targeting_the_tcb_or_image_is_refused() {
        for bad in [
            "/etc/systemd/system/x.service",
            "/usr/lib/systemd/system/vibed.service.d/x.conf",
            "/etc/ld.so.preload",
            "/etc/pam.d/sshd",
            "/etc/selinux/x",
            "/etc/vibeos/policy.d/00-x.toml",
            "/etc/profile.d/x.sh",
            "/etc/sudoers.d/x",
            "/etc/yum.repos.d/evil.repo",
            "/usr/bin/x",
            "/boot/x",
        ] {
            assert!(
                parse_proposal(&cfg(bad)).unwrap_err().contains("interdit"),
                "doit refuser {bad:?}"
            );
        }
        // Non absolu / traversée refusés.
        assert!(parse_proposal(&cfg("etc/x")).is_err());
        assert!(parse_proposal(&cfg("/etc/../usr/x"))
            .unwrap_err()
            .contains(".."));
        // Un chemin de config hors TCB passe.
        assert!(parse_proposal(&cfg("/opt/app/config.toml")).is_ok());
    }

    #[test]
    fn the_policy_kind_is_refused_pending_the_effective_policy_analyzer() {
        let v = json!({"kind": "policy", "justification": "x", "rollback": "y"});
        assert!(parse_proposal(&v).unwrap_err().contains("analyseur"));
    }

    #[test]
    fn control_chars_in_free_text_are_refused_but_newlines_pass() {
        // ESC dans la justification = injection ANSI stockée → refus.
        let mut v = pkg("ripgrep");
        v["justification"] = json!("avant\u{1b}[2Japrès");
        assert!(parse_proposal(&v)
            .unwrap_err()
            .contains("caractère de contrôle"));
        // Un saut de ligne dans la justification passe.
        let mut v2 = pkg("ripgrep");
        v2["justification"] = json!("ligne1\nligne2");
        assert!(parse_proposal(&v2).is_ok());
    }

    #[test]
    fn oversize_fields_and_nul_content_are_refused() {
        let mut v = pkg("ripgrep");
        v["justification"] = json!("x".repeat(MAX_FIELD + 1));
        assert!(parse_proposal(&v).unwrap_err().contains("trop long"));
        let mut c = cfg("/opt/x.conf");
        c["content"] = json!("a\0b");
        assert!(parse_proposal(&c).unwrap_err().contains("NUL"));
    }

    fn pkg_with(base: &Value, key: &str, val: &str) -> Value {
        let mut v = base.clone();
        v[key] = json!(val);
        v
    }
}
