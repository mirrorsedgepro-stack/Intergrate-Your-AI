# Nexus

One terminal for Claude Code, the Antigravity CLI (`agy`), OpenClaw and shells: on your own computer (Windows, macOS, Linux,
or a WSL distro) and on any Linux machines you can SSH into (a GPU workstation, a home server, a cloud VM, an NVIDIA DGX…), directly or over Tailscale, with team chat so several people
can share the machines without stepping on each other.

## Install

Download from [Releases](https://github.com/mirrorsedgepro-stack/AI-Orchestrator/releases):

| OS | File | Notes |
|---|---|---|
| Windows 10/11 | `Nexus-Setup-x.y.z.exe` | Updates itself. Unsigned: SmartScreen asks once (More info → Run anyway). |
| macOS (Apple Silicon / Intel) | `Nexus-x.y.z-mac-arm64.dmg` / `…-mac-x64.dmg` | Unsigned: the first time, right-click Nexus in Applications → Open. Nexus tells you when a new version is out. |
| Linux x86_64 / arm64 (Debian, Ubuntu, DGX OS…) | `Nexus-x.y.z-linux-amd64.deb` / `…-linux-arm64.deb` | `sudo apt install ./Nexus-*.deb`. Other distros: run from source. |

**From source** (any OS, Node 22+):

```
npm install && node node_modules/electron/install.js
npm start                                # Windows also: Start-Nexus.cmd
```

### Per-OS behaviour

- **Windows:** local sessions run in PowerShell (pwsh if installed) or Git Bash. **Each WSL distro gets its own machine card**
  (e.g. "WSL · Ubuntu") with Claude / Antigravity / OpenClaw / Shell running *inside* the distro; dropped files and pasted
  screenshots arrive as `/mnt/c/...` paths.
- **macOS:** local sessions use your login shell (zsh). Shortcuts use **⌘** (⌘⇧P palette, ⌘1…9, ⌘C / ⌘V); **Ctrl+C**
  always reaches the terminal. Nexus loads your login shell's PATH, so tools from Homebrew or `~/.local/bin` are found
  even when launched from Finder.
- **Linux:** local sessions use your login shell; shortcuts as on Windows (Ctrl+Shift+…).

## First-time setup for a Linux machine

1. **Settings** (Ctrl+,): enter the machine's IP/hostname and user, then Save. Blank fields fall back to `~/.ssh/config`, like `ssh` does.
2. **Set up SSH key**: opens a tab that creates `~/.ssh/id_ed25519` (if missing) and installs it on the machine. You type the Linux password once.
3. **Install tmux / Claude / Antigravity / OpenClaw**: runs `scripts/linux-bootstrap.sh` on the machine (may ask for your sudo password).
   OpenClaw's installer starts its setup wizard; if you skip it, run `openclaw onboard --install-daemon` once before using OpenClaw.
4. **Enable Claude alerts** (machine ⋯ menu): adds Claude Code hooks that ring Nexus when Claude finishes or needs input.
5. Launch Claude, Antigravity and OpenClaw (`openclaw tui`) from the sidebar. Each asks you to sign in the first time.

The machine needs `sshd` running (`sudo apt install openssh-server`).

## Features

| | |
|---|---|
| **Workspaces** | Save the current arrangement (agents, machines, folders, split sizes) and reopen it in one click: Ctrl+Shift+O. |
| **Session restore** | On start, Nexus reopens last time's layout: remote sessions reattach to their tmux sessions, local ones restart in the same folder. |
| **Project folders** | Shift+click or right-click a launcher (or Ctrl+Shift+N) to pick a folder: recent folders, git projects and top-level folders on that machine, or type any path. |
| **Images & files** | Ctrl+V with a screenshot on the clipboard saves it as a PNG on the session's machine and pastes the path. Drop files onto a pane to do the same. Remote copies go to `~/.nexus/uploads` and are deleted after 7 days. |
| **Agent alerts** | Bells and OSC 9/777 notifications mark a session as waiting and raise a Windows notification. With "Enable Claude alerts" Claude rings exactly when it stops or needs you; other agents fall back to output-activity detection. |
| **GPU stats** | Each machine card shows GPU load, temperature and power (`nvidia-smi`), RAM and CPU load, refreshed every 5 s. |
| **Auto-reconnect** | If a machine drops off the network, sessions show "reconnecting…" and reattach on their own (backoff up to 30 s). Nothing is lost: tmux kept them running. |
| **Resizable splits** | Drag the divider between panes; double-click it to reset. |
| **Send last reply** | Ctrl+Shift+S (or the ➤ button on an agent pane) puts the agent's latest answer into the broadcast composer, aimed at your other agents, ready to edit and send. |
| **Broadcast** | Ctrl+Shift+Enter: write one prompt, send it to several sessions. |

## Sharing your machines over Tailscale

Click the **Share** (people) icon in the title bar. Guests join your tailnet, but an access policy limits them to
SSH on your SSH machines, through **Tailscale SSH**: no keys to hand out, and removing a guest cuts their access at once.
They log in as each machine's configured user (the **User** field in Settings), so they share those accounts with you.

**Owner, once:**
1. **This PC on Tailscale**: Install Tailscale (opens `winget install Tailscale.Tailscale`), then sign in from the tray icon.
2. **Machines on your tailnet**: "Join Tailscale" on each machine runs `sudo tailscale up --ssh` there; type the sudo password and open the login link it prints. Nexus then records each machine's tailnet address and falls back to it whenever the LAN address can't be reached (a **TS** badge shows on the card).
3. **API token**: create an API access token at [login.tailscale.com → Settings → Keys](https://login.tailscale.com/admin/settings/keys) and paste it in. It's stored encrypted with your Windows account (DPAPI) and never leaves the Nexus main process. Tokens expire (max 90 days); paste a new one when invites start failing.
4. **Access rules → Review**: shows exactly what changes in your tailnet policy, then **Apply**. Nexus validates the policy with Tailscale first and only saves it if nobody edited it meanwhile. It:
   - creates `tag:nexus-spark` and tags the shared machines with it (the name is kept for existing setups);
   - narrows any "everyone can reach everything" rule to admins (you), so guests can't reach your other devices;
   - lets `group:nexus-guests` reach those machines on TCP 22 only, and adds a Tailscale SSH rule for you and the guests.

**Inviting someone:** enter their email and click **Invite**. Nexus adds them to the guest group, creates a Tailscale invite
(Tailscale also emails it), and gives you a ready-to-send message containing the invite link and a **Nexus invite code**.
**Remove** revokes the invite or deletes the user from your tailnet.

**Guest:** install Tailscale and accept the invite (signing in with the invited email), install Nexus, open **Share**, paste
the invite code under "Got an invite code?" and click **Add machines**. The code contains only the machines' tailnet names and
login users; no keys or tokens.

> Tip: Tailscale SSH and Nexus's own SSH key setup don't need password logins. If you enabled `PasswordAuthentication`
> on a machine to get started, consider turning it off again once your key works.

## Team chat and deconfliction

Open with the chat icon in the title bar (Ctrl+Shift+M / ⌘⇧M). Everyone using the same machines lands in the same chat
automatically: it lives on one of them (`~/.nexus/collab`, by default the one whose hostname sorts first; override in
Settings) and travels over each person's existing SSH connection, so there's no extra server or port.

- **Who's doing what:** the people list shows everyone online, their status (`/status Training on the GPU box until 3pm`)
  and which agent sessions they have open. The sidebar shows avatars on sessions someone else is also in.
- **Before you collide:** opening a session someone else has open asks first, offering **Watch read-only**
  (`tmux attach -r`). Starting an agent in a folder where someone's agent already runs warns you and offers to
  message them. Optionally, Nexus posts "started Claude in ~/project on gpu-box" when you launch an agent.
- **@mentions** raise a notification. The chat keeps the last ~2000 messages.

## How it works

| | |
|---|---|
| Local sessions | ConPTY via `node-pty` (PowerShell, Git Bash, `claude`, `agy`) |
| Remote sessions | `ssh2`: one connection per machine, one channel per session, each in its own tmux session on a private tmux server (`tmux -L nexus`) configured to be invisible: no status bar, no mouse capture, native scrollback. |
| SSH trust | Host keys are checked against `~/.ssh/known_hosts` (including hashed entries and `UserKnownHostsFile` from `~/.ssh/config`); unknown hosts are pinned on first use. Keys come from Settings, `~/.ssh/config` `IdentityFile`s, the default key files, then the Windows OpenSSH agent. Passphrase-protected keys prompt once per run. |
| Agent exits | You drop into your normal shell in the same pane. |
| Config | `%APPDATA%\Nexus\config.json` (machines, presets, workspaces, appearance, pinned host keys) |

To use password-protected keys without a prompt each run, start the Windows OpenSSH agent (admin PowerShell: `Set-Service ssh-agent -StartupType Automatic; Start-Service ssh-agent`, then `ssh-add`).

## Keys

| Keys | Action |
|---|---|
| Ctrl+Shift+P | Command palette: switch, launch on any machine, reattach, layouts |
| Ctrl+Shift+N · Ctrl+Shift+O | Launch in a folder · workspaces |
| Ctrl+1…9 · Ctrl+Tab · Ctrl+` | Jump to tab · next/prev · last used |
| Ctrl+Shift+L · Ctrl+Shift+D | Cycle layout (1 / 2 / 4 panes) · open current side by side |
| Ctrl+Alt+Arrows | Move focus between panes |
| Ctrl+Shift+Enter · Ctrl+Shift+S | Broadcast a prompt · send the focused agent's last reply |
| Ctrl+Shift+M | Team chat |
| Ctrl+C / Ctrl+V | Copy if text is selected (otherwise interrupt) / paste text or a screenshot |
| Shift+drag | Select text even when the app uses the mouse (Claude Code's full-screen UI). Selecting inside Claude itself also copies to the Windows clipboard. |
| Shift+Enter | Newline in Claude/Antigravity without submitting |
| Ctrl+Shift+F | Find in scrollback |
| Ctrl+Shift+W | Close (local) / detach (remote; it keeps running) |
| Ctrl+= / Ctrl+- / Ctrl+0 | Font size |

Right-click any terminal → **Send selection to …** to pipe output from one agent into another.

## Adding presets

Edit `presets` in the config file, e.g. a project-specific command (`icon` picks a logo: claude, antigravity, openclaw, git, terminal, powershell):

```json
{ "id": "claude-proj", "name": "Claude · api", "color": "#E08A62", "icon": "claude", "cmd": { "linux": "cd ~/code/api && claude" } }
```

## Development

```
npm test        # unit tests (ssh_config, known_hosts, remote commands, alert hooks, config migration, team chat, invites, palette)
npm run dist    # build dist/Nexus-Setup-<version>.exe
```

CI runs the tests on Windows, macOS and Linux on every push. To publish a release, bump `version` in `package.json`, then push a matching tag (`git tag v1.3.1 && git push --tags`). The Release workflow builds the Windows, macOS and Linux installers into a **draft** release, then `verify-and-publish.yml` scans them: **VirusTotal** (~70 engines, every installer) and **Microsoft Defender** (Windows installer) must both be clean, and ClamAV ([hugoalh/scan-virus-ghaction](https://github.com/hugoalh/scan-virus-ghaction)) runs as a reported check. Only then is the release published, with the VirusTotal reports, the scan log and SHA-256 checksums in its notes. (The ClamAV bundled in that action, 1.0.3 with 2023 signatures, flags the official Electron runtime itself as `Win.Trojan.Virut`, so it can't gate an Electron app.) The workflow needs a `VT_API_KEY` repository secret. Installed copies update from it.

## Credits

Logos in `assets/icons/`, found via [dashboardicons.com](https://dashboardicons.com):

- Claude, Antigravity, Git, Windows, Linux (Tux), NVIDIA: [selfh.st/icons](https://selfh.st/icons/), [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/)
- OpenClaw: from [openclaw/openclaw](https://github.com/openclaw/openclaw), [MIT](https://opensource.org/license/mit)
- PowerShell, Terminal: [homarr-labs/dashboard-icons](https://github.com/homarr-labs/dashboard-icons), [Apache 2.0](https://www.apache.org/licenses/LICENSE-2.0)

All product names and logos are trademarks of their respective owners.
