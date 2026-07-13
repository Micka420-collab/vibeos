import { describe, it, expect } from "vitest";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { checkPolicy } from "../src/policy-client.js";

/**
 * Stand up a fake vibed MCP socket that answers the `policy.check` tools/call
 * (id 2) with the given decision, so we exercise the REAL socket client
 * end-to-end (handshake framing, line parsing, result extraction).
 */
function mockVibed(decision: unknown): Promise<{ socketPath: string; close: () => void }> {
  const socketPath = path.join(
    os.tmpdir(),
    `vibed-mock-${process.pid}-${Math.floor(performance.now())}.sock`,
  );
  const server = net.createServer((conn) => {
    let buf = "";
    conn.on("data", (chunk: Buffer) => {
      buf += chunk.toString("utf8");
      let nl: number;
      while ((nl = buf.indexOf("\n")) >= 0) {
        const line = buf.slice(0, nl);
        buf = buf.slice(nl + 1);
        if (!line.trim()) continue;
        const msg = JSON.parse(line);
        if (msg.id === 2) {
          const text = typeof decision === "string" ? JSON.stringify({ decision }) : JSON.stringify(decision);
          conn.write(
            JSON.stringify({
              jsonrpc: "2.0",
              id: 2,
              result: { content: [{ type: "text", text }] },
            }) + "\n",
          );
        }
      }
    });
  });
  return new Promise((resolve) =>
    server.listen(socketPath, () => resolve({ socketPath, close: () => server.close() })),
  );
}

describe("checkPolicy — MCP client over vibed's socket", () => {
  it("parses an 'allow' decision end-to-end", async () => {
    const m = await mockVibed("allow");
    try {
      expect(await checkPolicy(m.socketPath, "fs.read", "/home/dev/x")).toBe("allow");
    } finally {
      m.close();
    }
  });

  it("parses a 'require_approval' decision (T2/T3)", async () => {
    const m = await mockVibed("require_approval");
    try {
      expect(await checkPolicy(m.socketPath, "pkg.install", "htop")).toBe("require_approval");
    } finally {
      m.close();
    }
  });

  it("is fail-safe: returns null on a missing socket", async () => {
    expect(await checkPolicy("/nonexistent/vibed.sock", "fs.read", undefined, 500)).toBeNull();
  });

  it("is fail-safe: returns null on an unexpected/garbage decision", async () => {
    const m = await mockVibed({ decision: "banana" });
    try {
      expect(await checkPolicy(m.socketPath, "fs.read", undefined)).toBeNull();
    } finally {
      m.close();
    }
  });
});
