# Homebrew formula for a tap repository (github.com/skrcka/homebrew-tap,
# Formula/flightlog.rb). Update `version` and the four sha256 values from the
# release's *.sha256 files on each release.
class Flightlog < Formula
  desc "Record coding-agent sessions as portable, redacted, resumable bundles"
  homepage "https://flightlog.sh"
  version "0.1.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/skrcka/flightlog/releases/download/v#{version}/flightlog-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_SHA256"
    end
    on_intel do
      url "https://github.com/skrcka/flightlog/releases/download/v#{version}/flightlog-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_SHA256"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/skrcka/flightlog/releases/download/v#{version}/flightlog-aarch64-unknown-linux-musl.tar.gz"
      sha256 "REPLACE_WITH_SHA256"
    end
    on_intel do
      url "https://github.com/skrcka/flightlog/releases/download/v#{version}/flightlog-x86_64-unknown-linux-musl.tar.gz"
      sha256 "REPLACE_WITH_SHA256"
    end
  end

  def install
    bin.install "flightlog"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/flightlog --version")
  end
end
