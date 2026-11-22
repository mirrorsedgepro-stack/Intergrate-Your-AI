# Nexus

One terminal for Claude Code, the Antigravity CLI (`agy`), OpenClaw and shells: on your own computer (Windows, macOS, Linux,
or a WSL distro) and on any Linux machines you can SSH into (a GPU workstation, a home server, a cloud VM, an NVIDIA DGX…), directly or over Tailscale, with team chat so several people
can share the machines without stepping on each other.

## Install

Download from [Releases](https://github.com/mirrorsedgepro-stack/Intergrate-Your-AI/releases):

| OS | File | Notes |
|---|---|---|
| Windows 10/11 | `Nexus-Setup-x.y.z.exe` | Tells you when a new version is out and installs it when you ask. Unsigned: SmartScreen asks once (More info → Run anyway). |
| macOS (Apple Silicon / Intel) | `Nexus-x.y.z-mac-arm64.zip` / `…-mac-x64.zip` | Unzip and drag Nexus into Applications. Unsigned: the first time, right-click Nexus in Applications → Open. Nexus tells you when a new version is out. |
| Linux x86_64 / arm64 (Debian, Ubuntu, DGX OS…) | `Nexus-x.y.z-linux-amd64.deb` / `…-linux-arm64.deb` | `sudo apt install ./Nexus-*.deb`. Other distros: run from source. |

**From source** (any OS, [Rust](https://rustup.rs) 1.85+):

```
cargo run --release
```

Nexus is a single native program written in Rust: no Electron, no Node, no browser engine. Coming from 1.x? See
[Upgrading from 1.x](#upgrading-from-1x).

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
| **Fleet view** | Ctrl+Shift+A (or the tray icon in the title bar, which counts who is waiting): every session as a live thumbnail with its state (working, idle, finished, needs you). Agents that stopped for you are listed first with what they asked, and can be answered in place: press 1 / 2 / 3 / Enter / Esc for a permission prompt, or type a reply. Sessions still running on your machines that aren't open in this window are listed too (with what Claude last reported, when *Enable Claude alerts* is on). |
| **Git worktrees** | Machine ⋯ menu → **In a new git worktree** (or "worktree" in the palette): pick a repository and a branch name, and the agent starts in its own working folder, `<repo>-worktrees/<branch>` beside the repository, so several agents on one project can't overwrite each other's files. Remove one with `git worktree remove` when the branch is merged. |
| **Prompt library** | In the broadcast box, **Save as prompt** keeps the text; **Prompts** (or typing "prompt" in the palette) puts a saved one back, ready to edit and send to any number of sessions. |
| **Phone alerts** | Settings → *Phone alerts*: give an [ntfy](https://ntfy.sh) topic address (ntfy.sh or your own server) and your phone is told when an agent finishes or needs you while Nexus isn't in front. By default only the session's name is sent; ticking *Include what the agent said* sends its message too. Anyone who knows the address can read the alerts, so use a long random topic. Offline mode does not stop these: they only ever go to the address you set. |
| **Session replay** | Machine ⋯ menu → **Replay recorded sessions…**: pick a recording (see *Session recording*) and scrub through it, or play it back at 1× to 64×. It opens on the last screen; the newest 6 MB of a recording are loaded. Recordings don't store the window size: set the columns to the width it was recorded at if a full-screen program looks scrambled. |
| **Workspaces** | Save the current arrangement (agents, machines, folders, split sizes) and reopen it in one click: Ctrl+Shift+O. |
| **Session restore** | On start, Nexus reopens last time's layout: remote sessions reattach to their tmux sessions, local ones restart in the same folder. |
| **Project folders** | Shift+click or right-click a launcher (or Ctrl+Shift+N) to pick a folder: recent folders, git projects and top-level folders on that machine, or type any path. |
| **Images & files** | Ctrl+V with a screenshot on the clipboard saves it as a PNG on the session's machine and pastes the path. Drop files onto a pane to do the same. Remote copies go to `~/.nexus/uploads` and are deleted after 7 days. |
| **Agent alerts** | Bells and OSC 9/777 notifications mark a session as waiting and raise a desktop notification (which leads back to that session). With "Enable Claude alerts" Claude rings exactly when it stops or needs you; other agents fall back to output-activity detection. |
| **Machine stats** | Each machine card (this PC, WSL distros and SSH machines) shows GPU load, GPU memory, temperature and power (`nvidia-smi`), RAM, CPU load, free disk on the home filesystem and network throughput in/out, refreshed every 5 s. |
| **Port forwarding** | Machine ⋯ menu → **Forward a port…**: a remote web UI (OpenClaw gateway, Jupyter, TensorBoard) becomes `http://localhost:<port>` over the SSH connection, loopback only. |
| **Jump hosts** | `ProxyJump` and `Match host` in `~/.ssh/config` are honoured; a machine can also set its own **Jump host** in Settings (like `ssh -J`). |
| **Session recording** | Settings → *Record remote sessions*: everything an agent prints goes to `~/.nexus/logs/<session>-<date>.log` on the machine (`tmux pipe-pane`), kept 30 days. |
| **Why Claude stopped** | With *Enable Claude alerts*, the hook records whether Claude is waiting for a permission (and for what) or finished (and its last reply); shown in the pane header and the notification. |
| **Claims & queue** | Team chat `/claim gpu-box 2h fine-tuning` marks a machine as yours on everyone's cards (`/release gpu-box` when done). Launching an agent on a claimed machine offers **Start when free**: it starts once the claim is gone and the GPU is idle. |
| **Export / import** | Ctrl+Shift+O → export workspaces and presets to a JSON file, or import a teammate's (sessions are re-pointed at your machines by address or name). **Publish my presets to the team** puts them on the chat hub for others to import. |
| **Offline mode** | Settings: no update checks, no internet installers. *Install tools* then uses installers you place in `~/.nexus/offline/` on each machine (`claude-install.sh`, `agy-install.sh`, `openclaw-install.sh`). |
| **MCP server** | Settings → *Let MCP clients on this PC control Nexus*, then `claude mcp add nexus -- "<Nexus folder>/nexus-mcp"` (Settings shows the exact command): Claude Code can list machines and sessions, launch agents, send text and read their output. Loopback only, token in `~/.nexus/mcp.json`. |
| **Auto-reconnect** | If a machine drops off the network, sessions show "reconnecting…" and reattach on their own (backoff up to 30 s). Nothing is lost: tmux kept them running. |
| **Resizable splits** | Drag the divider between panes; double-click it to reset. |
| **Send last reply** | Ctrl+Shift+S (or the ➤ button on an agent pane) puts the agent's latest answer into the broadcast composer, aimed at your other agents, ready to edit and send. |
| **Broadcast** | Ctrl+Shift+Enter: write one prompt, send it to several sessions. |

## Sharing your machines over Tailscale

Click the **Share** (people) icon in the title bar. Guests join your tailnet, but an access policy limits them to
SSH on your SSH machines, through **Tailscale SSH**: no keys to hand out, and removing a guest cuts their access at once.
By default each guest gets **their own Linux account** (`nexus-<name>`) on the shared machines, created when you invite
them (Nexus opens a tab per machine and asks for your sudo password) and locked when you remove them. Untick the box in
the Share panel to use the older shared-account mode, where guests log in as each machine's configured **User**.

**Owner, once:**
1. **This PC on Tailscale**: Install Tailscale (opens `winget install Tailscale.Tailscale`), then sign in from the tray icon.
2. **Machines on your tailnet**: "Join Tailscale" on each machine runs `sudo tailscale up --ssh` there; type the sudo password and open the login link it prints. Nexus then records each machine's tailnet address and falls back to it whenever the LAN address can't be reached (a **TS** badge shows on the card).
3. **Tailscale credential**: preferably an **OAuth client** ([login.tailscale.com → Settings → OAuth clients](https://login.tailscale.com/admin/settings/oauth)) with the scopes `devices:core` (tag `tag:nexus-spark`), `policy_file` and `users`; paste its secret (`tskey-client-…`). It has only the rights Nexus needs and never expires (Nexus exchanges it for one-hour access tokens). The tag must exist before the client can be created: add `"tag:nexus-spark": ["autogroup:admin"]` to `tagOwners` in your policy file first, or do step 4 once with an API token. An **API access token** (`tskey-api-…`, [Settings → Keys](https://login.tailscale.com/admin/settings/keys)) also works, but carries your full admin rights and expires within 90 days. Either is stored in your OS keychain (Windows Credential Manager / macOS Keychain / a Secret Service keyring such as gnome-keyring or KWallet), never in `config.json`.
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

> **Security:** in shared-account mode guests can read anything that account can (files, SSH keys, agent sign-ins,
> tmux sessions) and could leave a process running after removal; per-guest accounts avoid that, though guests still
> share the GPU and RAM. Invite codes only accept tailnet addresses on *your* tailnet, Nexus asks before adding them,
> and it never offers your SSH keys to invited machines. Updates are never downloaded silently: Nexus announces a new
> version and installs only when you ask.

## Team chat and deconfliction

Open with the chat icon in the title bar (Ctrl+Shift+M / ⌘⇧M). Everyone using the same machines lands in the same chat
automatically: it lives on one of them (`~/.nexus/collab`, by default the one whose hostname sorts first; override in
Settings) and travels over each person's existing SSH connection, so there's no extra server or port.

- **Who's doing what:** the people list shows everyone online, their status (`/status Training on the GPU box until 3pm`)
  and which agent sessions they have open. The sidebar shows avatars on sessions someone else is also in.
- **Before you collide:** opening a session someone else has open asks first, offering **Watch read-only**
  (`tmux attach -r`). Starting an agent in a folder where someone's agent already runs warns you and offers to
  message them. Optionally, Nexus posts "started Claude in ~/project on gpu-box" when you launch an agent.
- **Read-only watching** stops your own keystrokes reaching a session; it isn't a permission boundary between people sharing an account. Anyone who can write to the hub machine can post as any name.
- **@mentions** raise a notification. The chat keeps the last ~2000 messages.

## How it works

| | |
|---|---|
| The app | One Rust program: [egui](https://github.com/emilk/egui) draws the window, [alacritty_terminal](https://crates.io/crates/alacritty_terminal) is the terminal emulator. `nexus-mcp` beside it is the MCP stdio bridge. |
| Local sessions | A pseudo-terminal (ConPTY on Windows) via `portable-pty` (PowerShell, Git Bash, `claude`, `agy`) |
| Remote sessions | [`russh`](https://crates.io/crates/russh): one connection per machine, one channel per session, each in its own tmux session on a private tmux server (`tmux -L nexus`) configured to be invisible: no status bar, no mouse capture, native scrollback. |
| SSH trust | Host keys are checked against `~/.ssh/known_hosts` (including hashed entries and `UserKnownHostsFile` from `~/.ssh/config`). A host key Nexus hasn't seen before asks you to confirm its fingerprint once (tailnet addresses, already authenticated by WireGuard, are pinned without asking); a changed key is refused. Keys come from Settings, `~/.ssh/config` `IdentityFile`s, the default key files, then the Windows OpenSSH agent. Passphrase-protected keys prompt once per run. |
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
| Ctrl+Shift+M | Team chat (`/status …`, `/claim <machine> <2h|15:00> [note]`, `/release <machine>`) |
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

## Upgrading from 1.x

2.0 replaces the Electron app with a native Rust one. It reads the same `config.json`, so machines, presets, workspaces,
the last session and pinned host keys carry over, and sessions left running in tmux reattach as before. A few things
are different:

- **Install it once by hand.** 1.x can't update itself to 2.0; download the installer from Releases. On Windows it
  installs over the old version in the same folder. From 2.0 on, Nexus announces new versions itself and, when
  you ask, downloads the installer and checks it against the SHA-256 GitHub publishes for it. Windows then runs it;
  macOS unpacks the zip for you to drag across; Linux installs the `.deb` through the system's password prompt
  (when Nexus was installed from a `.deb`; otherwise the file is left in Downloads).
- **Tailscale credential:** paste it again in Share. 1.x kept it encrypted in a way only Electron can read; 2.0 keeps
  it in the OS keychain.
- **MCP:** the bridge is now a program, not a Node script. Re-register it: `claude mcp remove nexus`, then the
  `claude mcp add …` command Settings shows.
- **Terminal text:** the font comes from Settings → Font family if it is installed on this computer (otherwise a
  built-in monospace font). Italics are slanted versions of that font, emoji use the system's colour emoji font
  (Segoe UI Emoji, Apple Color Emoji, Noto Color Emoji), and input methods (IME) for composing CJK text work in the
  terminal. There are no ligatures.
- **Window:** the normal title bar of your OS. Starting Nexus a second time does nothing while it is already open.
- **Notifications:** on Linux, clicking one brings Nexus forward on that session. Windows and macOS don't report the
  click to Nexus, so there, coming back to Nexus within 20 seconds of a notification lands on its session.

## Development

The interface is styled after [Rust/UI](https://rust-ui.com/docs/components) (neutral surfaces, hairline borders,
light primary buttons, switches, badges), drawn natively in egui rather than with its web components. The UI typeface
is [Inter](https://rsms.me/inter/), bundled from `assets/fonts` under the SIL Open Font License
(`assets/fonts/Inter-LICENSE.txt`).


```
cargo test            # unit tests, plus end-to-end tests of the SSH layer against an in-process SSH server (with tmux where installed)
scripts/package.sh    # this platform's installer(s) into dist/  (Linux: .deb · macOS: .zip · Windows: NSIS .exe, run from Git Bash)
```

Useful when working on the UI: `NEXUS_SCREENSHOT=shot.png cargo run` saves a picture of the window three seconds after
start and quits (`NEXUS_OPEN=palette|settings|share|team|composer|workspaces|fleet|prompts|replay` opens that view first;
`NEXUS_CONFIG_DIR=<folder>` uses a config other than your own). It works under `xvfb-run`.

`scripts/ui-drive.py` (Linux, needs `python-xlib`) sends real mouse and keyboard events to a copy of Nexus running under
Xvfb and takes screenshots, for checking that controls respond: `python3 scripts/ui-drive.py :97 focus click:150,335 shot:out.png`.

CI runs the tests on Windows, macOS and Linux on every push and builds the installers on pull requests. To publish a
release, bump `version` in `Cargo.toml` and push it to main (or push a matching tag, `git tag v2.0.1 && git push --tags`).
The Release workflow builds the Windows, macOS and Linux installers into a **draft** release, then
`verify-and-publish.yml` scans them: **VirusTotal** (~70 engines, every installer) and **Microsoft Defender** (Windows
installer) must both pass, and ClamAV ([hugoalh/scan-virus-ghaction](https://github.com/hugoalh/scan-virus-ghaction))
runs as a reported check. Only then is the release published, with the VirusTotal reports, the scan log and SHA-256
checksums in its notes. VirusTotal passes a file with no detections, or with exactly one whose name marks it as a
heuristic or machine-learning guess (such as `…!ml` or plain `Malicious`) rather than a named threat; that detection is
listed in the release notes. Two detections on one file, or one naming a threat family, block the release. The
workflow needs a `VT_API_KEY` repository secret.

**Code signing** switches on when its repository secrets exist; without them installers are unsigned (the macOS app is
ad-hoc signed so it runs on Apple Silicon). Windows (Authenticode): `WINDOWS_CERT_PFX_BASE64` (the .pfx, base64) and
`WINDOWS_CERT_PASSWORD` sign both programs and the installer. macOS (Developer ID): `MACOS_CERT_P12_BASE64`,
`MACOS_CERT_PASSWORD` and `MACOS_SIGN_IDENTITY` (e.g. `Developer ID Application: Your Name (TEAMID)`) sign the app
with the hardened runtime; adding `APPLE_ID`, `APPLE_TEAM_ID` and `APPLE_APP_PASSWORD` (an app-specific
password) notarizes and staples them. The signing steps have not been run yet: no certificates were available.
Workflow actions are pinned to commit SHAs and kept current by Dependabot, which also watches
the Rust dependencies.

## Credits

Logos in `assets/icons/`, found via [dashboardicons.com](https://dashboardicons.com):

- Claude, Antigravity, Git, Windows, Linux (Tux), NVIDIA: [selfh.st/icons](https://selfh.st/icons/), [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/)
- OpenClaw: from [openclaw/openclaw](https://github.com/openclaw/openclaw), [MIT](https://opensource.org/license/mit)
- PowerShell, Terminal: [homarr-labs/dashboard-icons](https://github.com/homarr-labs/dashboard-icons), [Apache 2.0](https://www.apache.org/licenses/LICENSE-2.0)

All product names and logos are trademarks of their respective owners.
