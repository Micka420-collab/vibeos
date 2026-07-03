# ~/.config/fish/config.fish — VibeOS default fish configuration
# Shipped by the image via /etc/skel (VibeOS v0.1).
# Docs: ~/README.md and ~/.config/vibeos/welcome.md
#
# Design rules:
#   * every tool integration is guarded behind `command -q` — a missing tool
#     must NEVER break the shell (graceful degradation);
#   * nothing here writes to /usr — the OS is immutable, everything user-level
#     lives in $HOME (npm -g prefix, mise toolchains, ...).

# ---------------------------------------------------------------------------
# Environment & PATH (all shells, interactive or not)
# ---------------------------------------------------------------------------

# User-local binaries (pipx, cargo, local scripts, ...)
fish_add_path --global ~/.local/bin

# `npm install -g` must work on an immutable OS: redirect the global prefix
# into $HOME instead of /usr. The agent CLIs shipped in the image are already
# in the system PATH; this is only for what YOU install afterwards.
set -gx NPM_CONFIG_PREFIX $HOME/.npm-global
fish_add_path --global $NPM_CONFIG_PREFIX/bin

# Preferred editor: the "VibeVim" Neovim preset
if command -q nvim
    set -gx EDITOR nvim
    set -gx VISUAL nvim
end

# mise — per-project toolchains (node, python, rust, ...) installed in $HOME,
# bootc-friendly: reproducible without ever touching /usr.
if command -q mise
    mise activate fish | source
end

# ---------------------------------------------------------------------------
# Interactive shell only
# ---------------------------------------------------------------------------
if status is-interactive

    # -- Prompt -------------------------------------------------------------
    # Starship, themed VibeOS Dark via ~/.config/starship.toml
    if command -q starship
        starship init fish | source
    end

    # -- Smart cd -----------------------------------------------------------
    # zoxide replaces `cd`: `cd <fragment>` jumps to your frequent projects
    # (`cdi` for the interactive picker). Plain `cd path` still works.
    if command -q zoxide
        zoxide init fish --cmd cd | source
    end

    # -- Shell history = audit trail -----------------------------------------
    # Atuin records every command (what the agents ran included) in a local
    # SQLite database. Sync is OFF by default (opt-in, encrypted). Ctrl+R opens
    # the search TUI; the up arrow keeps its normal behavior.
    if command -q atuin
        atuin init fish --disable-up-arrow | source
    end

    # -- Modern CLI aliases ---------------------------------------------------
    if command -q eza
        alias ls 'eza --icons=auto --git'
        alias ll 'eza -l --icons=auto --git --group-directories-first'
        alias la 'eza -la --icons=auto --git --group-directories-first'
        alias lt 'eza --tree --level=2 --icons=auto'
    end
    if command -q bat
        alias cat 'bat --paging=never'
    end

    # -- Vibecoding shortcuts -------------------------------------------------
    # `vibe` : signature Zellij session — agent CLI + lazygit + vibed audit log
    if command -q zellij
        abbr -a vibe 'zellij --layout vibe attach --create vibe'
    end
    if command -q lazygit
        abbr -a lg lazygit
    end
    if command -q yazi
        abbr -a y yazi
    end

    # -- Greeting -------------------------------------------------------------
    function fish_greeting
        # Very first terminal on this machine: show the full welcome note once.
        set -l welcome ~/.config/vibeos/welcome.md
        set -l marker ~/.local/state/vibeos/welcome-shown
        if test -r $welcome; and not test -e $marker
            mkdir -p (dirname $marker)
            touch $marker
            if command -q bat
                bat --style=plain --paging=never --language=markdown $welcome
            else
                cat $welcome
            end
            return
        end
        # Afterwards: one quiet line.
        set_color brblack
        echo 'VibeOS · « vibe » ouvre la session agent + lazygit + logs · aide : ~/.config/vibeos/welcome.md'
        set_color normal
    end

end
