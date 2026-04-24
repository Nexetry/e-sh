# e-sh

> A unified, cross-platform remote connection manager built in Rust with `egui`.

`e-sh` (the **e** stands for **egui**) is a single-binary desktop application for
managing and launching remote sessions over **SSH** and **SFTP** from one
consistent interface.

---

## Overview

Managing remote machines often requires juggling several different clients: an SSH
terminal and an SFTP file browser. `e-sh` brings these workflows together into a
single native application that is fast to launch, easy to script, and consistent
across Linux, macOS, and Windows.

The goal is **one binary, one UI, both protocols you need every day** — without
sacrificing the keyboard-first, low-overhead feel that power users expect.

## Features

### Working today

- Native, GPU-accelerated UI powered by [`egui`](https://github.com/emilk/egui) + `egui_dock`
- Royal-TSX-style layout: connection tree on the left, dockable tabbed terminals, status bar
- Connection manager with add / edit / delete and right-click menu, grouped by tag
- Inline `+` button on the sidebar to create a new connection
- Polished Royal-TSX-style edit dialog (header / grouped sidebar / scrollable detail / footer)
- Toast notifications (info / success / warn / error) for connect, disconnect, save, delete, host-key, persistence, and session-end events
- Persistent TOML store for connections (per-OS config dir) with transparent backward-compatible migration
- **SSH** interactive shell:
  - `russh` 0.55 transport, `tokio` async runtime
  - `alacritty_terminal` 0.26 emulator rendered through a custom egui widget
  - Password, public-key, and **SSH agent** authentication (uses `$SSH_AUTH_SOCK`,
    tries each loaded identity in order until one is accepted)
  - `~` expansion + native file picker (`rfd`) for selecting private keys
  - Encrypted-key passphrases
  - 10 000-line scrollback with mouse-wheel scroll, click-and-drag selection,
    Cmd-C / Ctrl-Shift-C to copy, Cmd-V / Ctrl-Shift-V to paste, Shift-PageUp/PageDown
    and Shift-Home/End for keyboard scroll navigation
- **TOFU host-key verification**:
  - SHA-256 fingerprints stored in `host_keys.toml`
  - Interactive modal prompt on first connect and on key mismatch
  - Three actions: _Reject_, _Accept once_, _Accept and save_
- **SSH tunnels** (configurable per connection):
  - **Local** (`-L`) — forward a local port to a remote host:port through the session
  - **Remote** (`-R`) — bind a port on the remote server and forward back to a local target
  - **Dynamic** (`-D`) — local SOCKS5 proxy (no-auth, CONNECT) tunneled over the session
  - Per-session collapsible **Tunnels** status strip in each tab showing live state per tunnel
    (`pending` / `listening` / `failed` / `disabled`), bound port when the OS picked one, and
    full error on hover for failed tunnels
- **Chained jump hosts (ProxyJump)**:
  - Per-connection ordered hop list (max depth 8), reorderable in the edit dialog
  - True SSH-in-SSH via nested handshakes over `direct-tcpip` channels
  - Live "Resolved path" preview in the edit dialog

- **SFTP** dual-pane file browser:
  - `russh-sftp` 2.1.1 backend, opens its own SSH session per tab (independent of any shell tab)
  - Side-by-side **Local** and **Remote** panes with editable path bar and clickable breadcrumb path
  - Right-click everything: per-row menu (Open / Upload-or-Download / Rename / Delete) and empty-pane menu (New folder / Refresh / Up)
  - **Multi-select** with Finder/Explorer semantics: plain click = single, Cmd/Ctrl-click = toggle, Shift-click = range from anchor
  - Bulk **Upload / Download / Delete** across the current selection (`N items` label when >1); Rename stays single-only
  - **Drag-and-drop** files or folders from the OS into either pane to upload / move into the current dir
  - Recursive folder transfers (upload and download), with one aggregated transfer row per top-level command
  - Per-transfer **Cancel** button (cooperative, partial files left on disk); progress bar + bytes/total + status line
  - **Filter** field (🔍) per pane: case-insensitive substring match, with one-click clear
  - **Resizable columns** (Name / Size / Modified) and **click-to-sort** column headers (toggle ▲ / ▼); directories grouped first regardless of sort key
  - Folder / file / symlink icons; size auto-formatted (B / K / M / G / T)

- **Credential storage**:
  - Passwords and key passphrases are encrypted with a **master password** you set
    on first launch and stored in `secrets.enc.toml` next to your other config
  - Encryption is `age` passphrase mode (scrypt KDF + ChaCha20-Poly1305); the
    master password never touches disk and is held in memory only for the
    lifetime of the running app
  - `connections.toml` is sanitized on every save: secrets never touch disk in
    plaintext
  - Legacy plaintext `connections.toml` files are migrated transparently into
    the encrypted store on first unlock

- **Command palette + keyboard shortcuts**:
  - Press **Cmd-K** (macOS) / **Ctrl-K** (Linux / Windows) — or **Cmd/Ctrl-Shift-P** — to open a fuzzy command palette
  - Fuzzy search powered by `nucleo-matcher` (the same algorithm family as Helix / Zed)
  - Commands: _New connection_, _Open_, _Open SFTP_, _Edit_ (per saved connection), _Switch to tab_ (per open tab), _Close tab_, _Toggle sidebar_, _Lock secrets_, _Open recordings_, _Quit_
  - Global app shortcuts: **Cmd/Ctrl-B** toggle sidebar · **Cmd/Ctrl-W** close active tab · **Cmd/Ctrl-Q** quit

- **Session recording (opt-in, per connection)**:
  - Enable on a per-connection basis via **Advanced → Recording** in the edit dialog (SSH and SFTP only)
  - **SSH** sessions are recorded as gzipped [asciicast v2](https://docs.asciinema.org/manual/asciicast/v2/) (`<uuid>.cast.gz`) — replay with `asciinema play <(gunzip -c file.cast.gz)`
  - **SFTP** sessions are recorded as gzipped JSON Lines audit logs (`<uuid>.sftp.jsonl.gz`) with one event per operation (`list`, `upload`, `download`, `mkdir`, `rmdir`, `remove`, `rename`, `realpath`, `upload_cancelled`, `download_cancelled`) — inspect with `gunzip -c file.sftp.jsonl.gz | jq -c .`
  - Server output only — **your typed input (including passwords typed at prompts) is never captured**; however recordings are stored **plaintext on disk**, so treat them like logs
  - A built-in **Recordings** tab (open from the sidebar bottom link or the command palette) lists every recording with status (_Complete_ / _Incomplete_ / _Partial_ / _File missing_), size, duration, and shortcuts to **Reveal in Finder/Explorer**, **Copy path**, **Delete**, or **Clean up missing**
  - Manifests are stored at `recordings/recordings.toml` under your config dir; recordings never auto-expire — clean up manually

### Planned

- Plugin / scripting hooks

## Supported Protocols

| Protocol | Purpose                       | Status                                                                                                                                                                   |
| -------- | ----------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| SSH      | Interactive remote shell      | **Working** (password + pubkey + agent, TOFU, tunnels `-L`/`-R`/`-D`, chained ProxyJump, scrollback + copy/paste, encrypted secret store, opt-in asciicast v2 recording) |
| SFTP     | Secure file transfer / browse | **Working** (dual-pane browser, drag-drop, recursive transfers, multi-select, filter, sortable/resizable columns, cancel, opt-in JSONL audit recording)                  |

## Architecture

`e-sh` follows a layered architecture: a thin `egui` UI sits on top of a session
and connection manager, which dispatches to per-protocol adapters that wrap the
underlying transport libraries. Configuration and credentials are isolated in a
dedicated store so the UI and protocol layers never deal with secrets directly.

```
src/
├── app.rs                  EshApp: top-level eframe::App, wires everything together
├── core/
│   └── connection.rs       Connection / AuthMethod / ConnectionStore
├── config/
│   ├── store.rs            ConfigPaths + connections.toml load/save
│   └── host_keys.rs        TOFU HostKeyStore (host_keys.toml)
├── proto/
│   ├── ssh.rs              russh client, host-key verifier, PTY session
│   └── sftp.rs             russh-sftp client, recursive transfers, cancel registry
└── ui/
    ├── connection_tree.rs  Left-side tree + "+" button
    ├── dock.rs             egui_dock tab area + per-tab tunnels status strip
    ├── edit_dialog.rs      Add / edit connection modal (auth, jump chain, tunnels)
    ├── host_key_prompt.rs  TOFU prompt modal
    ├── sftp_tab.rs         Dual-pane SFTP browser (filter, sort, resize, multi-select)
    ├── status_bar.rs       Bottom status bar
    ├── toast.rs            Toast notification overlay
    └── terminal_widget/    alacritty_terminal renderer
```

The high-level system diagram lives at:

- [`doc/system_architecture/v1.drawio`](doc/system_architecture/v1.drawio)

Open it with [diagrams.net](https://app.diagrams.net) (formerly draw.io) or the
VS Code "Draw.io Integration" extension.

## Tech Stack

- **Language:** Rust (edition 2024, toolchain `1.85+`)
- **UI:** [`egui`](https://github.com/emilk/egui) `0.34`, `eframe` `0.34`, `egui_dock` `0.19`, `egui_extras` `0.34`
- **Async runtime:** `tokio` `1`
- **SSH:** `russh` `0.55`
- **SFTP:** `russh-sftp` `2.1.1`
- **Terminal emulator:** `alacritty_terminal` `0.26`
- **File picker:** `rfd` `0.15`
- **Encryption:** `age` `0.11` (scrypt + ChaCha20-Poly1305) for the credential store
- **Config:** TOML via `serde` + `toml`
- **Paths:** `directories` `6`
- **Logging:** `tracing` + `tracing-subscriber`

## Getting Started

### Prerequisites

- Rust toolchain **1.85+** (edition 2024 support)
- A C toolchain (for some transitive dependencies)
- Platform-specific GUI dependencies for `egui`/`eframe`
  (see the [eframe docs](https://github.com/emilk/egui/tree/master/crates/eframe))

### Build from Source

```bash
git clone https://github.com/nexetry/e-sh.git
cd e-sh
cargo build --release
```

### Run

```bash
cargo run --release
```

The compiled binary will be available at `target/release/e-sh`.

### Release builds

Native, host-only release scripts that produce versioned archives in `dist/`:

```bash
# macOS (universal arm64+x86_64) or Linux x86_64
./scripts/build-release.sh

# Windows x86_64 (PowerShell)
pwsh scripts/build-release.ps1
```

Output:

- **macOS**:
  - `dist/e-sh-<version>-macos-universal.tar.gz` — universal arm64+x86_64 `e-sh.app` bundle, ad-hoc signed (Gatekeeper will warn on first launch — see [First launch on macOS](#first-launch-on-macos))
  - `dist/e-sh-<version>-macos-universal.dmg` — drag-to-Applications installer (requires [`create-dmg`](https://github.com/create-dmg/create-dmg); falls back to plain `hdiutil` if unavailable)
- **Linux**:
  - `dist/e-sh-<version>-linux-x86_64.tar.gz` — raw `e-sh` binary + README
  - `dist/e-sh_<version>-1_amd64.deb` — Debian / Ubuntu package (requires [`cargo-deb`](https://github.com/kornelski/cargo-deb); installs to `/usr/bin/e-sh` with a `.desktop` entry)
- **Windows**:
  - `dist/e-sh-<version>-windows-x86_64.zip` — portable archive
  - `dist/e-sh-<version>-x86_64.msi` — MSI installer (requires [`cargo-wix`](https://github.com/volks73/cargo-wix) + WiX Toolset v3; run `cargo wix init` once to generate `wix/main.wxs`)

Each archive and installer ships with a matching `.sha256` checksum file.

Install the installer tooling (one-time, per host) with:

```bash
# macOS
brew install create-dmg

# Linux
cargo install cargo-deb

# Windows (PowerShell)
cargo install cargo-wix
```

For automated cross-platform release builds on git tag push (`vX.Y.Z`), the
GitHub Actions workflow at `.github/workflows/release.yml` builds all three
targets on their native runners (`macos-latest`, `ubuntu-latest`,
`windows-latest`) and attaches the artifacts to a GitHub Release.

### First launch on macOS

`e-sh` is **ad-hoc signed**, not signed with a paid Apple Developer ID, so on
first launch macOS Gatekeeper will show:

> _"Apple could not verify 'e-sh' is free of malware that may harm your Mac
> or compromise your privacy."_

This is expected for any unsigned / ad-hoc-signed app. To bypass:

**Easiest — Finder**

1. Open Finder, locate `e-sh.app` (Applications, `dist/`, wherever you put it).
2. **Right-click** (or Ctrl-click) the app → **Open**.
3. Click **Open** in the warning dialog.

macOS remembers this choice and won't prompt again for that copy.

**If right-click "Open" is blocked (macOS 14.4+ / Sequoia)**

```bash
xattr -dr com.apple.quarantine /Applications/e-sh.app
```

Adjust the path if you installed it elsewhere. Then double-click normally.

**If macOS still complains**

Open **System Settings → Privacy & Security**, scroll down — there will be a
message like _"e-sh was blocked"_ with an **Open Anyway** button. Click it
once.

> The Gatekeeper warning will reappear every time you install a new build
> (each new download / install gets a fresh quarantine flag). Until proper
> Developer ID signing + notarization is wired up, repeat the bypass per
> install.

## Usage

1. Launch `e-sh`.
2. Click the `+` next to **Connections** in the sidebar to create a new entry.
3. Fill in name, group, host, port, username, and choose an auth method:
   - **Password** — type the password directly (encrypted with your master password)
   - **Public key** — type the path or click `...` to browse; supply a passphrase
     if the key is encrypted (passphrase is encrypted with your master password)
   - **SSH agent** — uses your running ssh-agent (`$SSH_AUTH_SOCK`); each loaded
     identity is tried in order. Run `ssh-add` to load keys.
4. Save. Double-click the connection (or right-click → _Open_) to connect.
5. On first connect to a host you'll be prompted to verify its key fingerprint
   (TOFU). Choose _Accept and save_ to remember it.
6. Right-click any connection to _Open_, _Edit_, _Open SFTP_, or _Delete_ it.

### SFTP browser

- Open SFTP for any connection: right-click → _Open SFTP_ (uses the connection's existing auth + jump chain over a fresh SSH session).
- Two panes: **Local** (your machine) and **Remote** (the server). Each pane has:
  - A clickable breadcrumb path and an editable path text field (Enter to navigate).
  - A 🔍 **filter** input — case-insensitive substring match against entry names.
  - **Resizable** Name / Size / Modified columns; click any column header to sort, click again to flip direction (▲ / ▼). Folders are always grouped first.
- **Selection**: click = single, Cmd/Ctrl-click = toggle, Shift-click = range.
- **Right-click** an entry for: Open, Upload / Download (across panes), Rename (single-only), Delete. Right-click on empty space for: New folder, Refresh, Up.
- **Drag and drop** files or folders from your OS into either pane to upload / move into the current directory. Folder transfers recurse automatically.
- The bottom **Transfers** strip shows progress, bytes, and a per-transfer **Cancel** button. Use _Clear finished_ to prune completed rows.

## Configuration

Configuration is stored as TOML under the standard OS config directory:

- **Linux:** `~/.config/com.nexetry.e-sh/`
- **macOS:** `~/Library/Application Support/com.nexetry.e-sh/`
- **Windows:** `%APPDATA%\nexetry\e-sh\`

Files in that directory:

- `connections.toml` — saved connections (sanitized; never contains plaintext secrets)
- `secrets.enc.toml` — `age`-encrypted password / passphrase store, unlocked at
  startup with your master password
- `host_keys.toml` — TOFU host-key store (algorithm + SHA-256 fingerprint + first-seen timestamp)
- `recordings/` — session recordings directory (created on first opt-in recording):
  - `recordings.toml` — manifest index of all recordings (id, connection, kind, started/ended timestamps, size, status)
  - `<uuid>.cast.gz` — gzipped asciicast v2 files for SSH sessions
  - `<uuid>.sftp.jsonl.gz` — gzipped JSON Lines audit logs for SFTP sessions

> The first time you save a connection that needs a secret, `e-sh` asks you to
> set a master password. On every subsequent launch it asks you to enter that
> same password to unlock the encrypted store. The master password is held in
> memory only for the lifetime of the running app and is never written to disk.
> If you forget it, the encrypted secrets cannot be recovered — you will need
> to delete `secrets.enc.toml` and re-enter your credentials.

## Project Status

`e-sh` is in **early development (alpha)**. The SSH and SFTP MVPs are functional
end-to-end (connect, authenticate, render shell, browse + transfer files,
persist host keys), but expect breaking changes to config formats and APIs.

## Roadmap

- [x] Application skeleton with `eframe`/`egui`
- [x] Connection model + persistent storage (TOML)
- [x] Add / edit / delete connections from the UI
- [x] SSH adapter (interactive PTY) — `russh` + `alacritty_terminal`
- [x] Password + public-key authentication
- [x] TOFU host-key verification with persistence
- [x] Native file picker for private keys
- [x] Toast / inline error reporting
- [x] SSH tunnels (`-L`, `-R`, `-D` SOCKS5)
- [x] Chained jump host / bastion (ProxyJump, ordered N-hop)
- [x] Per-session tunnel status display (collapsible)
- [x] Royal-TSX-style edit dialog
- [x] SSH agent authentication
- [x] Credential storage via `age`-encrypted secret store
- [x] Terminal scrollback UI + selection / copy / paste
- [x] SFTP adapter (dual-pane browser, drag-drop, recursive transfers, multi-select, filter, sortable/resizable columns)
- [x] Command palette + keyboard-first navigation
- [x] Native installers (`.dmg` / `.deb` / `.msi`) in the release pipeline
- [x] Tabbed multi-session UI polish (split panes, drag-to-reorder)
- [x] Connections tab UI polish (drag-to-reorder, grey, ellipsised and small text sub title)
- [x] Session recording / logging (opt-in)
- [ ] Plugin / scripting hooks

## Screenshots

> _Coming soon._

## FAQ

**Q: Why another remote connection manager?**
Most existing tools are either single-protocol, web-based, or shipped as heavy
Electron apps. `e-sh` aims for a single, fast, native binary that covers the
common protocols with a consistent UX.

**Q: Why `egui` instead of GTK / Qt / Tauri?**
`egui` gives us a single Rust dependency, zero web runtime, and a UI that feels
the same on every OS — which matches the "single binary" goal.

**Q: Does it support agent forwarding / jump hosts / X11 forwarding?**
Chained jump hosts (ProxyJump) are supported today with up to 8 hops per
connection. SSH agent forwarding and X11 forwarding are not yet implemented.

**Q: Is it production ready?**
No. See [Project Status](#project-status).

**Q: How do I replay a recorded session?**
SSH recordings are [asciicast v2](https://docs.asciinema.org/manual/asciicast/v2/)
gzipped, so any asciicast-compatible player works. With
[`asciinema`](https://asciinema.org) installed:

```bash
asciinema play <(gunzip -c ~/Library/Application\ Support/com.nexetry.e-sh/recordings/<uuid>.cast.gz)
```

SFTP recordings are gzipped JSON Lines audit logs — one event per line. Inspect
them with:

```bash
gunzip -c ~/Library/Application\ Support/com.nexetry.e-sh/recordings/<uuid>.sftp.jsonl.gz | jq -c .
```

The built-in **Recordings** tab (sidebar bottom link, or `Cmd/Ctrl-K` →
_Open recordings_) lists every recording with status and shortcuts to _Reveal_,
_Copy path_, and _Delete_.

**Q: Are my passwords captured in recordings?**
No. `e-sh` only records **server output** (what you see on screen), not the
keys you type — so passwords, passphrases, and typed secrets never enter the
recording. That said, recordings are stored **plaintext on disk** (gzipped
only), so treat the `recordings/` directory like any other log directory:
anything the server printed (file contents you `cat`-ed, tokens the server
echoed, etc.) will be in there. Recording is **opt-in per connection** and
off by default.

**Q: macOS warns "Apple could not verify 'e-sh' is free of malware" — is the app unsafe?**
No. `e-sh` is **ad-hoc signed** (not signed with a paid Apple Developer ID),
so Gatekeeper cannot verify the publisher and shows this warning on first
launch for any download. The binary itself is the same one you (or GitHub
Actions) built from this source repo. To bypass, see
[First launch on macOS](#first-launch-on-macos) — short version: right-click
→ Open, or `xattr -dr com.apple.quarantine /Applications/e-sh.app`.
The warning will reappear after each fresh install until proper Developer ID
signing + notarization is added to the release pipeline (planned).

**Q: macOS shows an `AutoFill (e-sh)` process in Activity Monitor — is it accessing my Keychain?**
No. `e-sh` does not link `Security.framework` and does not call any Keychain
API (credentials are stored in `secrets.enc.toml` encrypted with your master
password via `age`). The `AutoFill (<AppName>)` process is
`com.apple.SafariPlatformSupport.Helper.xpc`, a system XPC helper that macOS
14+ AppKit preloads for any Cocoa application with a text-input responder
chain. You will see identical `AutoFill (…)` helpers listed for virtually
every native app on your system (Brave, WhatsApp, Raycast, etc.). The helper
is parented to `launchd` (not to `e-sh`), consumes only a few MB idle, and is
managed entirely by the OS — it is a cosmetic artifact of AppKit, not an
access by `e-sh`, and is outside application-developer control.

## Contributing

Contributions, bug reports, and feature requests are welcome. Until the
architecture stabilizes, please open an issue to discuss substantial changes
before sending a pull request.

1. Fork the repo and create a feature branch.
2. Run `cargo fmt` and `cargo clippy --all-targets` before committing.
3. Open a PR describing the change and the motivation.

## License

License terms have not yet been finalized. Until a `LICENSE` file is added,
all rights are reserved by the authors.
