import { describe, it, expect } from "vitest";
import { shouldAutoAllow } from "../src/gate.js";
import { mapToVibedCall } from "../src/tool-map.js";

describe("shouldAutoAllow — the governed auto-mode (ADR-014)", () => {
  it("auto-allows only a policy 'allow' (T0/T1)", () => {
    expect(shouldAutoAllow("allow")).toBe(true);
  });

  it("NEVER auto-allows require_approval (the T2/T3 floor is never skipped)", () => {
    expect(shouldAutoAllow("require_approval")).toBe(false);
  });

  it("never auto-allows a deny", () => {
    expect(shouldAutoAllow("deny")).toBe(false);
  });

  it("is fail-safe: a null/undefined decision (failed/absent check) prompts", () => {
    expect(shouldAutoAllow(null)).toBe(false);
    expect(shouldAutoAllow(undefined)).toBe(false);
  });
});

describe("mapToVibedCall — govern only vibeos:* tools", () => {
  it("maps a governed fs.read call with its path target", () => {
    expect(mapToVibedCall("mcp__vibeos__fs.read", { path: "/home/dev/x" })).toEqual({
      tool: "fs.read",
      target: "/home/dev/x",
    });
  });

  it("maps svc.status by unit and pkg.install by name", () => {
    expect(mapToVibedCall("mcp__vibeos__svc.status", { unit: "sshd.service" })?.target).toBe(
      "sshd.service",
    );
    expect(mapToVibedCall("mcp__vibeos__pkg.install", { name: "htop" })?.target).toBe("htop");
  });

  it("returns a tool with no target when none applies", () => {
    expect(mapToVibedCall("mcp__vibeos__os.status", {})).toEqual({
      tool: "os.status",
      target: undefined,
    });
  });

  it("does NOT govern native or non-vibeos tools (they fall through to prompt)", () => {
    expect(mapToVibedCall("Read", { file_path: "/x" })).toBeNull();
    expect(mapToVibedCall("mcp__other__foo", {})).toBeNull();
    expect(mapToVibedCall(42, {})).toBeNull();
    expect(mapToVibedCall("mcp__vibeos__fs.read", null)).toEqual({
      tool: "fs.read",
      target: undefined,
    });
  });
});
