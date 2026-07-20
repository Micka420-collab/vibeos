//! `browser.*` — navigation web pilotée par l'IA, gouvernée (ADR-017 option C,
//! ADR-022).
//!
//! **Pure et inerte** : cette couche mappe un verbe navigateur + ses arguments vers
//! une [`BrowserAction`] validée, **agnostique du transport**. Rien ici ne lance
//! `chromium`, ne parle CDP, ni n'atteint le réseau — le transport CDP
//! (`run_browser` dans le helper) et le proxy CONNECT sont des incréments ultérieurs,
//! revus séparément. La gouvernance est EN AMONT : `[rule.domains]` (déjà câblé via
//! `derive_domain`/`CallContext.domain`) décide quel hôte `navigate` peut atteindre,
//! et le plancher de tier (navigate/read/screenshot/click/fill = T1 ; submit = T2 —
//! ADR-017 décision 2) s'applique avant qu'on arrive ici.
//!
//! **Invariant de sécurité ADR-022 encodé ici** : le sélecteur et la valeur fournis
//! par l'agent sont portés comme **DONNÉES**, destinés à un **binding CDP par objet**
//! (`DOM.querySelector` par paramètre, puis `Input.dispatch*` / `Runtime.callFunctionOn`
//! avec le nœud en `arguments`) — **jamais interpolés** dans une source
//! `Runtime.evaluate`. C'est ce qui garde `browser.evaluate` (eval JS arbitraire)
//! EXCLU au lieu de le réintroduire par la bande : cette couche refuse les formes qui
//! n'ont de sens que comme injection JS (caractères de contrôle) et remet au transport
//! des données typées et propres. Le contenu d'une page reste une **entrée hostile**
//! (invariant du modèle de menace), quel que soit le tier des clics.

// Câblage catalogue + dispatch dans mcp.rs et transport `run_browser` = incréments
// suivants ; jusque-là, cette couche pure n'est pas encore appelée.
#![allow(dead_code)]

use crate::policy::Tier;
use serde_json::Value;

/// Bornes anti-DoS sur les entrées agent (audit + surface CDP). Généreuses : un
/// sélecteur CSS ou une valeur de formulaire légitimes restent loin dessous.
const MAX_SELECTOR: usize = 1024;
const MAX_VALUE: usize = 8192;

/// Une action navigateur validée, **agnostique du transport**. Le sélecteur/la valeur
/// sont des DONNÉES pour un binding CDP par objet, jamais du JS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BrowserAction {
    /// Aller à une URL. `host` est l'hôte validé (`domain::host_of`), que
    /// `[rule.domains]` a déjà autorisé en amont ; `url` est passée telle quelle à
    /// `Page.navigate`.
    Navigate { host: String, url: String },
    /// Lire le contenu de la page courante (texte/DOM) — entrée hostile.
    Read,
    /// Capture d'écran de la page courante.
    Screenshot,
    /// Cliquer l'élément désigné par `selector` (résolu par `DOM.querySelector`).
    Click { selector: String },
    /// Saisir `value` dans l'élément `selector`.
    Fill { selector: String, value: String },
    /// Soumettre le formulaire désigné par `selector` (le seul verbe en T2).
    Submit { selector: String },
}

/// Le tier ADR-017 (décision 2) d'un verbe navigateur, ou `None` si le verbe n'est
/// pas dans la surface décidée. Source unique de vérité que le catalogue mcp.rs
/// reflète ; `submit` est le seul T2 (« agir en soumettant un formulaire »).
pub(crate) fn verb_tier(verb: &str) -> Option<Tier> {
    match verb {
        "navigate" | "read" | "screenshot" | "click" | "fill" => Some(Tier::T1),
        "submit" => Some(Tier::T2),
        _ => None,
    }
}

