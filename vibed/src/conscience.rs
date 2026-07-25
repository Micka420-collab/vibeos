//! `conscience` — le citoyen confronte son caractère DÉCLARÉ à sa conduite ENREGISTRÉE.
//!
//! # Pourquoi
//!
//! ADR-029 donne au citoyen un caractère déterministe à la naissance, et
//! `tools::identity::operating_style` le traduit en conduite annoncée : « je
//! travaille de façon télégraphique, j'explore des alternatives, je confirme
//! l'irréversible ». C'est une DÉCLARATION. Rien, jusqu'ici, ne la confrontait
//! aux faits.
//!
//! Or les faits existent déjà, et ils sont infalsifiables : le journal d'audit
//! chaîné par SHA-256 enregistre chaque appel d'outil, sa décision et son issue.
//! Ce module met les deux face à face et dit, sans complaisance, quand ils ne
//! concordent pas — un citoyen qui se dit prudent et dont un appel sur trois se
//! fait refuser par le plancher n'est pas prudent, il est optimiste.
//!
//! # Ce que ce n'est PAS
//!
//! **Jamais une permission.** Exactement comme le style opératoire (ADR-029) :
//! le plancher de gouvernance — palier, approbation humaine, denylist, audit —
//! est décidé ailleurs et ne consulte JAMAIS ce module. Une conduite jugée
//! « cohérente » n'ouvre aucun droit ; une conduite « en tension » n'en ferme
//! aucun. C'est un miroir, pas un tribunal, et surtout pas un vigile.
//!
//! **Jamais une sanction.** Aucune décision automatique n'en découle. La seule
//! sortie est un texte pour l'humain qui lit `vibectl conscience`.
//!
//! **Jamais une divulgation.** Le journal contient des `target` — des chemins,
//! des hôtes, des unités — qui appartiennent à TOUS les appelants. Ce module
//! n'en ressort aucun : il ne compte que des agrégats, et ne nomme que des
//! outils. Voir `Conduite::observe` pour la borne sur les noms d'outils.
//!
//! # Honnêteté du verdict
//!
//! Un axe sans donnée dit **« sans données »**, jamais « cohérent ». La
//! tentation inverse — considérer l'absence de contre-exemple comme une preuve
//! de vertu — donnerait à une machine qui n'a rien fait un bulletin parfait.
//! C'est le mode de panne le plus insidieux d'un outil comme celui-ci, et il est
//! fermé par construction : `Verdict::SansDonnees` est un état à part entière,
//! et la cohérence globale ne se calcule que sur les axes qui en ont.

use std::collections::BTreeMap;

use serde_json::{json, Value};

/// Combien de noms d'outils distincts on garde au plus dans l'agrégat.
///
/// Le champ `tool` d'un enregistrement vient du `name` JSON-RPC de l'appelant.
/// Un appelant qui inventerait un nom neuf à chaque appel ferait grossir la
/// table sans borne — le journal, lui, est déjà écrit. On garde les plus
/// fréquents et on compte le reste en bloc : l'agrégat reste borné, et le fait
/// qu'on ait tronqué est DIT (`outils_tronques`), jamais tu.
const MAX_OUTILS_SUIVIS: usize = 12;

/// Longueur maximale d'un nom d'outil retenu. Au-delà, il est compté dans le
/// reste : un nom de 10 Mio n'a pas à voyager jusqu'au terminal de l'opérateur.
const MAX_LONGUEUR_NOM_OUTIL: usize = 64;

/// UN APPEL, DEUX ENREGISTREMENTS.
///
/// Le répartiteur écrit `started*` AVANT d'exécuter puis `ok*`/`error`/`panic`
/// après — c'est l'invariant « auditer avant d'agir », et il vaut pour le seul
/// chemin `Allow`. Un refus (`blocked*`), une approbation en attente
/// (`pending_approval`) ou un débit dépassé (`rate_limited`) n'écrivent qu'UNE
/// ligne.
///
/// Compter toutes les lignes ferait donc peser un appel autorisé DEUX fois et un
/// appel refusé une seule : le taux de refus serait mécaniquement divisé par
/// deux, et un citoyen imprudent passerait pour raisonnable. On ne compte que
/// les enregistrements TERMINAUX, c'est-à-dire tout sauf `started*`.
///
/// Conséquence assumée : un appel dont le `started` existe mais dont l'issue n'a
/// jamais été écrite (arrêt brutal entre les deux) n'est pas compté. On préfère
/// ne compter que ce qu'on a vu conclure — l'autre sens inventerait une issue.
fn est_terminal(record: &Value) -> bool {
    !record
        .get("outcome")
        .and_then(Value::as_str)
        .is_some_and(|o| o.starts_with("started"))
}

