# SECURITY-TOOLKIT — la trousse cybersécurité de VibeOS

> VibeOS est une distribution **security-first**. Comme Kali Linux, Parrot OS ou
> BlackArch, elle embarque une trousse d'outils de sécurité professionnelle —
> mais **gouvernée** : quand un agent IA veut se servir d'un outil, la demande
> passe par le moteur de politiques de `vibed`, avec **approbation humaine
> obligatoire** pour toute action active contre une cible.
>
> Ce document est le **catalogue de référence** : l'état de l'art 2025-2026
> des outils réellement utilisés par les meilleurs chercheurs, curé à partir
> d'une recherche multi-sources (listes Kali/BlackArch/Parrot, GitHub, OWASP,
> ProjectDiscovery, DEF CON/Black Hat). Il précise, pour chaque domaine, ce qui
> est **embarqué dans l'image** (RPM Fedora/RPM Fusion signés) et ce qui
> s'installe **à la demande** (Go/pip/pipx, conteneurs, distrobox).
>
> Dernière mise à jour : **2026-07-13**.

---

## ⚖️ Cadre d'usage autorisé — à lire en premier

Ces outils sont **doubles usage**. Ils sont fournis **exclusivement** pour :

- tester **vos propres** systèmes et réseaux ;
- des **engagements de test d'intrusion autorisés** (mandat écrit) ;
- des compétitions **CTF** et des laboratoires d'entraînement ;
- de la **recherche en sécurité** et de la **défense** (blue team, DFIR).

Les utiliser contre des systèmes tiers sans autorisation explicite est
**illégal**. VibeOS n'embarque **aucun** malware/ransomware prêt à l'emploi,
et le moteur de politiques est conçu pour qu'un agent IA ne puisse pas mener
d'action offensive sans le feu vert d'un humain.

---

## 🔐 Gouvernance : tiers de capacité T0–T3

Chaque outil reçoit un **tier VibeOS** qui gouverne son invocation **par un
agent** (l'opérateur humain, lui, s'en sert librement dans le shell) :

| Tier | Sens | Exemples | Invocation agent |
|---|---|---|---|
| **T0** | Passif / lecture seule | `dig`, `whois`, `exiftool`, `clamscan`, `sectools.list` | Autorisée |
| **T1** | Local, sans cible externe | cracking hors-ligne (`john`, `hashcat`), reverse (`radare2`, `gdb`), forensics (`sleuthkit`), audit (`lynis`) | Autorisée par défaut, auditée |
| **T2** | **Actif contre une cible** | scan (`nmap`, `masscan`), fuzzing web (`ffuf`, `gobuster`), brute-force en ligne (`hydra`), exploitation | **Approbation humaine obligatoire** |
| **T3** | **Destructif / haut risque** | MITM (`ettercap`, `dsniff`), déni de service, effacement | **Approbation humaine obligatoire** |

**État actuel (honnêteté v0.1) :**

- Les outils sont **installés** dans l'image et utilisables par l'opérateur.
- L'agent peut les **découvrir** en lecture seule via l'outil MCP **`sectools.list`** (T0) — nom, catégorie, tier, présence — mais **ne peut en exécuter aucun** : `sectools.list` n'exécute rien.
- Le **chemin d'exécution gouverné** (un futur outil MCP type `proc.run`/`tool.exec` qui lancerait un outil de la trousse) est lié au **flux d'approbation humaine de la Phase 4** : tant qu'il n'est pas livré, aucun agent ne lance d'outil T2/T3. C'est volontaire — on n'ouvre pas une surface « l'IA lance des outils offensifs » sans le garde-fou d'approbation qui va avec.
- Le journal d'audit de `vibed` reste la source de vérité de toute action agent.

Les tiers du manifeste vivent dans
[`os/rootfs/usr/share/vibeos/security-tools.tsv`](../os/rootfs/usr/share/vibeos/security-tools.tsv)
(lu par `sectools.list`) et doivent rester synchronisés avec la couche
`1d-bis` de [`os/Containerfile`](../os/Containerfile).

---

## 📦 Embarqué dans l'image (RPM signés Fedora / RPM Fusion)

Tout ce qui suit est installé par la couche **`1d-bis`** du Containerfile : des
**RPM signés**, cohérents avec la chaîne d'approvisionnement d'un OS immuable
(≈ 60 paquets, ~2 Gio). Marqué **[IMG]** dans les sections par domaine.

