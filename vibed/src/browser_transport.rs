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

#![cfg(unix)]

use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::io::{FromRawFd, RawFd};

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
    /// Construit le canal à partir des deux fds bruts du pipe.
    ///
    /// # Safety
    /// `write_fd` et `read_fd` doivent être des descripteurs **valides**, **possédés**
    /// (le `PipeChannel` en prend la propriété et les fermera au drop) et **non partagés**
    /// ailleurs. C'est le cas dans le transport : ce sont les extrémités du pipe créé pour
    /// la session `chromium`.
    pub unsafe fn from_raw_fds(write_fd: RawFd, read_fd: RawFd) -> Self {
        Self {
            to_peer: File::from_raw_fd(write_fd),
            from_peer: File::from_raw_fd(read_fd),
        }
    }
}

impl CdpChannel for PipeChannel {
    fn send(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        // write_all : jamais d'écriture partielle silencieuse (contrat CdpChannel).
        self.to_peer.write_all(bytes)
    }

    fn recv(&mut self) -> std::io::Result<Vec<u8>> {
        let mut buf = [0u8; 16 * 1024];
        // Read bloquant. `n == 0` = vrai EOF (le pair a fermé) → `Ok(vide)`, que
        // `CdpSession` traite en fermeture fail-closed. Jamais `Ok(vide)` sur autre chose.
        let n = self.from_peer.read(&mut buf)?;
        Ok(buf[..n].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Crée un pipe unidirectionnel `(read, write)` via `libc::pipe`.
    fn os_pipe() -> (RawFd, RawFd) {
        let mut fds = [0 as RawFd; 2];
        // # Safety : fds est un tableau de 2 i32, l'API attend exactement ça.
        let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
        assert_eq!(rc, 0, "libc::pipe a échoué");
        (fds[0], fds[1]) // (read, write)
    }

    /// Enveloppe un fd brut dans un `File` pour l'écriture/lecture côté test.
    fn as_file(fd: RawFd) -> File {
        // # Safety : fd valide et possédé par ce File.
        unsafe { File::from_raw_fd(fd) }
    }

    #[test]
    fn send_writes_all_bytes_to_the_peer() {
        // Pipe : le PipeChannel écrit sur `w`, le test lit sur `r`.
        let (r, w) = os_pipe();
        // # Safety : on donne à PipeChannel un fd d'écriture valide ; le fd de lecture est
        // un second pipe factice (non utilisé par send).
        let (r2, w2) = os_pipe();
        let mut chan = unsafe { PipeChannel::from_raw_fds(w, r2) };
        chan.send(b"Page.navigate\0").unwrap();
        // Lit ce que le canal a écrit.
        let mut out = [0u8; 64];
        let n = as_file(r).read(&mut out).unwrap();
        assert_eq!(&out[..n], b"Page.navigate\0");
        // Ferme le second pipe factice.
        let _ = as_file(w2);
    }

    #[test]
    fn recv_reads_what_the_peer_wrote() {
        // Le test écrit sur `w`, le PipeChannel lit sur `r`.
        let (r, w) = os_pipe();
        let (r2, w2) = os_pipe(); // pipe factice pour le côté écriture du canal
        let mut chan = unsafe { PipeChannel::from_raw_fds(w2, r) };
        let mut peer_w = as_file(w);
        peer_w.write_all(b"{\"id\":1,\"result\":{}}\0").unwrap();
        let got = chan.recv().unwrap();
        assert_eq!(got, b"{\"id\":1,\"result\":{}}\0");
        let _ = as_file(r2);
        drop(peer_w);
    }

    #[test]
    fn recv_returns_empty_on_true_eof() {
        let (r, w) = os_pipe();
        let (r2, w2) = os_pipe();
        let mut chan = unsafe { PipeChannel::from_raw_fds(w2, r) };
        // Ferme l'extrémité d'écriture du pair → EOF sur la lecture du canal.
        drop(as_file(w));
        let got = chan.recv().unwrap();
        assert!(got.is_empty(), "un vrai EOF doit donner un vec vide");
        let _ = as_file(r2);
    }

    #[test]
    fn a_full_cdp_exchange_drives_a_session_over_real_pipes() {
        // Bout en bout : un CdpSession pilote un PipeChannel sur de VRAIS pipes, un thread
        // « faux chromium » répond. Prouve que le transport + le codec + le pilote
        // s'assemblent sans chromium.
        use crate::cdp::CdpSession;
        use std::io::Write as _;

        let (agent_r, peer_w) = os_pipe(); // chromium→agent : le pair écrit, l'agent lit
        let (peer_r, agent_w) = os_pipe(); // agent→chromium : l'agent écrit, le pair lit

        // Faux pair : lit une commande, renvoie une réponse corrélée.
        let peer = std::thread::spawn(move || {
            let mut pr = as_file(peer_r);
            let mut pw = as_file(peer_w);
            let mut buf = [0u8; 4096];
            let n = pr.read(&mut buf).unwrap();
            // La commande émise porte "id":1 ; on renvoie un result pour id 1.
            assert!(buf[..n].windows(6).any(|w| w == b"\"id\":1"));
            pw.write_all(b"{\"id\":1,\"result\":{\"ok\":true}}\0")
                .unwrap();
        });

        let mut session = CdpSession::new(unsafe { PipeChannel::from_raw_fds(agent_w, agent_r) });
        let result = session
            .call("Page.enable", serde_json::json!({}), None)
            .unwrap();
        assert_eq!(result, serde_json::json!({"ok": true}));
        peer.join().unwrap();
    }
}
