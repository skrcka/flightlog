#!/bin/sh
# Install flightlog: https://github.com/skrcka/flightlog
#
#   curl -fsSL https://flightlog.sh/install.sh | sh
#
# FLIGHTLOG_VERSION=0.1.0   pin a version (default: latest release)
# FLIGHTLOG_INSTALL_DIR=…   where the binary goes (default: ~/.local/bin)
set -eu

REPO="skrcka/flightlog"
VERSION="${FLIGHTLOG_VERSION:-latest}"
BIN_DIR="${FLIGHTLOG_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf 'flightlog: %s\n' "$*"; }
die() { printf 'flightlog: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "'$1' is required"; }
need curl
need tar

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)               target=aarch64-apple-darwin ;;
  Darwin-x86_64)              target=x86_64-apple-darwin ;;
  Linux-x86_64)               target=x86_64-unknown-linux-musl ;;
  Linux-aarch64|Linux-arm64)  target=aarch64-unknown-linux-musl ;;
  *) die "no prebuilt binary for $(uname -s) $(uname -m); build from source: cargo install flightlog" ;;
esac

if [ "$VERSION" = latest ]; then
  base="https://github.com/$REPO/releases/latest/download"
else
  base="https://github.com/$REPO/releases/download/v${VERSION#v}"
fi
asset="flightlog-$target.tar.gz"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM
say "downloading $asset ($VERSION)"
curl -fsSL "$base/$asset" -o "$tmp/$asset" || die "download failed: $base/$asset"
curl -fsSL "$base/$asset.sha256" -o "$tmp/$asset.sha256" || die "checksum download failed"

expected=$(cut -d' ' -f1 < "$tmp/$asset.sha256")
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$tmp/$asset" | cut -d' ' -f1)
else
  actual=$(shasum -a 256 "$tmp/$asset" | cut -d' ' -f1)
fi
[ "$expected" = "$actual" ] || die "checksum mismatch for $asset"

tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$BIN_DIR"
install -m 755 "$tmp/flightlog" "$BIN_DIR/flightlog" 2>/dev/null || { cp "$tmp/flightlog" "$BIN_DIR/flightlog"; chmod 755 "$BIN_DIR/flightlog"; }
say "installed $("$BIN_DIR/flightlog" --version) to $BIN_DIR"

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) say "add $BIN_DIR to your PATH, e.g.:  echo 'export PATH=\"$BIN_DIR:\$PATH\"' >> ~/.profile" ;;
esac
