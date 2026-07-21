//! `browser.*` — navigation web pilotée par l'IA, gouvernée (ADR-017 option C,
//! ADR-022).
//!
//! **Agnostique du transport** : cette couche mappe un verbe navigateur + ses arguments
//! vers une [`BrowserAction`] validée ([`plan_action`]), parse un **batch** d'actions
//! ([`parse_batch`]) et le **pilote** contre une [`CdpSession`] abstraite ([`run_batch`] :
//! `attach_page` une fois, exécution en séquence, trois états, fail-closed, agrégat borné).
//! Elle ne **lance** pas `chromium` (`browser_transport::spawn_chromium`), ne possède pas les
//! fds/stdin-stdout du helper (le mode-entry `run_browser`, incrément suivant), et n'atteint
//! pas le réseau — testée de bout en bout avec un `CdpChannel` factice, sans `chromium`. Le
//! proxy CONNECT et le câblage dispatch sont des incréments ultérieurs. La gouvernance est
//! EN AMONT dans `vibed` : `[rule.domains]` (déjà câblé via
//! `derive_domain`/`CallContext.domain`) décide quel hôte `navigate` peut atteindre,
//! et le plancher de tier (navigate/read/screenshot/click/fill = T1 ; submit = T2 —
//! ADR-017 décision 2) s'applique avant qu'on arrive ici.
//!
//! **Invariant de sécurité ADR-022** : le sélecteur et la valeur fournis par l'agent
//! sont portés comme **DONNÉES**, destinés à un **binding CDP par objet**
//! (`DOM.querySelector` par paramètre, puis `Input.dispatch*` / `Runtime.callFunctionOn`
//! avec le nœud en `arguments`) — **jamais interpolés** dans une source
//! `Runtime.evaluate`. C'est ce binding, DANS LE TRANSPORT, qui garde `browser.evaluate`
//! (eval JS arbitraire) exclu.
//!
//! ⚠️ Cette couche **ne peut PAS** rendre sûr un transport qui interpolerait : les
//! caractères dangereux en JS (`"`, `'`, `` ` ``, `${`, `(`, `.`) sont **acceptés ici**
//! car un sélecteur CSS légitime en a besoin (`input[name="q"]`, `:has(...)`). Aucun
//! filtrage de sélecteur ne peut donc empêcher l'injection JS sans casser CSS — le
//! binding par-objet est la **seule** défense, et le transport DOIT l'appliquer
//! (invariant à réaffirmer à l'incrément transport ; à y ajouter aussi la ré-assertion
//! `host_of(url) == host` à la navigation — `derive_domain` extrait déjà l'hôte via le
//! MÊME `host_of`, donc gouvernance et exécution voient le même hôte). Ce que la
//! validation ci-dessous protège, c'est la **surface d'audit/CDP** (bornes ; refus des
//! caractères de contrôle et bidi qui spoofent la ligne d'audit), **pas** l'injection
//! JS. Le contenu d'une page reste une **entrée hostile** quel que soit le tier des
//! clics.

// Câblage catalogue + dispatch dans mcp.rs et transport `run_browser` = incréments
// suivants ; jusque-là, cette couche pure n'est pas encore appelée.
#![allow(dead_code)]

use crate::cdp::{CdpChannel, CdpSession};
use crate::policy::Tier;
use serde_json::{json, Value};

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
            // La valeur DOIT être présente mais PEUT être vide : `fill` avec `""` vide
            // légitimement un champ (Fable 5).
            value: validate_value(present_str(args, "value", verb)?)?,
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

/// Comme `req_str` mais autorise la chaîne vide — pour la valeur d'un `fill`, où `""`
/// vide légitimement un champ.
fn present_str<'a>(args: &'a Value, key: &str, verb: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("browser.{verb}: '{key}' manquant"))
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
    // Refuse les caractères de contrôle (Cc) ET les marques bidi/format (Cf : RTL
    // override U+202E, isolats, ZWSP, BOM…) : aucun n'a de sens dans un sélecteur CSS,
    // et tous cassent ou spoofent la ligne d'audit. `char::is_control()` ne couvre que
    // Cc, d'où le second test (Fable 5).
    if let Some(c) = sel
        .chars()
        .find(|c| c.is_control() || is_bidi_or_format(*c))
    {
        return Err(format!(
            "browser: sélecteur contient un caractère de contrôle/format ({:#06x}) — refusé",
            c as u32
        ));
    }
    Ok(sel.to_string())
}

/// Marques bidirectionnelles et caractères de format (catégorie Unicode Cf) qui n'ont
/// jamais de sens dans un sélecteur CSS mais spoofent le rendu de la ligne d'audit.
/// Liste explicite plutôt qu'une dépendance à une table Unicode.
fn is_bidi_or_format(c: char) -> bool {
    matches!(c,
        '\u{200B}'..='\u{200F}'      // ZWSP/ZWNJ/ZWJ, LRM/RLM
        | '\u{202A}'..='\u{202E}'    // LRE/RLE/PDF/LRO/RLO
        | '\u{2060}'                 // word joiner
        | '\u{2066}'..='\u{2069}'    // isolats LRI/RLI/FSI/PDI
        | '\u{FEFF}') // BOM / ZWNBSP
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
    // Refuse les caractères de contrôle C0/C1 (NUL, mais aussi ESC/BEL/CR/BS…) : rendus
    // dans la ligne d'audit de l'opérateur, ils y injecteraient des séquences terminal —
    // or l'audit EST le mécanisme de gouvernance sur un insider non fiable, le corrompre
    // est l'attaque, pas un détail cosmétique (Fable 5). On garde `\n`/`\t` (légitimes
    // dans un textarea) et les marques bidi (légitimes en texte RTL) ; l'échappement des
    // non-imprimables reste une exigence du rendu d'audit (ceinture + bretelles).
    if let Some(c) = val
        .chars()
        .find(|c| c.is_control() && *c != '\n' && *c != '\t')
    {
        return Err(format!(
            "browser.fill: valeur contient un caractère de contrôle ({:#04x}) — refusé",
            c as u32
        ));
    }
    Ok(val.to_string())
}

/// L'issue d'une action navigateur — **trois états** (ADR-022 addendum ratifié, Fable 5). Le type
/// force `submit` à traiter explicitement `Indeterminate` : un POST parti sans réponse ne doit
/// JAMAIS être `Failed` (sinon double-POST au retry). Les verbes non mutants
/// (navigate/read/screenshot/click/fill — les click-submitters étant bloqués côté click) ne
/// produisent que `Completed`/`Failed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ActionOutcome {
    /// Réussie ; `Value` = résultat JSON **opaque** (contenu de page hostile).
    Completed(Value),
    /// Échouée AVANT tout effet distant observable (resolve raté, exception page, refus vibed).
    Failed(String),
    /// `submit` uniquement : le `callFunctionOn` du submit a échoué APRÈS émission (flux CDP cassé)
    /// — le POST a PEUT-ÊTRE tiré. Ni `Completed` (pas de confirmation) ni `Failed` (retry =
    /// double-POST) : l'appelant NE DOIT PAS ré-exécuter le batch.
    Indeterminate(String),
}

#[cfg(test)]
impl ActionOutcome {
    /// Test helper : le `Value` d'un `Completed` (panique sinon). Nommé `unwrap` pour que les
    /// tests existants (`run_action(...).unwrap()`) marchent tels quels après le passage à
    /// `ActionOutcome`.
    fn unwrap(self) -> Value {
        match self {
            ActionOutcome::Completed(v) => v,
            other => panic!("attendu Completed, obtenu {other:?}"),
        }
    }
    /// Test helper : le message d'un `Failed` (panique sinon).
    fn unwrap_err(self) -> String {
        match self {
            ActionOutcome::Failed(e) => e,
            other => panic!("attendu Failed, obtenu {other:?}"),
        }
    }
}

/// Exécute une [`BrowserAction`] et rend son issue à **trois états** ([`ActionOutcome`]). `submit`
/// (T2, POST mutant) suit un chemin propre ([`run_submit`]) qui peut rendre `Indeterminate` ; les
/// autres verbes délèguent à [`run_simple_action`] (mappé `Ok → Completed`, `Err → Failed`).
/// Refuse d'emblée un `page` (sessionId) **vide** — jamais produit par `attach_page`, donne une
/// erreur vibed claire plutôt qu'un refus chromium cryptique (Fable 5).
pub(crate) fn run_action<C: CdpChannel>(
    session: &mut CdpSession<C>,
    action: &BrowserAction,
    page: &str,
) -> ActionOutcome {
    if page.is_empty() {
        return ActionOutcome::Failed(
            "browser: run_action sans sessionId de page — refusé".to_string(),
        );
    }
    // click/fill/submit : binding par-objet à TROIS états (effet distant possible — POST d'un
    // submit, d'un `onclick`/`onchange`). navigate/read/screenshot : sans effet distant via
    // binding → `Ok`/`Err` simple.
    match action {
        BrowserAction::Click { selector } => run_object_action(
            session,
            page,
            "click",
            selector,
            CLICK_FN,
            json!([]),
            json!({ "clicked": true }),
        ),
        BrowserAction::Fill { selector, value } => run_object_action(
            session,
            page,
            "fill",
            selector,
            FILL_FN,
            json!([{ "value": value }]),
            json!({ "filled": true }),
        ),
        BrowserAction::Submit { selector } => run_object_action(
            session,
            page,
            "submit",
            selector,
            SUBMIT_FN,
            json!([]),
            json!({ "submitted": true }),
        ),
        _ => match run_simple_action(session, action, page) {
            Ok(v) => ActionOutcome::Completed(v),
            Err(e) => ActionOutcome::Failed(e),
        },
    }
}

