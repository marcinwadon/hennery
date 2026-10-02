# The home-manager module (plan 7e-ii-a, distribution spec §4.3):
# `services.hennery.{collector,host}` as systemd user services, the units
# `hennery service install` writes (plan 7c, §6.3): `hennery-collector`
# and `hennery-host`, in `~/.config/systemd/user`, where `hennery doctor`
# reads them, with their PATH in `~/.config/hennery/service.env`, which
# doctor's check 5 reads (and, as the module's PATH is not the login
# shell's, may say has drifted: change it with `host.path`). Use this
# module or `hennery service install`, not both: the module's files are
# read-only links into the store. Linux only: on macOS, `hennery service
# install` writes the launchd agent.
#
# As there, a user service runs only while the user is logged in unless
# linger is on (`loginctl enable-linger`). A host without a pairing is
# skipped, not crash-looped; a revoked one (exit 78) is not restarted.
{ henneryFor }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  common = import ./common.nix { inherit lib; };
  cfg = config.services.hennery;
  exe = lib.getExe cfg.package;
  dataHome = config.xdg.dataHome;
  # The agents' PATH: what `host.path` gives, the user's and the system's
  # profiles, then the base system's directories, as plan 7c's captured
  # PATH ends. `sh` and `git` come from these; doctor's check 5 says when
  # either is missing.
  path = lib.concatStringsSep ":" (
    map (p: "${p}/bin") cfg.host.path
    ++ [
      "${config.home.profileDirectory}/bin"
      "/etc/profiles/per-user/${config.home.username}/bin"
      "/run/wrappers/bin"
      "/run/current-system/sw/bin"
      "/nix/var/nix/profiles/default/bin"
      "/usr/bin"
      "/bin"
    ]
  );
  # Where `hennery service install` writes the PATH, in its format
  # (`unit::env_file`), so doctor's check 5 reads it back.
  envFile = "${config.xdg.configHome}/hennery/service.env";
  envQuoted =
    text: "\"" + lib.replaceStrings [ "\\" "\"" "$" "`" ] [ "\\\\" "\\\"" "\\$" "\\`" ] text + "\"";
  unit = role: argv: extra: {
    Unit = {
      Description = "hennery ${role}";
      StartLimitIntervalSec = 300;
      StartLimitBurst = 10;
    } // extra.Unit or { };
    Service = {
      Type = "exec";
      ExecStart = common.execStart argv;
      EnvironmentFile = common.unitPath envFile;
      Environment = common.envWord "HENNERY_SERVICE=systemd";
      Restart = "on-failure";
      RestartSec = 3;
      KillMode = "mixed";
      TimeoutStopSec = 30;
    } // extra.Service or { };
    Install.WantedBy = [ "default.target" ];
  };
in
{
  options.services.hennery = common.options {
    defaultPackage = henneryFor pkgs.stdenv.hostPlatform.system;
    adapterPackages = pkgs.callPackage ../adapters.nix { };
    dataDirs = {
      collector = {
        default = "${dataHome}/hennery/collector";
        text = "Made 0700 by the collector.";
        listen = "";
      };
      host = {
        default = "${dataHome}/hennery/host";
        text = "Made 0700 by `hennery host join`.";
        join = "`hennery host join <url> --data-dir <this>`";
        start = "`systemctl --user start hennery-host`";
      };
    };
    defaultSource = "nix";
    defaultSourceText = lib.literalExpression ''"nix"'';
  };

  config = lib.mkIf (cfg.collector.enable || cfg.host.enable) {
    assertions = [
      {
        assertion = pkgs.stdenv.hostPlatform.isLinux;
        message = "services.hennery: the home-manager module writes systemd user services, on Linux only; on macOS run `hennery service install`.";
      }
    ];
    home.packages = [ cfg.package ];
    xdg.configFile."hennery/service.env".text = ''
      # Written by the home-manager module of hennery: the services' PATH.
      PATH=${envQuoted path}
    '';
    systemd.user.services = lib.mkMerge [
      (lib.mkIf cfg.collector.enable {
        hennery-collector = unit "collector" (common.collectorArgv exe cfg.collector) { };
      })
      (lib.mkIf cfg.host.enable {
        hennery-host = unit "host" (common.hostArgv exe cfg.host) {
          Unit.ConditionPathExists = common.unitPath "${cfg.host.dataDir}/host.key";
          Service.RestartPreventExitStatus = 78;
        };
      })
    ];
  };
}
