#!/usr/bin/env bash
# Creates an SSH key on this computer (if needed) and installs it on the remote machine, so Nexus can connect
# without a password. You type the remote password once, here. (macOS / Linux counterpart of setup-ssh-key.ps1)
HOST="$1"; USER_NAME="${2:-}"; PORT="${3:-22}"
[ -z "$HOST" ] && { echo "usage: setup-ssh-key.sh HOST [USER] [PORT]"; exit 1; }
# A host or user that starts with "-" would be read by ssh as an option (e.g. -oProxyCommand=…): refuse it.
case "$HOST" in -*) echo "  ✗ Host must not start with '-'"; exit 1;; esac
case "$USER_NAME" in -*) echo "  ✗ User must not start with '-'"; exit 1;; esac
TARGET="$HOST"; [ -n "$USER_NAME" ] && TARGET="$USER_NAME@$HOST"
KEY="$HOME/.ssh/id_ed25519"

printf '\n  \033[1;35mNexus  ·  SSH key setup for %s\033[0m\n\n' "$TARGET"
mkdir -p "$HOME/.ssh" && chmod 700 "$HOME/.ssh"
if [ -f "$KEY" ]; then
  printf '  \033[32m✓ Using existing key %s\033[0m\n' "$KEY"
else
  printf '  • Creating a new ed25519 key at %s\n' "$KEY"
  ssh-keygen -t ed25519 -q -f "$KEY" -N "" -C "nexus@$(hostname)" || { echo "  ✗ ssh-keygen failed"; exit 1; }
fi

PUB="$(cat "$KEY.pub")"
printf '\n  • Copying the public key to %s - enter the remote password when asked.\n\n' "$TARGET"
ssh -p "$PORT" -o StrictHostKeyChecking=accept-new -- "$TARGET" \
  "umask 077; mkdir -p ~/.ssh && touch ~/.ssh/authorized_keys && (grep -qxF '$PUB' ~/.ssh/authorized_keys || echo '$PUB' >> ~/.ssh/authorized_keys)" \
  || { printf '\n  \033[31m✗ Could not install the key (wrong password, or sshd not running on the remote?)\033[0m\n'; exit 1; }

printf '\n  • Testing passwordless login…\n'
if ssh -p "$PORT" -i "$KEY" -o IdentitiesOnly=yes -o BatchMode=yes -o ConnectTimeout=6 -- "$TARGET" 'echo nexus-ok' 2>/dev/null | grep -q nexus-ok; then
  printf '\n  \033[32m✓ All set. Close this tab (Ctrl+Shift+W / Cmd+Shift+W) and click Connect on the machine in Nexus.\033[0m\n\n'
else
  printf '  \033[31m✗ Key login still fails. Check that ~/.ssh on the remote is not group/world writable.\033[0m\n\n'
fi