/// Valide un verbe `browser.*` + ses arguments et construit la [`BrowserAction`].
/// Ferme sur un verbe inconnu, un argument manquant, une URL non http(s), ou une
/// entrée agent malformée (caractère de contrôle, dépassement de borne).
///
/// `verb` est la partie APRÈS `browser.` (p.ex. `"navigate"`, `"click"`).
pub(crate) fn plan_action(verb: &str, args: &Value) -> Result<BrowserAction, String> {
    match verb {
        "navigate" => {
            let url = req_str(args, "url", verb)?;
            // host_of accepte uniquement http(s) et un hôte propre (pas d'userinfo,
            // pas d'IPv6, pas de non-ASCII) : c'est la validation d'URL, et son
            // Some(host) est exactement ce que `[rule.domains]` a autorisé en amont.
            let host = crate::domain::host_of(url).ok_or_else(|| {
                format!("browser.navigate: URL http(s) invalide ou sans hôte : {url:?}")
            })?;
            Ok(BrowserAction::Navigate {
                host,
                url: url.to_string(),
            })
        }
        "read" => Ok(BrowserAction::Read),
        "screenshot" => Ok(BrowserAction::Screenshot),
        "click" => Ok(BrowserAction::Click {
            selector: validate_selector(req_str(args, "selector", verb)?)?,
        }),
        "fill" => Ok(BrowserAction::Fill {
            selector: validate_selector(req_str(args, "selector", verb)?)?,
            value: validate_value(req_str(args, "value", verb)?)?,
        }),
        "submit" => Ok(BrowserAction::Submit {
            selector: validate_selector(req_str(args, "selector", verb)?)?,
        }),
        other => Err(format!(
            "browser: verbe inconnu {other:?} (attendus : navigate, read, screenshot, \
             click, fill, submit)"
        )),
    }
}

/// Récupère un argument chaîne requis, non vide.
fn req_str<'a>(args: &'a Value, key: &str, verb: &str) -> Result<&'a str, String> {
    match args.get(key).and_then(Value::as_str) {
        Some(s) if !s.is_empty() => Ok(s),
        Some(_) => Err(format!("browser.{verb}: '{key}' est vide")),
        None => Err(format!("browser.{verb}: '{key}' manquant")),
    }
}

/// Un sélecteur CSS est porté tel quel comme **paramètre** de `DOM.querySelector`
/// (jamais interpolé dans du JS), donc il n'a pas besoin d'échappement JS — mais on
/// le borne et on refuse les caractères de contrôle, qui n'ont de sens dans un
/// sélecteur que pour tenter une injection ou casser l'audit. On garde `[]`, `"`,
/// `.`, `#`, `>`, `:`, `=`, espaces — la ponctuation CSS légitime.
fn validate_selector(sel: &str) -> Result<String, String> {
    if sel.len() > MAX_SELECTOR {
        return Err(format!(
            "browser: sélecteur trop long ({} > {MAX_SELECTOR} octets)",
            sel.len()
        ));
    }
    if let Some(c) = sel.chars().find(|c| c.is_control()) {
        return Err(format!(
            "browser: sélecteur contient un caractère de contrôle ({:#04x}) — refusé",
            c as u32
        ));
    }
    Ok(sel.to_string())
}

