#!/usr/bin/env bash
# build-deb.sh — builds a .deb package for TuxSCP
# Targets: Ubuntu 24.04 LTS (Noble) and 26.04 LTS (Plucky)
# Usage: ./packaging/build-deb.sh [--release-dir <dir>]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

PKG_NAME="tuxscp"
VERSION="$(grep '^version' "$ROOT_DIR/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')"
ARCH="$(dpkg --print-architecture 2>/dev/null || uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/')"
RELEASE_DIR="${1:-$ROOT_DIR/release}"
DEB_ROOT="$RELEASE_DIR/deb-staging"
DEB_FILE="$RELEASE_DIR/${PKG_NAME}_${VERSION}_${ARCH}.deb"

echo "==> Building TuxSCP v${VERSION} .deb (${ARCH})"

# ── 1. Build release binary ───────────────────────────────────────────────────
echo "--> Compiling release binary..."
cd "$ROOT_DIR"
cargo build --release
BINARY="$ROOT_DIR/target/release/$PKG_NAME"
if [[ ! -f "$BINARY" ]]; then
  echo "ERROR: release binary not found at $BINARY"
  exit 1
fi

# ── 2. Generate icon ──────────────────────────────────────────────────────────
echo "--> Generating icons..."
cd "$SCRIPT_DIR"
python3 gen-icon.py 2>/dev/null || echo "   (icon generation skipped — install rsvg-convert or inkscape)"

# ── 3. Create package layout ──────────────────────────────────────────────────
echo "--> Assembling package layout..."
rm -rf "$DEB_ROOT"
mkdir -p "$DEB_ROOT/DEBIAN"
mkdir -p "$DEB_ROOT/usr/bin"
mkdir -p "$DEB_ROOT/usr/share/applications"
mkdir -p "$DEB_ROOT/usr/share/doc/$PKG_NAME"
mkdir -p "$DEB_ROOT/usr/share/pixmaps"
for size in 16 32 48 64 128 256; do
    mkdir -p "$DEB_ROOT/usr/share/icons/hicolor/${size}x${size}/apps"
done

# Binary
install -m 755 "$BINARY" "$DEB_ROOT/usr/bin/$PKG_NAME"

# Desktop file
install -m 644 "$SCRIPT_DIR/tuxscp.desktop" "$DEB_ROOT/usr/share/applications/"

# Icons
for size in 16 32 48 64 128 256; do
    PNG="$SCRIPT_DIR/icons/tuxscp_${size}.png"
    if [[ -f "$PNG" ]]; then
        install -m 644 "$PNG" \
            "$DEB_ROOT/usr/share/icons/hicolor/${size}x${size}/apps/tuxscp.png"
    fi
done
if [[ -f "$SCRIPT_DIR/icons/tuxscp.png" ]]; then
    install -m 644 "$SCRIPT_DIR/icons/tuxscp.png" "$DEB_ROOT/usr/share/pixmaps/tuxscp.png"
fi

# Docs
cat > "$DEB_ROOT/usr/share/doc/$PKG_NAME/copyright" << 'EOF'
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: tuxscp
Source: https://github.com/example/tuxscp

Files: *
Copyright: 2024 TuxSCP Contributors
License: MIT
EOF

# ── 4. DEBIAN/control ────────────────────────────────────────────────────────
INSTALLED_SIZE=$(du -sk "$DEB_ROOT" | cut -f1)
cat > "$DEB_ROOT/DEBIAN/control" << EOF
Package: $PKG_NAME
Version: $VERSION
Architecture: $ARCH
Maintainer: TuxSCP Contributors <tuxscp@example.com>
Installed-Size: $INSTALLED_SIZE
Depends: libssl3 | libssl1.1, libgcc-s1
Recommends: openssh-client
Section: net
Priority: optional
Homepage: https://github.com/example/tuxscp
Description: Native Linux SSH/SFTP/FTP client
 TuxSCP is a native Linux file transfer client supporting SFTP, SCP,
 FTP and FTPS protocols. It provides a dual-pane file manager interface
 for managing local and remote files, with a transfer queue, session
 manager, and full file operation support.
 .
 Inspired by WinSCP, built in Rust with a native UI.
EOF

# ── 5. DEBIAN/postinst ────────────────────────────────────────────────────────
cat > "$DEB_ROOT/DEBIAN/postinst" << 'EOF'
#!/bin/sh
set -e
if which update-desktop-database > /dev/null 2>&1; then
    update-desktop-database -q /usr/share/applications
fi
if which gtk-update-icon-cache > /dev/null 2>&1; then
    gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
fi
EOF
chmod 755 "$DEB_ROOT/DEBIAN/postinst"

cat > "$DEB_ROOT/DEBIAN/postrm" << 'EOF'
#!/bin/sh
set -e
if which update-desktop-database > /dev/null 2>&1; then
    update-desktop-database -q /usr/share/applications || true
fi
if which gtk-update-icon-cache > /dev/null 2>&1; then
    gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
fi
EOF
chmod 755 "$DEB_ROOT/DEBIAN/postrm"

# ── 6. Build .deb ─────────────────────────────────────────────────────────────
echo "--> Building .deb with dpkg-deb..."
mkdir -p "$RELEASE_DIR"
dpkg-deb --build --root-owner-group "$DEB_ROOT" "$DEB_FILE"

echo ""
echo "✔  Package built: $DEB_FILE"
echo "   Install with:  sudo apt install $DEB_FILE"
echo "   Or:            sudo dpkg -i $DEB_FILE"
