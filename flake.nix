{
  description = "hennery: self-hosted cockpit for ACP coding agents";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # The web UI's tools (plan 4b): Node, pnpm and Playwright's browsers,
    # pinned apart from the Rust side, so a bump of either never drags the
    # other along. `@playwright/test` in web/package.json must be exactly
    # this nixpkgs' `playwright-driver` version.
    nixpkgs-web.url = "github:NixOS/nixpkgs/e158d9ed9b51c98974c5e66e1ba1c9e0255fecaa";
    flake-utils.url = "github:numtide/flake-utils";
    # The package and its checks (plan 7e-i, distribution spec §4.3).
    crane.url = "github:ipetkov/crane/v0.24.0";
    advisory-db = {
      url = "github:rustsec/advisory-db";
      flake = false;
    };
  };

  outputs = { nixpkgs, nixpkgs-web, flake-utils, crane, advisory-db, ... }:
    # The v1 platforms (distribution spec §1): Intel Macs are not one.
    flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ] (system:
      let
        pkgs = import nixpkgs { inherit system; };
        web = import nixpkgs-web { inherit system; };
        hennery = import ./nix/package.nix { inherit pkgs crane advisory-db; };
        # Chromium alone (headless): what the browser checks run on.
        browsers = web.playwright-driver.browsers.override {
          withChromium = false;
          withChromiumHeadlessShell = true;
          withFirefox = false;
          withWebkit = false;
          withFfmpeg = false;
        };
        webTools = [ web.nodejs_24 web.pnpm ];
        # The pinned adapters and their checks (plan 7e-ii-a).
        nixOutputs = import ./nix/outputs.nix { inherit pkgs nixpkgs; };
      in {
        packages.default = hennery.package;
        # The free adapter. The Claude adapter is unfree (distribution spec
        # §3.3), so it is no package of the flake's, which `nix flake check`
        # would evaluate: `nix build .#claude-acp` builds it once the
        # operator accepts its licence (`NIXPKGS_ALLOW_UNFREE=1 --impure`).
        packages.codex-acp = nixOutputs.codex-acp;
        legacyPackages = nixOutputs.legacyPackages;
        checks = hennery.checks // nixOutputs.checks;
        # Building the web UI only (CI's Rust and release jobs): no browsers.
        devShells.web = pkgs.mkShell { packages = webTools; };
        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
            sqlite
            # `webauthn-rs` links OpenSSL (plan 3c).
            openssl
            pkg-config
            # The PATH-capture tests run each login shell (plan 7c).
            bashInteractive
            zsh
            fish
          ] ++ webTools;
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
          # Playwright uses the flake's browsers, never a download of its own.
          PLAYWRIGHT_BROWSERS_PATH = "${browsers}";
          PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD = "1";
          PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS = "true";
        };
      });
}
