// tool-map.ts — map an ACP/SDK tool call to a (vibed tool, target) pair for
// `vibeos:policy.check`. Pure, unit-tested.
//
// VibeOS routes the agent's file/system operations through vibed's MCP server
// (native Read/Write/Edit are disabled by config — ADR-014 couche 1). vibed's
// tools surface to the model as `mcp__vibeos__<tool>`. We only govern those; any
// other tool falls through to the adapter's normal permission flow.

export interface VibedCall {
  /** The bare vibed tool name, e.g. `fs.read`. */
  tool: string;
  /** The non-secret subject (path / unit / package), if any. */
  target?: string;
}

/**
 * Extract the vibed tool + target from an SDK tool call, or `null` if the call
 * is not a governed `mcp__vibeos__*` tool. Defensive: an unexpected shape yields
 * `null` (which the caller treats as "prompt", fail-safe).
 */
export function mapToVibedCall(toolName: unknown, toolInput: unknown): VibedCall | null {
  if (typeof toolName !== "string") {
    return null;
  }
  const m = /^mcp__vibeos__(.+)$/.exec(toolName);
  if (!m) {
    return null;
  }
  const tool = m[1];
  const input = (toolInput && typeof toolInput === "object" ? toolInput : {}) as Record<
    string,
    unknown
  >;
  const pick = (k: string): string | undefined =>
    typeof input[k] === "string" ? (input[k] as string) : undefined;
  // Mirror the audit target vibed derives: path for fs.*, unit for svc.*,
  // package name for pkg.install.
  const target = pick("path") ?? pick("unit") ?? pick("name");
  return { tool, target };
}
