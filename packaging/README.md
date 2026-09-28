# Packaging

| Channel | How | Needs |
|---|---|---|
| Install script | `site/install.sh` via GitHub Pages at flightlog.sh; downloads the release archive and checks its SHA-256 | nothing per release |
| GitHub Releases | `.github/workflows/release.yml` on a `v*` tag builds macOS, Linux and Windows × x86-64/arm64 archives with `.sha256` files | nothing per release |
| crates.io | `cargo publish` | a crates.io API token (`cargo login`) |
| Homebrew | `homebrew/flightlog.rb` into a tap repo `skrcka/homebrew-tap` → `brew install skrcka/tap/flightlog` | the tap repo; fill version + sha256 per release |
| AUR | `aur/PKGBUILD` as `flightlog-bin` | an AUR account with an SSH key |

Release steps: bump `version` in `Cargo.toml` and `CHANGELOG.md`, tag `vX.Y.Z`,
push the tag. The release workflow builds the archives and publishes to
crates.io (secret `CARGO_REGISTRY_TOKEN`). Then:

```sh
packaging/update.sh X.Y.Z     # formula, PKGBUILD and .SRCINFO from the release's .sha256 files
```

- Homebrew: copy `homebrew/flightlog.rb` to `Formula/flightlog.rb` in
  `skrcka/homebrew-tap` and push. Checked with `brew audit --strict` and
  `brew test`.
- AUR: copy `aur/PKGBUILD` and `aur/.SRCINFO` into a clone of
  `ssh://aur@aur.archlinux.org/flightlog-bin.git` and push. `.SRCINFO` is
  written by the script and matches `makepkg --printsrcinfo`.