/// Exécute un verbe navigateur **sans mutation distante** (navigate/read/screenshot/click/fill)
/// contre une session CDP et renvoie un résultat JSON **opaque** (le contenu vient d'une page
/// hostile). `submit` est routé vers [`run_submit`] par [`run_action`] et n'atteint jamais ici.
///
/// Verbes SANS sélecteur — `navigate`/`read`/`screenshot` : leurs commandes CDP ne portent
/// aucune entrée agent interpolée (URL en paramètre, hôte déjà validé ; expressions
/// `Runtime.evaluate` CONSTANTES). `click`/`fill` : **binding CDP par-objet** ([`resolve_object`]
/// → [`call_on_object`]) — le sélecteur est un **paramètre** de `DOM.querySelector`, le nœud est
/// lié en `this` d'une fonction **constante**, la valeur d'un `fill` est un **argument** ;
/// **jamais** d'interpolation d'entrée agent dans une source `evaluate` (invariant ADR-022 qui
/// garde `browser.evaluate` exclu). `submit` (T2, POST mutant) reste différé : il exige le
/// refactor à **trois états** (`indeterminate` pour un POST parti sans réponse).
///
/// Toutes les commandes de page portent le `sessionId` de la **page** attachée (`page`,
/// produit par [`crate::browser_transport::attach_page`]) : sur `--remote-debugging-pipe`,
/// le vrai chromium exige ce `sessionId` pour `Page.*`/`Runtime.*` (protocole plat) — sans
/// lui il refuse tout. C'était le finding Fable 5 (le code utilisait `session_id: None`, la
/// cible navigateur) ; il est résolu ici en threadant `page` dans chaque `session.call`.
/// `page` vient de `vibed`/du transport (jamais de l'agent) et est du JSON opaque du pair —
/// porté comme paramètre CDP, jamais interpolé.
///
/// **Invariant à ré-asserter au transport (Fable 5)** : `run_browser` — l'appelant unique —
/// DOIT passer le `sessionId` de l'attach de SA propre session ; sinon l'audit attribuerait
/// l'action à la mauvaise page. Non vérifiable ici (`page` est opaque du pair), d'où l'invariant.
///
/// Reste à porter par l'incrément live (Fable 5) : la **synchronisation sur le chargement**
/// (`Page.loadEventFired` filtré sur le `frameId` renvoyé) doit atterrir AVANT le câblage
/// live — sinon un `read` juste après un `navigate` peut capturer la page PRÉCÉDENTE
/// (intégrité d'attribution ; le cas de la redirection hors-allowlist, lui, est bloqué à
/// l'egress par le proxy, pas ici — bon découpage).
fn run_simple_action<C: CdpChannel>(
    session: &mut CdpSession<C>,
    action: &BrowserAction,
    page: &str,
) -> Result<Value, String> {
    match action {
        BrowserAction::Navigate { host, url } => {
            // Ré-assertion de cohérence (promesse de l'en-tête) : l'hôte que la
            // gouvernance a vu (host, via host_of) DOIT être celui de l'URL exécutée.
            // Tient par construction (plan_action est le seul constructeur), mais le
            // type pub(crate) permettrait un Navigate incohérent d'un futur appelant,
            // et l'audit attribuerait alors la nav au mauvais hôte (Fable 5).
            if crate::domain::host_of(url).as_deref() != Some(host.as_str()) {
                return Err("browser.navigate: incohérence hôte/URL — refusé".to_string());
            }
            // Page.enable (pour les events de chargement à venir) puis Page.navigate.
            // `url` est un PARAMÈTRE CDP, jamais interpolé ; son hôte a déjà passé
            // `[rule.domains]` en amont. (La synchronisation sur le chargement complet —
            // attendre Page.loadEventFired — est un raffinement du prochain incrément.)
            session.call("Page.enable", json!({}), Some(page))?;
            let r = session.call("Page.navigate", json!({ "url": url }), Some(page))?;
            // Une navigation refusée par le navigateur remonte dans `errorText` — c'est du
            // texte du PAIR (hostile), repris dans une erreur vibed que l'audit affiche :
            // assaini (Cc/Cf, borné) pour ne pas laisser un chromium spoofer la ligne d'audit.
            if let Some(err) = r.get("errorText").and_then(Value::as_str) {
                return Err(format!(
                    "browser.navigate: navigation refusée : {}",
                    crate::cdp::sanitize_peer_text(err)
                ));
            }
            Ok(json!({ "navigated": url, "frameId": r.get("frameId") }))
        }
        BrowserAction::Read => {
            // Expression CONSTANTE (aucune entrée agent) : aucune surface d'injection.
            let r = session.call(
                "Runtime.evaluate",
                json!({
                    "expression": "document.documentElement.outerHTML",
                    "returnByValue": true,
                }),
                Some(page),
            )?;
            // FAIL-CLOSED (Fable 5) : une page hostile peut faire LEVER l'expression
            // (getters piégés sur documentElement/outerHTML) → CDP répond « succès »
            // avec `exceptionDetails` et un `result` SANS `value`. Sans ce contrôle on
            // renverrait Ok({html:""}) = CLOAKING : la page se présente vide à l'agent
            // qui audite tout en s'affichant normalement à l'humain, et la décision
            // suivante s'appuie sur une observation forgée.
            if r.get("exceptionDetails").is_some() {
                return Err(
                    "browser.read: l'évaluation a levé une exception côté page — refusé"
                        .to_string(),
                );
            }
            let html = r
                .get("result")
                .and_then(|x| x.get("value"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    "browser.read: résultat CDP sans chaîne 'value' — refusé".to_string()
                })?;
            // Contenu de page = ENTRÉE HOSTILE, renvoyé comme donnée opaque.
            Ok(json!({ "html": html }))
        }
        BrowserAction::Screenshot => {
            let r = session.call(
                "Page.captureScreenshot",
                json!({ "format": "png" }),
                Some(page),
            )?;
            // Fail-closed : une donnée absente/vide/non-chaîne est une erreur, pas une
            // capture « vide » silencieuse (Fable 5).
            let data = r
                .get("data")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    "browser.screenshot: réponse CDP sans données PNG — refusé".to_string()
                })?;
            Ok(json!({ "screenshot_png_base64": data }))
        }
        // click/fill/submit sont routés vers `run_object_action` par `run_action` (chemin à trois
        // états) et n'atteignent JAMAIS ici. Refus DÉFENSIF plutôt qu'un `unreachable!`/panic (Fable
        // 5) — un panic dans un daemon sécurité est pire qu'une erreur si un futur appelant interne
        // les envoyait par erreur.
        BrowserAction::Click { .. } | BrowserAction::Fill { .. } | BrowserAction::Submit { .. } => {
            Err(
                "browser: verbe d'interaction routé hors de run_simple_action — bug interne"
                    .to_string(),
            )
        }
    }
}

/// `click` : fonction CONSTANTE. Garde de TIER (Fable 5) — refuse un SUBMITTER de formulaire
/// (`<button>` submit/défaut, `<input type=submit|image>`, ou un DESCENDANT via `closest`) : le
/// POST mutant passe par `browser.submit` (T2). ⚠️ Une navigation par click (`<a href>`) n'est PAS
/// gouvernée par `[rule.domains]` de `navigate` — c'est le PROXY egress qui la contrôle.
const CLICK_FN: &str =
    "function() { const b = this.closest ? this.closest('button, input') : null; \
     if (b && b.form) { const t = (b.getAttribute('type') || \
     (b.tagName === 'BUTTON' ? 'submit' : '')).toLowerCase(); \
     if ((b.tagName === 'BUTTON' && t === 'submit') || \
     (b.tagName === 'INPUT' && (t === 'submit' || t === 'image'))) \
     throw new Error('submitter de formulaire — utilisez browser.submit (T2)'); } \
     this.click(); }";

/// `fill` : fonction CONSTANTE ; la valeur est `arguments[0]`, JAMAIS interpolée. Garde
/// anti-cloaking (Fable 5) : une cible sans propriété `value` (contenteditable, générique) échoue
/// au lieu de mentir `filled: true`.
const FILL_FN: &str = "function(v) { if (!('value' in this)) \
     throw new Error('cible sans propriété value — fill inapplicable'); \
     this.focus(); this.value = v; \
     this.dispatchEvent(new Event('input', { bubbles: true })); \
     this.dispatchEvent(new Event('change', { bubbles: true })); }";