Le reste du catalogue (Go/pip/conteneurs) est **curé mais non embarqué** :
l'espace utilisateur immuable se prête mal aux binaires Go/pip dans `/usr`, et
la philosophie VibeOS est d'installer ces outils **par utilisateur** (`pipx`,
`go install`, `uvx`) ou dans un **distrobox** dédié. Chaque section indique la
voie recommandée.

---

## 1. Reconnaissance passive & OSINT

Empreinte d'une cible sans (ou avec peu de) contact direct.

| Outil | Tier | Voie | Rôle |
|---|---|---|---|
| **subfinder** (ProjectDiscovery) | T0 | `go install` | Énumération passive de sous-domaines (standard de facto du bug bounty ; remplace Sublist3r) |
| **amass** (OWASP) | T2 | `go install` | Cartographie d'attack surface (OSINT passif + DNS actif) |
| **dnsx** / **asnmap** / **tlsx** / **uncover** | T0–T2 | `go install` | Suite ProjectDiscovery : DNS massif, ASN, empreinte TLS, agrégation Shodan/Censys |
| **urlfinder** / **gau** | T0 | `go install` | URLs historiques (Wayback/CommonCrawl) — successeurs de waybackurls |
| **theHarvester** | T0 | `pipx` | E-mails/sous-domaines/IP depuis 30+ sources |
| **SpiderFoot** | T2 | conteneur | Automatisation OSINT (200+ modules, graphe) |
| **holehe** / **maigret** | T0 | `pipx` | Présence d'un e-mail / pseudo sur des centaines de sites |
| **gitleaks** / **trufflehog** | T1 | `go install` / binaire | Secrets dans l'historique git |
| **Maltego CE** | T0 | binaire | Link-analysis graphique (édition Community) |
| **dig** (bind-utils) | **T0 [IMG]** | RPM | Requêtes DNS |
| **whois** | **T0 [IMG]** | RPM | Enregistrement domaine/IP |
| **exiftool** | **T0 [IMG]** | RPM | Métadonnées de fichiers/images |
| **dnsenum** | **T2 [IMG]** | RPM | Énumération DNS + transfert de zone |

## 2. Scan réseau & énumération de services

| Outil | Tier | Voie | Rôle |
|---|---|---|---|
| **Nmap** (+ NSE) | **T2 [IMG]** | RPM | Scanner de référence : ports, services, OS, scripts |
| **Masscan** | **T2 [IMG]** | RPM | Scanner de ports asynchrone à l'échelle d'Internet |
| **RustScan** / **naabu** | T2 | COPR / `go install` | Scanners de ports modernes ultra-rapides (feeder de nmap) |
| **NetExec** (`nxc`, ex-CrackMapExec) | T2 | `pipx` | Énumération/exploitation SMB/LDAP/WinRM/MSSQL (remplace CrackMapExec abandonné) |
| **Impacket** | **T2 [IMG]** | RPM | Boîte à outils protocoles réseau (SMB/MSRPC/Kerberos) |
| **enum4linux-ng** / **smbmap** / **ldapdomaindump** | T2 | `pipx` | Énumération SMB/LDAP |
| **kerbrute** | T2 | `go install` | Énumération/brute Kerberos (pré-auth AS-REP) |
| **net-snmp** (`snmpwalk`) | T2 | RPM (dispo) | Interrogation SNMP |
| **hping3** | **T2 [IMG]** | RPM | Paquets TCP/IP forgés, scan, probing |
| **arp-scan** / **nbtscan** | **T2 [IMG]** | RPM | Découverte d'hôtes ARP / NetBIOS |
| **tcpdump** / **tshark** (wireshark-cli) / **ngrep** | **T1 [IMG]** | RPM | Capture et analyse de trafic |
| **tcpreplay** | **T2 [IMG]** | RPM | Rejeu de trafic capturé |

## 3. Sécurité applicative web & API

