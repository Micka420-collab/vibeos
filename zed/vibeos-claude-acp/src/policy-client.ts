// policy-client.ts — a minimal line-delimited JSON-RPC 2.0 (MCP) client for
// vibed's socket. Opens a one-shot connection, initializes, calls the T0
// `policy.check` tool and returns the classification. ANY error (socket absent,
// timeout, bad response) resolves to `null`, which the gate treats as "prompt"
// (fail-safe — never auto-allow on uncertainty).

import net from "node:net";
import type { PolicyDecision } from "./gate.js";

/** Default path of vibed's MCP socket (root:vibeos-agents 0660). */
export const DEFAULT_SOCKET = "/run/vibed/mcp.sock";

/**
 * Ask vibed to classify a hypothetical `(tool, target)` call via `policy.check`.
 * Read-only on vibed's side — it never executes, approves, or touches the
 * approval store (ADR-014). Returns the decision, or `null` on any failure.
 */
export function checkPolicy(
  socketPath: string,
  tool: string,
  target: string | undefined,
  timeoutMs = 2000,
): Promise<PolicyDecision | null> {
  return new Promise((resolve) => {
    let settled = false;
    const sock = net.createConnection(socketPath);
    let buf = "";

    const finish = (value: PolicyDecision | null) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      sock.destroy();
      resolve(value);
    };
    const timer = setTimeout(() => finish(null), timeoutMs);

    sock.on("error", () => finish(null));
    sock.on("close", () => finish(null));

    sock.on("connect", () => {
      const send = (obj: unknown) => sock.write(JSON.stringify(obj) + "\n");
      send({ jsonrpc: "2.0", id: 1, method: "initialize", params: {} });
      send({ jsonrpc: "2.0", method: "notifications/initialized" });
      const args: Record<string, unknown> = { tool };
      if (target) args.target = target;
      send({
        jsonrpc: "2.0",
        id: 2,
        method: "tools/call",
        params: { name: "policy.check", arguments: args },
      });
    });

    sock.on("data", (chunk: Buffer) => {
      buf += chunk.toString("utf8");
      let nl: number;
      while ((nl = buf.indexOf("\n")) >= 0) {
        const line = buf.slice(0, nl);
        buf = buf.slice(nl + 1);
        if (!line.trim()) continue;
        let msg: any;
        try {
          msg = JSON.parse(line);
        } catch {
          continue;
        }
        if (msg && msg.id === 2) {
          // tools/call result: content[0].text carries policy.check's JSON.
          try {
            const text = msg.result?.content?.[0]?.text;
            const parsed = JSON.parse(text);
            const d = parsed?.decision;
            if (d === "allow" || d === "deny" || d === "require_approval") {
              return finish(d);
            }
          } catch {
            /* fall through to null */
          }
          return finish(null);
        }
      }
    });
  });
}
