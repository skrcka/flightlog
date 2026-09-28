#!/bin/sh
# Regenerate the Homebrew formula and the AUR package for a release:
#
#   packaging/update.sh 0.2.0
#
# Reads the checksums from the release's *.sha256 assets, so run it after the
# release workflow has finished.
set -eu
version="${1:?usage: packaging/update.sh <version>}"
version="${version#v}"
here=$(cd "$(dirname "$0")" && pwd)
base="https://github.com/skrcka/flightlog/releases/download/v$version"
sum() { curl -fsSL "$base/flightlog-$1.sha256" | cut -d' ' -f1; }

mac_arm=$(sum aarch64-apple-darwin)
mac_x86=$(sum x86_64-apple-darwin)
lin_arm=$(sum aarch64-unknown-linux-musl)
lin_x86=$(sum x86_64-unknown-linux-musl)
desc="Record coding-agent sessions as portable, redacted, resumable bundles"

cat > "$here/homebrew/flightlog.rb" <<RUBY
class Flightlog < Formula
  desc "$desc"
  homepage "https://flightlog.sh"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/skrcka/flightlog/releases/download/v$version/flightlog-aarch64-apple-darwin.tar.gz"
      sha256 "$mac_arm"
    end
    on_intel do
      url "https://github.com/skrcka/flightlog/releases/download/v$version/flightlog-x86_64-apple-darwin.tar.gz"
      sha256 "$mac_x86"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/skrcka/flightlog/releases/download/v$version/flightlog-aarch64-unknown-linux-musl.tar.gz"
      sha256 "$lin_arm"
    end
    on_intel do
      url "https://github.com/skrcka/flightlog/releases/download/v$version/flightlog-x86_64-unknown-linux-musl.tar.gz"
      sha256 "$lin_x86"
    end
  end

  def install
    bin.install "flightlog"
    man1.install Dir["man/*.1"]
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/flightlog --version")
  end
end
RUBY

cat > "$here/aur/PKGBUILD" <<PKG
# Maintainer: flightlog authors <https://github.com/skrcka/flightlog>
pkgname=flightlog-bin
pkgver=$version
pkgrel=1
pkgdesc="$desc"
arch=('x86_64' 'aarch64')
url="https://flightlog.sh"
license=('MIT' 'Apache-2.0')
provides=('flightlog')
conflicts=('flightlog')
options=('!strip' '!debug')
source_x86_64=("flightlog-\$pkgver-x86_64.tar.gz::https://github.com/skrcka/flightlog/releases/download/v\$pkgver/flightlog-x86_64-unknown-linux-musl.tar.gz")
source_aarch64=("flightlog-\$pkgver-aarch64.tar.gz::https://github.com/skrcka/flightlog/releases/download/v\$pkgver/flightlog-aarch64-unknown-linux-musl.tar.gz")
sha256sums_x86_64=('$lin_x86')
sha256sums_aarch64=('$lin_arm')

package() {
  install -Dm755 flightlog "\$pkgdir/usr/bin/flightlog"
  install -Dm644 -t "\$pkgdir/usr/share/man/man1" man/*.1
  install -Dm644 LICENSE-MIT "\$pkgdir/usr/share/licenses/\$pkgname/LICENSE-MIT"
  install -Dm644 LICENSE-APACHE "\$pkgdir/usr/share/licenses/\$pkgname/LICENSE-APACHE"
}
PKG

# What \`makepkg --printsrcinfo\` prints for this PKGBUILD.
cat > "$here/aur/.SRCINFO" <<SRC
pkgbase = flightlog-bin
	pkgdesc = $desc
	pkgver = $version
	pkgrel = 1
	url = https://flightlog.sh
	arch = x86_64
	arch = aarch64
	license = MIT
	license = Apache-2.0
	provides = flightlog
	conflicts = flightlog
	options = !strip
	options = !debug
	source_x86_64 = flightlog-$version-x86_64.tar.gz::https://github.com/skrcka/flightlog/releases/download/v$version/flightlog-x86_64-unknown-linux-musl.tar.gz
	sha256sums_x86_64 = $lin_x86
	source_aarch64 = flightlog-$version-aarch64.tar.gz::https://github.com/skrcka/flightlog/releases/download/v$version/flightlog-aarch64-unknown-linux-musl.tar.gz
	sha256sums_aarch64 = $lin_arm

pkgname = flightlog-bin
SRC
echo "updated homebrew/flightlog.rb, aur/PKGBUILD, aur/.SRCINFO for $version"
