# e-sh

> A unified, cross-platform remote connection manager built in Rust with `egui`.

`e-sh` (the **e** stands for **egui**) is a single-binary desktop application for
managing and launching remote sessions over **SSH**, **SFTP**, **RDP**, and **VNC**
from one consistent interface.

---

## Overview

Managing remote machines often requires juggling several different clients: an SSH
terminal, an SFTP file browser, an RDP viewer, and a VNC client. `e-sh` brings these
workflows together into a single native application that is fast to launch, easy to
script, and consistent across Linux, macOS, and Windows.

The goal is **one binary, one UI, every protocol you need** — without sacrificing
the keyboard-first, low-overhead feel that power users expect.

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
  - `russh` 0.60 transport, `tokio` async runtime
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
  - All passwords and key passphrases live in the OS-native secret store
    (macOS Keychain, Windows Credential Manager, Linux Secret Service)
  - `connections.toml` is sanitized on every save: secrets never touch disk in
    plaintext
  - Legacy plaintext `connections.toml` files are migrated transparently on
    first load

### Planned

- Cross-platform release builds for Linux, macOS, Windows
- Keyboard-first navigation + command palette
- RDP and VNC backends

## Supported Protocols

| Protocol | Purpose                       | Status                                                                                                                        |
| -------- | ----------------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| SSH      | Interactive remote shell      | **Working** (password + pubkey + agent, TOFU, tunnels `-L`/`-R`/`-D`, chained ProxyJump, scrollback + copy/paste, OS keyring) |
| SFTP     | Secure file transfer / browse | **Working** (dual-pane browser, drag-drop, recursive transfers, multi-select, filter, sortable/resizable columns, cancel)     |
| RDP      | Remote desktop (Windows)      | Planned                                                                                                                       |
| VNC      | Remote desktop (generic)      | Planned                                                                                                                       |

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
- **SSH:** `russh` `0.60`
- **SFTP:** `russh-sftp` `2.1.1`
- **Terminal emulator:** `alacritty_terminal` `0.26`
- **File picker:** `rfd` `0.15`
- **Keyring:** `keyring` `3` (Keychain / Credential Manager / Secret Service)
- **Config:** TOML via `serde` + `toml`
- **Paths:** `directories` `6`
- **Logging:** `tracing` + `tracing-subscriber`
- **Planned:** `ironrdp`, `vnc-rs`

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

## Usage

1. Launch `e-sh`.
2. Click the `+` next to **Connections** in the sidebar to create a new entry.
3. Fill in name, group, host, port, username, and choose an auth method:
   - **Password** — type the password directly (saved to OS keyring)
   - **Public key** — type the path or click `...` to browse; supply a passphrase
     if the key is encrypted (passphrase is saved to OS keyring)
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

- `connections.toml` — saved connections
- `host_keys.toml` — TOFU host-key store (algorithm + SHA-256 fingerprint + first-seen timestamp)

> Passwords and key passphrases are stored in your OS-native secret store
> (Keychain / Credential Manager / Secret Service). `connections.toml` only
> ever holds non-secret metadata.

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
- [x] Credential storage via OS keyring
- [x] Terminal scrollback UI + selection / copy / paste
- [x] SFTP adapter (dual-pane browser, drag-drop, recursive transfers, multi-select, filter, sortable/resizable columns)
- [ ] Tabbed multi-session UI polish (split panes, drag-to-reorder)
- [ ] RDP adapter
- [ ] VNC adapter
- [ ] Command palette + keyboard-first navigation
- [ ] Session recording / logging (opt-in)
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
