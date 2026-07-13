// gate.ts — the governed auto-mode decision (VibeOS, docs/DECISIONS.md ADR-014,
// couche 2). Pure logic, no I/O — the heart of the fork, fully unit-tested.

/** The classification vibed returns for a hypothetical tool call. */
export type PolicyDecision = "allow" | "deny" | "require_approval";

/**
 * Given vibed's classification of a tool call, decide whether to AUTO-ALLOW it
 * (skip the editor's permission prompt) or fall through to the normal prompt.
 *
 *   - `"allow"` (a T0/T1 call the policy engine already permits) -> auto-allow.
 *   - EVERYTHING else — `"deny"`, `"require_approval"` (T2/T3), or `null`
 *     (policy check failed/unavailable, or the tool is not governed) -> prompt.
 *
 * The T2/T3 approval floor is NEVER skipped here, and an absent/failed check is
 * fail-safe (prompt, never silently allow). This function alone can therefore
 * never let a T2/T3 call through without a human — and even if it did, vibed
 * re-enforces the decision at execution (this is only a UX hint; see ADR-014).
 */
export function shouldAutoAllow(decision: PolicyDecision | null | undefined): boolean {
  return decision === "allow";
}
