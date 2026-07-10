#!/usr/bin/env bash
# build-rpm.sh — builds an .rpm package for TuxSCP (Fedora / RHEL / openSUSE).
# Packages the already-built release binary; run `cargo build --release` first
# (the Makefile `rpm` target does this for you).
# Usage: ./packaging/build-rpm.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

PKG_NAME="tuxscp"
VERSION="$(grep '^version' "$ROOT_DIR/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')"
ARCH="$(uname -m)"
RELEASE_DIR="$ROOT_DIR/release"
TOP="$RELEASE_DIR/rpmbuild"
BINARY="$ROOT_DIR/target/release/$PKG_NAME"

echo "==> Building TuxSCP v${VERSION} .rpm (${ARCH})"

if [[ ! -f "$BINARY" ]]; then
  echo "--> Compiling release binary..."
  (cd "$ROOT_DIR" && cargo build --release)
fi

rm -rf "$TOP"
mkdir -p "$TOP"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}
SPEC="$TOP/SPECS/$PKG_NAME.spec"
DATE="$(LC_ALL=C date '+%a %b %d %Y')"

cat > "$SPEC" << EOF
%global src_root $ROOT_DIR
# The binary is prebuilt and stripped — skip the debuginfo subpackage.
%global debug_package %{nil}

Name:           $PKG_NAME
Version:        $VERSION
Release:        1%{?dist}
Summary:        Native Linux SSH/SFTP/SCP/FTP client
License:        MIT
URL:            https://github.com/ProfessorCam/tuxSCP
Requires:       openssl-libs

%description
TuxSCP is a native Linux file-transfer client supporting SFTP, SCP, FTP and
FTPS. It provides a dual-pane file manager for local and remote files, a
transfer queue with progress and cancellation, a session manager, and a
light/dark Office-inspired interface.

%install
rm -rf %{buildroot}
install -Dm0755 %{src_root}/target/release/$PKG_NAME %{buildroot}%{_bindir}/$PKG_NAME
install -Dm0644 %{src_root}/packaging/tuxscp.desktop %{buildroot}%{_datadir}/applications/$PKG_NAME.desktop
for s in 16 32 48 64 128 256 512; do
    png="%{src_root}/packaging/icons/tuxscp_\${s}.png"
    if [ -f "\$png" ]; then
        install -Dm0644 "\$png" %{buildroot}%{_datadir}/icons/hicolor/\${s}x\${s}/apps/$PKG_NAME.png
    fi
done

%files
%{_bindir}/$PKG_NAME
%{_datadir}/applications/$PKG_NAME.desktop
%{_datadir}/icons/hicolor/*/apps/$PKG_NAME.png

%post
/bin/touch --no-create %{_datadir}/icons/hicolor &>/dev/null || :
/usr/bin/gtk-update-icon-cache -qtf %{_datadir}/icons/hicolor &>/dev/null || :
/usr/bin/update-desktop-database -q &>/dev/null || :

%postun
/usr/bin/gtk-update-icon-cache -qtf %{_datadir}/icons/hicolor &>/dev/null || :
/usr/bin/update-desktop-database -q &>/dev/null || :

%changelog
* $DATE TuxSCP Contributors <tuxscp@users.noreply.github.com> - $VERSION-1
- Multi-protocol (SFTP/SCP/FTP/FTPS), recursive transfers, dark mode,
  Back/Forward/Up navigation, overwrite confirmation.
EOF

echo "--> Running rpmbuild..."
rpmbuild -bb --define "_topdir $TOP" "$SPEC"

mkdir -p "$RELEASE_DIR"
find "$TOP/RPMS" -name '*.rpm' -exec cp -f {} "$RELEASE_DIR/" \;

echo ""
echo "✔  Package(s) built:"
ls -1 "$RELEASE_DIR"/*.rpm
echo "   Install with:  sudo dnf install ./release/${PKG_NAME}-${VERSION}-1.*.${ARCH}.rpm"
