import { describe, it, expect } from "vitest";
import {
  MAX_EXCERPT_CHARS,
  MAX_JOURNAL_ROWS,
  mapThinking,
  mapTierVerdict,
  mapSessionJournal,
} from "../src/insight.js";

describe("mapThinking — reasoning entries for the editor (layer 3)", () => {
  it("maps agent.thinking lines to bounded, flattened excerpts", () => {
    const entries = mapThinking({
      session_id: "s",
      lines: [
        { ts_unix: 100, session_id: "s", block: { delta: "first\n  thought" } },
        { ts_unix: 200, session_id: "s", block: { text: "second" } },
        { ts_unix: 300, session_id: "s", block: { thinking: "third" } },
      ],
      count: 3,
      truncated: false,
    });
    expect(entries).toHaveLength(3);
    expect(entries[0]).toEqual({ ts: 100, excerpt: "first thought", truncated: false });
    expect(entries[1].excerpt).toBe("second");
    expect(entries[2].excerpt).toBe("third");
  });

  it("bounds an oversized block at MAX_EXCERPT_CHARS and flags it", () => {
    const [entry] = mapThinking({
      lines: [{ ts_unix: 1, block: { delta: "x".repeat(MAX_EXCERPT_CHARS * 3) } }],
    });
    expect(entry.excerpt).toHaveLength(MAX_EXCERPT_CHARS);
    expect(entry.truncated).toBe(true);
  });

  it("summarizes a textless block by its JSON, still bounded", () => {
    const [entry] = mapThinking({ lines: [{ ts_unix: 1, block: { kind: "tool_use", n: 4 } }] });
    expect(entry.excerpt).toContain("tool_use");
    expect(entry.excerpt.length).toBeLessThanOrEqual(MAX_EXCERPT_CHARS);
  });

  it("is fail-safe: absent/malformed payloads yield an empty list, never a throw", () => {
    expect(mapThinking(null)).toEqual([]);
    expect(mapThinking(undefined)).toEqual([]);
    expect(mapThinking("garbage")).toEqual([]);
    expect(mapThinking({ lines: "not an array" })).toEqual([]);
    expect(mapThinking({ lines: [null, 42, { block: null }, { ts_unix: "x" }] })).toEqual([]);
  });
});

describe("mapTierVerdict — the tier indicator (layer 3)", () => {
  it("maps the three vibed decisions with their tier", () => {
    expect(mapTierVerdict({ tool: "fs.read", decision: "allow", tier: "T0" })).toEqual({
      tier: "T0",
      verdict: "allow",
      requiresApproval: false,
      label: "T0 · allowed",
    });
    expect(mapTierVerdict({ decision: "require_approval", tier: "T2" })).toMatchObject({
      requiresApproval: true,
      label: "T2 · approval required",
    });
    expect(mapTierVerdict({ decision: "deny", tier: null })).toMatchObject({
      verdict: "deny",
      label: "T? · DENIED",
    });
  });

  it("is fail-safe: a malformed payload reads 'unknown', never more permissive", () => {
    for (const bad of [null, undefined, 7, "x", {}, { decision: "sure!" }]) {
      const v = mapTierVerdict(bad);
      expect(v.verdict).toBe("unknown");
      expect(v.requiresApproval).toBe(false);
      expect(v.label).toContain("unknown");
    }
  });
});

describe("mapSessionJournal — the session journal (layer 3)", () => {
  const row = (ts: number, tool: string, outcome: string | null, target = "/tmp/x") => ({
    ts_unix: ts,
    when: "…",
    tool,
    target,
    decision: null,
    outcome,
    pid: 1,
  });

  it("summarizes counts per outcome and surfaces refusals first", () => {
    const journal = mapSessionJournal({
      activity: [row(3, "fs.read", "ok"), row(2, "fs.write", "refused_policy"), row(1, "os.status", "ok")],
      truncated: false,
    });
    expect(journal.counts).toEqual({ ok: 2, refused_policy: 1 });
    expect(journal.refusals).toBe(1);
    expect(journal.rows[0]).toMatchObject({ tool: "fs.write", refused: true });
    expect(journal.rows).toHaveLength(3);
    expect(journal.truncated).toBe(false);
  });

  it("caps displayed rows at MAX_JOURNAL_ROWS without losing the counts", () => {
    const many = Array.from({ length: MAX_JOURNAL_ROWS + 5 }, (_, i) => row(i, "fs.read", "ok"));
    const journal = mapSessionJournal({ activity: many });
    expect(journal.rows).toHaveLength(MAX_JOURNAL_ROWS);
    expect(journal.counts.ok).toBe(MAX_JOURNAL_ROWS + 5);
    expect(journal.truncated).toBe(true);
  });

  it("honours vibed's own truncated flag", () => {
    const journal = mapSessionJournal({ activity: [row(1, "fs.read", "ok")], truncated: true });
    expect(journal.truncated).toBe(true);
  });

  it("is fail-safe: absent/malformed payloads yield the empty journal", () => {
    for (const bad of [null, undefined, [], "x", { activity: "nope" }]) {
      expect(mapSessionJournal(bad)).toEqual({ rows: [], counts: {}, refusals: 0, truncated: false });
    }
    // Malformed items inside a valid array are skipped, not fatal.
    const journal = mapSessionJournal({ activity: [null, 42, { tool: "" }, row(1, "fs.read", null)] });
    expect(journal.rows).toHaveLength(1);
    expect(journal.rows[0].outcome).toBeNull();
  });
});
