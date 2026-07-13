//! End-to-end MCP integration tests — Phase 2 exit criterion (ROADMAP.md §4:
//! "tests d'intégration MCP (handshake, appels d'outils, refus de politique)
//! exécutés en CI").
//!
//! These tests drive the REAL connection handler (`mcp::handle_connection`)
//! over a REAL unix socketpair, with the REAL policy shipped in the
//! repository (`security/policy.d/default.toml`) and a scratch audit log, and
//! assert both the wire behavior (JSON-RPC 2.0 / MCP envelopes) and the audit
//! trail — the same double bookkeeping vibed does in production.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;

use vibed::audit::{AuditLog, Caller};
use vibed::mcp;
use vibed::policy::PolicyEngine;

/// uid stamped on the test connection's peer credentials; asserted back from
/// the audit records.
const TEST_UID: u32 = 4242;

fn repo_policy_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR = <repo>/vibed at compile time.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("vibed/ has a repo root parent")
        .join("security")
        .join("policy.d")
}

/// A live in-process vibed serving one client connection.
struct Server {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    audit_dir: PathBuf,
    approval_dir: PathBuf,
    scratch: PathBuf,
}

impl Server {
    /// Spawn `handle_connection` on one end of a socketpair, exactly as
    /// main.rs does after accept(): shipped policy, JSONL audit log, caller
    /// identity captured at accept time.
    fn start(tag: &str) -> Self {
        // Generous limiter so the e2e handshake + a few calls never trip it.
        Self::start_with_limiter(tag, vibed::ratelimit::RateLimiter::default())
    }

