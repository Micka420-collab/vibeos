//! Transport fd du navigateur (ADR-022) : le pont entre les descripteurs du pipe
//! `--remote-debugging-pipe` de `chromium-headless` et le pilote [`crate::cdp::CdpSession`].
//!
//! Ce module fait de l'**I/O** (contrairement au codec `cdp`, pur) mais reste **mince** :
//! il ne fait que déplacer des octets. Le cadrage (NUL-délimité) est le travail du codec ;
//! la corrélation et la sécurité, celui de `CdpSession`. Le lancement de `chromium`, la
//! boucle `run_browser` et le proxy CONNECT sont des incréments ultérieurs.
//!
//! **Contrat `CdpChannel` respecté ici** (cf. sa doc) : `send` écrit **intégralement**
//! (`write_all`), `recv` est un **read bloquant** et ne renvoie `Ok(vide)` que sur un
//! **vrai EOF** (le pair a fermé son écriture). Le garde-fou de temps ultime est le
//! `RuntimeMaxSec` de l'unité transitoire durcie (ADR-019/022) : un `chromium` qui ne
//! répond jamais fait tuer l'unité entière, pas boucler `vibed`.
//!
//! **Invariants que l'incrément `run_browser` DEVRA tenir** (revue Fable 5) :
//! - **SIGPIPE doit être ignoré** dans le process helper : un pipe n'a pas d'équivalent
//!   `MSG_NOSIGNAL`, donc la disposition process-wide est la seule garde. Un `chromium`
//!   hostile qui ferme sa lecture pendant un `write_all` lèverait sinon SIGPIPE et
//!   **tuerait le helper par signal** avant qu'il n'émette son erreur bornée. Le runtime
//!   Rust std installe `SIG_IGN` avant `main` (donc sûr aujourd'hui — `write_all` renvoie
//!   `EPIPE`), mais `run_browser` doit le **réaffirmer** (`libc::signal(SIGPIPE, SIG_IGN)`)
//!   pour survivre à un futur flag de build ou une entrée non-std.
//! - les fds du pipe doivent être créés **`O_CLOEXEC`** et `dup2` **uniquement** sur les
//!   fds 3/4 de l'enfant ; ce sont les **seuls** fds supplémentaires que `chromium` hérite.
//! - `chromium` ne doit **PAS** hériter du **stdout** du helper : ce stdout EST le canal de
//!   résultat `systemd-run --pipe` que `vibed` parse — un `chromium` hostile le forgerait.
//!   Rediriger le stdio de l'enfant vers null / stderr capturé.

#![cfg(unix)]

use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::io::OwnedFd;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::io::{FromRawFd, RawFd};

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
}
