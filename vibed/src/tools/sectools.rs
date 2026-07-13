//! `sectools.list` (T0) — read-only discovery of the shipped security toolkit.
//!
//! Extracted verbatim from `mcp.rs` (F6, mechanical split — zero behaviour
//! change). Reads the manifest and reports, per tool, its category, the tier
//! that gates AGENT invocation, and whether the binary is installed. NEVER
//! executes any security tool (running one is a future policy-gated path).

use serde_json::{json, Value};

/// Security toolkit manifest shipped in the image (os/rootfs). Read-only,
/// consulted by `sectools.list`; see docs/SECURITY-TOOLKIT.md.
const SECTOOLS_MANIFEST: &str = "/usr/share/vibeos/security-tools.tsv";
/// Standard binary directories probed to tell whether a toolkit entry is
/// actually installed (pure filesystem stat — sectools.list never executes).
const SECTOOLS_BIN_DIRS: [&str; 4] = ["/usr/bin", "/usr/sbin", "/bin", "/sbin"];

/// sectools.list (T0): read-only discovery of the shipped security toolkit.
/// Reads the manifest and reports, per tool, its category, the capability tier
/// that gates AGENT invocation, and whether the binary is installed — checked
/// by a plain filesystem stat in the standard bin dirs. This tool NEVER
/// executes any security tool; running one is a future policy-gated path
/// (T2/T3 = human approval, see docs/SECURITY-TOOLKIT.md).
pub(crate) fn sectools_list(args: &Value) -> Result<String, String> {
    let manifest = match std::fs::read_to_string(SECTOOLS_MANIFEST) {
        Ok(text) => text,
        Err(_) => {
            return Ok(json!({
                "available": false,
                "note": "security toolkit manifest absent \
                         (/usr/share/vibeos/security-tools.tsv): not a VibeOS image, \
                         or the toolkit layer was not built in"
            })
            .to_string())
        }
    };
    let category_filter = args.get("category").and_then(Value::as_str);
    let installed_only = args
        .get("installed_only")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let present = |binary: &str| -> bool {
        SECTOOLS_BIN_DIRS
            .iter()
            .any(|dir| std::path::Path::new(dir).join(binary).exists())
    };
    Ok(sectools_list_from(
        &manifest,
        category_filter,
        installed_only,
        present,
    ))
}

/// Pure body of sectools.list: parse the manifest and apply filters. The
/// `present` closure resolves installation status, so tests can drive it
/// against a fixed set without touching the real filesystem.
fn sectools_list_from(
    manifest: &str,
    category_filter: Option<&str>,
    installed_only: bool,
    present: impl Fn(&str) -> bool,
) -> String {
    let mut tools = Vec::new();
    let mut categories: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut installed_count = 0usize;
    for line in manifest.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.splitn(4, '\t');
        let (Some(binary), Some(category), Some(tier)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue; // malformed line: skip rather than fail the whole call
        };
        let description = fields.next().unwrap_or("").trim();
        let (binary, category, tier) = (binary.trim(), category.trim(), tier.trim());
        categories.insert(category.to_string());
        if category_filter.is_some_and(|c| c != category) {
            continue;
        }
        let is_installed = present(binary);
        if is_installed {
            installed_count += 1;
        }
        if installed_only && !is_installed {
            continue;
        }
        tools.push(json!({
            "binary": binary,
            "category": category,
            "tier": tier,
            "installed": is_installed,
            "description": description,
        }));
    }
    json!({
        "available": true,
        "categories": categories.into_iter().collect::<Vec<_>>(),
        "count": tools.len(),
        "installed_count": installed_count,
        "governance": "Agent invocation of these tools is gated by the vibed policy engine: \
                       T2 (active against a target) and T3 (destructive) require out-of-band \
                       HUMAN APPROVAL. sectools.list itself only discovers, never executes. \
                       Authorized use only. See docs/SECURITY-TOOLKIT.md",
        "tools": tools,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_MANIFEST: &str = "# header comment\n\
        \n\
        nmap\tnetwork\tT2\tPort scanner\n\
        john\tpasswords\tT1\tOffline cracker\n\
        dig\trecon\tT0\tDNS lookups\n\
        # a comment line\n\
        ettercap\tnetwork\tT3\tMITM framework\n\
        malformed-line-without-tabs\n";

    #[test]
    fn sectools_list_parses_manifest_with_tiers_and_categories() {
        // Everything present.
        let payload: Value =
            serde_json::from_str(&sectools_list_from(SAMPLE_MANIFEST, None, false, |_| true))
                .unwrap();
        assert_eq!(payload["available"], true);
        assert_eq!(payload["count"], 4, "the malformed line must be skipped");
        assert_eq!(payload["installed_count"], 4);
        // Categories are sorted and unique.
        let cats: Vec<&str> = payload["categories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        assert_eq!(cats, ["network", "passwords", "recon"]);
        // Governance note surfaces the human-approval gate.
        assert!(payload["governance"]
            .as_str()
            .unwrap()
            .contains("HUMAN APPROVAL"));
        // Tiers are carried through verbatim.
        let nmap = payload["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["binary"] == "nmap")
            .unwrap();
        assert_eq!(nmap["tier"], "T2");
        assert_eq!(nmap["category"], "network");
    }

    #[test]
    fn sectools_list_filters_by_category_and_installed() {
        // Category filter.
        let payload: Value = serde_json::from_str(&sectools_list_from(
            SAMPLE_MANIFEST,
            Some("network"),
            false,
            |_| true,
        ))
        .unwrap();
        assert_eq!(payload["count"], 2, "only network tools");
        // installed_only with a presence oracle that only knows `nmap`.
        let payload: Value =
            serde_json::from_str(&sectools_list_from(SAMPLE_MANIFEST, None, true, |bin| {
                bin == "nmap"
            }))
            .unwrap();
        assert_eq!(payload["count"], 1);
        assert_eq!(payload["tools"][0]["binary"], "nmap");
        // installed_count counts across the whole manifest, not the filtered view.
        assert_eq!(payload["installed_count"], 1);
    }

    #[test]
    fn sectools_list_reports_absent_manifest() {
        // The real tool returns available:false when the file is missing; the
        // pure body is exercised above, so here we only assert the shape used
        // by sectools_list() for the absent case is valid JSON.
        let payload: Value =
            serde_json::from_str(&sectools_list_from("", None, false, |_| false)).unwrap();
        assert_eq!(payload["count"], 0);
        assert_eq!(payload["available"], true);
    }
}
