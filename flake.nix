{
  description = "Dev environment for the flashcard app";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

        # Pinned to match rust-toolchain.toml / CI's dtolnay/rust-toolchain
        # steps exactly, not `.latest` — "latest" drifts independently of
        # both, which is how local/Nix/CI ended up on three different
        # rustc versions despite all three claiming to track "stable".
        rustToolchain = pkgs.rust-bin.stable."1.99.1".default.override {
          extensions = [ "rust-src" "rust-analyzer" "clippy" "rustfmt" ];
          targets = [ "wasm32-unknown-unknown" ];
        };

        nativeBuildTools = with pkgs; [
          rustToolchain
          cargo-leptos
          cargo-nextest
          cargo-watch
          sqlx-cli
          binaryen # wasm-opt, used by cargo-leptos's release profile
          just
          flyctl
          pkg-config
          cmake # builds llama.cpp, vendored by crates/importer's llama-cpp-2 dependency
        ] ++ lib.optionals stdenv.isLinux [ mold ];

        buildLibs = with pkgs; [ openssl ]
          ++ lib.optionals stdenv.isDarwin [
            darwin.apple_sdk.frameworks.Security
            darwin.apple_sdk.frameworks.SystemConfiguration
          ];
      in
      {
        devShells.default = pkgs.mkShell {
          packages = nativeBuildTools ++ buildLibs;

          # sqlx-cli / sqlx macros need this at compile time if you use the
          # offline-checked query macros against a live DB.
          shellHook = ''
            export PKG_CONFIG_PATH="${pkgs.openssl.dev}/lib/pkgconfig:$PKG_CONFIG_PATH"
          '';
        };
      });
}
