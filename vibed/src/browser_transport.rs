//! Transport fd du navigateur (ADR-022) : le pont entre les descripteurs du pipe
//! `--remote-debugging-pipe` de `chromium-headless` et le pilote [`crate::cdp::CdpSession`].
//!
//! Ce module fait de l'**I/O** (contrairement au codec `cdp`, pur) mais reste **mince** :
//! il ne fait que déplacer des octets. Le cadrage (NUL-délimité) est le travail du codec ;
//! la corrélation et la sécurité, celui de `CdpSession`. Le **lancement** de `chromium`
//! ([`spawn_chromium`]) atterrit ici ; la boucle `run_browser` (pilotage d'un batch
//! d'actions) et le proxy CONNECT sont des incréments ultérieurs.
//!
//! **Contrat `CdpChannel` respecté ici** (cf. sa doc) : `send` écrit **intégralement**
//! (`write_all`), `recv` est un **read bloquant** et ne renvoie `Ok(vide)` que sur un
//! **vrai EOF** (le pair a fermé son écriture). Le garde-fou de temps ultime est le
//! `RuntimeMaxSec` de l'unité transitoire durcie (ADR-019/022) : un `chromium` qui ne
//! répond jamais fait tuer l'unité entière, pas boucler `vibed`.
//!
//! **Invariants du lancement** (revue Fable 5) — les deux fds/stdout sont désormais **tenus
//! par [`spawn_chromium`]** ; SIGPIPE reste à la charge du **`main` du helper** :
//! - **SIGPIPE doit être ignoré** dans le process helper : un pipe n'a pas d'équivalent
//!   `MSG_NOSIGNAL`, donc la disposition process-wide est la seule garde. Un `chromium`
//!   hostile qui ferme sa lecture pendant un `write_all` lèverait sinon SIGPIPE et
//!   **tuerait le helper par signal** avant qu'il n'émette son erreur bornée. Le runtime
//!   Rust std installe `SIG_IGN` avant `main` (donc sûr aujourd'hui — `write_all` renvoie
//!   `EPIPE`), mais `run_browser` doit le **réaffirmer** (`libc::signal(SIGPIPE, SIG_IGN)`)
//!   pour survivre à un futur flag de build ou une entrée non-std. **(Pas dans `spawn_chromium`
//!   — c'est une disposition process-wide, pas par-spawn.)**
//! - les fds du pipe sont créés **`O_CLOEXEC`** et `dup2` **uniquement** sur les fds 3/4 de
//!   l'enfant ; ce sont les **seuls** fds supplémentaires que `chromium` hérite. ✔ [`spawn_chromium`]
//! - `chromium` n'hérite **PAS** du **stdout** du helper : ce stdout EST le canal de résultat
//!   `systemd-run --pipe` que `vibed` parse — un `chromium` hostile le forgerait. Le stdio de
//!   l'enfant est redirigé vers `/dev/null`. ✔ [`spawn_chromium`]

#![cfg(unix)]

