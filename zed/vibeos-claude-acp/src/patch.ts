// patch.ts — apply the VibeOS governance gate to the adapter by wrapping
// `canUseTool` (ADR-014, couche 2). Extracted from the entry point so the gate
// logic is unit-tested against a fake agent, deterministically, with no live
// upstream, socket, or LLM.

import type { PolicyDecision } from "./gate.js";
import { shouldAutoAllow } from "./gate.js";
import { mapToVibedCall } from "./tool-map.js";

/** vibed's classification for a `(tool, target)` — returns null on any failure. */
export type PolicyCheck = (tool: string, target: string | undefined) => Promise<PolicyDecision | null>;

type PermissionCallback = (
  toolName: unknown,
  toolInput: unknown,
  options: unknown,
) => Promise<unknown>;

/** The minimal shape of `ClaudeAcpAgent` we patch: a class with a prototype
 *  method `canUseTool(sessionId) => callback`. */
export interface GateableAgent {
  prototype: { canUseTool: (this: unknown, sessionId: string) => PermissionCallback };
}

/**
 * Patch `AgentClass.prototype.canUseTool` so a governed `vibeos:*` tool call the
 * policy engine already permits (T0/T1 -> `allow`) runs WITHOUT an editor prompt,
 * while everything else — `deny`, `require_approval` (T2/T3), an unknown/failed
 * check, or a non-vibeos tool — falls through to the adapter's normal prompt.
 *
 * Deterministic and LLM-free: the decision comes ONLY from `check` (vibed's
 * `policy.check`), never a model classifier. The T2/T3 approval floor is never
 * skipped, and a failed check is fail-safe (prompt). This never touches vibed's
 * approval store — it only READS the classification (ADR-014).
 */
export function applyGovernanceGate(AgentClass: GateableAgent, check: PolicyCheck): void {
  const original = AgentClass.prototype.canUseTool;
  AgentClass.prototype.canUseTool = function (this: unknown, sessionId: string) {
    const base = original.call(this, sessionId);
    return async (toolName: unknown, toolInput: unknown, options: unknown) => {
      const call = mapToVibedCall(toolName, toolInput);
      if (call) {
        let decision: PolicyDecision | null = null;
        try {
          decision = await check(call.tool, call.target);
        } catch {
          decision = null;
        }
        if (shouldAutoAllow(decision)) {
          return { behavior: "allow", updatedInput: toolInput };
        }
      }
      return base(toolName, toolInput, options);
    };
  };
}
