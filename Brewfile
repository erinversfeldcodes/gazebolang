# Host-level tooling this project needs on macOS, installed with:
#   brew bundle install
#
# This is the bootstrap layer only. Once these are installed, `nix develop`
# provides the actual Rust toolchain and build tools (see flake.nix) — don't
# duplicate those here.
#
# Rust itself is managed by rustup + rust-toolchain.toml, not Homebrew:
# install rustup from https://rustup.rs (or `brew install rustup-init` then
# run `rustup-init`) rather than Homebrew's standalone `rust` formula, which
# doesn't respect this repo's toolchain file.
#
# Nix (for `nix develop`) isn't installed via Homebrew either — it needs its
# own installer for the dedicated volume + daemon setup on macOS.
# See https://nixos.org/download.

brew "just"           # task runner — see justfile
brew "gh"             # GitHub CLI
brew "flyctl"         # Fly.io deploys and PR previews
brew "colima"         # Docker runtime for macOS (no Docker Desktop needed)
brew "docker"         # Docker CLI
brew "docker-compose" # `docker compose` — local Postgres, see docker-compose.yml
brew "docker-buildx"  # `docker build` — builds the Dockerfile
brew "node"           # needed for `npx neon@latest` (Neon project/branch setup)
brew "cmake"          # builds llama.cpp, vendored by crates/importer's llama-cpp-2 dependency