| Outil | Tier | Voie | Rôle |
|---|---|---|---|
| **Burp Suite** (CE/Pro) | T2 | binaire | Proxy d'interception de référence (Pro = **commercial**) |
| **Caido** | T2 | binaire | Alternative moderne à Burp (Rust), montante |
| **OWASP ZAP** | T2 | conteneur | Scanner web open-source de référence |
| **mitmproxy** | T2 | `pipx` | Proxy d'interception scriptable |
| **nuclei** (ProjectDiscovery) | T2 | `go install` | Scanner de vulnérabilités par templates (état de l'art) |
| **httpx** / **katana** | T2 | `go install` | Sonde HTTP massive / crawler |
| **feroxbuster** / **dirsearch** | T2 | binaire / `pipx` | Découverte de contenu (modernes) |
| **sqlmap** | T2 | `pipx` | Exploitation d'injection SQL automatisée |
| **wpscan** | T2 | `gem`/conteneur | Audit WordPress |
| **dalfox** | T2 | `go install` | Scanner/exploiteur XSS |
| **jwt_tool** | T1 | `pipx` | Analyse/forge de JWT |
| **ffuf** | **T2 [IMG]** | RPM | Fuzzer web rapide (découverte, paramètres) |
| **gobuster** | **T2 [IMG]** | RPM | Brute-force répertoires/DNS/vhosts |
| **wfuzz** / **whatweb** | **T2 [IMG]** | RPM | Fuzzing web / fingerprint de technologies |

## 4. Frameworks d'exploitation & Command-and-Control (red team)

> **Doubles usage forts** : tous T2/T3, approbation humaine obligatoire côté
> agent. Fournis pour du red-teaming **autorisé**.

| Outil | Tier | Voie | Rôle |
|---|---|---|---|
| **Metasploit Framework** | T2 | binaire/installeur | Framework d'exploitation de référence |
| **Sliver** (BishopFox) | T2 | binaire | C2 moderne en Go (remplace largement Empire) |
| **Mythic** / **Havoc** / **AdaptixC2** | T2 | conteneur/build | C2 modernes multi-agents |
| **Cobalt Strike** | T2 | **commercial** | C2 red-team commercial (référence, signalé) |
| **Impacket** | **T2 [IMG]** | RPM | `secretsdump`, `psexec`, `wmiexec`, relais NTLM… |
| **NetExec** (`nxc`) | T2 | `pipx` | Post-exploitation réseau AD |
| **BloodHound CE** + **SharpHound** | T1/T2 | conteneur/binaire | Cartographie des chemins d'attaque Active Directory |
| **Certipy** | T2 | `pipx` | Exploitation AD CS (ESC1-ESC16) |
| **Responder** | T3 | `pipx`/git | Empoisonnement LLMNR/NBT-NS/mDNS (capture de hash) |

## 5. Attaques de mots de passe & cracking

| Outil | Tier | Voie | Rôle |
|---|---|---|---|
| **hashcat** | **T1 [IMG]** | RPM | Cracking GPU (profite de la **couche NVIDIA/CUDA** amd64 de VibeOS) |
| **John the Ripper** (jumbo) | **T1 [IMG]** | RPM | Cracking hors-ligne polyvalent |
| **hydra** / **medusa** / **ncrack** | **T2 [IMG]** | RPM | Brute-force en ligne de services réseau |
| **name-that-hash** / **haiti** | T0 | `pipx` | Identification de type de hash |
| **kerbrute** / **patator** | T2 | `go`/`pipx` | Brute Kerberos / multi-protocoles |
| **SecLists** / **rockyou** | — | git | Dictionnaires de référence (à installer par l'utilisateur) |

## 6. Sans-fil & radio (Wi-Fi, Bluetooth, SDR)

> Prérequis **matériel** : carte Wi-Fi en mode monitor, dongle SDR (RTL-SDR,
> HackRF)… selon l'outil.

| Outil | Tier | Voie | Rôle |
|---|---|---|---|
| **aircrack-ng** (suite) | **T2 [IMG]** | RPM Fusion | Analyse/récupération de clés Wi-Fi WEP/WPA |
| **hcxtools** | **T1 [IMG]** | RPM | Extraction de captures Wi-Fi pour hashcat |
| **hcxdumptool** | T2 | RPM (dispo) | Capture PMKID/handshakes |
| **kismet** | **T1 [IMG]** | RPM | Sniffer sans-fil passif / IDS |
| **reaver** / **pixiewps** | **T2/T1 [IMG]** | RPM | Attaque WPS (pixie-dust) |
| **wifite2** / **eaphammer** / **airgeddon** | T2 | `pipx`/build | Automatisation d'attaques Wi-Fi |
| **bettercap** | T2/T3 | `go install` | Cadre MITM réseau/Wi-Fi/BLE moderne |
| **BlueZ** (`bluetoothctl`, `btmon`) | T1 | RPM (dispo) | Reconnaissance Bluetooth |
| **GNU Radio** / **gqrx** / **rtl_433** / **URH** | T1/T2 | RPM/build | SDR (démodulation, rejeu) — pile lourde, overlay optionnel |

## 7. Rétro-ingénierie & analyse de malware (surtout T1, local)

| Outil | Tier | Voie | Rôle |
|---|---|---|---|
| **Ghidra** (NSA) | T1 | binaire | Suite de reverse de référence (décompilateur) |
| **radare2** / **rizin** | **T1 [IMG]** | RPM | Frameworks de reverse en ligne de commande |
| **Cutter** | T1 | RPM (dispo) | GUI de rizin |
| **binwalk** | **T1 [IMG]** | RPM | Analyse/extraction de firmware |
| **YARA** / **YARA-X** / **capa** (Mandiant) | **T1 [IMG]** (yara) | RPM/`pipx` | Classification de malware par motifs / capacités |
| **gdb** (+ pwndbg/GEF) | **T1 [IMG]** | RPM | Débogage dynamique |
| **strace** / **ltrace** | **T1 [IMG]** | RPM | Trace des appels système / bibliothèque |
| **checksec** / **patchelf** / **upx** | **T1 [IMG]** | RPM | Durcissement binaire, ELF, packing |
| **pwntools** (`pwn`) | **T1 [IMG]** | RPM | Framework de développement d'exploits/CTF |
| **frida** / **FLOSS** / **Detect It Easy** | T1 | `pipx`/binaire | Instrumentation dynamique, désobfuscation |

## 8. Forensics & DFIR

> Majoritairement **défensif** (T0/T1) : lecture d'images, mémoire, journaux.

| Outil | Tier | Voie | Rôle |
|---|---|---|---|
| **Volatility 3** | T1 | `pipx` | Analyse de mémoire (framework de référence) |
| **The Sleuth Kit** (`fls`, `mmls`) | **T1 [IMG]** | RPM | Analyse de systèmes de fichiers |
| **foremost** / **scalpel** / **testdisk** | **T1 [IMG]** | RPM | Carving et récupération de fichiers/partitions |
| **chntpw** / **steghide** | **T1 [IMG]** | RPM | Édition registre Windows hors-ligne / stéganographie |
| **plaso** (log2timeline) | T1 | `pipx` | Super-timeline forensique |
| **Chainsaw** / **Hayabusa** | T1 | binaire | Analyse rapide des journaux d'événements Windows |
| **Velociraptor** | T1 | binaire | Chasse/collecte DFIR à grande échelle (montant) |
| **bulk_extractor** / **ewf-tools** | T1 | RPM (dispo) | Extraction de features / images EWF |
| **YARA** | **T1 [IMG]** | RPM | Détection sur disque/mémoire |

## 9. Sécurité cloud, conteneurs & supply-chain (défensif, T0/T1)

| Outil | Tier | Voie | Rôle |
|---|---|---|---|
| **Trivy** | T1 | binaire | Scanner tout-en-un (images, IaC, secrets, SBOM) |
| **Grype** + **Syft** | T1 | binaire | Scan de vulnérabilités + génération de SBOM |
| **OSV-Scanner** | T1 | binaire | Vulnérabilités de dépendances (base OSV) |
| **Semgrep** / **OpenGrep** | T1 | `pipx`/binaire | SAST par règles |
| **gitleaks** / **trufflehog** / **Kingfisher** | T1 | binaire | Détection de secrets |
| **Checkov** / **KICS** / **tfsec** | T1 | `pipx`/binaire | Analyse IaC (Terraform, K8s…) |
| **Prowler** / **ScoutSuite** | T1 | `pipx` | Audit de posture AWS/Azure/GCP |
| **kube-bench** / **kubescape** / **kube-hunter** | T1/T2 | binaire/conteneur | Audit Kubernetes (CIS, exposition) |
| **Pacu** / **CloudFox** | T2 | `pipx`/binaire | Exploitation cloud (AWS) — engagement autorisé |
| **cosign** (Sigstore) | T1 | binaire | Signature/vérification d'artefacts (**déjà utilisé par la CI de VibeOS**) |
| **openscap** (`oscap`) + **scap-security-guide** | **T1 [IMG]** | RPM | Conformité SCAP / durcissement |
| **Suricata** | **T1 [IMG]** | RPM | IDS/IPS réseau |
| **clamav** / **rkhunter** / **lynis** / **chkrootkit** / **aide** | **T0/T1 [IMG]** | RPM | Antivirus, anti-rootkit, audit, intégrité fichiers |

## 10. Sécurité de l'IA & des LLM — l'angle « natif-IA »

> **Le domaine le plus pertinent pour VibeOS.** Un OS où des agents IA agissent
> au niveau système doit pouvoir **tester et défendre ses propres agents**.
> Majoritairement T1 (test local des modèles/agents), voie `pipx`.

| Outil | Tier | Voie | Rôle |
|---|---|---|---|
| **garak** (NVIDIA) | T1 | `pipx` | Scanner de vulnérabilités de LLM (injection, jailbreak, fuite) — le « nmap des LLM » |
| **PyRIT** (Microsoft) | T1 | `pipx` | Framework de red-teaming IA automatisé |
| **promptfoo** | T1 | `npx` | Évaluation + red-team de prompts/agents |
| **deepteam** / **giskard** | T1 | `pipx` | Red-team et test de robustesse de modèles |
| **NeMo Guardrails** (NVIDIA) / **LLM Guard** / **Guardrails AI** | T1 | `pipx` | **Défense** : garde-fous contre l'injection de prompt |
| **LlamaFirewall** / **Prompt Guard 2** (Meta) | T1 | `pipx` | Pare-feu et classifieur d'injection pour agents |
| **Adversarial Robustness Toolbox** (IBM) / **Foolbox** | T1 | `pipx` | Attaques/défenses adversariales (ML classique) |
| **modelscan** / **picklescan** | T1 | `pipx` | Détection de modèles malveillants (pickle, désérialisation) |
| **Counterfit** (Microsoft) | T1 | `pipx` | Automatisation d'évaluation de sécurité ML |

Ces outils recoupent directement le **modèle de menace** de VibeOS
([THREAT-MODEL.md](THREAT-MODEL.md) : S1 injection de prompt, S4 modèle
empoisonné). À terme, VibeOS pourrait s'auto-tester avec `garak`/`PyRIT` dans
sa propre CI.

---

## Installer les outils non embarqués

L'OS étant immuable, on **ne modifie jamais `/usr`**. Voies recommandées :

```bash
# Outils Python (isolés par outil) :
pipx install sqlmap volatility3 semgrep prowler
pipx install garak pyrit          # sécurité IA/LLM

# Outils Go (dans ~/go/bin) :
go install github.com/projectdiscovery/nuclei/v3/cmd/nuclei@latest
go install github.com/projectdiscovery/subfinder/v2/cmd/subfinder@latest

# Binaires précompilés (Trivy, Grype, Sliver…) : télécharger la release
# officielle, vérifier la somme de contrôle/signature, poser dans ~/.local/bin.

# Environnement lourd ou incompatible immuable : un conteneur dédié.
distrobox create --name pentest --image kalilinux/kali-rolling
distrobox enter pentest
```

Les dictionnaires (**SecLists**, rockyou) se clonent dans `~/wordlists`.

---

## Étendre la trousse embarquée

1. Ajouter le paquet à la couche **`1d-bis`** de [`os/Containerfile`](../os/Containerfile) (RPM Fedora/RPM Fusion **uniquement** — supply chain).
2. Ajouter la ligne correspondante à [`os/security-tools.txt`](../os/security-tools.txt) (manifeste build) **et** à [`os/rootfs/usr/share/vibeos/security-tools.tsv`](../os/rootfs/usr/share/vibeos/security-tools.tsv) (binaire, catégorie, **tier**, description — lu par `sectools.list`).
3. Documenter l'outil dans la section de domaine ci-dessus.
4. Vérifier le tier : par défaut **T2** pour tout ce qui touche une cible, **T3** pour le destructif. En cas de doute, tier plus élevé (fail-safe).
