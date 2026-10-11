#!/usr/bin/env bash
# Nexus remote bootstrap: installs tmux, Claude Code, the Antigravity CLI (agy) and OpenClaw on this Linux machine.
# Safe to run repeatedly - anything already installed is skipped.
set -u

step() { printf '\n\033[1;35m▸ %s\033[0m\n' "$*"; }
ok()   { printf '  \033[32m✓ %s\033[0m\n' "$*"; }
warn() { printf '  \033[33m! %s\033[0m\n' "$*"; }
have() { command -v "$1" >/dev/null 2>&1; }

SUDO=""
[ "$(id -u)" -ne 0 ] && SUDO="sudo"

pkg_install() {
  if have apt-get; then $SUDO apt-get update -qq && $SUDO apt-get install -y "$@"
  elif have dnf; then $SUDO dnf install -y "$@"
  elif have yum; then $SUDO yum install -y "$@"
  elif have pacman; then $SUDO pacman -S --noconfirm --needed "$@"
  elif have zypper; then $SUDO zypper install -y "$@"
  elif have apk; then $SUDO apk add "$@"
  else warn "No known package manager - install $* manually"; return 1
  fi
}

add_path_line() {
  for rc in "$HOME/.profile" "$HOME/.bashrc"; do
    [ -f "$rc" ] || touch "$rc"
    grep -qxF "$1" "$rc" || printf '\n%s\n' "$1" >> "$rc"
  done
}

printf '\033[1;36mNexus bootstrap on %s\033[0m\n' "$(hostname)"

step "Base tools (tmux, curl, git)"
missing=""
for t in tmux curl git; do have "$t" || missing="$missing $t"; done
if [ -n "$missing" ]; then pkg_install $missing; fi
have tmux && ok "$(tmux -V)" || warn "tmux missing - sessions will not survive disconnects"

step "Claude Code"
if have claude; then
  ok "claude $(claude --version 2>/dev/null | head -n1)"
else
  curl -fsSL https://claude.ai/install.sh | bash
  export PATH="$HOME/.local/bin:$PATH"
  add_path_line 'export PATH="$HOME/.local/bin:$PATH"'
  have claude && ok "claude installed" || warn "Claude install did not finish - see output above"
fi

step "Antigravity CLI (agy)"
if have agy; then
  ok "agy $(agy --version 2>/dev/null | head -n1)"
else
  curl -fsSL https://antigravity.google/cli/install.sh | bash
  export PATH="$HOME/.local/bin:$PATH"
  add_path_line 'export PATH="$HOME/.local/bin:$PATH"'
  have agy && ok "agy installed" || warn "Antigravity install did not finish - see output above"
fi

step "OpenClaw"
if have openclaw; then
  ok "openclaw $(openclaw --version 2>/dev/null | head -n1)"
else
  # The installer brings its own Node.js if needed, then starts the onboarding wizard (model sign-in, gateway).
  curl -fsSL https://openclaw.ai/install.sh | bash
  export PATH="$HOME/.local/bin:$PATH"
  hash -r
  have openclaw && ok "openclaw installed" || warn "OpenClaw install did not finish - see output above"
fi
if have openclaw && ! openclaw gateway status >/dev/null 2>&1; then
  warn "OpenClaw is not set up yet. Run once: openclaw onboard --install-daemon"
fi

step "Summary"
for t in tmux claude agy openclaw; do
  if have "$t"; then ok "$t"; else warn "$t not found"; fi
done
printf '\n\033[1mNext:\033[0m launch Claude, Antigravity and OpenClaw from Nexus. Each asks you to sign in the first time.\n'
printf 'Close this tab with Ctrl+Shift+W, then click refresh on the machine card.\n\n'
