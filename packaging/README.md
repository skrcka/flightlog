# Packaging

| Channel | How | Needs |
|---|---|---|
| Install script | `site/install.sh` via GitHub Pages at flightlog.sh; downloads the release archive and checks its SHA-256 | nothing per release |
| GitHub Releases | `.github/workflows/release.yml` on a `v*` tag builds macOS/Linux × x86-64/arm64 archives with `.sha256` files | nothing per release |
| crates.io | `cargo publish` | a crates.io API token (`cargo login`) |
| Homebrew | `homebrew/flightlog.rb` into a tap repo `skrcka/homebrew-tap` → `brew install skrcka/tap/flightlog` | the tap repo; fill version + sha256 per release |
| AUR | `aur/PKGBUILD` as `flightlog-bin` | an AUR account with an SSH key |

Release steps: bump `version` in `Cargo.toml` and `CHANGELOG.md`, tag `vX.Y.Z`,
push the tag; when the release workflow finishes, `cargo publish` and update
the formula and PKGBUILD from the `.sha256` assets.
