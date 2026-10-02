# The flake's outputs for one system from plan 7e-ii-a: the pinned adapters
# (`nix/adapters.nix`), their checks, and the modules' checks. Kept apart
# from `flake.nix` so the flake names them in three lines.
{
  pkgs,
  nixpkgs,
  system,
  hennery,
  nixosModule,
  homeManagerModule,
}:
let
  inherit (nixpkgs) lib;
  adapters = pkgs.callPackage ./adapters.nix { };
  adapterCheck = pkgs.callPackage ./adapter-check.nix { };
  # Gated on the system's name, never on `pkgs.stdenv`.
  linux = lib.hasSuffix "-linux" system;
  modules = import ./tests/modules.nix {
    inherit
      pkgs
      nixpkgs
      hennery
      nixosModule
      homeManagerModule
      ;
  };
in
{
  codex-acp = adapters.codex;
  # Not evaluated by `nix flake check`: the Claude adapter needs the
  # operator's consent to its licence.
  legacyPackages = {
    claude-acp = adapters.claude;
    claude-acp-runs = adapterCheck adapters.claude;
  };
  checks = {
    codex-acp-runs = adapterCheck adapters.codex;
    unpack-refuses = pkgs.callPackage ./unpack-check.nix { };
    adapter-check-cases = pkgs.callPackage ./adapter-check-cases.nix { inherit adapterCheck; };
  }
  // lib.optionalAttrs linux {
    inherit (modules) modules-eval home-manager-doctor;
  }
  // lib.optionalAttrs (system == "x86_64-linux") {
    # Virtual machines need KVM, which only the x86_64 runner has.
    nixos = import ./tests/nixos.nix { inherit pkgs nixosModule; };
  };
}
