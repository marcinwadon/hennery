# The flake's outputs for one system from plan 7e-ii-a: the pinned adapters
# (`nix/adapters.nix`) and their checks. Kept apart from `flake.nix` so the
# flake names them in three lines.
{
  pkgs,
  nixpkgs,
}:
let
  inherit (nixpkgs) lib;
  adapters = pkgs.callPackage ./adapters.nix { };
  adapterCheck = pkgs.callPackage ./adapter-check.nix { };
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
  };
}
