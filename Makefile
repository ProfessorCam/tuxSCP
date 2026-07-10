.PHONY: all build release deb rpm appimage packages clean test fmt check install uninstall

BINARY   := tuxscp
PREFIX   ?= /usr/local
VERSION  := $(shell grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')
ARCH     := $(shell uname -m)

all: build

# Development build
build:
	cargo build

# Optimised release binary only
release:
	cargo build --release

# Run in development mode
run:
	cargo run

# Build .deb package (requires dpkg-deb)
deb: release
	@echo "Building .deb package v$(VERSION)..."
	@bash packaging/build-deb.sh
	@ls -lh release/*.deb

# Build .AppImage (downloads appimagetool automatically)
# Build .rpm package (requires rpmbuild)
rpm: release
	@echo "Building .rpm package v$(VERSION)..."
	@bash packaging/build-rpm.sh
	@ls -lh release/*.rpm

appimage: release
	@echo "Building AppImage v$(VERSION)..."
	@BUNDLE_LIBS=1 bash packaging/build-appimage.sh
	@ls -lh release/*.AppImage

# Build all packages
packages: deb rpm appimage

# Install to PREFIX (default /usr/local)
install: release
	install -Dm755 target/release/$(BINARY)       $(DESTDIR)$(PREFIX)/bin/$(BINARY)
	install -Dm644 packaging/tuxscp.desktop      $(DESTDIR)$(PREFIX)/share/applications/tuxscp.desktop
	@if [ -f packaging/icons/tuxscp_256.png ]; then \
	    install -Dm644 packaging/icons/tuxscp_256.png \
	        $(DESTDIR)$(PREFIX)/share/icons/hicolor/256x256/apps/tuxscp.png; \
	fi

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/$(BINARY)
	rm -f $(DESTDIR)$(PREFIX)/share/applications/tuxscp.desktop
	rm -f $(DESTDIR)$(PREFIX)/share/icons/hicolor/256x256/apps/tuxscp.png

test:
	cargo test

fmt:
	cargo fmt

check:
	cargo clippy -- -D warnings

clean:
	cargo clean
	rm -rf release/deb-staging release/TuxSCP.AppDir