use std::fs::File;
use std::io::{Error, ErrorKind, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

use crate::cdp::CdpChannel;

/// Un [`CdpChannel`] au-dessus des deux descripteurs du pipe CDP de `chromium` :
/// on **écrit** les commandes sur `to_peer` (le fd que `chromium` lit, conventionnellement
/// le fd 3) et on **lit** les réponses/événements sur `from_peer` (le fd que `chromium`
/// écrit, le fd 4).
pub struct PipeChannel {
    to_peer: File,
    from_peer: File,
}

impl PipeChannel {
    /// Construit le canal à partir des deux extrémités **possédées** du pipe : `write`
    /// (ce que `chromium` lit) et `read` (ce que `chromium` écrit). Prendre des
    /// [`OwnedFd`] rend la propriété **type-sûre** — pas de double-close possible, aucun
    /// `unsafe` ici (l'unsafe vit au seul endroit où `pipe(2)` produit les fds bruts).
    pub fn from_fds(write: OwnedFd, read: OwnedFd) -> Self {
        Self {
            to_peer: File::from(write),
            from_peer: File::from(read),
        }
    }
}

impl CdpChannel for PipeChannel {
    fn send(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        // write_all : jamais d'écriture partielle silencieuse (contrat CdpChannel) ; il
        // réessaie déjà `Interrupted` en interne.
        self.to_peer.write_all(bytes)
    }

    fn recv(&mut self) -> std::io::Result<Vec<u8>> {
        let mut buf = [0u8; 16 * 1024];
        // Read bloquant, avec réessai sur `Interrupted` (un signal spurieux ne doit pas
        // gâcher l'appel). `n == 0` = vrai EOF (le pair a fermé) → `Ok(vide)`, que
        // `CdpSession` traite en fermeture fail-closed. Jamais `Ok(vide)` sur autre chose.
        let n = loop {
            match self.from_peer.read(&mut buf) {
                Ok(n) => break n,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        };
        Ok(buf[..n].to_vec())
    }
}

/// Argv **durci** de `chromium-headless` pour une session `browser.*` (ADR-022). Tous les
/// flags sont **fixes** (aucune entrée agent n'atteint l'argv) ; `profile_dir` (profil
/// **éphémère**, sans identifiants) et `proxy` (l'IP du proxy CONNECT qui évalue
/// `[rule.domains]`) viennent de `vibed`, pas de l'agent.
///
/// **Deux flags ne sont JAMAIS émis** (invariant ADR-022, testé) : `--no-sandbox`
/// (le sandbox userns de Chromium doit rester — c'est sa défense de couche 1) et
/// `--remote-debugging-port` (le pilotage passe par le **pipe**, jamais par un port TCP
/// qui exposerait CDP au réseau). L'ensemble d'argv étant **fermé**, un **test golden**
/// le verrouille EXACTEMENT (plus fort qu'un denylist : échoue sur tout ajout futur).
///
/// **Ce qui garantit le confinement egress** (revue Fable 5) : c'est le **plancher réseau
/// du sandbox** (`IPAddressDeny=any` + seul `127.66.0.1/32`, eBPF-cgroup dans le netns du
/// navigateur, `sandbox.rs`), PAS `--proxy-server`. Les flags anti-contournement ci-dessus
/// (WebRTC/QUIC, `<-loopback>`) sont de la **défense en profondeur** pour que l'argv porte
/// son propre poids si jamais chromium tournait hors du sandbox.
///
/// **Invariants pour le lanceur (`run_browser`)** : `chromium_bin` DOIT être le binaire
/// `headless_shell` (déjà headless — pas de `--headless` ; un `chromium` générique
/// tenterait un GUI) ; et l'argv DOIT être passé à **`execve` comme vecteur**, jamais joint
/// en chaîne shell (sinon un espace dans une valeur redeviendrait un flag).
pub fn chromium_argv(chromium_bin: &str, profile_dir: &str, proxy: Option<&str>) -> Vec<String> {
    let mut argv = vec![
        chromium_bin.to_string(),
        // Pilotage CDP par PIPE (fds 3/4), pas un port : zéro surface réseau CDP.
        "--remote-debugging-pipe".to_string(),
        // Profil éphémère, sans identifiants (ADR-022 supersède le profil persistant).
        format!("--user-data-dir={profile_dir}"),
        // Isolation par site pour du contenu multi-origine hostile (Spectre / cross-origin).
        "--site-per-process".to_string(),
        // Défense EN PROFONDEUR sous le plancher réseau du sandbox (IPAddressDeny=any) :
        // couper les canaux qui contourneraient le proxy CONNECT au niveau chromium —
        // WebRTC en UDP direct, et QUIC/HTTP3 (UDP qu'un proxy CONNECT ne tunnelise pas).
        "--force-webrtc-ip-handling-policy=disable_non_proxied_udp".to_string(),
        "--disable-quic".to_string(),
        // Fiabilité sous /dev/shm restreint (sinon crash des renderers sur pages lourdes).
        "--disable-dev-shm-usage".to_string(),
        // Coupe toute la télémétrie / le réseau de fond (+ DoH : pas de DNS direct hors proxy).
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--disable-background-networking".to_string(),
        "--disable-sync".to_string(),
        "--disable-component-update".to_string(),
        "--disable-domain-reliability".to_string(),
        "--disable-breakpad".to_string(),
        "--no-pings".to_string(),
        "--disable-features=Translate,MediaRouter,OptimizationHints,DnsOverHttps".to_string(),
    ];
    if let Some(p) = proxy {
        // Tout l'egress passe par le proxy CONNECT — Y COMPRIS la boucle locale : `<-loopback>`
        // RETIRE la règle de contournement IMPLICITE de chromium pour localhost/loopback
        // (une liste VIDE ne suffit pas). Se connecter au proxy à 127.66.0.1 n'est pas
        // affecté : la liste régit les URLs de destination, pas le saut vers le proxy.
        argv.push(format!("--proxy-server={p}"));
        argv.push("--proxy-bypass-list=<-loopback>".to_string());
    }
    argv
}

/// Crée une page vierge et s'y **attache**, renvoyant le `sessionId` que le pilotage des
/// pages (`Page.*`/`Runtime.*`) doit porter (protocole « flat »).
///
/// Sur `--remote-debugging-pipe`, la connexion initiale est la cible **navigateur** ; sans
/// ce `sessionId`, le vrai `chromium` **refuse** toute commande de page (finding Fable 5
/// sur `run_action`, qui utilisait `session_id: None`). `run_browser` appellera ceci une
/// fois, puis passera le `sessionId` à chaque `session.call` de la BrowserAction.
///
/// **Contrat sur `Err` (Fable 5)** : un échec ⇒ **jeter la session ET l'unité transitoire**,
/// jamais réessayer `attach_page` sur la même session. Le poison de [`crate::cdp::CdpSession`]
/// ne couvre que le chemin **protocole** ; une erreur applicative (ou un `sessionId`/`targetId`
/// absent) laisse la session saine mais peut déjà avoir créé une cible orpheline `about:blank`
/// — un réessai en fuirait une de plus. La cible orpheline meurt de toute façon avec l'unité
/// (`RuntimeMaxSec`/`MemoryMax`), donc pas de `Target.closeTarget` compensatoire (qui pourrait
/// lui-même échouer/empoisonner — mauvais échange).
///
/// **Invariant mono-locataire (Fable 5, à tenir aux incréments live)** : un `chromium` par
/// session, profil éphémère neuf, **une** page dans le contexte par défaut — donc pas d'autre
/// cible avec qui partager cookies/stockage. Le jour où un `chromium` sert **plusieurs** tâches,
/// `Target.createBrowserContext` (+ `Target.setAutoAttach` pour popups/OOPIF) devient
/// **obligatoire**. `sessionId`/`targetId` sont du **JSON opaque du pair** : un `chromium`
/// hostile peut mentir dessus (il ne fait que mal router SES propres commandes) — ne jamais les
/// interpoler dans un shell/chemin/log non assaini en aval.
pub fn attach_page<C: crate::cdp::CdpChannel>(
    session: &mut crate::cdp::CdpSession<C>,
) -> Result<String, String> {
    let created = session.call(
        "Target.createTarget",
        serde_json::json!({ "url": "about:blank" }),
        None,
    )?;
    let target_id = created
        .get("targetId")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "browser: Target.createTarget sans targetId — refusé".to_string())?;
    // `flatten: true` = protocole plat : les messages de la page portent un `sessionId`
    // sur la MÊME connexion (pas de socket séparé).
    let attached = session.call(
        "Target.attachToTarget",
        serde_json::json!({ "targetId": target_id, "flatten": true }),
        None,
    )?;
    attached
        .get("sessionId")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "browser: Target.attachToTarget sans sessionId — refusé".to_string())
}

