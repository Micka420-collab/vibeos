-- ~/.config/nvim/lua/plugins/ai.lua — VibeVim AI layer (VibeOS v0.1)
--
-- codecompanion.nvim (Apache-2.0) wires AI into Neovim two ways:
--   * "ollama" adapter → 100% local models, works fully OFFLINE (default);
--   * ACP adapters     → drive the agent CLIs shipped in the VibeOS image
--                        (Claude Code, Gemini CLI) straight from the editor.
--
-- Honesty note (v0.1): this talks to ollama / the agent CLIs directly.
-- Routing these calls through the vibed policy tiers (MCP socket
-- /run/vibed/mcp.sock) is a Phase 2 deliverable — not active yet.

return {
  -- Colorscheme: VibeOS Dark is a MIT fork of Catppuccin (Mocha flavour).
  { "catppuccin/nvim", name = "catppuccin", opts = { flavour = "mocha" } },
  { "LazyVim/LazyVim", opts = { colorscheme = "catppuccin" } },

  {
    "olimorris/codecompanion.nvim",
    dependencies = {
      "nvim-lua/plenary.nvim",
      "nvim-treesitter/nvim-treesitter",
    },
    cmd = { "CodeCompanion", "CodeCompanionChat", "CodeCompanionActions" },
    keys = {
      { "<leader>aa", "<cmd>CodeCompanionChat Toggle<cr>", mode = { "n", "v" }, desc = "AI chat (codecompanion)" },
      { "<leader>ai", ":CodeCompanion ", mode = { "n", "v" }, desc = "AI inline prompt" },
      { "<leader>ax", "<cmd>CodeCompanionActions<cr>", mode = { "n", "v" }, desc = "AI actions palette" },
    },
    opts = {
      strategies = {
        -- Default: local models — no API key, no network needed.
        -- To drive the shipped agent CLIs instead, set the adapter to
        -- "claude_code" or "gemini_cli" (built-in ACP adapters that use
        -- the binaries already present in the image).
        chat = { adapter = "ollama" },
        inline = { adapter = "ollama" },
      },
      adapters = {
        ollama = function()
          return require("codecompanion.adapters").extend("ollama", {
            schema = {
              -- Pull the model once (network): `ollama pull qwen2.5-coder:7b`
              -- Sized for ~8 GB VRAM; swap for any model you have pulled.
              model = { default = "qwen2.5-coder:7b" },
              num_ctx = { default = 16384 },
            },
          })
        end,
        -- ACP adapters ("claude_code", "gemini_cli") ship with codecompanion;
        -- no extra configuration is required when the CLIs are in PATH.
      },
    },
  },
}
