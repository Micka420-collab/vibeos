-- ~/.config/nvim/init.lua — "VibeVim", the VibeOS Neovim preset (v0.1)
--
-- Minimal bootstrap: lazy.nvim (plugin manager) + LazyVim (curated base
-- distribution, Apache-2.0) + the VibeOS AI layer in lua/plugins/ai.lua.
--
-- FIRST LAUNCH: plugins are downloaded and installed automatically — this
-- needs network access ONCE. Every launch afterwards works offline
-- (local models are served by ollama, see lua/plugins/ai.lua).

-- Bootstrap lazy.nvim ---------------------------------------------------------
local lazypath = vim.fn.stdpath("data") .. "/lazy/lazy.nvim"
if not (vim.uv or vim.loop).fs_stat(lazypath) then
  local out = vim.fn.system({
    "git", "clone", "--filter=blob:none", "--branch=stable",
    "https://github.com/folke/lazy.nvim.git", lazypath,
  })
  if vim.v.shell_error ~= 0 then
    vim.api.nvim_echo({
      { "VibeVim: failed to clone lazy.nvim (network is required once, on first launch):\n", "ErrorMsg" },
      { out, "WarningMsg" },
      { "\nPress any key to exit...", "MoreMsg" },
    }, true, {})
    vim.fn.getchar()
    os.exit(1)
  end
end
vim.opt.rtp:prepend(lazypath)

-- Leader keys must be set before lazy.nvim loads ------------------------------
vim.g.mapleader = " "
vim.g.maplocalleader = "\\"

-- Plugins ----------------------------------------------------------------------
require("lazy").setup({
  spec = {
    -- LazyVim base: sane editor defaults, LSP, treesitter, telescope, ...
    { "LazyVim/LazyVim", import = "lazyvim.plugins" },
    -- VibeOS layer: lua/plugins/*.lua (AI: lua/plugins/ai.lua)
    { import = "plugins" },
  },
  defaults = { lazy = false, version = false },
  install = { colorscheme = { "catppuccin", "habamax" } },
  -- Offline-friendly: never phone home to check for plugin updates.
  checker = { enabled = false },
  change_detection = { notify = false },
})