/// Lance `chromium-headless` en mode `--remote-debugging-pipe` et rend le processus enfant
/// ainsi que le [`PipeChannel`] déjà branché sur les descripteurs CDP. Le pilotage passe par
/// les **fds 3** (chromium **lit** les commandes) et **4** (chromium **écrit** les réponses),
/// la convention `--remote-debugging-pipe` — jamais un port TCP.
///
/// **Invariants tenus ici (ADR-022, revue Fable 5)** :
/// - **stdout de l'enfant → `/dev/null`** : le stdout du helper EST le canal de résultat
///   `systemd-run --pipe` que `vibed` parse ; un `chromium` hostile qui en hériterait le
///   **forgerait**. Le CDP passe par le fd 4 **séparé**, jamais par stdout.
/// - **seuls les fds 3/4** atteignent l'enfant : les deux extrémités **côté helper** du pipe
///   sont `O_CLOEXEC` (créées via `pipe2`) et se ferment donc à l'`exec` ; l'enfant n'hérite
///   d'**aucun** autre descripteur du helper.
/// - **placement robuste sur 3/4** : les extrémités côté enfant sont d'abord **relogées**
///   au-dessus de 10 (`F_DUPFD_CLOEXEC`) avant le `dup2` vers 3/4 — ce qui évite l'écrasement
///   croisé si l'allocation initiale tombait justement sur 3 ou 4 — puis le `FD_CLOEXEC` de
///   3/4 est **explicitement retiré** (le `dup2` le retire déjà, sauf le cas `oldfd == newfd`).
///
/// **À la charge de l'appelant (`run_browser`)**, PAS ici : réaffirmer `SIGPIPE = SIG_IGN`
/// (disposition **process-wide** du helper, pas par-spawn), et le garde-fou de **temps**
/// (`RuntimeMaxSec` de l'unité transitoire — un `chromium` muet fait tuer l'unité, pas boucler).
/// `chromium_bin` DOIT être le binaire `headless_shell` ; `profile_dir` un répertoire
/// **éphémère** (ADR-022) ; `proxy` l'IP du proxy CONNECT (ou `None`). L'argv est **fixe**
/// (cf. [`chromium_argv`]) — aucune entrée agent ne l'atteint.
///
/// Le `Child` rendu doit être **attendu/tué** par l'appelant ; laisser tomber le
/// [`PipeChannel`] ferme les extrémités côté helper (l'enfant voit alors EOF sur sa lecture).
pub fn spawn_chromium(
    chromium_bin: &str,
    profile_dir: &str,
    proxy: Option<&str>,
) -> std::io::Result<(Child, PipeChannel)> {
    // Chemins ABSOLUS obligatoires (cohérent avec `run_cli` du helper deploy, Fable 5) : un
    // `chromium_bin` relatif se résoudrait via le PATH hérité, un `profile_dir` relatif contre
    // le cwd du helper — deux dérives évitables sur le chemin critique.
    if !chromium_bin.starts_with('/') || !profile_dir.starts_with('/') {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "spawn_chromium: chromium_bin et profile_dir doivent être des chemins absolus",
        ));
    }

    // Deux pipes O_CLOEXEC. Convention CDP : l'enfant LIT ses commandes sur le fd 3 et ÉCRIT
    // ses réponses sur le fd 4.
    //   commandes : le helper écrit sur `to_child_w`, l'enfant lit sur `to_child_r`  (→ fd 3)
    //   réponses  : l'enfant écrit sur `from_child_w` (→ fd 4), le helper lit sur `from_child_r`
    let (to_child_r, to_child_w) = cloexec_pipe()?;
    let (from_child_r, from_child_w) = cloexec_pipe()?;

    // Fds bruts côté enfant, capturés par valeur pour le `pre_exec` (qui tourne DANS l'enfant
    // après le fork). Les `OwnedFd` correspondants restent vivants jusqu'après `spawn` — donc
    // ouverts au moment du fork — puis sont fermés côté helper juste après.
    let child_cmd_fd = to_child_r.as_raw_fd(); // deviendra le fd 3 de l'enfant
    let child_resp_fd = from_child_w.as_raw_fd(); // deviendra le fd 4 de l'enfant

    let argv = chromium_argv(chromium_bin, profile_dir, proxy);
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        // Env VERROUILLÉ comme l'argv (Fable 5) : l'unité durcie fournit déjà un env minimal,
        // mais `env_clear` + `HOME=profile_dir` ferme l'autre moitié de l'`execve` (LD_PRELOAD,
        // proxies d'env, XDG_*…) — même discipline que `run_cli`. HOME sur le profil éphémère
        // garde toute écriture HOME-relative de chromium dans le profil jeté. (Env exact à
        // revalider contre le vrai chromium à l'E2E — `headless_shell` tolère un env quasi-vide
        // avec `--user-data-dir`.)
        .env_clear()
        .env("HOME", profile_dir)
        .stdin(Stdio::null()) // chromium n'a aucune entrée standard à lire
        .stdout(Stdio::null()) // ISOLATION du canal de résultat systemd-run (CDP = fd 4)
        .stderr(Stdio::null()); // les diagnostics de chromium ne polluent pas le résultat

    // SAFETY : le `pre_exec` ne fait que des appels **async-signal-safe** (`fcntl`, `dup2`) sur
    // des fds capturés par valeur ; aucune allocation, aucun état verrouillé du parent touché.
    unsafe {
        cmd.pre_exec(move || place_cdp_fds(child_cmd_fd, child_resp_fd));
    }

    let child = cmd.spawn()?;

    // Le helper n'a plus besoin des extrémités CÔTÉ ENFANT. Les fermer est **requis** : tant
    // que le helper garde `from_child_w` ouvert, sa propre lecture sur `from_child_r` ne verrait
    // jamais l'EOF quand chromium meurt (il resterait un écrivain vivant).
    drop(to_child_r);
    drop(from_child_w);

    // Le helper écrit les commandes sur `to_child_w` (que chromium lit) et lit les réponses sur
    // `from_child_r` (que chromium écrit).
    Ok((child, PipeChannel::from_fds(to_child_w, from_child_r)))
}