/// `submit` : fonction CONSTANTE ; exige un `<form>` puis `this.submit()` (soumission DOM brute,
/// sans donner la main à un `onsubmit` de page hostile — choix de gouvernance).
const SUBMIT_FN: &str =
    "function() { if (this.tagName !== 'FORM') throw new Error('cible pas un <form>'); \
     this.submit(); }";

/// Cœur commun de **click/fill/submit** : le **binding par-objet** (`resolve_object` → `callFunctionOn`
/// avec le nœud lié en `this` d'une fonction CONSTANTE) ET la sémantique à **trois états** (Fable 5).
/// Ces trois verbes peuvent avoir un **effet distant** (POST d'un submit, d'un `onclick`/`onchange`)
/// — d'où la distinction :
/// - `resolve_object` raté, ou `exceptionDetails` (la fonction a levé) → **`Failed`** (rien n'a été
///   exécuté côté page) ;
/// - `callFunctionOn` échoué **APRÈS émission** (flux CDP cassé ⇒ session **empoisonnée**) →
///   **`Indeterminate`** : l'effet distant a PEUT-ÊTRE eu lieu → **NE PAS réessayer** (double-effet) ;
/// - `callFunctionOn` en `Err` **sans** poison → erreur applicative CDP (commande rejetée AVANT
///   exécution) → **`Failed`**.
///
/// **Invariant load-bearing (Fable 5)** : la classification `Failed` du bras applicatif n'est correcte
/// que parce que `function_declaration` est **synchrone** et `awaitPromise: false` — Chrome sérialise
/// alors le `result` AVANT toute destruction de contexte (navigation committée). Rendre la fonction
/// `async` / `awaitPromise: true` ferait qu'une erreur « context destroyed » APRÈS un effet distant
/// arriverait en `Cdp` (non empoisonné) → `Failed` → double-effet. **Ne pas changer sans reclasser.**
///
/// **`success_body` n'est PAS un effet confirmé (Fable 5)** : `clicked`/`filled`/`submitted: true`
/// signifie « la fonction a été invoquée sans lever », PAS « le POST/effet réseau a abouti »
/// (`HTMLFormElement.submit()` sur un form détaché est un no-op silencieux ; `tagName` est spoofable).
/// L'aval NE DOIT PAS traiter ce booléen comme une confirmation de mutation.
fn run_object_action<C: CdpChannel>(
    session: &mut CdpSession<C>,
    page: &str,
    verb: &str,
    selector: &str,
    function_declaration: &str,
    arguments: Value,
    success_body: Value,
) -> ActionOutcome {
    // resolve : tout échec ici est AVANT toute exécution de la fonction → Failed (rien n'est parti).
    let object_id = match resolve_object(session, page, selector) {
        Ok(o) => o,
        Err(e) => return ActionOutcome::Failed(format!("browser.{verb}: {e}")),
    };
    let r = session.call(
        "Runtime.callFunctionOn",
        json!({
            "objectId": object_id,
            "functionDeclaration": function_declaration,
            "arguments": arguments,
            "returnByValue": true,
            "awaitPromise": false,
        }),
        Some(page),
    );
    match r {
        Ok(resp) => {
            if resp.get("exceptionDetails").is_some() {
                ActionOutcome::Failed(format!("browser.{verb}: exception côté page — refusé"))
            } else {
                ActionOutcome::Completed(success_body)
            }
        }
        Err(e) => {
            let e = crate::cdp::sanitize_peer_text(&e);
            if session.is_poisoned() {
                ActionOutcome::Indeterminate(format!(
                    "browser.{verb}: flux CDP cassé APRÈS émission — effet distant indéterminé, NE \
                     PAS réessayer : {e}"
                ))
            } else {
                ActionOutcome::Failed(format!("browser.{verb}: {e}"))
            }
        }
    }
}

/// Résout `selector` en un `objectId` CDP **sans jamais interpoler** le sélecteur dans du JS :
/// `DOM.getDocument` → `DOM.querySelector` (le sélecteur est un **paramètre**) → `DOM.resolveNode`.
/// C'est le cœur du **binding par-objet** (ADR-022) qui garde `browser.evaluate` exclu : le nœud
/// devient un `objectId` que [`call_on_object`] liera en `this` d'une fonction **constante**.
/// Fail-closed : `nodeId == 0` (rien ne matche — `querySelector` renvoie 0, pas une erreur CDP),
/// ou une réponse sans `nodeId`/`objectId`, est une erreur.
fn resolve_object<C: CdpChannel>(
    session: &mut CdpSession<C>,
    page: &str,
    selector: &str,
) -> Result<String, String> {
    let doc = session.call("DOM.getDocument", json!({ "depth": 0 }), Some(page))?;
    let root = doc
        .get("root")
        .and_then(|r| r.get("nodeId"))
        .and_then(Value::as_i64)
        .ok_or_else(|| "browser: DOM.getDocument sans nodeId racine — refusé".to_string())?;
    // Le sélecteur est un PARAMÈTRE de querySelector — jamais du JS interpolé.
    let found = session.call(
        "DOM.querySelector",
        json!({ "nodeId": root, "selector": selector }),
        Some(page),
    )?;
    let node_id = found
        .get("nodeId")
        .and_then(Value::as_i64)
        .ok_or_else(|| "browser: DOM.querySelector sans nodeId — refusé".to_string())?;
    if node_id == 0 {
        return Err("browser: aucun élément ne matche le sélecteur — refusé".to_string());
    }
    let resolved = session.call("DOM.resolveNode", json!({ "nodeId": node_id }), Some(page))?;
    resolved
        .get("object")
        .and_then(|o| o.get("objectId"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "browser: DOM.resolveNode sans objectId — refusé".to_string())
}

/// Cap DUR d'actions par batch (ADR-022 addendum ratifié, Fable 5) : la garantie « chromium
/// jeté par appel » s'érode avec la longueur — un batch de 200 actions serait un `chromium`
/// hostile vivant longtemps dans une unité approuvée **une fois**. On borne à un petit nombre.
pub(crate) const MAX_BATCH_STEPS: usize = 16;

/// Borne de l'**agrégat** remonté (ADR-022 addendum, Fable 5) : borner chaque résultat ne
/// suffit pas ; 16 lectures bâtiraient un JSON que le cap du pipe tronquerait salement. On
/// arrête le batch dès que le cumul dépasse ce seuil ET qu'il reste des étapes (voir `run_batch`).
const MAX_BATCH_RESULT_BYTES: usize = 4 * 1024 * 1024;

/// Borne **par étape** (Fable 5) : le cap d'agrégat seul est crevable par un **unique** body
/// géant (une page hostile servant ~16 Mio d'outerHTML passe sous `MAX_FRAME` du codec). On
/// borne donc chaque résultat AVANT de l'agréger — fail-closed : un body sur-taille devient une
/// étape `failed`, l'agent ne reçoit pas de données hostiles partielles.
const MAX_STEP_RESULT_BYTES: usize = 4 * 1024 * 1024;

/// L'état d'une action dans un batch (ADR-022 addendum, Fable 5). Le type à **trois** variantes
/// force le futur incrément `submit` à traiter explicitement `Indeterminate` — le mapping
/// `Err → Failed` du choke point de `run_batch` ne peut pas produire un double-POST en silence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepStatus {
    Completed,
    Failed,
    /// **RÉSERVÉ** : un `submit` dont le POST est parti mais dont la réponse a timeout. **Jamais**
    /// `Failed` (sinon double-POST au retry). Aucun verbe actuel ne le produit ; quand `submit`
    /// atterrira, `run_action` devra distinguer « échec AVANT émission » (`Failed`) d'« échec
    /// APRÈS émission réseau » (`Indeterminate`) — cela changera la signature de `run_action`.
    Indeterminate,
}

impl StepStatus {
    fn as_str(self) -> &'static str {
        match self {
            StepStatus::Completed => "completed",
            StepStatus::Failed => "failed",
            StepStatus::Indeterminate => "indeterminate",
        }
    }
}

/// Un batch d'actions navigateur validées, exécuté par un **seul** `chromium` (modèle de
/// session **batch éphémère**, ADR-022 addendum ratifié). Chaque action est déjà passée par
/// [`plan_action`]. Les étapes `assert` déclaratives (vérif-avant-`submit`) sont un incrément
/// ultérieur — non incluses ici pour ne pas porter un type inexécutable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrowserBatch {
    pub steps: Vec<BrowserAction>,
}

/// Clés JSON autorisées pour un verbe donné, ou `None` si le verbe est inconnu (auquel cas on
/// laisse [`plan_action`] produire l'erreur « verbe inconnu » claire, sans la masquer).
fn allowed_step_keys(verb: &str) -> Option<&'static [&'static str]> {
    Some(match verb {
        "navigate" => &["verb", "url"],
        "read" | "screenshot" => &["verb"],
        "click" | "submit" => &["verb", "selector"],
        "fill" => &["verb", "selector", "value"],
        _ => return None,
    })
}