/// Ce que le journal dit qu'il s'est PASSÉ. Agrégats seulement — aucune cible,
/// aucun argument, rien qui puisse fuir d'un appelant vers un autre.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Conduite {
    /// Appels retenus : enregistrements TERMINAUX (voir `est_terminal`), après
    /// filtrage éventuel par uid.
    pub appels: u64,
    /// Décision `deny` — le plancher a refusé.
    pub refuses: u64,
    /// Décision `require_approval` — un humain a été sollicité.
    pub approbations_exigees: u64,
    /// Issue en échec, décision d'autorisation mise à part.
    pub echecs: u64,
    /// Appels par outil, bornés à `MAX_OUTILS_SUIVIS`.
    pub par_outil: BTreeMap<String, u64>,
    /// Appels dont le nom d'outil n'a pas été retenu (table pleine, ou nom trop
    /// long). Compté, jamais ignoré.
    pub outils_tronques: u64,
    /// Lignes du journal illisibles rencontrées pendant le balayage.
    pub illisibles: u64,
}

impl Conduite {
    /// Intègre un enregistrement d'audit. `uid` filtre sur `caller_uid` quand il
    /// est fourni ; `None` prend tout le journal (usage opérateur/root).
    ///
    /// Un enregistrement sans `tool` exploitable est compté dans les appels mais
    /// pas attribué : il a bien eu lieu, on ne sait juste pas à quoi l'imputer.
    pub fn observe(&mut self, record: &Value, uid: Option<u32>) {
        if let Some(want) = uid {
            match record.get("caller_uid").and_then(Value::as_u64) {
                Some(got) if got == u64::from(want) => {}
                // Pas le bon appelant, ou pas d'appelant établi : on n'impute
                // rien. Un enregistrement sans `caller_uid` n'est PAS supposé
                // être le nôtre — fail-closed, comme partout ailleurs ici.
                _ => return,
            }
        }
        // La ligne d'ouverture d'un appel autorisé n'est pas un appel de plus.
        if !est_terminal(record) {
            return;
        }
        self.appels += 1;

        let decision = record.get("decision").and_then(Value::as_str);
        match decision {
            Some("deny") => self.refuses += 1,
            Some("require_approval") => self.approbations_exigees += 1,
            _ => {}
        }
        // Échec d'EXÉCUTION, pas de gouvernance : un refus n'est pas un échec,
        // c'est le système qui fonctionne. Les compter ensemble donnerait
        // « 6 refus · 6 échecs » pour six événements, et ferait lire douze
        // problèmes là où il y en a six.
        if decision == Some("allow") {
            match record.get("outcome").and_then(Value::as_str) {
                // `ok`, `ok_open_mode`, `ok_approved:<uid>` — toutes les formes
                // du succès partagent le préfixe.
                Some(o) if o.starts_with("ok") => {}
                // Une issue absente ou inconnue compte comme un échec : mieux
                // vaut sur-compter que déclarer un succès qu'on n'a pas lu.
                _ => self.echecs += 1,
            }
        }

        let outil = record.get("tool").and_then(Value::as_str).unwrap_or("");
        if outil.is_empty() || outil.len() > MAX_LONGUEUR_NOM_OUTIL {
            self.outils_tronques += 1;
            return;
        }
        if let Some(n) = self.par_outil.get_mut(outil) {
            *n += 1;
        } else if self.par_outil.len() < MAX_OUTILS_SUIVIS {
            self.par_outil.insert(outil.to_string(), 1);
        } else {
            self.outils_tronques += 1;
        }
    }

