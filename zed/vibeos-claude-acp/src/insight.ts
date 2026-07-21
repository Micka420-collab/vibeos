// insight.ts — layer 3 of the Zed initiative (§9bis): pure mappers turning
// vibed's observability payloads into display-ready shapes for the editor —
// the agent's reasoning (`agent.thinking`), the policy verdict for a tool
// call (`policy.check`), and the session journal (`agent.activity`).
//
// Pure logic, no I/O, mirror of gate.ts's discipline: every mapper is
// FAIL-SAFE on an absent/malformed payload (it returns an empty/inactive
// shape, never throws — a broken socket must degrade the display, never the
// editor), and every free-text excerpt is BOUNDED before it reaches the UI.
// The final rendering inside Zed is validated on-machine (Tier B,
// machine-gated); these mappers and their tests are the part that can be
// proven here.

/** Upper bound on any excerpt shown inline in the editor. */
export const MAX_EXCERPT_CHARS = 280;
/** Upper bound on journal rows surfaced to the UI. */
export const MAX_JOURNAL_ROWS = 20;

/** One display-ready reasoning entry (from `agent.thinking`'s `lines`). */
export interface ThinkingEntry {
  /** Unix seconds of the block (0 when absent — sorts first, shows as such). */
  ts: number;
  /** Bounded, single-line excerpt of the block's text content. */
  excerpt: string;
  /** True when the excerpt was cut at MAX_EXCERPT_CHARS. */
  truncated: boolean;
}

const asRecord = (v: unknown): Record<string, unknown> | null =>
  v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null;

/** Bound + flatten a string for inline display (newlines become spaces). */
function excerptOf(text: string): { excerpt: string; truncated: boolean } {
  const flat = text.replace(/\s+/g, " ").trim();
  if (flat.length <= MAX_EXCERPT_CHARS) {
    return { excerpt: flat, truncated: false };
  }
  return { excerpt: flat.slice(0, MAX_EXCERPT_CHARS), truncated: true };
}

/**
 * Map an `agent.thinking` result to display-ready reasoning entries.
 * A reasoning block is arbitrary JSON (the stream-json tap stores it
 * verbatim); the text lives in `delta`/`text`/`thinking` when present —
 * otherwise the block is summarized by its JSON, still bounded.
 */
export function mapThinking(raw: unknown): ThinkingEntry[] {
  const payload = asRecord(raw);
  const lines = payload?.lines;
  if (!Array.isArray(lines)) {
    return [];
  }
  const entries: ThinkingEntry[] = [];
  for (const line of lines) {
    const rec = asRecord(line);
    if (!rec) {
      continue;
    }
    const ts = typeof rec.ts_unix === "number" && Number.isFinite(rec.ts_unix) ? rec.ts_unix : 0;
    const block = asRecord(rec.block);
    const text =
      typeof block?.delta === "string"
        ? block.delta
        : typeof block?.text === "string"
          ? block.text
          : typeof block?.thinking === "string"
            ? block.thinking
            : block
              ? JSON.stringify(block)
              : "";
    if (!text) {
      continue;
    }
    entries.push({ ts, ...excerptOf(text) });
  }
  return entries;
}

/** Display-ready verdict for one governed tool call (from `policy.check`). */
export interface TierVerdict {
  /** "T0".."T3", or null when vibed did not classify (unknown tool, deny). */
  tier: string | null;
  /** vibed's decision verbatim; "unknown" on a malformed payload. */
  verdict: "allow" | "deny" | "require_approval" | "unknown";
  /** True exactly for require_approval — the human gate indicator. */
  requiresApproval: boolean;
  /** Short label for a status bar, e.g. "T2 · approval required". */
  label: string;
}

/**
 * Map a `policy.check` result to a status-bar verdict. Fail-safe: anything
 * malformed yields verdict "unknown" (displayed, never hidden) — the UI hint
 * must never look more permissive than vibed's actual enforcement.
 */
export function mapTierVerdict(raw: unknown): TierVerdict {
  const payload = asRecord(raw);
  const decision = payload?.decision;
  const tier = typeof payload?.tier === "string" ? payload.tier : null;
  const verdict =
    decision === "allow" || decision === "deny" || decision === "require_approval"
      ? decision
      : ("unknown" as const);
  const wording: Record<TierVerdict["verdict"], string> = {
    allow: "allowed",
    deny: "DENIED",
    require_approval: "approval required",
    unknown: "verdict unknown",
  };
  return {
    tier,
    verdict,
    requiresApproval: verdict === "require_approval",
    label: `${tier ?? "T?"} · ${wording[verdict]}`,
  };
}

/** One display-ready journal row (from `agent.activity`'s `activity`). */
export interface JournalRow {
  ts: number;
  tool: string;
  target: string | null;
  outcome: string | null;
  /** True for a refused call — surfaced first, never buried. */
  refused: boolean;
}

/** Display-ready summary of the session journal. */
export interface SessionJournal {
  /** Rows newest-first (vibed's order), refusals first, capped. */
  rows: JournalRow[];
  /** Outcome -> count over ALL received rows (not just the displayed cap). */
  counts: Record<string, number>;
  /** Refusal count over all received rows. */
  refusals: number;
  /** True when vibed truncated, or when the cap here dropped rows. */
  truncated: boolean;
}

const EMPTY_JOURNAL: SessionJournal = { rows: [], counts: {}, refusals: 0, truncated: false };

/**
 * Map an `agent.activity` result to a session-journal summary. Refusals
 * (outcome starting with "refused"/"denied") are counted and pulled ahead of
 * the cap — a journal that silently hides its refusals would defeat the point.
 */
export function mapSessionJournal(raw: unknown): SessionJournal {
  const payload = asRecord(raw);
  const activity = payload?.activity;
  if (!Array.isArray(activity)) {
    return EMPTY_JOURNAL;
  }
  const rows: JournalRow[] = [];
  const counts: Record<string, number> = {};
  let refusals = 0;
  for (const item of activity) {
    const rec = asRecord(item);
    if (!rec || typeof rec.tool !== "string" || rec.tool === "") {
      continue;
    }
    const outcome = typeof rec.outcome === "string" ? rec.outcome : null;
    const refused = outcome !== null && /^(refused|denied)/.test(outcome);
    if (outcome !== null) {
      counts[outcome] = (counts[outcome] ?? 0) + 1;
    }
    if (refused) {
      refusals += 1;
    }
    rows.push({
      ts: typeof rec.ts_unix === "number" && Number.isFinite(rec.ts_unix) ? rec.ts_unix : 0,
      tool: rec.tool,
      target: typeof rec.target === "string" ? rec.target : null,
      outcome,
      refused,
    });
  }
  // Stable partition: refusals first, both halves keeping vibed's
  // newest-first order, then cap.
  const ordered = [...rows.filter((r) => r.refused), ...rows.filter((r) => !r.refused)];
  const capped = ordered.slice(0, MAX_JOURNAL_ROWS);
  return {
    rows: capped,
    counts,
    refusals,
    truncated: payload?.truncated === true || capped.length < ordered.length,
  };
}
