// vibeos-claude-acp — VibeOS governance extension for the Claude Code ACP
// adapter (Zed). docs/DECISIONS.md ADR-014, couche 2.
//
// It does ONE surgical thing: patch `ClaudeAcpAgent.prototype.canUseTool` so the
// permission decision for a governed `vibeos:*` tool comes from vibed's policy
// engine instead of an editor prompt — a policy Allow (T0/T1) runs WITHOUT a
// prompt, everything else (deny / require_approval / unknown / failure) falls
// through to the adapter's normal human prompt. The T2/T3 approval floor is
// never skipped, and this path NEVER touches vibed's approval store.
//
// Why prototype-patch and not a subclass: `createSession` is private upstream
// and `runAcp()` constructs the base class internally with non-exported wiring,
// so a subclass cannot be injected. Patching the one public method uses only the
// exported surface (`ClaudeAcpAgent`, `runAcp`) and rebases by a dependency bump.
//
// Disabling the native Read/Write/Edit tools (couche 1) is done by CONFIG
// (`permissions.deny` under a Zed-scoped `CLAUDE_CONFIG_DIR`), not here — see the
// README and os/rootfs/etc/skel/.config/vibeos/zed-claude/.

import { ClaudeAcpAgent, runAcp } from "@agentclientprotocol/claude-agent-acp";
import { checkPolicy, DEFAULT_SOCKET } from "./policy-client.js";
import { applyGovernanceGate, type GateableAgent } from "./patch.js";

const SOCKET = process.env.VIBED_MCP_SOCKET ?? DEFAULT_SOCKET;

applyGovernanceGate(ClaudeAcpAgent as unknown as GateableAgent, (tool, target) =>
  checkPolicy(SOCKET, tool, target),
);

runAcp();
