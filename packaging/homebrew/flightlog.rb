class Flightlog < Formula
  desc "Record coding-agent sessions as portable, redacted, resumable bundles"
  homepage "https://flightlog.sh"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/skrcka/flightlog/releases/download/v0.2.1/flightlog-aarch64-apple-darwin.tar.gz"
      sha256 "e63ba7c1cb51f1c2f13a3ce8b37c00abd7f3b437bf615befc1211b85d8112d67"
    end
    on_intel do
      url "https://github.com/skrcka/flightlog/releases/download/v0.2.1/flightlog-x86_64-apple-darwin.tar.gz"
      sha256 "661e7f533aae046ca0d6954cfa926c31808fe804e0b77231c3ea7a1bf39568c4"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/skrcka/flightlog/releases/download/v0.2.1/flightlog-aarch64-unknown-linux-musl.tar.gz"
      sha256 "01fe82fc4e253f8fbca105009e3145eb5131981886bb864077796ad4699cef2d"
    end
    on_intel do
      url "https://github.com/skrcka/flightlog/releases/download/v0.2.1/flightlog-x86_64-unknown-linux-musl.tar.gz"
      sha256 "eac2a6802b7b43092980b669a6081f6a7322b4c2bcc5b231d93590df86886fff"
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