/// Parse le **payload de contrôle** du batch (ce que `vibed` descend au helper sur stdin) :
/// `{ "steps": [ { "verb": "navigate", "url": "…" }, … ] }`. **Fail-closed** : refuse un
/// `steps` absent / non-tableau / **vide** / au-delà de [`MAX_BATCH_STEPS`], une étape sans
/// `verb`, une **clé inconnue** (Fable 5 : sinon une étape `{"verb":"read","assert":…}` d'un
/// futur incrément passerait comme un `Read` nu — la vérif que l'agent croit avoir demandée ne
/// tournerait jamais, en silence), ou une étape mal formée (déléguée à [`plan_action`]). La
/// gouvernance (tier = max, `[rule.domains]` par `navigate`, screening `fill`, approbation T2)
/// est **en amont dans `vibed`**, avant l'envoi.
pub(crate) fn parse_batch(payload: &Value) -> Result<BrowserBatch, String> {
    let steps_json = payload
        .get("steps")
        .and_then(Value::as_array)
        .ok_or_else(|| "browser.batch: 'steps' manquant ou pas un tableau — refusé".to_string())?;
    if steps_json.is_empty() {
        return Err("browser.batch: batch vide — refusé".to_string());
    }
    if steps_json.len() > MAX_BATCH_STEPS {
        return Err(format!(
            "browser.batch: {} étapes > cap {MAX_BATCH_STEPS} — refusé (confinement)",
            steps_json.len()
        ));
    }
    let mut steps = Vec::with_capacity(steps_json.len());
    for (i, s) in steps_json.iter().enumerate() {
        let verb = s
            .get("verb")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("browser.batch: étape {i} sans 'verb' — refusé"))?;
        // Whitelist de clés (fail-closed) — SEULEMENT pour un verbe connu, pour ne pas masquer
        // le « verbe inconnu » de plan_action ci-dessous.
        if let Some(allowed) = allowed_step_keys(verb) {
            if let Some(obj) = s.as_object() {
                for k in obj.keys() {
                    if !allowed.contains(&k.as_str()) {
                        return Err(format!(
                            "browser.batch: étape {i} ({verb}) : clé inconnue {k:?} — refusé"
                        ));
                    }
                }
            }
        }
        steps.push(plan_action(verb, s).map_err(|e| format!("browser.batch: étape {i}: {e}"))?);
    }
    // (Fable 5 F1, BLOQUANT) `submit` (POST mutant) DOIT être la DERNIÈRE étape (donc ≤1 par batch).
    // Sinon un `submit` COMPLETED peut précéder une étape qui échoue → le batch devient "failed"
    // (contractuellement REJOUABLE) alors que le POST a DÉJÀ tiré → double-POST, déclenchable par
    // une page de confirmation hostile qui lève à la lecture (`read` fail-closed → step Failed).
    // Terminal ⇒ un `submit` completed est toujours le dernier ⇒ batch "completed" (jamais rejoué) ;
    // un `submit` failed/indeterminate en dernier ⇒ aucun submit antérieur n'a tiré ⇒ "failed"
    // redevient idempotent et "indeterminate" porte seul le drapeau NE-PAS-RÉESSAYER.
    if let Some(pos) = steps
        .iter()
        .position(|s| matches!(s, BrowserAction::Submit { .. }))
    {
        if pos != steps.len() - 1 {
            return Err(
                "browser.batch: 'submit' doit être la DERNIÈRE étape du batch (≤1 par batch, \
                 anti double-POST) — refusé"
                    .to_string(),
            );
        }
    }
    Ok(BrowserBatch { steps })
}

/// Taille sérialisée (octets) d'un `Value`, pour les bornes. Un `Value` déjà construit se
/// sérialise toujours (clés String, ni NaN ni Inf) ; en cas d'échec improbable on renvoie
/// `usize::MAX` — **fail-closed** (un body non sérialisable trébuche sur le cap par-étape).
fn json_size(v: &Value) -> usize {
    serde_json::to_vec(v).map(|b| b.len()).unwrap_or(usize::MAX)
}

