# TuxSCP

A native Linux SSH/SFTP/FTP file transfer client — the WinSCP experience, rebuilt in Rust for Ubuntu and friends.

![TuxSCP](packaging/icons/tuxscp.png)

## Features

- **Dual-pane file manager** — local and remote side by side
- **Multiple simultaneous connections** via tabs
- **Protocols**: SFTP, SCP, FTP, FTPS
- **Authentication**: password, SSH public key, SSH agent, keyboard-interactive
- **Transfer queue** with progress bars, speed, ETA and cancellation
- **Session manager** — save and quickly reconnect to servers
- **File operations**: upload, download, rename, delete, mkdir, chmod
- **Keyboard shortcuts**: F5 refresh, Delete key, standard navigation
- **Native GTK look** on any modern Linux desktop

## Requirements

- Ubuntu 24.04 LTS (Noble) or 26.04 LTS (Plucky), or any distro with glibc ≥ 2.35
- OpenSSL 3.x (libssl3) — already installed on all modern Ubuntu systems

## Installation

### .deb package (recommended)

```bash
sudo apt install ./tuxscp_*.deb
```

### AppImage (portable, no install required)

```bash
chmod +x TuxSCP-*.AppImage
./TuxSCP-*.AppImage
```

Download the latest release from the [Releases](../../releases) page.

## Building from source

### Prerequisites

```bash
# Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Packaging tools (optional — only needed for .deb / .AppImage)
sudo apt install dpkg-dev librsvg2-bin
```

### Build

```bash
# Development build
make build

# Optimised release binary
make release

# Run directly
make run

# Build .deb package
make deb

# Build .AppImage
make appimage

# Build both packages
make packages
```

The release binary lands at `target/release/tuxscp`.  
Packages are written to the `release/` directory.

### Install system-wide

```bash
sudo make install          # installs to /usr/local
sudo make PREFIX=/usr install   # installs to /usr (system default)
```

## Usage

Launch TuxSCP from your application menu or run `tuxscp` in a terminal.

1. Click **New Connection** (or press the plug icon in the toolbar)
2. Enter host, port, username, and choose your authentication method
3. Click **Connect** — the remote file tree opens in a new tab
4. Drag or double-click files to transfer; right-click for more options
5. Open additional connections with the **+** tab button

### Keyboard shortcuts

| Key | Action |
|-----|--------|
| F5 | Refresh active panel |
| Delete | Delete selected files |
| Enter / double-click | Open directory / download file |
| Click `..` | Go up one directory |

## Project structure

```
src/
  main.rs           — entry point
  app.rs            — application state, tab management, main UI loop
  worker.rs         — SSH/SFTP/FTP worker thread (one per connection)
  models/
    connection.rs   — ConnectionParams, Protocol, AuthMethod
    file_entry.rs   — FileEntry with display helpers
    transfer.rs     — Transfer, TransferStatus, TransferDirection
    session.rs      — SavedSession, SessionStore (JSON persistence)
  ui/
    connect_dialog.rs   — New Connection dialog
    file_panel.rs       — dual-pane file list widget
    toolbar.rs          — top toolbar
    transfer_panel.rs   — transfer queue panel
    session_manager.rs  — saved sessions manager
packaging/
  build-deb.sh      — builds the .deb
  build-appimage.sh — builds the .AppImage
  gen-icon.py       — generates PNG icons from SVG
  tuxscp.desktop    — freedesktop .desktop entry
```

## Contributing

Pull requests are welcome. Run `make check` (clippy) and `make fmt` before submitting.

## License

MIT — see [LICENSE](LICENSE) for details.

---

*Inspired by [WinSCP](https://winscp.net). Built with [egui](https://github.com/emilk/egui) and [ssh2-rs](https://github.com/alexcrichton/ssh2-rs).*