    /// Like `start`, but with a caller-provided rate limiter (for the flood test).
    fn start_with_limiter(tag: &str, limiter: vibed::ratelimit::RateLimiter) -> Self {
        let scratch =
            std::env::temp_dir().join(format!("vibed-mcp-e2e-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).expect("create scratch dir");
        let audit_dir = scratch.join("audit");
        let approval_dir = scratch.join("approvals");

        let policy =
            Arc::new(PolicyEngine::load_dir(&repo_policy_dir()).expect("shipped policy must load"));
        let audit = Arc::new(AuditLog::new(audit_dir.clone()));
        let limiter = Arc::new(limiter);
        let caller = Caller {
            uid: Some(TEST_UID),
            gid: Some(TEST_UID),
            pid: Some(1),
        };

        let (client, server) = UnixStream::pair().expect("unix socketpair");
        tokio::spawn(mcp::handle_connection(
            server,
            policy,
            audit,
            limiter,
            caller,
            approval_dir.clone(),
        ));

        let (read_half, write_half) = client.into_split();
        Self {
            reader: BufReader::new(read_half),
            writer: write_half,
            audit_dir,
            approval_dir,
            scratch,
        }
    }

    /// Send one raw line and read one response line.
    async fn roundtrip_raw(&mut self, line: &str) -> Value {
        let mut out = line.to_string();
        out.push('\n');
        self.writer
            .write_all(out.as_bytes())
            .await
            .expect("client write");
        let mut resp = String::new();
        self.reader.read_line(&mut resp).await.expect("client read");
        assert!(
            !resp.is_empty(),
            "server closed the connection unexpectedly"
        );
        serde_json::from_str(&resp).expect("response line is valid JSON")
    }

    /// JSON-RPC request -> parsed response.
    async fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.roundtrip_raw(
            &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string(),
        )
        .await
    }

    /// tools/call helper returning (isError, concatenated text content).
    async fn tool_call(&mut self, id: u64, name: &str, arguments: Value) -> (bool, String) {
        let resp = self
            .request(
                id,
                "tools/call",
                json!({"name": name, "arguments": arguments}),
            )
            .await;
        assert_eq!(resp["id"], id, "response id must echo the request id");
        let result = &resp["result"];
        assert!(
            !result.is_null(),
            "tools/call must answer with a result envelope, got: {resp}"
        );
        let is_error = result["isError"].as_bool().unwrap_or(false);
        let text = result["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|b| b["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        (is_error, text)
    }

    /// Parsed audit records written so far, across the daily files in the audit
    /// directory (chronological). handle_connection audits before responding,
    /// so once a response was read the records are on disk.
    fn audit_records(&self) -> Vec<Value> {
        let Ok(read_dir) = std::fs::read_dir(&self.audit_dir) else {
            return Vec::new();
        };
        let mut files: Vec<PathBuf> = read_dir
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("vibed-") && n.ends_with(".jsonl"))
            })
            .collect();
        files.sort();
        files
            .iter()
            .filter_map(|f| std::fs::read_to_string(f).ok())
            .flat_map(|c| {
                c.lines()
                    .map(|l| serde_json::from_str(l).expect("audit line is valid JSON"))
                    .collect::<Vec<Value>>()
            })
            .collect()
    }

    fn cleanup(&self) {
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

#[tokio::test]
async fn handshake_tools_list_and_t0_call_end_to_end() {
    let mut srv = Server::start("handshake");

    // initialize: protocol revision + server identity.
    let resp = srv.request(1, "initialize", json!({})).await;
    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(resp["result"]["serverInfo"]["name"], "vibed");

    // notifications/initialized is a notification (no id): no response line
    // may be produced for it — the NEXT response must answer the ping.
    let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string();
    let resp = srv
        .roundtrip_raw(&format!(
            "{note}\n{}",
            json!({"jsonrpc": "2.0", "id": 2, "method": "ping"})
        ))
        .await;
    assert_eq!(
        resp["id"], 2,
        "the notification must not consume a response slot"
    );

    // tools/list: the full v0.2 tool surface, each with its tier annotation.
    let resp = srv.request(3, "tools/list", json!({})).await;
    let tools = resp["result"]["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for expected in [
        "os.status",
        "fs.read",
        "fs.list",
        "fs.write",
        "pkg.install",
        "svc.restart",
        "svc.status",
        "sectools.list",
        "memory.query",
        "memory.append",
    ] {
        assert!(
            names.contains(&expected),
            "tools/list must expose {expected}"
        );
    }
    for tool in tools {
        assert!(
            tool["annotations"]["vibeosTier"].is_string(),
            "every tool carries its tier annotation: {tool}"
        );
        assert!(
            tool["inputSchema"]["type"] == "object",
            "every tool ships a JSON Schema: {tool}"
        );
    }

    // T0 call end-to-end: os.status executes and returns system JSON.
    let (is_error, text) = srv.tool_call(4, "os.status", json!({})).await;
    assert!(!is_error, "os.status (T0) must execute: {text}");
    let payload: Value = serde_json::from_str(&text).expect("os.status returns JSON");
    assert!(
        payload.get("uptime_seconds").is_some(),
        "os.status payload must carry uptime_seconds"
    );

    // Audit: the allowed call produced started + ok records with the caller
    // identity captured at accept time.
    let records = srv.audit_records();
    let os_status: Vec<&Value> = records
        .iter()
        .filter(|r| r["tool"] == "os.status")
        .collect();
    assert_eq!(os_status.len(), 2, "one 'started' + one 'ok' record");
    assert_eq!(os_status[0]["decision"], "allow");
    assert_eq!(os_status[0]["outcome"], "started");
    assert_eq!(os_status[1]["outcome"], "ok");
    assert_eq!(os_status[0]["caller_uid"], TEST_UID);

    srv.cleanup();
}

#[tokio::test]
async fn t2_refusal_and_builtin_denylist_are_explicit_and_audited() {
    let mut srv = Server::start("refusals");

    // T2 (pkg.install): the tier floor makes approval mandatory — explicit
    // refusal on the wire, pending_approval in the audit trail.
    let (is_error, text) = srv
        .tool_call(1, "pkg.install", json!({"name": "htop"}))
        .await;
    assert!(is_error, "a T2 call without approval must be refused");
    assert!(
        text.contains("requires human approval"),
        "the refusal must say why: {text}"
    );

    // Built-in denylist: /etc/shadow is refused in code, before any policy.
    let (is_error, text) = srv
        .tool_call(2, "fs.read", json!({"path": "/etc/shadow"}))
        .await;
    assert!(is_error, "reading /etc/shadow must be refused");
    assert!(
        text.contains("built-in denylist"),
        "the refusal must name the denylist: {text}"
    );

    // Unknown tool: absolute default-deny.
    let (is_error, text) = srv.tool_call(3, "disk.wipe", json!({})).await;
    assert!(is_error, "an unknown tool must be denied");
    assert!(text.contains("denied"), "unexpected refusal text: {text}");

    // Audit trail tells the full story, with the caller identity on each line.
    let records = srv.audit_records();
    let by_tool =
        |name: &str| -> Vec<&Value> { records.iter().filter(|r| r["tool"] == name).collect() };
    let pkg = by_tool("pkg.install");
    assert_eq!(pkg.len(), 1);
    assert_eq!(pkg[0]["decision"], "require_approval");
    assert_eq!(pkg[0]["outcome"], "pending_approval");
    assert_eq!(pkg[0]["target"], "htop");
    let shadow = by_tool("fs.read");
    assert_eq!(shadow.len(), 1);
    assert_eq!(shadow[0]["decision"], "deny");
    assert_eq!(shadow[0]["outcome"], "blocked_builtin_denylist");
    assert_eq!(shadow[0]["target"], "/etc/shadow");
    let wipe = by_tool("disk.wipe");
    assert_eq!(wipe.len(), 1);
    assert_eq!(wipe[0]["decision"], "deny");
    assert_eq!(wipe[0]["outcome"], "blocked");
    for record in &records {
        assert_eq!(record["caller_uid"], TEST_UID);
    }

    srv.cleanup();
}

/// Regression for the deny-list bypass: a BARE critical unit name (no
/// `.service` suffix) must be DENIED outright by the shipped policy, not merely
/// queued for approval. The dispatcher canonicalizes the unit (`sshd` ->
/// `sshd.service`) BEFORE the policy decision, so the fully-qualified deny-list
/// matches it. Without that, an agent evaded the deny-list by dropping the
/// suffix and could then have `vibed`/`sshd` restarted via an approval that
/// looked routine. Exercised over the REAL socket (the canonicalization lives
/// in handle_tools_call, not in policy.evaluate).
#[tokio::test]
async fn svc_restart_bare_critical_unit_is_denied_not_merely_pending() {
    let mut srv = Server::start("svc-restart-bare-deny");

    for bare in ["vibed", "sshd", "polkit"] {
        let (is_error, text) = srv
            .tool_call(1, "svc.restart", json!({ "unit": bare }))
            .await;
        assert!(is_error, "restart of bare '{bare}' must be refused");
        assert!(
            text.contains("denied"),
            "bare '{bare}' must be DENIED outright, not queued for approval; got: {text}"
        );
        assert!(
            !text.contains("requires human approval"),
            "bare '{bare}' must NOT reach the approval queue (deny-list bypass): {text}"
        );
    }

    // The audit trail confirms the decision was `deny` (not require_approval),
    // and the canonicalized unit is the recorded target.
    let records = srv.audit_records();
    let svc: Vec<&Value> = records
        .iter()
        .filter(|r| r["tool"] == "svc.restart")
        .collect();
    assert_eq!(svc.len(), 3, "three svc.restart attempts audited");
    for r in &svc {
        assert_eq!(
            r["decision"], "deny",
            "must be denied, not require_approval: {r}"
        );
    }
    assert!(
        svc.iter().any(|r| r["target"] == "vibed.service"),
        "the canonicalized unit (vibed.service) must be the audit target: {svc:?}"
    );

    srv.cleanup();
}

/// Full T2 human-approval chain over the real socket: an agent's `svc.restart`
/// is refused (tier floor), the operator grants it out of band (as `vibectl
/// approve` does), the agent re-issues the SAME call, the one-shot grant flips
/// the decision to Allow, the backend runs, and the audit trail records WHO
/// approved. This is the "agent asks -> human approves -> action executes ->
/// audit proves it" demonstration, minus the real systemctl success (asserted
/// hermetically in the `svc_restart_actually_invokes_systemctl` unit test — a
/// non-root test process cannot restart a real unit).
#[tokio::test]
async fn t2_svc_restart_human_approval_chain_end_to_end() {
    let mut srv = Server::start("svc-restart-approval");
    let unit = "vibeos-approval-e2e.service";

    // 1. No grant yet: the T2 floor refuses and records a pending request.
    let (is_error, text) = srv.tool_call(1, "svc.restart", json!({"unit": unit})).await;
    assert!(is_error, "a T2 svc.restart without a grant must be refused");
    assert!(
        text.contains("requires human approval"),
        "the refusal must explain why: {text}"
    );

    // 2. Operator approves out of band — exactly what `vibectl approve <id>`
    //    does: read the one pending request and mint a one-shot grant for it.
    let pending = vibed::approval::list_pending(&srv.approval_dir);
    assert_eq!(
        pending.len(),
        1,
        "one pending approval expected: {pending:?}"
    );
    let id = pending[0]["id"].as_str().expect("pending id").to_string();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs();
    const OPERATOR_UID: u32 = 0;
    vibed::approval::approve(&srv.approval_dir, &id, Some(OPERATOR_UID), now)
        .expect("operator approve mints a grant");

    // 3. Agent re-issues the identical call. The one-shot grant is consumed and
    //    the decision flips to Allow (we do not assert on the systemctl outcome:
    //    a non-root process cannot restart a real unit — the governance chain is
    //    the claim here).
    let _ = srv.tool_call(2, "svc.restart", json!({"unit": unit})).await;

    // 4. The audit trail proves every link of the chain.
    let records = srv.audit_records();
    let svc: Vec<&Value> = records
        .iter()
        .filter(|r| r["tool"] == "svc.restart")
        .collect();

    assert!(
        svc.iter()
            .any(|r| r["decision"] == "require_approval" && r["outcome"] == "pending_approval"),
        "first call must be audited require_approval/pending_approval: {svc:?}"
    );

    let started = svc
        .iter()
        .find(|r| {
            r["decision"] == "allow"
                && r["outcome"]
                    .as_str()
                    .is_some_and(|o| o.starts_with("started_approved"))
        })
        .expect("the approved re-issue must audit an allow/started_approved record");
    assert!(
        started["outcome"].as_str().unwrap().contains("by_uid=0"),
        "the approver uid (0) must be recorded in the audit outcome: {started:?}"
    );

    for r in &svc {
        assert_eq!(r["target"], unit, "the unit is the audit target");
        assert_eq!(r["caller_uid"], TEST_UID, "the agent uid is stamped");
    }

    // One-shot: the grant was consumed, nothing lingers in the store.
    assert!(
        vibed::approval::list_pending(&srv.approval_dir).is_empty(),
        "no pending request should remain once approved and consumed"
    );

    srv.cleanup();
}

#[tokio::test]
async fn per_uid_rate_limit_refuses_a_flood_and_audits_it() {
    // Capacity 2, no refill: the third tool call in the burst is over-limit.
    let mut srv =
        Server::start_with_limiter("ratelimit", vibed::ratelimit::RateLimiter::new(2.0, 0.0));

    // The handshake methods are not tools/call, so they cost no tokens.
    let _ = srv.request(1, "initialize", json!({})).await;

    // Two T0 calls fit in the burst.
    let (e1, _) = srv.tool_call(2, "os.status", json!({})).await;
    let (e2, _) = srv.tool_call(3, "os.status", json!({})).await;
    assert!(!e1 && !e2, "the first two calls are within the burst");

    // The third is refused BY THE LIMITER — before policy/execution.
    let (e3, text3) = srv.tool_call(4, "os.status", json!({})).await;
    assert!(e3, "the third call must be rate-limited");
    assert!(
        text3.contains("rate limit exceeded"),
        "the refusal must name the rate limit: {text3}"
    );

    // The rejection is a security signal: it lands in the tamper-evident audit.
    let records = srv.audit_records();
    let limited: Vec<&Value> = records
        .iter()
        .filter(|r| r["outcome"] == "rate_limited")
        .collect();
    assert_eq!(limited.len(), 1, "exactly one rate-limited record");
    assert_eq!(limited[0]["tool"], "os.status");
    assert_eq!(limited[0]["decision"], "deny");
    assert_eq!(limited[0]["caller_uid"], TEST_UID);
    // The two allowed calls produced started+ok; the limited one did NOT execute
    // (no 'ok' for the third), so os.status has 2*2 + 1 = 5 records total.
    let os_status = records.iter().filter(|r| r["tool"] == "os.status").count();
    assert_eq!(os_status, 5, "2 allowed (started+ok) + 1 rate-limited");

    srv.cleanup();
}

#[tokio::test]
async fn policy_check_is_rate_limited_like_any_tool() {
    // Explicit confirmation (not assumption): the T0 dry-run policy.check goes
    // through the SAME per-uid rate limiter as every other tool — the limiter is
    // applied before dispatch, tool-agnostically. Capacity 1: the 2nd call is
    // refused.
    let mut srv =
        Server::start_with_limiter("rl-policy", vibed::ratelimit::RateLimiter::new(1.0, 0.0));
    let _ = srv.request(1, "initialize", json!({})).await;

    let (e1, t1) = srv
        .tool_call(2, "policy.check", json!({"tool": "os.status"}))
        .await;
    assert!(!e1, "first policy.check within the burst: {t1}");

    let (e2, t2) = srv
        .tool_call(3, "policy.check", json!({"tool": "os.status"}))
        .await;
    assert!(e2, "second policy.check must be rate-limited");
    assert!(t2.contains("rate limit exceeded"), "unexpected: {t2}");

    let limited = srv
        .audit_records()
        .into_iter()
        .filter(|r| r["outcome"] == "rate_limited" && r["tool"] == "policy.check")
        .count();
    assert_eq!(limited, 1, "the rate-limited policy.check is audited");

    srv.cleanup();
}

#[tokio::test]
async fn protocol_errors_are_json_rpc_errors() {
    let mut srv = Server::start("protocol");

    // Unknown method -> -32601 (method not found).
    let resp = srv.request(1, "resources/list", json!({})).await;
    assert_eq!(resp["error"]["code"], -32601);

    // Invalid JSON -> -32700 (parse error) with a null id.
    let resp = srv.roundtrip_raw("this is not json").await;
    assert_eq!(resp["error"]["code"], -32700);
    assert!(resp["id"].is_null());

    // tools/call without a tool name -> -32602 (invalid params).
    let resp = srv.request(2, "tools/call", json!({"arguments": {}})).await;
    assert_eq!(resp["error"]["code"], -32602);

    srv.cleanup();
}

#[tokio::test]
async fn memory_query_is_reachable_over_the_wire() {
    let mut srv = Server::start("memory");

    // The store may or may not exist on the machine running the tests; the
    // contract here is transport-level: the T0 call is ALLOWED by the shipped
    // policy and answers with a well-formed payload either way.
    let (is_error, text) = srv.tool_call(1, "memory.query", json!({"limit": 1})).await;
    assert!(!is_error, "memory.query (T0) must be allowed: {text}");
    let payload: Value = serde_json::from_str(&text).expect("memory.query returns JSON");
    assert!(
        payload.get("initialized").is_some(),
        "payload must state whether the store is initialized"
    );

    srv.cleanup();
}