/// La valeur d'un `fill` est du texte saisi par l'utilisateur, porté comme DONNÉE
/// (jamais du JS). On la borne (anti-DoS) et on refuse seulement le NUL (qui casse
/// une chaîne C côté CDP) ; le reste — accents, ponctuation, sauts de ligne d'un
/// textarea — est légitime et préservé.
fn validate_value(val: &str) -> Result<String, String> {
    if val.len() > MAX_VALUE {
        return Err(format!(
            "browser.fill: valeur trop longue ({} > {MAX_VALUE} octets)",
            val.len()
        ));
    }
    if val.contains('\0') {
        return Err("browser.fill: valeur contient un octet NUL — refusé".to_string());
    }
    Ok(val.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_surface_matches_adr_017_decision_2() {
        // navigate/read/screenshot/click/fill = T1 ; submit = T2 ; rien d'autre.
        for v in ["navigate", "read", "screenshot", "click", "fill"] {
            assert_eq!(verb_tier(v), Some(Tier::T1), "{v} doit être T1");
        }
        assert_eq!(verb_tier("submit"), Some(Tier::T2));
        // browser.evaluate n'est PAS dans la surface — exclu par construction.
        assert_eq!(verb_tier("evaluate"), None);
        assert_eq!(verb_tier("download"), None);
    }

    #[test]
    fn navigate_requires_a_valid_http_url_and_extracts_the_host() {
        let a = plan_action("navigate", &json!({"url": "https://github.com/vibeos/x"})).unwrap();
        assert_eq!(
            a,
            BrowserAction::Navigate {
                host: "github.com".to_string(),
                url: "https://github.com/vibeos/x".to_string(),
            }
        );
        // Pas d'URL => refus.
        assert!(plan_action("navigate", &json!({}))
            .unwrap_err()
            .contains("manquant"));
        // Schéma non http(s) / hôte inétablissable => refus (host_of renvoie None).
        for bad in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "not a url",
            "http://",
        ] {
            assert!(
                plan_action("navigate", &json!({ "url": bad })).is_err(),
                "doit refuser {bad:?}"
            );
        }
    }

    #[test]
    fn read_and_screenshot_take_no_args() {
        assert_eq!(
            plan_action("read", &json!({})).unwrap(),
            BrowserAction::Read
        );
        assert_eq!(
            plan_action("screenshot", &json!({})).unwrap(),
            BrowserAction::Screenshot
        );
    }

    #[test]
    fn click_fill_submit_carry_the_selector_as_data() {
        assert_eq!(
            plan_action("click", &json!({"selector": "button[type=\"submit\"]"})).unwrap(),
            BrowserAction::Click {
                selector: "button[type=\"submit\"]".to_string()
            }
        );
        assert_eq!(
            plan_action("fill", &json!({"selector": "#q", "value": "hello world"})).unwrap(),
            BrowserAction::Fill {
                selector: "#q".to_string(),
                value: "hello world".to_string()
            }
        );
        assert_eq!(
            plan_action("submit", &json!({"selector": "form#login"})).unwrap(),
            BrowserAction::Submit {
                selector: "form#login".to_string()
            }
        );
        // Sélecteur/valeur manquants => refus.
        assert!(plan_action("click", &json!({})).is_err());
        assert!(plan_action("fill", &json!({"selector": "#q"})).is_err());
    }

    #[test]
    fn control_chars_in_a_selector_are_refused_but_css_punctuation_is_kept() {
        // Un saut de ligne dans un sélecteur ne sert qu'à casser l'audit / tenter
        // une injection : refusé.
        assert!(plan_action("click", &json!({"selector": "a\nb"}))
            .unwrap_err()
            .contains("caractère de contrôle"));
        // La ponctuation CSS légitime passe.
        for sel in ["#id", ".cls", "div > a", "input[name=\"q\"]", "a:hover"] {
            assert!(
                plan_action("click", &json!({ "selector": sel })).is_ok(),
                "doit accepter {sel:?}"
            );
        }
    }

    #[test]
    fn a_fill_value_keeps_text_but_refuses_nul_and_bounds_length() {
        // Accents, ponctuation, saut de ligne (textarea) préservés.
        let a = plan_action("fill", &json!({"selector": "#c", "value": "café\nligne2"})).unwrap();
        assert_eq!(
            a,
            BrowserAction::Fill {
                selector: "#c".to_string(),
                value: "café\nligne2".to_string()
            }
        );
        // NUL refusé.
        assert!(
            plan_action("fill", &json!({"selector": "#c", "value": "a\0b"}))
                .unwrap_err()
                .contains("NUL")
        );
        // Borne de longueur.
        let long = "x".repeat(MAX_VALUE + 1);
        assert!(
            plan_action("fill", &json!({"selector": "#c", "value": long}))
                .unwrap_err()
                .contains("trop longue")
        );
    }

    #[test]
    fn an_unknown_verb_is_refused() {
        assert!(plan_action("evaluate", &json!({"expression": "1+1"}))
            .unwrap_err()
            .contains("verbe inconnu"));
        assert!(plan_action("execute", &json!({})).is_err());
    }
}