/// Crée un pipe unidirectionnel `(read, write)` **possédé**, les deux extrémités `O_CLOEXEC`
/// dès la création (`pipe2` — pas de fenêtre de course avant un `fcntl`), de sorte qu'aucune
/// ne fuie à travers un `exec` non voulu.
fn cloexec_pipe() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as RawFd; 2];
    // SAFETY : `fds` est un tableau de 2 i32, exactement ce que `pipe2` attend.
    let rc = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
    if rc != 0 {
        return Err(Error::last_os_error());
    }
    // SAFETY : `pipe2` vient de créer ces deux fds valides et possédés.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// Place les extrémités côté enfant du pipe CDP sur les **fds 3 et 4** (convention
/// `--remote-debugging-pipe`), **sans** `FD_CLOEXEC` (pour survivre à l'`exec`). Tourne DANS
/// l'enfant après le fork : **uniquement des appels async-signal-safe**, aucune allocation.
///
/// **Contrainte (Fable 5) : seules les erreurs `Error::last_os_error()`/`from_raw_os_error`
/// sont permises ici.** Un `Error::new(..., "msg")` **allouerait** post-fork (deadlock possible
/// sur le lock malloc tenu par un autre thread au moment du fork) et son `raw_os_error()` serait
/// `None` → remappé `EINVAL` par std, diagnostic perdu.
fn place_cdp_fds(cmd_fd: RawFd, resp_fd: RawFd) -> std::io::Result<()> {
    // SAFETY : appels bruts async-signal-safe ; on ne touche aucun état alloué du parent.
    unsafe {
        // 1) Reloger les sources AU-DESSUS de 10 (`F_DUPFD_CLOEXEC`) : si `cmd_fd`/`resp_fd`
        //    valaient déjà 3 ou 4, un `dup2` direct risquerait d'écraser l'un par l'autre. Les
        //    copies relogées sont `O_CLOEXEC` (elles se ferment à l'exec).
        let tmp_cmd = libc::fcntl(cmd_fd, libc::F_DUPFD_CLOEXEC, 10);
        if tmp_cmd < 0 {
            return Err(Error::last_os_error());
        }
        let tmp_resp = libc::fcntl(resp_fd, libc::F_DUPFD_CLOEXEC, 10);
        if tmp_resp < 0 {
            return Err(Error::last_os_error());
        }
        // 2) Placer sur 3 (chromium lit) et 4 (chromium écrit). `dup2` retire `FD_CLOEXEC` de
        //    la cible (sauf si `oldfd == newfd`, impossible ici car `tmp_* >= 10`).
        if libc::dup2(tmp_cmd, 3) < 0 {
            return Err(Error::last_os_error());
        }
        if libc::dup2(tmp_resp, 4) < 0 {
            return Err(Error::last_os_error());
        }
        // 3) Réaffirmer l'absence de `FD_CLOEXEC` sur 3/4 (ceinture + bretelles).
        if libc::fcntl(3, libc::F_SETFD, 0) < 0 {
            return Err(Error::last_os_error());
        }
        if libc::fcntl(4, libc::F_SETFD, 0) < 0 {
            return Err(Error::last_os_error());
        }
        // 4) Défense en profondeur (Fable 5) : marquer `CLOEXEC` **tout** fd `>= 5` (parasites
        //    éventuels du helper — sockets futurs, `LISTEN_FDS`) **sans les fermer**. L'invariant
        //    « seuls 3/4 atteignent l'enfant » ne doit pas dépendre de l'hygiène CLOEXEC de TOUT
        //    le helper. `CLOSE_RANGE_CLOEXEC` et **non** un close : fermer casserait le pipe de
        //    report d'échec d'`exec` de std (déjà CLOEXEC) — son EOF signifierait alors « exec
        //    réussi » → `Ok` sur un enfant mort. Syscall **brut** (pas le wrapper glibc) :
        //    indépendant de la version glibc, kernel >= 5.11 (trivialement tenu sur Fedora 44).
        //    Nos fds temporaires (`tmp_* >= 10`) et les extrémités côté helper (déjà CLOEXEC)
        //    sont couverts sans effet de bord ; 3/4 (< 5) restent hors de portée.
        if libc::syscall(
            libc::SYS_close_range,
            5 as libc::c_long,
            libc::c_uint::MAX as libc::c_long,
            libc::CLOSE_RANGE_CLOEXEC as libc::c_long,
        ) < 0
        {
            return Err(Error::last_os_error());
        }
    }
    Ok(())
}

