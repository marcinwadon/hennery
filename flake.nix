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

  outputs = { self, nixpkgs, nixpkgs-web, flake-utils, crane, advisory-db, ... }:
    # The v1 platforms (distribution spec §1): Intel Macs are not one.
    flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ] (system:
      let
        pkgs = import nixpkgs { inherit system; overlays = [ (import ./nix/node-security-overlay.nix) ]; };
        web = import nixpkgs-web { inherit system; overlays = [ (import ./nix/node-security-overlay.nix) ]; };
        # The web UI (plan 7e-ii-b), built with nixpkgs-web's Node and pnpm.
        webUi = import ./nix/web.nix { pkgs = web; };
        hennery = import ./nix/package.nix {
          inherit pkgs crane advisory-db webUi;
          sourceRevision = self.rev or "unrecorded (dirty source)";
        };
        # Chromium alone (headless): what the browser checks run on.
        browsers = web.playwright-driver.browsers.override {
          withChromium = false;
          withChromiumHeadlessShell = true;
          withFirefox = false;
          withWebkit = false;
          withFfmpeg = false;
        };
        # The pnpm the web build is pinned to (`nix/web.nix`).
        webTools = [ web.nodejs_24 webUi.pnpm ];
      in {
        packages.default = hennery.package;
        packages.web = webUi;
        checks = hennery.checks;
        # Building the web UI only (CI's Rust and release jobs): no browsers.
        devShells.web = pkgs.mkShell { packages = webTools; };
        # Offline release-policy tests and verification tools, pinned
        # by flake.lock. Publication credentials are never supplied here.
        devShells.release = pkgs.mkShell {
          packages = [
            (pkgs.python3.withPackages (ps: [ ps.pyyaml ]))
            pkgs.actionlint
            pkgs.cargo-audit
            pkgs.skopeo
            pkgs.shellcheck
            pkgs.gh
            pkgs.git
            pkgs.gnupg
            pkgs.jq
            web.nodejs_24
            pkgs.oras
          ];
        };
        # R4's non-publishing OCI/registry contract tests. Uses the same pinned
        # nixpkgs as the Rust checks; Docker Engine itself is provided by CI.
        devShells.image = pkgs.mkShell {
          packages = with pkgs; [ (python3.withPackages (p: [ p.pyyaml ])) actionlint shellcheck skopeo ];
        };
        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            cargo-nextest
            rustc
            clippy
            rustfmt
            rust-analyzer
            git
            # R3: pinned binstall acceptance and an isolated MSRV toolchain.
            actionlint
            cargo-binstall
            rustup
            sqlite
            # `webauthn-rs` links OpenSSL (plan 3c).
            openssl
            pkg-config
            # The PATH-capture tests run each login shell (plan 7c).
            bashInteractive
            zsh
            fish
            # Execute the shipped same-machine TLS proxy recipes (W2).
            caddy
            nginx
          ] ++ webTools;
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
          # Playwright uses the flake's browsers, never a download of its own.
          PLAYWRIGHT_BROWSERS_PATH = "${browsers}";
          PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD = "1";
          PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS = "true";
        };
      });
}
