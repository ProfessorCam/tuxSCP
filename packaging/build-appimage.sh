#!/usr/bin/env bash
# build-appimage.sh — builds a self-contained .AppImage for TuxSCP
# Compatible with Ubuntu 24.04+ and any glibc ≥ 2.35 Linux distro.
# Usage: ./packaging/build-appimage.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

PKG_NAME="tuxscp"
VERSION="$(grep '^version' "$ROOT_DIR/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')"
ARCH="$(uname -m)"
RELEASE_DIR="$ROOT_DIR/release"
APPDIR="$RELEASE_DIR/TuxSCP.AppDir"
APPIMAGE_FILE="$RELEASE_DIR/TuxSCP-${VERSION}-${ARCH}.AppImage"

echo "==> Building TuxSCP v${VERSION} AppImage (${ARCH})"

# ── 1. Build release binary ───────────────────────────────────────────────────
echo "--> Compiling release binary..."
cd "$ROOT_DIR"
cargo build --release
BINARY="$ROOT_DIR/target/release/$PKG_NAME"

# ── 2. Generate icon ──────────────────────────────────────────────────────────
echo "--> Generating icons..."
cd "$SCRIPT_DIR"
python3 gen-icon.py 2>/dev/null || echo "   (icon generation skipped)"

# ── 3. Assemble AppDir ───────────────────────────────────────────────────────
echo "--> Assembling AppDir..."
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin"
mkdir -p "$APPDIR/usr/share/applications"
mkdir -p "$APPDIR/usr/share/icons/hicolor/256x256/apps"

install -m 755 "$BINARY" "$APPDIR/usr/bin/$PKG_NAME"
install -m 644 "$SCRIPT_DIR/tuxscp.desktop" "$APPDIR/usr/share/applications/"

# AppImage requires the .desktop and icon at the AppDir root
cp "$SCRIPT_DIR/tuxscp.desktop" "$APPDIR/"

if [[ -f "$SCRIPT_DIR/icons/tuxscp_256.png" ]]; then
    cp "$SCRIPT_DIR/icons/tuxscp_256.png" "$APPDIR/tuxscp.png"
    cp "$SCRIPT_DIR/icons/tuxscp_256.png" \
       "$APPDIR/usr/share/icons/hicolor/256x256/apps/tuxscp.png"
elif [[ -f "$SCRIPT_DIR/icons/tuxscp.svg" ]]; then
    cp "$SCRIPT_DIR/icons/tuxscp.svg" "$APPDIR/tuxscp.svg"
fi

# AppRun entry point
cat > "$APPDIR/AppRun" << 'EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
export PATH="$HERE/usr/bin:$PATH"
exec "$HERE/usr/bin/tuxscp" "$@"
EOF
chmod +x "$APPDIR/AppRun"

# ── 4. Optionally bundle OpenSSL libs ────────────────────────────────────────
# If running in a container with older glibc, bundle necessary libs.
BUNDLE_LIBS="${BUNDLE_LIBS:-0}"
if [[ "$BUNDLE_LIBS" == "1" ]]; then
    echo "--> Bundling shared libraries..."
    mkdir -p "$APPDIR/usr/lib"
    for lib in libssl.so.3 libcrypto.so.3; do
        lib_path="$(ldconfig -p | grep "$lib" | awk '{print $NF}' | head -1)"
        if [[ -n "$lib_path" ]]; then
            cp "$lib_path" "$APPDIR/usr/lib/"
            echo "   Bundled: $lib"
        fi
    done
    # Patch AppRun to add lib path
    sed -i 's|exec |export LD_LIBRARY_PATH="$HERE/usr/lib:$LD_LIBRARY_PATH"\nexec |' "$APPDIR/AppRun"
fi

# ── 5. Download appimagetool ──────────────────────────────────────────────────
TOOL="$RELEASE_DIR/appimagetool"
if [[ ! -x "$TOOL" ]]; then
    echo "--> Downloading appimagetool..."
    TOOL_ARCH="$ARCH"
    [[ "$ARCH" == "x86_64" ]] && TOOL_ARCH="x86_64"
    [[ "$ARCH" == "aarch64" ]] && TOOL_ARCH="aarch64"
    curl -fsSL \
        "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-${TOOL_ARCH}.AppImage" \
        -o "$TOOL"
    chmod +x "$TOOL"
fi

# ── 6. Build AppImage ────────────────────────────────────────────────────────
echo "--> Building AppImage..."
mkdir -p "$RELEASE_DIR"
ARCH="$ARCH" "$TOOL" --no-appstream "$APPDIR" "$APPIMAGE_FILE" 2>&1

chmod +x "$APPIMAGE_FILE"

echo ""
echo "✔  AppImage built: $APPIMAGE_FILE"
echo "   Run with:       chmod +x $APPIMAGE_FILE && $APPIMAGE_FILE"
