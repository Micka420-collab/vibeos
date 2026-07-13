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
    audit_path: PathBuf,
    scratch: PathBuf,
}

impl Server {
    /// Spawn `handle_connection` on one end of a socketpair, exactly as
    /// main.rs does after accept(): shipped policy, JSONL audit log, caller
    /// identity captured at accept time.
    fn start(tag: &str) -> Self {
        let scratch =
            std::env::temp_dir().join(format!("vibed-mcp-e2e-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).expect("create scratch dir");
        let audit_path = scratch.join("audit.jsonl");

        let policy =
            Arc::new(PolicyEngine::load_dir(&repo_policy_dir()).expect("shipped policy must load"));
        let audit = Arc::new(AuditLog::new(audit_path.clone()));
        let caller = Caller {
            uid: Some(TEST_UID),
            gid: Some(TEST_UID),
            pid: Some(1),
        };

        let (client, server) = UnixStream::pair().expect("unix socketpair");
        tokio::spawn(mcp::handle_connection(server, policy, audit, caller));

        let (read_half, write_half) = client.into_split();
        Self {
            reader: BufReader::new(read_half),
            writer: write_half,
            audit_path,
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

    /// Parsed audit records written so far. handle_connection audits before
    /// responding, so once a response was read the records are on disk.
    fn audit_records(&self) -> Vec<Value> {
        let Ok(content) = std::fs::read_to_string(&self.audit_path) else {
            return Vec::new();
        };
        content
            .lines()
            .map(|l| serde_json::from_str(l).expect("audit line is valid JSON"))
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
        "fs.write",
        "pkg.install",
        "svc.restart",
        "svc.status",
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
