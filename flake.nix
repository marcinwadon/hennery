{
  description = "hennery: self-hosted cockpit for ACP coding agents";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    # The package and its checks (plan 7e-i, distribution spec §4.3).
    crane.url = "github:ipetkov/crane/v0.24.0";
    advisory-db = {
      url = "github:rustsec/advisory-db";
      flake = false;
    };
  };

  outputs = { nixpkgs, flake-utils, crane, advisory-db, ... }:
    # The v1 platforms (distribution spec §1): Intel Macs are not one.
    flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ] (system:
      let
        pkgs = import nixpkgs { inherit system; };
        hennery = import ./nix/package.nix { inherit pkgs crane advisory-db; };
      in {
        packages.default = hennery.package;
        checks = hennery.checks;
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
