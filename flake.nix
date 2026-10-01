{
  description = "hennery: self-hosted cockpit for ACP coding agents";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { nixpkgs, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let pkgs = import nixpkgs { inherit system; };
      in {
        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
            nodejs_24
            pnpm
            sqlite
            # `webauthn-rs` links OpenSSL (plan 3c).
            openssl
            pkg-config
            # The PATH-capture tests run each login shell (plan 7c).
            bashInteractive
            zsh
            fish
          ];
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        };
      });
}