/// Récupère un champ chaîne requis qui DOIT être un chemin **absolu** (chemin de config venant de
/// `vibed`, cohérent avec la garde de [`spawn_chromium`]).
fn req_abs_field(payload: &serde_json::Value, key: &str) -> Result<String, String> {
    let s = payload
        .get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("browser: champ '{key}' manquant ou non-chaîne — refusé"))?;
    if !s.starts_with('/') {
        return Err(format!(
            "browser: '{key}' doit être un chemin absolu : {s:?}"
        ));
    }
    Ok(s.to_string())
}

/// Valide la **requête de contrôle** `browser` (ce que root `vibed` descend sur le stdin du
/// helper). `vibed` est le SEUL écrivain du pipe → la requête est **de confiance** ; les pages que
/// `chromium` pilotera, elles, sont **hostiles**. La validation du **batch** délègue à
/// [`crate::tools::browser::parse_batch`] — source de vérité UNIQUE (cap d'étapes, whitelist de
/// clés, `submit` terminal…). Rend `(chromium_bin, profile_dir, proxy, batch)`.
fn parse_browser_request(
    payload: &serde_json::Value,
) -> Result<
    (
        String,
        String,
        Option<String>,
        crate::tools::browser::BrowserBatch,
    ),
    String,
> {
    let chromium_bin = req_abs_field(payload, "chromium_bin")?;
    let profile_dir = req_abs_field(payload, "profile_dir")?;
    // `proxy` = autorité du proxy CONNECT (ex "127.66.0.1:8888"), ou absent (aucun egress ouvert :
    // fail-closed correct — sans listener, chromium ne peut rien atteindre).
    let proxy = payload
        .get("proxy")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let batch = crate::tools::browser::parse_batch(payload)?;
    Ok((chromium_bin, profile_dir, proxy, batch))
}

