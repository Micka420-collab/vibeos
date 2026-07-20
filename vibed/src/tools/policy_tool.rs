//! `policy.capabilities` (T0) — manifeste de capacités dérivé de la politique.
//!
//! Rend les règles chargées en un JSON que l'agent lit pour **planifier** : quels outils,
//! à quel tier, avec quel mode d'approbation et quelles contraintes de cibles. C'est la
//! moitié « efficacité » du citoyen — planifier dans la réalité au lieu de découvrir par
//! refus (idée Fable « manifeste de capacités »).
//!
//! **Dérivé, donc il ne peut pas sur-promettre** : il lit les MÊMES règles que
//! `PolicyEngine::evaluate`. Mais c'est un **INDICATIF**, pas un contrat : la décision
//! fait foi via `evaluate` à l'appel (premier-match des règles, plancher de tier,
//! prédicat `[rule.domains]`, verdict `[rule.deploy]`, contraintes de contexte). Tout ce
//! qu'aucune règle `allow` ne couvre est refusé par défaut. Le champ `reason` (notes
//! opérateur) n'est **pas** exposé.

use crate::policy::{Action, Approval, PolicyEngine, Rule, Tier};
use serde_json::{json, Value};

/// Rend le manifeste JSON (chaîne) des capacités.
pub(crate) fn capabilities(policy: &PolicyEngine) -> Result<String, String> {
    let rules: Vec<Value> = policy.rules().iter().map(render_rule).collect();
    let manifest = json!({
        "note": "Manifeste DÉRIVÉ de la politique chargée : INDICATIF pour la \
                 planification. La décision fait foi via l'évaluation à l'appel \
                 (premier-match des règles, plancher de tier, prédicat [rule.domains], \
                 verdict [rule.deploy], contraintes de contexte). Tout ce qu'aucune règle \
                 'allow' ne couvre est refusé par défaut.",
        "rule_count": rules.len(),
        "rules": rules,
    });
    Ok(manifest.to_string())
}

fn render_rule(r: &Rule) -> Value {
    // Décision de base HORS CONTEXTE : le plancher de tier transforme un allow T2/T3 +
    // approbation humaine en approbation ; un allow T0/T1 reste allow ; un deny reste
    // deny. Les contraintes et le contexte peuvent encore refuser à l'appel — d'où
    // « base_decision », pas « decision ».
    let base_decision = match r.action {
        Action::Deny => "deny",
        Action::Allow if r.tier >= Tier::T2 && r.approval == Approval::Human => "require_approval",
        Action::Allow => "allow",
    };

    let mut constraints = serde_json::Map::new();
    if let Some(p) = &r.paths {
        if let Some(a) = &p.allowed {
            constraints.insert("paths_allowed".into(), json!(a));
        }
        if !p.denied.is_empty() {
            constraints.insert("paths_denied".into(), json!(p.denied));
        }
    }
    if let Some(s) = &r.services {
        if let Some(a) = &s.allowed {
            constraints.insert("services_allowed".into(), json!(a));
        }
        if !s.denied.is_empty() {
            constraints.insert("services_denied".into(), json!(s.denied));
        }
    }
    if let Some(d) = &r.domains {
        if let Some(o) = &d.only {
            constraints.insert("domains_only".into(), json!(o));
        }
    }
    if let Some(dp) = &r.deploy {
        let pairs: Vec<Value> = dp
            .allowed
            .iter()
            .map(|a| json!({ "provider": a.provider, "target": a.target }))
            .collect();
        constraints.insert("deploy_allowed".into(), json!(pairs));
    }

    json!({
        "id": r.id,
        "tools": r.tools,
        "tier": r.tier.as_str(),
        "action": match r.action {
            Action::Allow => "allow",
            Action::Deny => "deny",
        },
        "approval": match r.approval {
            Approval::None => "none",
            Approval::Human => "human",
        },
        "base_decision": base_decision,
        "constraints": Value::Object(constraints),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::PolicyEngine;

    fn engine(toml: &str) -> PolicyEngine {
        use std::sync::atomic::{AtomicU64, Ordering};
        // Répertoire UNIQUE par appel : les tests tournent en parallèle dans un même
        // process, un dossier partagé (par PID) les ferait se clobberer (course).
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("vibed-polcap-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("t.toml"), toml).unwrap();
        let e = PolicyEngine::load_dir(&dir).expect("charge");
        let _ = std::fs::remove_dir_all(&dir);
        e
    }

    #[test]
    fn manifest_reflects_tiers_actions_and_the_tier_floor() {
        let e = engine(
            "[[rule]]\nid=\"obs\"\ntools=[\"os.status\"]\ntier=\"T0\"\naction=\"allow\"\n\
             [[rule]]\nid=\"pkg\"\ntools=[\"pkg.install\"]\ntier=\"T2\"\naction=\"allow\"\napproval=\"human\"\n\
             [[rule]]\nid=\"deny\"\ntools=[\"*\"]\ntier=\"T3\"\naction=\"deny\"\n",
        );
        let m: Value = serde_json::from_str(&capabilities(&e).unwrap()).unwrap();
        assert_eq!(m["rule_count"], 3);
        let rules = m["rules"].as_array().unwrap();
        // T0 allow → base_decision allow.
        assert_eq!(rules[0]["tier"], "T0");
        assert_eq!(rules[0]["base_decision"], "allow");
        // T2 allow + human → require_approval (plancher de tier reflété).
        assert_eq!(rules[1]["base_decision"], "require_approval");
        assert_eq!(rules[1]["approval"], "human");
        // deny → deny.
        assert_eq!(rules[2]["action"], "deny");
        assert_eq!(rules[2]["base_decision"], "deny");
    }

    #[test]
    fn manifest_surfaces_constraints_but_not_reason() {
        let e = engine(
            "[[rule]]\nid=\"logs\"\ntools=[\"log.read\"]\ntier=\"T0\"\naction=\"allow\"\n\
             reason=\"note opérateur privée\"\n[rule.services]\nallowed=[\"sshd.service\"]\n",
        );
        let m: Value = serde_json::from_str(&capabilities(&e).unwrap()).unwrap();
        let r0 = &m["rules"][0];
        assert_eq!(r0["constraints"]["services_allowed"][0], "sshd.service");
        // Le champ reason n'est jamais exposé au manifeste.
        assert!(r0.get("reason").is_none());
        assert!(!capabilities(&e).unwrap().contains("note opérateur privée"));
    }

    #[test]
    fn an_empty_policy_renders_an_empty_but_valid_manifest() {
        let e = PolicyEngine::from_rules(vec![]);
        let m: Value = serde_json::from_str(&capabilities(&e).unwrap()).unwrap();
        assert_eq!(m["rule_count"], 0);
        assert!(m["rules"].as_array().unwrap().is_empty());
        assert!(m["note"].as_str().unwrap().contains("refusé par défaut"));
    }
}
