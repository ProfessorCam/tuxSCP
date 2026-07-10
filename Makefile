.PHONY: all build release deb appimage clean test fmt check install uninstall

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
appimage: release
	@echo "Building AppImage v$(VERSION)..."
	@bash packaging/build-appimage.sh
	@ls -lh release/*.AppImage

# Build both packages
packages: deb appimage

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