/// Réaffirme `SIGPIPE = SIG_IGN` (disposition **process-wide** du helper). Le runtime std l'installe
/// déjà avant `main`, mais on le réaffirme (cf. doc du module) : un `chromium` hostile qui ferme sa
/// lecture pendant un `write_all` lèverait sinon SIGPIPE et **tuerait le helper par signal** avant
/// son erreur bornée. Avec `SIG_IGN`, `write_all` renvoie `EPIPE` (une `Err` propre), jamais un signal.
fn reaffirm_sigpipe_ignored() {
    // SAFETY : poser `SIG_IGN` sur `SIGPIPE` est sûr et idempotent ; aucune donnée partagée touchée.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}

/// Spawn `chromium`, pilote le batch, teardown. **Non testable unitairement** (vrai `chromium` +
/// vrai CDP sur pipe) — validé **E2E sur target** ; ses briques ([`spawn_chromium`],
/// [`crate::tools::browser::run_batch`]) sont testées séparément. `run_batch` **consomme** la
/// session (drop → EOF sur le pipe → `chromium` voit la fin de son entrée) ; on tue puis reap
/// l'enfant ensuite (le `RuntimeMaxSec` de l'unité transitoire est le garde-fou ultime).
fn drive_batch(
    chromium_bin: &str,
    profile_dir: &str,
    proxy: Option<&str>,
    batch: &crate::tools::browser::BrowserBatch,
) -> serde_json::Value {
    let (mut child, chan) = match spawn_chromium(chromium_bin, profile_dir, proxy) {
        Ok(cc) => cc,
        Err(e) => {
            return serde_json::json!({
                "batch": "failed", "spawned": false,
                "error": format!("browser: spawn chromium : {e}"),
            });
        }
    };
    let session = crate::cdp::CdpSession::new(chan);
    let result = crate::tools::browser::run_batch(session, batch);
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// Mode **`browser`** du helper `vibed-tool` (dispatché par `sandbox::helper::run`). Lit la requête
/// de contrôle sur stdin (bornée, comme `run_deploy`), la valide, spawn `chromium`, pilote le batch
/// et rend le résultat JSON **sérialisé**. Le contenu des pages y reste **opaque** (entrée hostile).
/// Réaffirme `SIGPIPE=SIG_IGN` avant tout spawn/écriture.
pub fn run_browser() -> Result<String, String> {
    reaffirm_sigpipe_ignored();
    let mut payload = String::new();
    std::io::stdin()
        .take(1 << 20)
        .read_to_string(&mut payload)
        .map_err(|e| format!("browser: lecture du payload de contrôle : {e}"))?;
    let payload: serde_json::Value = serde_json::from_str(payload.trim())
        .map_err(|e| format!("browser: payload de contrôle malformé : {e}"))?;
    let (chromium_bin, profile_dir, proxy, batch) = parse_browser_request(&payload)?;
    Ok(drive_batch(&chromium_bin, &profile_dir, proxy.as_deref(), &batch).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::io::{FromRawFd, RawFd};

    #[test]
    fn chromium_argv_is_an_exact_golden_set() {
        // L'ensemble étant FERMÉ (aucune entrée agent), on verrouille l'argv EXACT : plus
        // fort qu'un denylist, ça échoue sur TOUT ajout futur (Fable 5).
        let argv = chromium_argv("BIN", "PROF", None);
        let expected = [
            "BIN",
            "--remote-debugging-pipe",
            "--user-data-dir=PROF",
            "--site-per-process",
            "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
            "--disable-quic",
            "--disable-dev-shm-usage",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-background-networking",
            "--disable-sync",
            "--disable-component-update",
            "--disable-domain-reliability",
            "--disable-breakpad",
            "--no-pings",
            "--disable-features=Translate,MediaRouter,OptimizationHints,DnsOverHttps",
        ];
        assert_eq!(
            argv.iter().map(String::as_str).collect::<Vec<_>>(),
            expected
        );
    }

    #[test]
    fn chromium_argv_never_emits_any_sandbox_or_cdp_defeating_flag() {
        // Documentaire (le golden ci-dessus est la vraie garde) : aucune des formes qui
        // cassent le sandbox ou exposent/déplacent CDP (Fable 5).
        let argv = chromium_argv("bin", "/p", Some("http://127.66.0.1:8888"));
        for forbidden in [
            "--no-sandbox",
            "--disable-setuid-sandbox",
            "--no-zygote",
            "--single-process",
            "--disable-web-security",
            "--disable-site-isolation-trials",
        ] {
            assert!(
                !argv.iter().any(|a| a == forbidden),
                "{forbidden} ne doit jamais être émis"
            );
        }
        for prefix in ["--remote-debugging-port", "--remote-debugging-address"] {
            assert!(
                !argv.iter().any(|a| a.starts_with(prefix)),
                "{prefix} ne doit jamais être émis (pipe uniquement)"
            );
        }
    }

    #[test]
    fn chromium_argv_forces_all_egress_including_loopback_through_the_proxy() {
        let argv = chromium_argv("bin", "/p", Some("http://127.66.0.1:8888"));
        assert!(argv
            .iter()
            .any(|a| a == "--proxy-server=http://127.66.0.1:8888"));
        // `<-loopback>` retire le contournement implicite du loopback (une liste vide ne
        // suffit pas — Fable 5).
        assert!(argv.iter().any(|a| a == "--proxy-bypass-list=<-loopback>"));
        // Anti-contournement au niveau chromium présents.
        assert!(argv.iter().any(|a| a == "--disable-quic"));
        assert!(argv
            .iter()
            .any(|a| a == "--force-webrtc-ip-handling-policy=disable_non_proxied_udp"));
        // Sans proxy, pas de flag proxy.
        assert!(!chromium_argv("bin", "/p", None)
            .iter()
            .any(|a| a.starts_with("--proxy-server")));
    }

    /// Crée un pipe unidirectionnel `(read, write)` **possédé** via `libc::pipe`. L'unique
    /// `unsafe` du transport vit ici, au point où `pipe(2)` produit les fds bruts.
    fn os_pipe() -> (OwnedFd, OwnedFd) {
        let mut fds = [0 as RawFd; 2];
        // # Safety : `fds` est un tableau de 2 i32, exactement ce que `pipe` attend.
        let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
        assert_eq!(rc, 0, "libc::pipe a échoué");
        // # Safety : `pipe(2)` vient de créer ces deux fds valides et possédés.
        unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) }
    }

    #[test]
    fn send_writes_all_bytes_to_the_peer() {
        let (r, w) = os_pipe(); // le canal écrit sur w, le test lit sur r
        let (r2, w2) = os_pipe(); // pipe factice pour le côté lecture du canal
        let mut chan = PipeChannel::from_fds(w, r2);
        chan.send(b"Page.navigate\0").unwrap();
        let mut out = [0u8; 64];
        let n = File::from(r).read(&mut out).unwrap();
        assert_eq!(&out[..n], b"Page.navigate\0");
        drop(w2);
    }

    #[test]
    fn recv_reads_what_the_peer_wrote() {
        let (r, w) = os_pipe(); // le canal lit sur r, le test écrit sur w
        let (r2, w2) = os_pipe(); // pipe factice pour le côté écriture du canal
        let mut chan = PipeChannel::from_fds(w2, r);
        let mut peer_w = File::from(w);
        peer_w.write_all(b"{\"id\":1,\"result\":{}}\0").unwrap();
        let got = chan.recv().unwrap();
        assert_eq!(got, b"{\"id\":1,\"result\":{}}\0");
        drop(r2);
        drop(peer_w);
    }

    #[test]
    fn recv_returns_empty_on_true_eof() {
        let (r, w) = os_pipe();
        let (r2, w2) = os_pipe();
        let mut chan = PipeChannel::from_fds(w2, r);
        drop(w); // ferme l'écriture du pair → EOF sur la lecture du canal
        assert!(
            chan.recv().unwrap().is_empty(),
            "un vrai EOF doit donner un vec vide"
        );
        drop(r2);
    }

    #[test]
    fn a_full_cdp_exchange_drives_a_session_over_real_pipes() {
        // Bout en bout : un CdpSession pilote un PipeChannel sur de VRAIS pipes, un thread
        // « faux chromium » répond. Prouve que transport + codec + pilote s'assemblent
        // sans chromium.
        use crate::cdp::CdpSession;

        let (agent_r, peer_w) = os_pipe(); // chromium→agent : le pair écrit, l'agent lit
        let (peer_r, agent_w) = os_pipe(); // agent→chromium : l'agent écrit, le pair lit

        let peer = std::thread::spawn(move || {
            let mut pr = File::from(peer_r);
            let mut pw = File::from(peer_w);
            let mut buf = [0u8; 4096];
            let n = pr.read(&mut buf).unwrap();
            // La commande émise porte "id":1 ; on renvoie un result corrélé.
            assert!(buf[..n].windows(6).any(|w| w == b"\"id\":1"));
            pw.write_all(b"{\"id\":1,\"result\":{\"ok\":true}}\0")
                .unwrap();
        });

        let mut session = CdpSession::new(PipeChannel::from_fds(agent_w, agent_r));
        let result = session
            .call("Page.enable", serde_json::json!({}), None)
            .unwrap();
        assert_eq!(result, serde_json::json!({"ok": true}));
        peer.join().unwrap();
    }

    /// Lit une trame CDP NUL-délimitée sur `pr` (octet par octet, comme un vrai pair
    /// qui n'a pas de cadre supérieur) et la désérialise. Sert les faux chromium.
    fn read_cdp_frame(pr: &mut File) -> serde_json::Value {
        let mut frame = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            let n = pr.read(&mut byte).unwrap();
            assert_eq!(n, 1, "pair CDP : EOF avant la fin de trame");
            if byte[0] == 0 {
                break;
            }
            frame.push(byte[0]);
        }
        serde_json::from_slice(&frame).expect("trame de commande JSON valide")
    }

    #[test]
    fn attach_page_creates_a_target_then_attaches_and_returns_the_session_id() {
        // Faux « chromium » sur de VRAIS pipes : répond à la séquence EXACTE d'attach_page
        // — Target.createTarget → {targetId}, puis Target.attachToTarget (flat, portant CE
        // targetId) → {sessionId}. Prouve que create+attach s'assemblent sans chromium et
        // que le targetId est bien propagé de la 1re commande à la 2de.
        use crate::cdp::CdpSession;

        let (agent_r, peer_w) = os_pipe(); // chromium→agent
        let (peer_r, agent_w) = os_pipe(); // agent→chromium

        let peer = std::thread::spawn(move || {
            let mut pr = File::from(peer_r);
            let mut pw = File::from(peer_w);

            // 1) createTarget sur about:blank, AU NIVEAU NAVIGATEUR (pas de sessionId) →
            //    renvoie un targetId corrélé sur l'id émis.
            let c1 = read_cdp_frame(&mut pr);
            assert_eq!(c1["method"], "Target.createTarget");
            assert_eq!(
                c1["params"]["url"], "about:blank",
                "create doit viser about:blank"
            );
            assert!(
                c1.get("sessionId").is_none(),
                "create doit rester au niveau navigateur"
            );
            let id1 = c1["id"].as_u64().unwrap();
            pw.write_all(
                format!("{{\"id\":{id1},\"result\":{{\"targetId\":\"T1\"}}}}\0").as_bytes(),
            )
            .unwrap();

            // 2) attachToTarget — DOIT porter le targetId reçu et flatten:true (protocole plat),
            //    et rester AU NIVEAU NAVIGATEUR (c'est lui qui établit la session de page).
            let c2 = read_cdp_frame(&mut pr);
            assert_eq!(c2["method"], "Target.attachToTarget");
            assert_eq!(
                c2["params"]["targetId"], "T1",
                "attach doit cibler le targetId créé"
            );
            assert_eq!(
                c2["params"]["flatten"], true,
                "attach doit demander le protocole plat"
            );
            assert!(
                c2.get("sessionId").is_none(),
                "attach doit rester au niveau navigateur"
            );
            let id2 = c2["id"].as_u64().unwrap();
            pw.write_all(
                format!("{{\"id\":{id2},\"result\":{{\"sessionId\":\"S1\"}}}}\0").as_bytes(),
            )
            .unwrap();
        });

        let mut session = CdpSession::new(PipeChannel::from_fds(agent_w, agent_r));
        let sid = attach_page(&mut session).unwrap();
        assert_eq!(sid, "S1");
        peer.join().unwrap();
    }

    #[test]
    fn attach_page_refuses_an_attach_without_a_session_id() {
        // Fail-closed SYMÉTRIQUE du test createTarget : la cible est créée (targetId T1) mais
        // attachToTarget répond SANS sessionId → attach_page doit échouer sur la 2de étape,
        // jamais renvoyer une session vide. Couvre la branche « sessionId — refusé ».
        use crate::cdp::CdpSession;

        let (agent_r, peer_w) = os_pipe();
        let (peer_r, agent_w) = os_pipe();

        let peer = std::thread::spawn(move || {
            let mut pr = File::from(peer_r);
            let mut pw = File::from(peer_w);
            let c1 = read_cdp_frame(&mut pr);
            assert_eq!(c1["method"], "Target.createTarget");
            let id1 = c1["id"].as_u64().unwrap();
            pw.write_all(
                format!("{{\"id\":{id1},\"result\":{{\"targetId\":\"T1\"}}}}\0").as_bytes(),
            )
            .unwrap();
            let c2 = read_cdp_frame(&mut pr);
            assert_eq!(c2["method"], "Target.attachToTarget");
            let id2 = c2["id"].as_u64().unwrap();
            pw.write_all(format!("{{\"id\":{id2},\"result\":{{}}}}\0").as_bytes())
                .unwrap(); // pas de sessionId
        });

        let mut session = CdpSession::new(PipeChannel::from_fds(agent_w, agent_r));
        let err = attach_page(&mut session).unwrap_err();
        assert!(
            err.contains("sessionId"),
            "doit refuser sans sessionId : {err}"
        );
        peer.join().unwrap();
    }

    #[test]
    fn attach_page_refuses_a_create_target_without_a_target_id() {
        // Fail-closed : un pair qui répond à createTarget SANS targetId (createTarget id=1
        // déterministe) doit faire échouer attach_page AVANT tout attach — jamais deviner.
        use crate::cdp::CdpSession;

        let (agent_r, peer_w) = os_pipe();
        let (peer_r, agent_w) = os_pipe();

        let peer = std::thread::spawn(move || {
            let mut pr = File::from(peer_r);
            let mut pw = File::from(peer_w);
            let c1 = read_cdp_frame(&mut pr);
            assert_eq!(c1["method"], "Target.createTarget");
            pw.write_all(b"{\"id\":1,\"result\":{}}\0").unwrap(); // pas de targetId
        });

        let mut session = CdpSession::new(PipeChannel::from_fds(agent_w, agent_r));
        let err = attach_page(&mut session).unwrap_err();
        assert!(
            err.contains("targetId"),
            "doit refuser sans targetId : {err}"
        );
        peer.join().unwrap();
    }

    #[test]
    fn spawn_chromium_wires_fds_3_and_4_and_isolates_stdout() {
        // Faux « chromium » : un script qui échoue le fd 3 vers le fd 4 (`cat 0<&3 1>&4`) et
        // IGNORE ses flags. Il prouve d'un coup les invariants du lancement :
        //   - le helper écrit sur le fd 3 et l'enfant le reçoit (câblage commandes) ;
        //   - l'enfant écrit sur le fd 4 et le helper le lit (câblage réponses) ;
        //   - la réponse arrive par le fd 4, JAMAIS par stdout (mis à /dev/null par
        //     spawn_chromium) — l'isolation du canal de résultat systemd-run.
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("vibed-spawn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("fake-chromium");
        // Le `printf` forge une sortie sur SON stdout AVANT l'écho fd3→fd4 : si stdout fuyait
        // dans le canal (le pipe réponses branché à la fois en stdout ET en fd 4), "FORGED\0"
        // arriverait AVANT "PING\0" et l'assert d'égalité casserait. C'est le test NÉGATIF
        // gratuit de l'isolation stdout (Fable 5) — avec stdout à /dev/null, "FORGED\0" est
        // jeté et seul l'écho du fd 4 remonte.
        std::fs::write(&fake, "#!/bin/sh\nprintf 'FORGED\\0'\nexec cat 0<&3 1>&4\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let fake_bin = fake.to_string_lossy().into_owned();

        // Réessaie la course ETXTBSY (fichier fraîchement écrit puis exécuté), comme les tests
        // de sandbox.rs. Détection par errno, pas par chaîne (locale/formulation, Fable 5).
        let (mut child, mut chan) = (|| {
            for _ in 0..50 {
                match spawn_chromium(&fake_bin, "/tmp/ephemeral-profile", None) {
                    Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) => {
                        std::thread::sleep(std::time::Duration::from_millis(20))
                    }
                    other => return other,
                }
            }
            spawn_chromium(&fake_bin, "/tmp/ephemeral-profile", None)
        })()
        .expect("spawn_chromium réussit");

        chan.send(b"PING\0").unwrap();
        // Accumule jusqu'à l'écho complet : une lecture de pipe peut fragmenter.
        let mut got = Vec::new();
        while got.len() < 5 {
            let chunk = chan.recv().unwrap();
            assert!(!chunk.is_empty(), "EOF inattendu avant l'écho complet");
            got.extend_from_slice(&chunk);
        }
        assert_eq!(
            got, b"PING\0",
            "le helper doit relire sur le fd 4 ce qu'il a écrit sur le fd 3"
        );

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_browser_request_extracts_config_and_delegates_batch_validation() {
        use serde_json::json;
        let payload = json!({
            "chromium_bin": "/usr/lib64/chromium-browser/headless_shell",
            "profile_dir": "/run/vibed-tool/abc/profile",
            "proxy": "127.66.0.1:8888",
            "steps": [
                { "verb": "navigate", "url": "https://github.com/x" },
                { "verb": "read" }
            ]
        });
        let (bin, prof, proxy, batch) = parse_browser_request(&payload).unwrap();
        assert_eq!(bin, "/usr/lib64/chromium-browser/headless_shell");
        assert_eq!(prof, "/run/vibed-tool/abc/profile");
        assert_eq!(proxy.as_deref(), Some("127.66.0.1:8888"));
        assert_eq!(batch.steps.len(), 2);
    }

    #[test]
    fn parse_browser_request_is_fail_closed() {
        use serde_json::json;
        // chromium_bin absent.
        assert!(parse_browser_request(
            &json!({ "profile_dir": "/p", "steps": [{"verb": "read"}] })
        )
        .unwrap_err()
        .contains("chromium_bin"));
        // chromium_bin relatif → refusé (chemin absolu exigé).
        assert!(parse_browser_request(&json!({
            "chromium_bin": "headless_shell", "profile_dir": "/p", "steps": [{"verb": "read"}]
        }))
        .unwrap_err()
        .contains("absolu"));
        // profile_dir relatif → refusé.
        assert!(parse_browser_request(&json!({
            "chromium_bin": "/bin/x", "profile_dir": "prof", "steps": [{"verb": "read"}]
        }))
        .unwrap_err()
        .contains("absolu"));
        // proxy absent → None (fail-closed egress : sans listener chromium n'atteint rien).
        let (_, _, proxy, _) = parse_browser_request(&json!({
            "chromium_bin": "/bin/x", "profile_dir": "/p", "steps": [{"verb": "read"}]
        }))
        .unwrap();
        assert!(proxy.is_none());
        // Batch invalide (submit non-terminal) → délégué à parse_batch (source de vérité unique).
        assert!(parse_browser_request(&json!({
            "chromium_bin": "/bin/x", "profile_dir": "/p",
            "steps": [{"verb": "submit", "selector": "form"}, {"verb": "read"}]
        }))
        .unwrap_err()
        .contains("DERNIÈRE étape"));
    }
}
