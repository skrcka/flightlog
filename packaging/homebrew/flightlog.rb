class Flightlog < Formula
  desc "Record coding-agent sessions as portable, redacted, resumable bundles"
  homepage "https://flightlog.sh"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/skrcka/flightlog/releases/download/v0.2.0/flightlog-aarch64-apple-darwin.tar.gz"
      sha256 "2fd18f044397e7bfa1611838dbe4357de3dae4b2e9bb97a460cd5b29152dcfad"
    end
    on_intel do
      url "https://github.com/skrcka/flightlog/releases/download/v0.2.0/flightlog-x86_64-apple-darwin.tar.gz"
      sha256 "5d87bd3dc23ccc2eb07bc4b162fd42c59bf93850fbd8ce5efdb529613d18c24e"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/skrcka/flightlog/releases/download/v0.2.0/flightlog-aarch64-unknown-linux-musl.tar.gz"
      sha256 "aa9d062b18dc4ccec2382231e841043fa84cf764e57cd406b30fd3b2c640c12e"
    end
    on_intel do
      url "https://github.com/skrcka/flightlog/releases/download/v0.2.0/flightlog-x86_64-unknown-linux-musl.tar.gz"
      sha256 "f6475b5ecad21534decaa48d92cd24858dc0e9b3390e18829a5c2a77b46f6a3f"
    end
  end

  def install
    bin.install "flightlog"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/flightlog --version")
  end
end