    /// Nombre d'outils DISTINCTS effectivement observés. Borne basse assumée
    /// quand la table a débordé : on ne sait pas combien il y en avait de plus,
    /// et on ne le devine pas.
    pub fn outils_distincts(&self) -> usize {
        self.par_outil.len()
    }

    /// Le préfixe de famille d'un outil (`memory.query` -> `memory`), pour les
    /// axes qui raisonnent par famille plutôt que par outil exact.
    fn appels_de_famille(&self, famille: &str) -> u64 {
        self.par_outil
            .iter()
            .filter(|(k, _)| k.split('.').next() == Some(famille))
            .map(|(_, n)| *n)
            .sum()
    }
}

/// Le verdict d'UN axe. `SansDonnees` est un état à part entière, jamais un
/// synonyme discret de « cohérent ».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Coherent,
    EnTension,
    SansDonnees,
}

impl Verdict {
    fn as_str(self) -> &'static str {
        match self {
            Verdict::Coherent => "cohérent",
            Verdict::EnTension => "en tension",
            Verdict::SansDonnees => "sans données",
        }
    }
}

/// En dessous de ce nombre d'appels, aucun axe ne conclut : trois appels ne
/// disent rien du caractère de personne. Refuser de conclure est un résultat.
const APPELS_MINIMUM: u64 = 20;

/// Seuil « trait haut » / « trait bas », alignés sur les bandes de
/// `tools::identity::operating_style` pour que les deux racontent la même
/// histoire (un style annoncé « prudent » et un axe qui juge la prudence
/// doivent partir du même chiffre).
const TRAIT_HAUT: i64 = 66;
const TRAIT_BAS: i64 = 33;

/// Part de refus au-delà de laquelle un citoyen qui se DIT prudent ne l'est
/// visiblement pas : il découvre le plancher en le heurtant.
const REFUS_TOLERES_SI_PRUDENT: f64 = 0.10;

