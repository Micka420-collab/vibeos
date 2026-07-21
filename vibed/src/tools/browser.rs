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

/// Exécute une [`BrowserAction`] contre une session CDP et renvoie un résultat JSON
/// **opaque** (le contenu vient d'une page hostile). Le transport `run_browser`
/// construira la [`CdpSession`] sur les fds du pipe puis appellera ceci.
///
/// **Incrément 1** : les verbes SANS sélecteur — `navigate`/`read`/`screenshot`. Leurs
/// commandes CDP ne portent aucune entrée agent interpolée (URL en paramètre, hôte déjà
/// validé ; expressions `Runtime.evaluate` CONSTANTES). `click`/`fill`/`submit`, qui
/// exigent le **binding CDP par-objet** du sélecteur/valeur (`DOM.querySelector` par
/// paramètre → `Input.dispatch*`/`Runtime.callFunctionOn` avec le nœud en `arguments`,
/// jamais d'interpolation — invariant ADR-022), arrivent à l'incrément suivant.
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
pub(crate) fn run_action<C: CdpChannel>(
    session: &mut CdpSession<C>,
    action: &BrowserAction,
    page: &str,
) -> Result<Value, String> {
    // Un sessionId de page vide n'est jamais légitime (`attach_page` rend une chaîne non
    // vide) : refuser tôt donne une erreur vibed claire plutôt qu'un refus chromium
    // cryptique en aval (Fable 5).
    if page.is_empty() {
        return Err("browser: run_action sans sessionId de page — refusé".to_string());
    }
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
            // Une navigation refusée par le navigateur remonte dans `errorText`.
            if let Some(err) = r.get("errorText").and_then(Value::as_str) {
                return Err(format!("browser.navigate: navigation refusée : {err}"));
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
        BrowserAction::Click { .. } | BrowserAction::Fill { .. } | BrowserAction::Submit { .. } => {
            Err(
                "browser: click/fill/submit — incrément suivant (binding CDP par-objet)"
                    .to_string(),
            )
        }
    }
}

/// Cap DUR d'actions par batch (ADR-022 addendum ratifié, Fable 5) : la garantie « chromium
/// jeté par appel » s'érode avec la longueur — un batch de 200 actions serait un `chromium`
/// hostile vivant longtemps dans une unité approuvée **une fois**. On borne à un petit nombre.
pub(crate) const MAX_BATCH_STEPS: usize = 16;

/// Borne de l'**agrégat** remonté (ADR-022 addendum, Fable 5) : borner chaque résultat ne
/// suffit pas ; 16 lectures de 1 Mio bâtiraient un JSON de 16 Mio que le cap du pipe
/// tronquerait salement. On arrête le batch dès que le cumul des résultats dépasse ce seuil.
const MAX_BATCH_RESULT_BYTES: usize = 4 * 1024 * 1024;

/// Un batch d'actions navigateur validées, exécuté par un **seul** `chromium` (modèle de
/// session **batch éphémère**, ADR-022 addendum ratifié). Chaque action est déjà passée par
/// [`plan_action`]. Les étapes `assert` déclaratives (vérif-avant-`submit`) sont un incrément
/// ultérieur — non incluses ici pour ne pas porter un type inexécutable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrowserBatch {
    pub steps: Vec<BrowserAction>,
}

/// Parse le **payload de contrôle** du batch (ce que `vibed` descend au helper sur stdin) :
/// `{ "steps": [ { "verb": "navigate", "url": "…" }, … ] }`. **Fail-closed** : refuse un
/// `steps` absent / non-tableau / **vide** / au-delà de [`MAX_BATCH_STEPS`], ou une étape sans
/// `verb` / mal formée (déléguée à [`plan_action`], qui rejette verbe inconnu, URL non-http(s),
/// caractères de contrôle…). La gouvernance (tier = max des verbes, `[rule.domains]` par
/// `navigate`, screening des `fill`, approbation T2) est **en amont dans `vibed`**, avant l'envoi.
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
        steps.push(plan_action(verb, s).map_err(|e| format!("browser.batch: étape {i}: {e}"))?);
    }
    Ok(BrowserBatch { steps })
}

/// Taille sérialisée (octets) d'un `Value`, pour la borne d'agrégat. Un `Value` déjà construit
/// se sérialise toujours (clés String, ni NaN ni Inf) ; en cas d'échec improbable on compte 0.
fn json_size(v: &Value) -> usize {
    serde_json::to_vec(v).map(|b| b.len()).unwrap_or(0)
}

