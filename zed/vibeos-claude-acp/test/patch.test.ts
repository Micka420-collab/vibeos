import { describe, it, expect, vi } from "vitest";
import { applyGovernanceGate, type PolicyCheck, type GateableAgent } from "../src/patch.js";

/**
 * A minimal fake agent class whose `canUseTool(sid)` returns a spy callback,
 * standing in for the upstream `ClaudeAcpAgent`. Lets us assert the gate logic
 * (auto-allow vs fall-through-to-prompt) without the real adapter, a socket, or
 * a model — the whole point of extracting the gate into `patch.ts`.
 */
function makeFakeAgentClass(baseResult: unknown) {
  const baseCb = vi.fn(async () => baseResult);
  const AgentClass = {
    prototype: { canUseTool: (_sid: string) => baseCb },
  } as unknown as GateableAgent;
  return { AgentClass, baseCb };
}

describe("applyGovernanceGate — policy-driven, deterministic, LLM-free", () => {
  it("auto-allows a governed T0/T1 'allow' WITHOUT calling the editor prompt", async () => {
    const { AgentClass, baseCb } = makeFakeAgentClass({ behavior: "deny" });
    const check: PolicyCheck = async () => "allow";
    applyGovernanceGate(AgentClass, check);
    const cb = AgentClass.prototype.canUseTool.call({}, "s");
    const res = await cb("mcp__vibeos__fs.read", { path: "/x" }, {});
    expect(res).toEqual({ behavior: "allow", updatedInput: { path: "/x" } });
    expect(baseCb).not.toHaveBeenCalled();
  });

  it("NEVER skips the prompt for require_approval — the T2/T3 floor holds", async () => {
    const { AgentClass, baseCb } = makeFakeAgentClass({ behavior: "deny" });
    applyGovernanceGate(AgentClass, async () => "require_approval");
    const cb = AgentClass.prototype.canUseTool.call({}, "s");
    await cb("mcp__vibeos__pkg.install", { name: "htop" }, {});
    expect(baseCb).toHaveBeenCalledOnce();
  });

  it("PROOF OF DETERMINISM: same input x20 -> identical decision, exactly one policy check each, zero LLM", async () => {
    const { AgentClass, baseCb } = makeFakeAgentClass({ behavior: "deny" });
    // The ONLY dependency in the decision path is this policy check; there is no
    // model / network / randomness anywhere in `patch.ts` or `gate.ts`.
    const check = vi.fn(async (): Promise<"allow"> => "allow");
    applyGovernanceGate(AgentClass, check);
    const cb = AgentClass.prototype.canUseTool.call({}, "s");
    const results: unknown[] = [];
    for (let i = 0; i < 20; i++) {
      results.push(await cb("mcp__vibeos__fs.read", { path: "/x" }, {}));
    }
    for (const r of results) {
      expect(r).toEqual({ behavior: "allow", updatedInput: { path: "/x" } });
    }
    // Exactly one policy call per tool call — deterministic, no hidden classifier.
    expect(check).toHaveBeenCalledTimes(20);
    // An auto-allowed call never touches the editor prompt.
    expect(baseCb).not.toHaveBeenCalled();
  });

  it("fail-safe: a THROWN policy check -> prompt (never auto-allow)", async () => {
    const { AgentClass, baseCb } = makeFakeAgentClass({ behavior: "deny" });
    applyGovernanceGate(AgentClass, async () => {
      throw new Error("vibed unreachable");
    });
    const cb = AgentClass.prototype.canUseTool.call({}, "s");
    await cb("mcp__vibeos__fs.write", { path: "/x" }, {});
    expect(baseCb).toHaveBeenCalledOnce();
  });

  it("does not govern non-vibeos tools — they go straight to the prompt", async () => {
    const { AgentClass, baseCb } = makeFakeAgentClass({ behavior: "deny" });
    const check = vi.fn(async (): Promise<"allow"> => "allow");
    applyGovernanceGate(AgentClass, check);
    const cb = AgentClass.prototype.canUseTool.call({}, "s");
    await cb("SomeNativeTool", { file_path: "/x" }, {});
    expect(check).not.toHaveBeenCalled(); // never even asked vibed
    expect(baseCb).toHaveBeenCalledOnce();
  });
});