/// Confronte le caractère aux faits. Pure : mêmes entrées, même sortie.
///
/// `traits` est la table déjà bornée d'`identity_value` (0..=100). `conduite`
/// est l'agrégat du journal. La sortie est prête à sérialiser, et porte à la
/// fois le verdict par axe et de quoi le contester : chaque observation dit sur
/// QUOI elle s'appuie, pour qu'un humain qui n'est pas d'accord puisse aller
/// vérifier plutôt que devoir croire.
pub fn confronter(traits: &BTreeMap<String, i64>, conduite: &Conduite) -> Value {
    let trait_de = |k: &str| traits.get(k).copied();
    let assez = conduite.appels >= APPELS_MINIMUM;

    let mut axes: Vec<Value> = Vec::new();

    // --- Prudence : se dire prudent et heurter le plancher sont incompatibles.
    {
        let (verdict, constat) = match (trait_de("caution"), assez) {
            (_, false) => (
                Verdict::SansDonnees,
                format!(
                    "{} appel(s) enregistré(s) : trop peu pour dire quoi que ce soit (seuil {APPELS_MINIMUM}).",
                    conduite.appels
                ),
            ),
            (None, _) => (
                Verdict::SansDonnees,
                "aucun trait de prudence déclaré.".to_string(),
            ),
            (Some(caution), true) => {
                let part = conduite.refuses as f64 / conduite.appels as f64;
                if caution >= TRAIT_HAUT && part > REFUS_TOLERES_SI_PRUDENT {
                    (
                        Verdict::EnTension,
                        format!(
                            "prudence déclarée à {caution}, et pourtant {} appel(s) sur {} ont été REFUSÉS par le plancher ({:.0} %). Se dire prudent et découvrir la limite en la heurtant ne va pas ensemble.",
                            conduite.refuses,
                            conduite.appels,
                            part * 100.0
                        ),
                    )
                } else if caution < TRAIT_BAS && conduite.refuses == 0 {
                    (
                        Verdict::EnTension,
                        format!(
                            "prudence déclarée à {caution} — basse — mais aucun refus sur {} appels. Cette conduite est plus prudente que le caractère annoncé.",
                            conduite.appels
                        ),
                    )
                } else {
                    (
                        Verdict::Coherent,
                        format!(
                            "prudence déclarée à {caution}, {} refus sur {} appels ({:.0} %).",
                            conduite.refuses,
                            conduite.appels,
                            part * 100.0
                        ),
                    )
                }
            }
        };
        axes.push(json!({
            "axe": "prudence",
            "verdict": verdict.as_str(),
            "constat": constat,
            "appuyé_sur": "decision == deny dans le journal d'audit",
        }));
    }

    // --- Curiosité : annoncer qu'on explore, et n'utiliser qu'un outil.
    {
        let distincts = conduite.outils_distincts();
        let (verdict, constat) = match (trait_de("curiosity"), assez) {
            (_, false) => (
                Verdict::SansDonnees,
                format!(
                    "{} appel(s) : trop peu (seuil {APPELS_MINIMUM}).",
                    conduite.appels
                ),
            ),
            (None, _) => (
                Verdict::SansDonnees,
                "aucun trait de curiosité déclaré.".to_string(),
            ),
            (Some(curiosity), true) => {
                if curiosity >= TRAIT_HAUT && distincts <= 1 {
                    (
                        Verdict::EnTension,
                        format!(
                            "curiosité déclarée à {curiosity}, mais {} outil distinct sur {} appels. Explorer, ça se voit.",
                            distincts, conduite.appels
                        ),
                    )
                } else {
                    (
                        Verdict::Coherent,
                        format!(
                            "curiosité déclarée à {curiosity}, {distincts} outil(s) distinct(s) sur {} appels.",
                            conduite.appels
                        ),
                    )
                }
            }
        };
        axes.push(json!({
            "axe": "curiosité",
            "verdict": verdict.as_str(),
            "constat": constat,
            "appuyé_sur": if conduite.outils_tronques > 0 {
                "nombre d'outils distincts (BORNE BASSE : la table a débordé)"
            } else {
                "nombre d'outils distincts appelés"
            },
        }));
    }

    // --- Initiative : annoncer qu'on propose, et ne rien proposer.
    {
        let propositions = conduite.appels_de_famille("propose");
        let (verdict, constat) = match (trait_de("initiative"), assez) {
            (_, false) => (
                Verdict::SansDonnees,
                format!(
                    "{} appel(s) : trop peu (seuil {APPELS_MINIMUM}).",
                    conduite.appels
                ),
            ),
            (None, _) => (
                Verdict::SansDonnees,
                "aucun trait d'initiative déclaré.".to_string(),
            ),
            (Some(initiative), true) => {
                if initiative >= TRAIT_HAUT && propositions == 0 {
                    (
                        Verdict::EnTension,
                        format!(
                            "initiative déclarée à {initiative}, et aucune proposition (`propose.*`) en {} appels.",
                            conduite.appels
                        ),
                    )
                } else {
                    (
                        Verdict::Coherent,
                        format!(
                            "initiative déclarée à {initiative}, {propositions} proposition(s) en {} appels.",
                            conduite.appels
                        ),
                    )
                }
            }
        };
        axes.push(json!({
            "axe": "initiative",
            "verdict": verdict.as_str(),
            "constat": constat,
            "appuyé_sur": "appels de la famille propose.*",
        }));
    }

    // --- Concision : AUCUNE donnée, et on le dit.
    axes.push(json!({
        "axe": "concision",
        "verdict": Verdict::SansDonnees.as_str(),
        "constat": "le journal d'audit n'enregistre aucune taille de sortie : cet axe n'est pas mesurable ici. Inventer un indicateur de substitution donnerait un verdict qui ne repose sur rien.",
        "appuyé_sur": "rien — c'est le propos",
    }));

    let avec_donnees = axes
        .iter()
        .filter(|a| a["verdict"] != Verdict::SansDonnees.as_str())
        .count();
    let coherents = axes
        .iter()
        .filter(|a| a["verdict"] == Verdict::Coherent.as_str())
        .count();
    let tensions = axes
        .iter()
        .filter(|a| a["verdict"] == Verdict::EnTension.as_str())
        .count();

    json!({
        "appels": conduite.appels,
        "refuses": conduite.refuses,
        "approbations_exigees": conduite.approbations_exigees,
        "echecs": conduite.echecs,
        "outils_distincts": conduite.outils_distincts(),
        "outils_tronques": conduite.outils_tronques,
        "lignes_illisibles": conduite.illisibles,
        "axes": axes,
        // La cohérence ne se calcule QUE sur les axes qui ont des données. Un
        // citoyen sans conduite n'obtient pas un bulletin parfait par défaut :
        // il obtient « aucun axe mesurable ».
        "axes_mesurables": avec_donnees,
        "axes_coherents": coherents,
        "axes_en_tension": tensions,
        "verdict": if avec_donnees == 0 {
            "aucun axe mesurable"
        } else if tensions == 0 {
            "conduite conforme au caractère annoncé"
        } else {
            "conduite en tension avec le caractère annoncé"
        },
        "portée": "OBSERVATION SEULE. Le plancher de gouvernance (palier, approbation humaine, denylist, audit) est décidé ailleurs et ne consulte jamais ce rapport. Aucune tension ne restreint quoi que ce soit ; aucune cohérence n'autorise quoi que ce soit.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(tool: &str, decision: &str, outcome: &str, uid: u64) -> Value {
        json!({"tool": tool, "decision": decision, "outcome": outcome, "caller_uid": uid})
    }

    fn traits(caution: i64, curiosity: i64, initiative: i64) -> BTreeMap<String, i64> {
        BTreeMap::from([
            ("caution".to_string(), caution),
            ("curiosity".to_string(), curiosity),
            ("initiative".to_string(), initiative),
        ])
    }

    fn conduite_de(records: &[Value], uid: Option<u32>) -> Conduite {
        let mut c = Conduite::default();
        for r in records {
            c.observe(r, uid);
        }
        c
    }

    #[test]
    fn le_filtre_par_uid_ne_laisse_passer_que_l_appelant_demande() {
        let records = vec![
            rec("fs.read", "allow", "ok", 1000),
            rec("fs.read", "allow", "ok", 1001),
            // Aucun `caller_uid` : appelant NON ÉTABLI. Il ne doit jamais être
            // imputé à quelqu'un — c'est le fail-closed du module.
            json!({"tool": "fs.read", "decision": "allow", "outcome": "ok"}),
        ];
        assert_eq!(conduite_de(&records, Some(1000)).appels, 1);
        assert_eq!(conduite_de(&records, Some(1001)).appels, 1);
        // Sans filtre, l'opérateur voit tout, y compris l'appel sans identité.
        assert_eq!(conduite_de(&records, None).appels, 3);
    }

    #[test]
    fn trop_peu_d_appels_ne_conclut_rien_plutot_que_de_flatter() {
        // Le mode de panne le plus insidieux : une machine qui n'a rien fait
        // obtiendrait un bulletin parfait si l'absence de contre-exemple valait
        // preuve de vertu.
        let c = conduite_de(&[rec("fs.read", "allow", "ok", 0)], None);
        let v = confronter(&traits(90, 90, 90), &c);
        assert_eq!(v["verdict"], "aucun axe mesurable");
        assert_eq!(v["axes_mesurables"], 0);
        assert_eq!(v["axes_coherents"], 0);
        for axe in v["axes"].as_array().unwrap() {
            assert_eq!(axe["verdict"], "sans données", "axe {}", axe["axe"]);
        }
    }

    #[test]
    fn un_citoyen_qui_se_dit_prudent_et_se_fait_refuser_est_dit_en_tension() {
        // 30 appels, 6 refus = 20 % — au-dessus du seuil de 10 %.
        let mut records: Vec<Value> = (0..24).map(|_| rec("fs.read", "allow", "ok", 0)).collect();
        records.extend((0..6).map(|_| rec("fs.write", "deny", "denied", 0)));
        let c = conduite_de(&records, None);

        let prudent = confronter(&traits(90, 50, 50), &c);
        let axe = &prudent["axes"][0];
        assert_eq!(axe["axe"], "prudence");
        assert_eq!(axe["verdict"], "en tension", "{axe}");
        assert_eq!(
            prudent["verdict"],
            "conduite en tension avec le caractère annoncé"
        );

        // MÊME conduite, caractère moins prudent : plus de tension. C'est bien
        // la CONFRONTATION qui est jugée, pas la conduite dans l'absolu.
        let insouciant = confronter(&traits(50, 50, 50), &c);
        assert_eq!(insouciant["axes"][0]["verdict"], "cohérent");
    }

    #[test]
    fn se_dire_curieux_en_n_utilisant_qu_un_outil_est_une_tension() {
        let records: Vec<Value> = (0..25).map(|_| rec("fs.read", "allow", "ok", 0)).collect();
        let c = conduite_de(&records, None);
        let v = confronter(&traits(50, 90, 50), &c);
        let curiosite = v["axes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["axe"] == "curiosité")
            .unwrap();
        assert_eq!(curiosite["verdict"], "en tension", "{curiosite}");
        assert!(curiosite["constat"]
            .as_str()
            .unwrap()
            .contains("1 outil distinct"));
    }

    #[test]
    fn se_dire_entreprenant_sans_jamais_proposer_est_une_tension() {
        let records: Vec<Value> = (0..25).map(|_| rec("fs.read", "allow", "ok", 0)).collect();
        let sans = confronter(&traits(50, 50, 90), &conduite_de(&records, None));
        let axe = |v: &Value| {
            v["axes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|a| a["axe"] == "initiative")
                .unwrap()
                .clone()
        };
        assert_eq!(axe(&sans)["verdict"], "en tension");

        // Une seule proposition suffit à lever la tension : l'axe demande un
        // signe d'initiative, pas un quota.
        let mut avec = records.clone();
        avec.push(rec("propose.change", "require_approval", "ok", 0));
        assert_eq!(
            axe(&confronter(&traits(50, 50, 90), &conduite_de(&avec, None)))["verdict"],
            "cohérent"
        );
    }

    #[test]
    fn la_concision_dit_qu_elle_n_est_pas_mesurable_plutot_que_d_inventer() {
        let records: Vec<Value> = (0..25).map(|_| rec("fs.read", "allow", "ok", 0)).collect();
        let v = confronter(&traits(50, 50, 50), &conduite_de(&records, None));
        let concision = v["axes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["axe"] == "concision")
            .unwrap();
        assert_eq!(concision["verdict"], "sans données");
        // Et elle ne compte PAS comme un axe cohérent.
        assert_eq!(v["axes_mesurables"], 3);
    }

    #[test]
    fn la_table_d_outils_est_bornee_et_le_dit() {
        // Un appelant qui invente un nom neuf à chaque appel ne doit pas faire
        // grossir l'agrégat sans borne — et la troncature doit être VISIBLE.
        let records: Vec<Value> = (0..100)
            .map(|i| rec(&format!("faux.outil{i}"), "allow", "ok", 0))
            .collect();
        let c = conduite_de(&records, None);
        assert_eq!(c.appels, 100);
        assert_eq!(c.par_outil.len(), MAX_OUTILS_SUIVIS);
        assert_eq!(c.outils_tronques, 100 - MAX_OUTILS_SUIVIS as u64);

        let v = confronter(&traits(50, 50, 50), &c);
        assert_eq!(v["outils_tronques"], 100 - MAX_OUTILS_SUIVIS as u64);
        let curiosite = v["axes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["axe"] == "curiosité")
            .unwrap();
        assert!(
            curiosite["appuyé_sur"]
                .as_str()
                .unwrap()
                .contains("BORNE BASSE"),
            "la troncature doit être annoncée : {curiosite}"
        );
    }

    #[test]
    fn un_nom_d_outil_demesure_ne_voyage_pas_jusqu_a_l_operateur() {
        let enorme = "x".repeat(MAX_LONGUEUR_NOM_OUTIL + 1);
        let c = conduite_de(&[rec(&enorme, "allow", "ok", 0)], None);
        assert_eq!(c.appels, 1);
        assert!(c.par_outil.is_empty());
        assert_eq!(c.outils_tronques, 1);
    }

    #[test]
    fn les_issues_inconnues_comptent_comme_des_echecs_pas_comme_des_succes() {
        // Fail-closed sur une issue future qu'on ne connaît pas encore : mieux
        // vaut sur-compter un échec que déclarer un succès qu'on n'a pas lu.
        let c = conduite_de(
            &[
                rec("fs.read", "allow", "ok", 0),
                rec("fs.read", "allow", "error", 0),
                rec("fs.read", "allow", "une-issue-future", 0),
                // Les formes décorées du succès restent des succès.
                rec("fs.read", "allow", "ok_open_mode", 0),
                rec("fs.read", "allow", "ok_approved:0", 0),
            ],
            None,
        );
        assert_eq!(c.echecs, 2);
        assert_eq!(c.appels, 5);
    }

    /// LE piège du journal : un appel AUTORISÉ écrit deux lignes (`started`
    /// puis `ok`), un appel REFUSÉ n'en écrit qu'une. Compter les lignes
    /// diviserait le taux de refus par deux — un citoyen imprudent passerait
    /// pour raisonnable, sans que personne ne voie l'erreur.
    #[test]
    fn un_appel_autorise_ecrit_deux_lignes_et_ne_compte_qu_une_fois() {
        let mut records = Vec::new();
        // 20 appels autorisés = 40 lignes.
        for _ in 0..20 {
            records.push(rec("fs.read", "allow", "started", 0));
            records.push(rec("fs.read", "allow", "ok", 0));
        }
        // 5 refus = 5 lignes.
        for _ in 0..5 {
            records.push(rec("fs.write", "deny", "blocked", 0));
        }
        let c = conduite_de(&records, None);
        assert_eq!(c.appels, 25, "45 lignes, mais 25 appels");
        assert_eq!(c.refuses, 5);
        assert_eq!(c.echecs, 0);

        // Le taux de refus est bien 20 %, pas 11 % (5 sur 45 lignes). Avec un
        // caractère prudent, ça doit se voir — et c'est exactement l'écart qui
        // aurait fait passer un citoyen imprudent pour raisonnable.
        let v = confronter(&traits(90, 50, 50), &c);
        assert_eq!(v["axes"][0]["verdict"], "en tension", "{}", v["axes"][0]);
        assert!(
            v["axes"][0]["constat"]
                .as_str()
                .unwrap()
                .contains("5 appel(s) sur 25"),
            "le dénominateur doit être les APPELS, pas les lignes : {}",
            v["axes"][0]
        );
    }

    #[test]
    fn les_ouvertures_decorees_sont_aussi_des_ouvertures() {
        // `started_open_mode` et `started_approved:0` sont des lignes
        // d'ouverture au même titre que `started` — les rater ferait
        // recompter double les appels en mode ouvert et les appels approuvés,
        // c'est-à-dire précisément les plus sensibles.
        let c = conduite_de(
            &[
                rec("fs.read", "allow", "started_open_mode", 0),
                rec("fs.read", "allow", "ok_open_mode", 0),
                rec("svc.restart", "allow", "started_approved:0", 0),
                rec("svc.restart", "allow", "ok_approved:0", 0),
            ],
            None,
        );
        assert_eq!(c.appels, 2);
        assert_eq!(c.echecs, 0);
    }

    #[test]
    fn un_refus_n_est_pas_un_echec_d_execution() {
        // Le compter deux fois afficherait « 3 refus · 3 échecs » pour trois
        // événements, et ferait lire six problèmes là où il y en a trois.
        let c = conduite_de(
            &[
                rec("fs.write", "deny", "blocked", 0),
                rec("fs.write", "deny", "blocked_builtin_denylist", 0),
                rec("svc.restart", "require_approval", "pending_approval", 0),
            ],
            None,
        );
        assert_eq!(c.appels, 3);
        assert_eq!(c.refuses, 2);
        assert_eq!(c.approbations_exigees, 1);
        assert_eq!(c.echecs, 0, "un refus est le système qui fonctionne");
    }

    #[test]
    fn la_portee_dit_explicitement_que_ce_n_est_pas_une_permission() {
        let v = confronter(&traits(50, 50, 50), &Conduite::default());
        let portee = v["portée"].as_str().unwrap();
        assert!(portee.contains("OBSERVATION SEULE"));
        assert!(portee.contains("ne consulte jamais"));
    }
}