/// Exécute un [`BrowserBatch`] contre une `CdpSession` (un `chromium`, une page) et rend un
/// résultat JSON **agrégé** et **opaque** (le contenu vient de pages hostiles). Modèle **batch
/// éphémère** (ADR-022 addendum ratifié) :
/// - **`attach_page` UNE fois** → le `sessionId` de page threadé dans chaque [`run_action`] ;
/// - exécution **en séquence**, continuité au sein du batch (le `read` voit ce que le `navigate`
///   précédent a chargé — c'est TOUT l'intérêt du batch vs process-par-verbe) ;
/// - **trois états par action** (addendum, Fable 5) : `completed` (Ok), `failed` (Err), et
///   `indeterminate` **réservé** au `submit` dont le POST est parti sans réponse (jamais
///   `failed`, sinon double-POST au retry). Les verbes actuels (navigate/read/screenshot) sont
///   des lectures/navigations **sans mutation distante** → jamais `indeterminate` ; cet état
///   apparaîtra avec le binding de `submit` ;
/// - **fail-closed** : une action non `completed` **arrête** le batch (les suivantes ne tirent
///   pas — l'agent avait planifié en supposant le succès des précédentes) ;
/// - **agrégat borné** : arrêt si le cumul des résultats dépasse [`MAX_BATCH_RESULT_BYTES`]
///   (le cap du pipe côté `vibed` reste le garde-fou dur ultime).
///
/// Ne panique jamais : rend toujours un JSON décrivant ce qui a eu lieu (même un `attach`
/// raté), pour que la ligne d'audit et l'agent voient l'état réel.
pub(crate) fn run_batch<C: CdpChannel>(session: &mut CdpSession<C>, batch: &BrowserBatch) -> Value {
    let page = match crate::browser_transport::attach_page(session) {
        Ok(p) => p,
        Err(e) => {
            return json!({ "batch": "failed", "attached": false, "error": e, "steps": [] });
        }
    };

    let mut results = Vec::with_capacity(batch.steps.len());
    let mut agg_bytes = 0usize;
    let mut batch_status = "completed";

    for (i, action) in batch.steps.iter().enumerate() {
        let (status, body) = match run_action(session, action, &page) {
            Ok(v) => ("completed", v),
            Err(e) => ("failed", json!({ "error": e })),
        };
        agg_bytes += json_size(&body);
        results.push(json!({ "index": i, "status": status, "result": body }));

        // Fail-closed : la première action non réussie arrête le batch.
        if status != "completed" {
            batch_status = status;
            break;
        }
        // Borne d'agrégat : on ne lance pas l'action suivante si on a déjà trop accumulé.
        if agg_bytes > MAX_BATCH_RESULT_BYTES {
            batch_status = "truncated";
            break;
        }
    }

    json!({ "batch": batch_status, "attached": true, "steps": results })
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

    #[test]
    fn click_fill_submit_are_deferred_to_the_next_increment() {
        let chan = FakeCdp::new(vec![]);
        let mut s = CdpSession::new(chan);
        let err = run_action(
            &mut s,
            &BrowserAction::Click {
                selector: "#x".into(),
            },
            "PAGE-SID",
        )
        .unwrap_err();
        assert!(err.contains("incrément suivant"));
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
        let mut s = CdpSession::new(chan);
        let batch = BrowserBatch {
            steps: vec![
                BrowserAction::Navigate {
                    host: "github.com".to_string(),
                    url: "https://github.com/x".to_string(),
                },
                BrowserAction::Read,
            ],
        };
        let out = run_batch(&mut s, &batch);
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
        // read (Runtime.evaluate id3) réussit ; click échoue (deferred, aucun appel CDP) →
        // le batch s'arrête, le 2e read ne tire PAS.
        let chan = cdp_with_attach(vec![cdp_frame(
            json!({"id": 3, "result": {"result": {"value": "<html>a</html>"}}}),
        )]);
        let mut s = CdpSession::new(chan);
        let batch = BrowserBatch {
            steps: vec![
                BrowserAction::Read,
                BrowserAction::Click {
                    selector: "#x".to_string(),
                },
                BrowserAction::Read,
            ],
        };
        let out = run_batch(&mut s, &batch);
        assert_eq!(out["batch"], "failed");
        let steps = out["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 2, "le 3e read ne doit PAS tirer (fail-closed)");
        assert_eq!(steps[0]["status"], "completed");
        assert_eq!(steps[1]["status"], "failed");
        assert!(steps[1]["result"]["error"]
            .as_str()
            .unwrap()
            .contains("incrément suivant"));
    }

    #[test]
    fn run_batch_reports_a_failed_attach_without_panicking() {
        // createTarget id1 répond sans targetId → attach_page échoue → batch failed, attached=false.
        let chan = FakeCdp::new(vec![cdp_frame(json!({"id": 1, "result": {}}))]);
        let mut s = CdpSession::new(chan);
        let batch = BrowserBatch {
            steps: vec![BrowserAction::Read],
        };
        let out = run_batch(&mut s, &batch);
        assert_eq!(out["batch"], "failed");
        assert_eq!(out["attached"], false);
        assert_eq!(out["steps"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn run_batch_bounds_the_aggregate_and_stops_when_over_cap() {
        // Un premier read renvoie un HTML > cap d'agrégat → batch "truncated", le 2e read
        // ne tire pas.
        let big = "x".repeat(MAX_BATCH_RESULT_BYTES + 1);
        let chan = cdp_with_attach(vec![cdp_frame(
            json!({"id": 3, "result": {"result": {"value": big}}}),
        )]);
        let mut s = CdpSession::new(chan);
        let batch = BrowserBatch {
            steps: vec![BrowserAction::Read, BrowserAction::Read],
        };
        let out = run_batch(&mut s, &batch);
        assert_eq!(out["batch"], "truncated");
        assert_eq!(
            out["steps"].as_array().unwrap().len(),
            1,
            "le 2e read ne doit pas tirer après dépassement de la borne d'agrégat"
        );
    }
}