/// Exécute un [`BrowserBatch`] contre une `CdpSession` (un `chromium`, une page) et rend un
/// résultat JSON **agrégé** et **opaque** (le contenu `result` vient de pages hostiles). Modèle
/// **batch éphémère** (ADR-022 addendum ratifié) :
/// - **`attach_page` UNE fois** → le `sessionId` de page threadé dans chaque [`run_action`] ;
/// - exécution **en séquence**, continuité au sein du batch (le `read` voit ce que le `navigate`
///   précédent a chargé — c'est TOUT l'intérêt du batch vs process-par-verbe) ;
/// - **trois états par action** ([`StepStatus`]) : `completed`, `failed`, et `indeterminate`
///   réservé au futur `submit` mutant (les verbes actuels sont sans mutation distante) ;
/// - **fail-closed** : une action non `completed` **arrête** le batch (les suivantes ne tirent
///   pas — l'agent avait planifié en supposant le succès des précédentes) ;
/// - **borne par étape** : un `result` sur-taille ([`MAX_STEP_RESULT_BYTES`]) devient `failed` ;
/// - **borne d'agrégat** : `truncated` **seulement s'il reste des étapes non tirées** (Fable 5 :
///   un batch entièrement complété ne doit JAMAIS rendre `truncated`, ça inviterait un
///   ré-exécution). Le cap du pipe côté `vibed` reste le garde-fou dur ultime.
///
/// **Consomme** la session (par valeur, Fable 5) : un batch = une session = un `chromium` jetable
/// ; on ne peut pas ré-exécuter un batch sur la même session (contrat d'`attach_page`). Ne panique
/// jamais : rend toujours un JSON décrivant l'état réel (même un `attach` raté). `steps_total`
/// rend le résultat **auto-porteur** (l'audit distingue « 2/2 complété » de « arrêté à 2/5 »).
pub(crate) fn run_batch<C: CdpChannel>(mut session: CdpSession<C>, batch: &BrowserBatch) -> Value {
    let total = batch.steps.len();
    // Défense EN PROFONDEUR (Fable 5) — l'invariant « submit TERMINAL » (anti double-POST) vit dans
    // `parse_batch`, mais `BrowserBatch.steps` est `pub(crate)` : on le RÉ-ASSERTE ici, exactement
    // comme `run_simple_action` ré-asserte la cohérence hôte/URL d'un `Navigate`. Sans ça, un futur
    // constructeur de batch (câblage transport, refactor) qui contournerait `parse_batch` ferait
    // s'évaporer la garantie en silence. Un `submit` non-terminal ⇒ batch failed SANS rien exécuter
    // (aucun POST tiré), avant même d'attacher/spawner.
    if let Some(pos) = batch
        .steps
        .iter()
        .position(|s| matches!(s, BrowserAction::Submit { .. }))
    {
        if pos != total - 1 {
            return json!({
                "batch": "failed", "attached": false,
                "error": "browser.batch: 'submit' non-terminal — refusé (anti double-POST ; \
                          invariant de parse_batch ré-asserté)",
                "steps": [], "steps_total": total,
            });
        }
    }
    let page = match crate::browser_transport::attach_page(&mut session) {
        Ok(p) => p,
        Err(e) => {
            return json!({
                "batch": "failed", "attached": false, "error": e,
                "steps": [], "steps_total": total,
            });
        }
    };

    let mut results = Vec::with_capacity(total);
    let mut agg_bytes = 0usize;
    let mut batch_status = "completed";

    for (i, action) in batch.steps.iter().enumerate() {
        let (status, body, size) = match run_action(&mut session, action, &page) {
            ActionOutcome::Completed(v) => {
                let n = json_size(&v);
                if n > MAX_STEP_RESULT_BYTES {
                    // Borne par étape (Fable 5) : on ne pousse PAS le body hostile sur-taille.
                    let e = json!({ "error": format!(
                        "browser.batch: résultat d'étape trop grand ({n} octets > cap \
                         {MAX_STEP_RESULT_BYTES}) — refusé"
                    ) });
                    let en = json_size(&e);
                    (StepStatus::Failed, e, en)
                } else {
                    (StepStatus::Completed, v, n)
                }
            }
            ActionOutcome::Failed(e) => {
                let b = json!({ "error": e });
                let n = json_size(&b);
                (StepStatus::Failed, b, n)
            }
            // `submit` dont le POST est parti sans réponse : Indeterminate → le batch s'ARRÊTE
            // (fail-closed comme Failed) mais l'appelant NE DOIT PAS ré-exécuter (double-POST).
            ActionOutcome::Indeterminate(e) => {
                let b = json!({ "error": e });
                let n = json_size(&b);
                (StepStatus::Indeterminate, b, n)
            }
        };
        agg_bytes += size;
        results.push(json!({ "index": i, "status": status.as_str(), "result": body }));

        // Fail-closed : la première action non `completed` arrête le batch.
        if status != StepStatus::Completed {
            batch_status = status.as_str();
            break;
        }
        // Borne d'agrégat — `truncated` UNIQUEMENT s'il reste des étapes (Fable 5, F1).
        if agg_bytes > MAX_BATCH_RESULT_BYTES && i + 1 < total {
            batch_status = "truncated";
            break;
        }
    }

    json!({ "batch": batch_status, "attached": true, "steps": results, "steps_total": total })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Faux pair CDP en mémoire (un `recv` vide = EOF). Restitue les réponses
    /// pré-chargées une par morceau — donc arrivant APRÈS l'émission de chaque commande
    /// (sinon la purge-avant-émission de `CdpSession` les prendrait pour des pré-stages).
    struct FakeCdp {
        sent: Vec<Vec<u8>>,
        inbox: std::collections::VecDeque<Vec<u8>>,
    }
    impl FakeCdp {
        fn new(chunks: Vec<Vec<u8>>) -> Self {
            Self {
                sent: Vec::new(),
                inbox: chunks.into(),
            }
        }
    }
    impl CdpChannel for FakeCdp {
        fn send(&mut self, b: &[u8]) -> std::io::Result<()> {
            self.sent.push(b.to_vec());
            Ok(())
        }
        fn recv(&mut self) -> std::io::Result<Vec<u8>> {
            Ok(self.inbox.pop_front().unwrap_or_default())
        }
    }
    fn cdp_frame(v: Value) -> Vec<u8> {
        let mut b = serde_json::to_vec(&v).unwrap();
        b.push(0);
        b
    }

    #[test]
    fn navigate_issues_enable_then_navigate_with_the_url_as_a_param() {
        let chan = FakeCdp::new(vec![
            cdp_frame(json!({"id": 1, "result": {}})), // Page.enable
            cdp_frame(json!({"id": 2, "result": {"frameId": "F1"}})), // Page.navigate
        ]);
        let mut s = CdpSession::new(chan);
        let out = run_action(
            &mut s,
            &BrowserAction::Navigate {
                host: "github.com".into(),
                url: "https://github.com/x".into(),
            },
            "PAGE-SID",
        )
        .unwrap();
        assert_eq!(out["navigated"], "https://github.com/x");
        assert_eq!(out["frameId"], "F1");
        // Les deux commandes CIBLENT LA PAGE (sessionId threadé) : sans lui le vrai chromium
        // refuserait toute commande Page.* (finding Fable 5). URL en PARAMÈTRE, jamais du JS.
        let sent = s.into_channel().sent;
        let enable: Value = serde_json::from_slice(&sent[0][..sent[0].len() - 1]).unwrap();
        assert_eq!(enable["method"], "Page.enable");
        assert_eq!(enable["sessionId"], "PAGE-SID");
        let nav: Value = serde_json::from_slice(&sent[1][..sent[1].len() - 1]).unwrap();
        assert_eq!(nav["method"], "Page.navigate");
        assert_eq!(nav["params"]["url"], "https://github.com/x");
        assert_eq!(nav["sessionId"], "PAGE-SID");
    }

    #[test]
    fn navigate_surfaces_a_browser_error_text() {
        let chan = FakeCdp::new(vec![
            cdp_frame(json!({"id": 1, "result": {}})),
            cdp_frame(json!({"id": 2, "result": {"errorText": "net::ERR_NAME_NOT_RESOLVED"}})),
        ]);
        let mut s = CdpSession::new(chan);
        let err = run_action(
            &mut s,
            &BrowserAction::Navigate {
                host: "x".into(),
                url: "https://x".into(),
            },
            "PAGE-SID",
        )
        .unwrap_err();
        assert!(err.contains("ERR_NAME_NOT_RESOLVED"), "{err}");
    }

    #[test]
    fn navigate_sanitizes_a_hostile_error_text() {
        // `errorText` vient du PAIR (hostile) : un chromium malveillant y glisse des ctrl/bidi ;
        // l'erreur vibed (affichée à l'audit) doit être assainie (Fable 5).
        let chan = FakeCdp::new(vec![
            cdp_frame(json!({"id": 1, "result": {}})),
            cdp_frame(json!({"id": 2, "result": {"errorText": "bad\u{1b}[2K\u{202e}spoof"}})),
        ]);
        let mut s = CdpSession::new(chan);
        let err = run_action(
            &mut s,
            &BrowserAction::Navigate {
                host: "x".into(),
                url: "https://x".into(),
            },
            "PAGE-SID",
        )
        .unwrap_err();
        assert!(!err.contains('\u{1b}'), "ESC doit être retiré : {err:?}");
        assert!(!err.contains('\u{202e}'), "RTL override doit être retiré");
        assert!(err.contains("navigation refusée"));
    }

    #[test]
    fn read_evaluates_a_constant_expression_and_returns_html() {
        let chan = FakeCdp::new(vec![cdp_frame(
            json!({"id": 1, "result": {"result": {"value": "<html>hi</html>"}}}),
        )]);
        let mut s = CdpSession::new(chan);
        let out = run_action(&mut s, &BrowserAction::Read, "PAGE-SID").unwrap();
        assert_eq!(out["html"], "<html>hi</html>");
        // L'expression est CONSTANTE : aucune entrée agent, aucune surface d'injection.
        // La commande cible la PAGE (sessionId threadé).
        let sent = s.into_channel().sent;
        let ev: Value = serde_json::from_slice(&sent[0][..sent[0].len() - 1]).unwrap();
        assert_eq!(ev["method"], "Runtime.evaluate");
        assert_eq!(
            ev["params"]["expression"],
            "document.documentElement.outerHTML"
        );
        assert_eq!(ev["sessionId"], "PAGE-SID");
    }

    #[test]
    fn screenshot_returns_base64_png() {
        let chan = FakeCdp::new(vec![cdp_frame(
            json!({"id": 1, "result": {"data": "aGVsbG8="}}),
        )]);
        let mut s = CdpSession::new(chan);
        let out = run_action(&mut s, &BrowserAction::Screenshot, "PAGE-SID").unwrap();
        assert_eq!(out["screenshot_png_base64"], "aGVsbG8=");
        // Golden : la capture cible la PAGE (sessionId threadé) — verrouillé comme les
        // autres trames pour qu'une régression future ne repasse pas ce site en None.
        let sent = s.into_channel().sent;
        let shot: Value = serde_json::from_slice(&sent[0][..sent[0].len() - 1]).unwrap();
        assert_eq!(shot["method"], "Page.captureScreenshot");
        assert_eq!(shot["sessionId"], "PAGE-SID");
    }

    /// Les 4 réponses CDP qui satisfont le binding par-objet : getDocument → querySelector →
    /// resolveNode → callFunctionOn. `qs_node` = nodeId renvoyé par querySelector (0 = rien).
    fn cdp_object_binding(qs_node: i64, object_id: &str, call_result: Value) -> Vec<Vec<u8>> {
        vec![
            cdp_frame(json!({"id": 1, "result": {"root": {"nodeId": 1}}})),
            cdp_frame(json!({"id": 2, "result": {"nodeId": qs_node}})),
            cdp_frame(json!({"id": 3, "result": {"object": {"objectId": object_id}}})),
            cdp_frame(json!({"id": 4, "result": call_result})),
        ]
    }

    #[test]
    fn click_binds_by_object_selector_as_param_and_constant_function() {
        let chan = FakeCdp::new(cdp_object_binding(42, "OBJ1", json!({"result": {}})));
        let mut s = CdpSession::new(chan);
        // Sélecteur DISTINCTIF (un token qui n'apparaît pas dans la fonction constante) pour que
        // le test de non-fuite ne collisionne pas avec les mots de la garde submitter.
        let out = run_action(
            &mut s,
            &BrowserAction::Click {
                selector: "#zzUniqSel777".into(),
            },
            "PAGE-SID",
        )
        .unwrap();
        assert_eq!(out["clicked"], true);
        let sent = s.into_channel().sent;
        // querySelector porte le sélecteur en PARAMÈTRE (jamais du JS).
        let qs: Value = serde_json::from_slice(&sent[1][..sent[1].len() - 1]).unwrap();
        assert_eq!(qs["method"], "DOM.querySelector");
        assert_eq!(qs["params"]["selector"], "#zzUniqSel777");
        assert_eq!(qs["sessionId"], "PAGE-SID");
        // callFunctionOn : fonction CONSTANTE (this.click()) qui NE contient PAS le sélecteur ;
        // objectId = celui résolu.
        let call: Value = serde_json::from_slice(&sent[3][..sent[3].len() - 1]).unwrap();
        assert_eq!(call["method"], "Runtime.callFunctionOn");
        assert_eq!(call["params"]["objectId"], "OBJ1");
        let f = call["params"]["functionDeclaration"].as_str().unwrap();
        assert!(f.contains("this.click()"));
        assert!(
            !f.contains("zzUniqSel777"),
            "le sélecteur ne doit JAMAIS entrer dans la source de la fonction : {f}"
        );
        // Garde de submitter présente (Fable 5 F1) — régression si retirée.
        assert!(
            f.contains("browser.submit"),
            "la garde anti-submitter T2 doit être dans la fonction : {f}"
        );
    }

    #[test]
    fn fill_passes_the_value_as_argument_never_in_the_function_source() {
        let chan = FakeCdp::new(cdp_object_binding(7, "OBJ2", json!({"result": {}})));
        let mut s = CdpSession::new(chan);
        // Une valeur qui, INTERPOLÉE, s'échapperait vers du JS arbitraire — elle DOIT rester un
        // argument opaque, c'est ce qui garde browser.evaluate exclu même sur entrée hostile.
        let evil = "\"; fetch('//evil'); //";
        let out = run_action(
            &mut s,
            &BrowserAction::Fill {
                selector: "#q".into(),
                value: evil.into(),
            },
            "PAGE-SID",
        )
        .unwrap();
        assert_eq!(out["filled"], true);
        let sent = s.into_channel().sent;
        let call: Value = serde_json::from_slice(&sent[3][..sent[3].len() - 1]).unwrap();
        assert_eq!(call["method"], "Runtime.callFunctionOn");
        // La valeur hostile est dans arguments[0], PAS dans la source de la fonction.
        assert_eq!(call["params"]["arguments"][0]["value"], evil);
        let f = call["params"]["functionDeclaration"].as_str().unwrap();
        assert!(
            !f.contains("fetch"),
            "la valeur hostile ne doit JAMAIS entrer dans la source : {f}"
        );
        assert!(f.contains("this.value = v"));
    }

    #[test]
    fn click_fails_closed_when_the_selector_matches_nothing() {
        // querySelector renvoie nodeId 0 (rien ne matche) — pas une erreur CDP, mais fail-closed.
        let chan = FakeCdp::new(vec![
            cdp_frame(json!({"id": 1, "result": {"root": {"nodeId": 1}}})),
            cdp_frame(json!({"id": 2, "result": {"nodeId": 0}})),
        ]);
        let mut s = CdpSession::new(chan);
        let err = run_action(
            &mut s,
            &BrowserAction::Click {
                selector: "#none".into(),
            },
            "PAGE-SID",
        )
        .unwrap_err();
        assert!(err.contains("aucun élément ne matche"), "{err}");
    }

    #[test]
    fn click_fails_closed_on_a_page_side_exception() {
        let chan = FakeCdp::new(cdp_object_binding(
            5,
            "O",
            json!({"result": {}, "exceptionDetails": {"text": "boom"}}),
        ));
        let mut s = CdpSession::new(chan);
        let err = run_action(
            &mut s,
            &BrowserAction::Click {
                selector: "#x".into(),
            },
            "PAGE-SID",
        )
        .unwrap_err();
        assert!(err.contains("exception"), "{err}");
    }

    #[test]
    fn submit_completes_on_a_form_with_a_constant_function() {
        let chan = FakeCdp::new(cdp_object_binding(5, "F", json!({"result": {}})));
        let mut s = CdpSession::new(chan);
        let out = run_action(
            &mut s,
            &BrowserAction::Submit {
                selector: "form#login".into(),
            },
            "PAGE-SID",
        );
        assert_eq!(out, ActionOutcome::Completed(json!({"submitted": true})));
        // Fonction CONSTANTE : exige un <form> et appelle this.submit() ; le sélecteur n'y entre pas.
        let sent = s.into_channel().sent;
        let call: Value = serde_json::from_slice(&sent[3][..sent[3].len() - 1]).unwrap();
        let f = call["params"]["functionDeclaration"].as_str().unwrap();
        assert!(f.contains("this.submit()"));
        assert!(f.contains("'FORM'"));
        assert!(
            !f.contains("login"),
            "le sélecteur ne doit JAMAIS entrer dans la source : {f}"
        );
        // (Fable 5 F2) Verrou golden : `awaitPromise: false` — invariant load-bearing de la
        // classification Failed/Indeterminate (fonction synchrone, résultat sérialisé AVANT toute
        // destruction de contexte). Le passer à true casserait l'anti-double-POST.
        assert_eq!(call["params"]["awaitPromise"], false);
    }

    #[test]
    fn click_is_indeterminate_on_a_protocol_break_after_dispatch() {
        // (Fable 5 F6) Un click peut muter (onclick → fetch POST). Comme submit, un callFunctionOn
        // ÉMIS puis flux cassé (poison) → Indeterminate (effet distant indéterminé, ne pas
        // réessayer) — l'unification via run_object_action ferme le trou onclick-POST.
        let chan = FakeCdp::new(vec![
            cdp_frame(json!({"id": 1, "result": {"root": {"nodeId": 1}}})),
            cdp_frame(json!({"id": 2, "result": {"nodeId": 5}})),
            cdp_frame(json!({"id": 3, "result": {"object": {"objectId": "O"}}})),
            // pas de réponse au callFunctionOn (id 4) → poison après émission.
        ]);
        let mut s = CdpSession::new(chan);
        let out = run_action(
            &mut s,
            &BrowserAction::Click {
                selector: "#buy".into(),
            },
            "PAGE-SID",
        );
        assert!(
            matches!(out, ActionOutcome::Indeterminate(_)),
            "attendu Indeterminate : {out:?}"
        );
    }

    #[test]
    fn submit_fails_on_a_page_exception_not_a_form() {
        // callFunctionOn répond avec exceptionDetails (cible pas un <form>) → Failed (rien soumis).
        let chan = FakeCdp::new(cdp_object_binding(
            5,
            "F",
            json!({"result": {}, "exceptionDetails": {"text": "not a form"}}),
        ));
        let mut s = CdpSession::new(chan);
        let out = run_action(
            &mut s,
            &BrowserAction::Submit {
                selector: "#x".into(),
            },
            "PAGE-SID",
        );
        assert!(
            matches!(out, ActionOutcome::Failed(_)),
            "attendu Failed : {out:?}"
        );
    }

    #[test]
    fn submit_is_indeterminate_on_a_protocol_break_after_dispatch() {
        // getDocument/querySelector/resolveNode OK, mais le callFunctionOn du submit est ÉMIS puis
        // le flux se casse (pas de réponse → EOF → session empoisonnée) → Indeterminate : le POST
        // a PEUT-ÊTRE tiré, ne PAS réessayer. JAMAIS Failed (ce serait le double-POST, Fable 5).
        let chan = FakeCdp::new(vec![
            cdp_frame(json!({"id": 1, "result": {"root": {"nodeId": 1}}})),
            cdp_frame(json!({"id": 2, "result": {"nodeId": 5}})),
            cdp_frame(json!({"id": 3, "result": {"object": {"objectId": "F"}}})),
            // Pas de réponse au callFunctionOn (id 4) → EOF après émission.
        ]);
        let mut s = CdpSession::new(chan);
        let out = run_action(
            &mut s,
            &BrowserAction::Submit {
                selector: "form".into(),
            },
            "PAGE-SID",
        );
        assert!(
            matches!(out, ActionOutcome::Indeterminate(_)),
            "attendu Indeterminate : {out:?}"
        );
    }

    #[test]
    fn submit_fails_when_resolve_fails_before_any_submission() {
        // Aucune réponse : getDocument échoue → resolve échoue → Failed (l'échec est AVANT le
        // callFunctionOn du submit, donc rien n'est parti), JAMAIS Indeterminate — même si la
        // session est empoisonnée par l'erreur protocole du getDocument.
        let chan = FakeCdp::new(vec![]);
        let mut s = CdpSession::new(chan);
        let out = run_action(
            &mut s,
            &BrowserAction::Submit {
                selector: "form".into(),
            },
            "PAGE-SID",
        );
        assert!(
            matches!(out, ActionOutcome::Failed(_)),
            "attendu Failed : {out:?}"
        );
    }

    #[test]
    fn an_empty_page_session_id_is_refused_before_any_command() {
        // Garde-fou : un sessionId de page vide (jamais produit par attach_page) est refusé
        // en tête, avec une erreur vibed claire, avant toute émission CDP (Fable 5).
        let chan = FakeCdp::new(vec![]);
        let mut s = CdpSession::new(chan);
        let err = run_action(&mut s, &BrowserAction::Read, "").unwrap_err();
        assert!(err.contains("sans sessionId de page"), "{err}");
        assert!(
            s.into_channel().sent.is_empty(),
            "aucune commande ne doit partir sans sessionId de page"
        );
    }

    #[test]
    fn read_fails_closed_on_a_page_side_exception_no_cloaking() {
        // Page hostile : Runtime.evaluate répond « succès » avec exceptionDetails et sans
        // value → on doit refuser, PAS renvoyer un html vide (cloaking).
        let chan = FakeCdp::new(vec![cdp_frame(json!({
            "id": 1,
            "result": {"result": {"type": "object"}, "exceptionDetails": {"text": "Uncaught"}}
        }))]);
        let mut s = CdpSession::new(chan);
        assert!(run_action(&mut s, &BrowserAction::Read, "PAGE-SID")
            .unwrap_err()
            .contains("exception"));
    }

    #[test]
    fn read_fails_closed_when_the_value_is_missing_or_not_a_string() {
        let chan = FakeCdp::new(vec![cdp_frame(json!({"id": 1, "result": {"result": {}}}))]);
        let mut s = CdpSession::new(chan);
        assert!(run_action(&mut s, &BrowserAction::Read, "PAGE-SID")
            .unwrap_err()
            .contains("sans chaîne 'value'"));
    }

    #[test]
    fn screenshot_fails_closed_on_missing_data() {
        let chan = FakeCdp::new(vec![cdp_frame(json!({"id": 1, "result": {}}))]);
        let mut s = CdpSession::new(chan);
        assert!(run_action(&mut s, &BrowserAction::Screenshot, "PAGE-SID")
            .unwrap_err()
            .contains("sans données PNG"));
    }

    #[test]
    fn navigate_refuses_a_host_url_mismatch() {
        // Un Navigate incohérent (host ne correspond pas à l'URL) est refusé avant tout
        // appel CDP — l'audit ne peut pas être trompé sur l'hôte réellement atteint.
        let chan = FakeCdp::new(vec![]);
        let mut s = CdpSession::new(chan);
        let err = run_action(
            &mut s,
            &BrowserAction::Navigate {
                host: "github.com".into(),
                url: "https://evil.example/x".into(),
            },
            "PAGE-SID",
        )
        .unwrap_err();
        assert!(err.contains("incohérence hôte/URL"), "{err}");
        // Preuve DIRECTE : aucune trame n'a été émise avant le refus (pas seulement
        // « inbox vide ⇒ ç'aurait échoué autrement »).
        assert!(
            s.into_channel().sent.is_empty(),
            "aucune commande CDP ne doit partir avant le refus host/URL"
        );
    }

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
    fn control_and_bidi_chars_in_a_selector_are_refused_but_css_punctuation_is_kept() {
        // Un saut de ligne (Cc) ou un RTL-override (Cf, U+202E) ne servent qu'à casser
        // ou spoofer l'audit : refusés.
        for bad in ["a\nb", "a\u{202E}b", "a\u{200B}b"] {
            assert!(
                plan_action("click", &json!({ "selector": bad }))
                    .unwrap_err()
                    .contains("contrôle"),
                "doit refuser {bad:?}"
            );
        }
        // La ponctuation CSS légitime passe — Y COMPRIS les caractères dangereux-en-JS
        // (`\"`, `(`, `.`) : cette couche NE filtre PAS l'injection (c'est le binding
        // par-objet du transport qui protège). Un payload d'injection all-printable
        // passe donc ici par conception, et c'est correct.
        for sel in [
            "#id",
            ".cls",
            "div > a",
            "input[name=\"q\"]",
            "a:hover",
            "a:has(> b)",
            "x\"];fetch('//evil')//",
        ] {
            assert!(
                plan_action("click", &json!({ "selector": sel })).is_ok(),
                "doit accepter {sel:?}"
            );
        }
    }

    #[test]
    fn a_fill_value_keeps_text_but_refuses_controls_and_bounds_length() {
        // Accents, saut de ligne (textarea) et texte RTL (marques bidi) préservés.
        let a = plan_action("fill", &json!({"selector": "#c", "value": "café\nשלום"})).unwrap();
        assert_eq!(
            a,
            BrowserAction::Fill {
                selector: "#c".to_string(),
                value: "café\nשלום".to_string()
            }
        );
        // Un champ vidé (valeur "") est légitime.
        assert_eq!(
            plan_action("fill", &json!({"selector": "#c", "value": ""})).unwrap(),
            BrowserAction::Fill {
                selector: "#c".to_string(),
                value: String::new()
            }
        );
        // ESC/NUL/BEL (caractères de contrôle) refusés — ils corrompraient la ligne
        // d'audit de l'opérateur.
        for bad in ["a\u{1b}b", "a\0b", "a\u{07}b"] {
            assert!(
                plan_action("fill", &json!({"selector": "#c", "value": bad}))
                    .unwrap_err()
                    .contains("caractère de contrôle"),
                "doit refuser {bad:?}"
            );
        }
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

    // ----- batch : parse_batch + run_batch (modèle de session batch, ADR-022 addendum) -----

    #[test]
    fn parse_batch_accepts_a_sequence_and_maps_each_step() {
        let b = parse_batch(&json!({
            "steps": [
                { "verb": "navigate", "url": "https://github.com/x" },
                { "verb": "read" },
                { "verb": "screenshot" }
            ]
        }))
        .unwrap();
        assert_eq!(b.steps.len(), 3);
        assert_eq!(
            b.steps[0],
            BrowserAction::Navigate {
                host: "github.com".to_string(),
                url: "https://github.com/x".to_string(),
            }
        );
        assert_eq!(b.steps[1], BrowserAction::Read);
        assert_eq!(b.steps[2], BrowserAction::Screenshot);
    }

    #[test]
    fn parse_batch_is_fail_closed() {
        // 'steps' absent / pas un tableau / vide.
        assert!(parse_batch(&json!({})).unwrap_err().contains("manquant"));
        assert!(parse_batch(&json!({"steps": "x"}))
            .unwrap_err()
            .contains("manquant"));
        assert!(parse_batch(&json!({"steps": []}))
            .unwrap_err()
            .contains("vide"));
        // Au-delà du cap.
        let many: Vec<Value> = (0..MAX_BATCH_STEPS + 1)
            .map(|_| json!({"verb": "read"}))
            .collect();
        assert!(parse_batch(&json!({ "steps": many }))
            .unwrap_err()
            .contains("cap"));
        // Étape sans 'verb'.
        assert!(parse_batch(&json!({"steps": [{"url": "https://x"}]}))
            .unwrap_err()
            .contains("sans 'verb'"));
        // Verbe inconnu / args invalides → délégué à plan_action.
        assert!(parse_batch(&json!({"steps": [{"verb": "evaluate"}]}))
            .unwrap_err()
            .contains("verbe inconnu"));
        assert!(
            parse_batch(&json!({"steps": [{"verb": "navigate", "url": "file:///x"}]})).is_err()
        );
    }

    /// Fabrique un `FakeCdp` dont les 2 premières réponses satisfont `attach_page`
    /// (createTarget id1 → targetId, attachToTarget id2 → sessionId), suivies de `rest`.
    fn cdp_with_attach(rest: Vec<Vec<u8>>) -> FakeCdp {
        let mut chunks = vec![
            cdp_frame(json!({"id": 1, "result": {"targetId": "T1"}})),
            cdp_frame(json!({"id": 2, "result": {"sessionId": "S1"}})),
        ];
        chunks.extend(rest);
        FakeCdp::new(chunks)
    }

    #[test]
    fn run_batch_attaches_once_then_runs_steps_in_sequence() {
        // navigate (Page.enable id3, Page.navigate id4) puis read (Runtime.evaluate id5).
        let chan = cdp_with_attach(vec![
            cdp_frame(json!({"id": 3, "result": {}})),
            cdp_frame(json!({"id": 4, "result": {"frameId": "F1"}})),
            cdp_frame(json!({"id": 5, "result": {"result": {"value": "<html>hi</html>"}}})),
        ]);
        let s = CdpSession::new(chan);
        let batch = BrowserBatch {
            steps: vec![
                BrowserAction::Navigate {
                    host: "github.com".to_string(),
                    url: "https://github.com/x".to_string(),
                },
                BrowserAction::Read,
            ],
        };
        let out = run_batch(s, &batch);
        assert_eq!(out["batch"], "completed");
        assert_eq!(out["attached"], true);
        let steps = out["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0]["status"], "completed");
        assert_eq!(steps[0]["result"]["navigated"], "https://github.com/x");
        assert_eq!(steps[1]["status"], "completed");
        assert_eq!(steps[1]["result"]["html"], "<html>hi</html>");
    }

    #[test]
    fn run_batch_is_fail_closed_and_stops_on_the_first_non_completed_step() {
        // read (Runtime.evaluate id3) réussit ; click échoue à son resolve (querySelector id5 →
        // nodeId 0, rien ne matche → Failed) → le batch s'arrête, le 3e read ne tire PAS. (click
        // et non submit : submit doit être terminal, mais on teste ici l'arrêt sur étape faillible
        // AU MILIEU.)
        let chan = cdp_with_attach(vec![
            cdp_frame(json!({"id": 3, "result": {"result": {"value": "<html>a</html>"}}})), // read
            cdp_frame(json!({"id": 4, "result": {"root": {"nodeId": 1}}})), // click getDocument
            cdp_frame(json!({"id": 5, "result": {"nodeId": 0}})),           // querySelector → 0
        ]);
        let s = CdpSession::new(chan);
        let batch = BrowserBatch {
            steps: vec![
                BrowserAction::Read,
                BrowserAction::Click {
                    selector: "#none".to_string(),
                },
                BrowserAction::Read,
            ],
        };
        let out = run_batch(s, &batch);
        assert_eq!(out["batch"], "failed");
        let steps = out["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 2, "le 3e read ne doit PAS tirer (fail-closed)");
        assert_eq!(steps[0]["status"], "completed");
        assert_eq!(steps[1]["status"], "failed");
        assert!(steps[1]["result"]["error"]
            .as_str()
            .unwrap()
            .contains("browser.click"));
    }

    #[test]
    fn run_batch_surfaces_an_indeterminate_interaction_and_stops() {
        // click dont le callFunctionOn est ÉMIS puis le flux se casse (poison après émission) →
        // step INDETERMINATE → le batch s'arrête, statut "indeterminate" : l'appelant ne DOIT PAS
        // ré-exécuter (double-effet). Le read suivant ne tire pas. (click, non-terminal, illustre
        // le chemin indeterminate d'une INTERACTION au milieu d'un batch.)
        let chan = cdp_with_attach(vec![
            cdp_frame(json!({"id": 3, "result": {"root": {"nodeId": 1}}})), // getDocument
            cdp_frame(json!({"id": 4, "result": {"nodeId": 5}})),           // querySelector
            cdp_frame(json!({"id": 5, "result": {"object": {"objectId": "O"}}})), // resolveNode
                                                                            // pas de réponse au callFunctionOn (id 6) → poison après émission
        ]);
        let s = CdpSession::new(chan);
        let batch = BrowserBatch {
            steps: vec![
                BrowserAction::Click {
                    selector: "#buy".to_string(),
                },
                BrowserAction::Read,
            ],
        };
        let out = run_batch(s, &batch);
        assert_eq!(out["batch"], "indeterminate");
        let steps = out["steps"].as_array().unwrap();
        assert_eq!(
            steps.len(),
            1,
            "le read après une interaction indéterminée ne tire pas"
        );
        assert_eq!(steps[0]["status"], "indeterminate");
    }

    #[test]
    fn run_batch_defensively_refuses_a_non_terminal_submit_without_executing() {
        // Défense en profondeur (Fable 5) : même si un batch avec `submit` non-terminal est
        // construit directement (contournant parse_batch), run_batch le REFUSE avant d'attacher/
        // exécuter — aucun POST tiré. FakeCdp vide : si run_batch exécutait quoi que ce soit, il
        // échouerait autrement.
        let chan = FakeCdp::new(vec![]);
        let s = CdpSession::new(chan);
        let batch = BrowserBatch {
            steps: vec![
                BrowserAction::Submit {
                    selector: "form".to_string(),
                },
                BrowserAction::Read,
            ],
        };
        let out = run_batch(s, &batch);
        assert_eq!(out["batch"], "failed");
        assert_eq!(out["attached"], false);
        assert_eq!(out["steps"].as_array().unwrap().len(), 0);
        assert!(out["error"].as_str().unwrap().contains("non-terminal"));
    }

    #[test]
    fn run_batch_reports_a_failed_attach_without_panicking() {
        // createTarget id1 répond sans targetId → attach_page échoue → batch failed, attached=false.
        let chan = FakeCdp::new(vec![cdp_frame(json!({"id": 1, "result": {}}))]);
        let s = CdpSession::new(chan);
        let batch = BrowserBatch {
            steps: vec![BrowserAction::Read],
        };
        let out = run_batch(s, &batch);
        assert_eq!(out["batch"], "failed");
        assert_eq!(out["attached"], false);
        assert_eq!(out["steps"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn run_batch_caps_a_single_oversized_step_result_as_failed() {
        // (F2) Un SEUL read renvoyant un HTML > cap PAR ÉTAPE devient `failed` (le body hostile
        // n'est pas poussé), pas `completed` — la borne par-étape empêche un unique body géant
        // de crever le budget.
        let big = "x".repeat(MAX_STEP_RESULT_BYTES + 1);
        let chan = cdp_with_attach(vec![cdp_frame(
            json!({"id": 3, "result": {"result": {"value": big}}}),
        )]);
        let s = CdpSession::new(chan);
        let batch = BrowserBatch {
            steps: vec![BrowserAction::Read],
        };
        let out = run_batch(s, &batch);
        assert_eq!(out["batch"], "failed");
        let steps = out["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0]["status"], "failed");
        assert!(steps[0]["result"]["error"]
            .as_str()
            .unwrap()
            .contains("trop grand"));
    }

    #[test]
    fn run_batch_does_not_mark_truncated_when_the_last_step_completes() {
        // (F1) Deux reads sous le cap PAR ÉTAPE mais dont le cumul dépasse le cap d'agrégat AU
        // DERNIER step : le batch est `completed` (tout a tourné), JAMAIS `truncated` — sinon un
        // consommateur rationnel re-exécuterait un batch déjà entièrement fait (double-exécution).
        let half = "x".repeat(MAX_BATCH_RESULT_BYTES / 2 + 100_000); // 2 × > cap d'agrégat
        let chan = cdp_with_attach(vec![
            cdp_frame(json!({"id": 3, "result": {"result": {"value": half.clone()}}})),
            cdp_frame(json!({"id": 4, "result": {"result": {"value": half}}})),
        ]);
        let s = CdpSession::new(chan);
        let batch = BrowserBatch {
            steps: vec![BrowserAction::Read, BrowserAction::Read],
        };
        let out = run_batch(s, &batch);
        assert_eq!(
            out["batch"], "completed",
            "un batch entièrement complété n'est jamais truncated"
        );
        assert_eq!(out["steps"].as_array().unwrap().len(), 2);
        assert_eq!(out["steps_total"], 2);
    }

    #[test]
    fn run_batch_truncates_only_when_steps_remain_after_the_aggregate_cap() {
        // Trois reads : après 2, le cumul dépasse le cap d'agrégat ET il reste un step →
        // `truncated`, le 3e ne tire pas.
        let half = "x".repeat(MAX_BATCH_RESULT_BYTES / 2 + 100_000);
        let chan = cdp_with_attach(vec![
            cdp_frame(json!({"id": 3, "result": {"result": {"value": half.clone()}}})),
            cdp_frame(json!({"id": 4, "result": {"result": {"value": half}}})),
        ]);
        let s = CdpSession::new(chan);
        let batch = BrowserBatch {
            steps: vec![
                BrowserAction::Read,
                BrowserAction::Read,
                BrowserAction::Read,
            ],
        };
        let out = run_batch(s, &batch);
        assert_eq!(out["batch"], "truncated");
        assert_eq!(
            out["steps"].as_array().unwrap().len(),
            2,
            "le 3e read ne tire pas"
        );
        assert_eq!(out["steps_total"], 3);
    }

    #[test]
    fn parse_batch_rejects_unknown_keys_the_assert_trap() {
        // (F5) Une clé inconnue (ex un futur `assert` sur un `read`) est refusée fail-closed —
        // sinon l'étape passerait comme un `Read` nu et la vérif ne tournerait jamais, en silence.
        assert!(
            parse_batch(&json!({"steps": [{"verb": "read", "assert": {"x": 1}}]}))
                .unwrap_err()
                .contains("clé inconnue")
        );
        assert!(parse_batch(
            &json!({"steps": [{"verb": "navigate", "url": "https://x", "extra": 1}]})
        )
        .unwrap_err()
        .contains("clé inconnue"));
        // Contrôle négatif : les clés légitimes par verbe passent.
        assert!(parse_batch(
            &json!({"steps": [{"verb": "fill", "selector": "#q", "value": "hi"}]})
        )
        .is_ok());
    }

    #[test]
    fn parse_batch_requires_submit_to_be_the_last_step() {
        // (F1, BLOQUANT) submit non-terminal → refusé (anti double-POST par batch).
        assert!(parse_batch(&json!({"steps": [
            {"verb": "submit", "selector": "form"}, {"verb": "read"}
        ]}))
        .unwrap_err()
        .contains("DERNIÈRE étape"));
        // Deux submits → refusé (le 1er n'est pas le dernier).
        assert!(parse_batch(&json!({"steps": [
            {"verb": "submit", "selector": "a"}, {"verb": "submit", "selector": "b"}
        ]}))
        .unwrap_err()
        .contains("DERNIÈRE étape"));
        // submit EN DERNIER (le flux prévu navigate→fill→submit) → OK.
        assert!(parse_batch(&json!({"steps": [
            {"verb": "navigate", "url": "https://x.com"},
            {"verb": "fill", "selector": "#q", "value": "hi"},
            {"verb": "submit", "selector": "form"}
        ]}))
        .is_ok());
        // submit seul (dernier ET premier) → OK.
        assert!(parse_batch(&json!({"steps": [{"verb": "submit", "selector": "form"}]})).is_ok());
    }
}
