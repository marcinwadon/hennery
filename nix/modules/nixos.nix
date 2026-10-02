# The NixOS module (plan 7e-ii-a, distribution spec §4.3):
# `services.hennery.{collector,host}` as system services.
#
# - The collector runs as a system user of its own, `hennery` (kernel spec
#   §10: a separate OS user is what protects its credentials from agents),
#   in a hardened unit.
# - The host runs as the login user it serves (`host.user`): its agents use
#   that user's home, credentials and projects.
#
# Both units keep plan 7c's service model: `HENNERY_SERVICE=systemd` (each
# process writes its own rotating log, here in a `LogsDirectory=`), a
# restart on failure under a start limit, `KillMode=mixed` so the host stops
# its adapters itself, and the binary's `--data-dir` on the command line.
# A host revoked by its collector exits 78 and is not restarted (distribution
# spec §5.2). A host without a pairing is skipped, not crash-looped.
#
# `hennery doctor` reads the host's `--agent` words from
# `/etc/systemd/system/hennery-host.service`; it does not judge a system
# unit otherwise (its check 10 looks for user services).
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
  # A user that is not there fails the assertion below, not an evaluation.
  hostUser =
    config.users.users.${cfg.host.user} or {
      isNormalUser = false;
      group = "nogroup";
      home = "/var/empty";
    };
  restart = {
    unitConfig = {
      StartLimitIntervalSec = 300;
      StartLimitBurst = 10;
    };
    serviceConfig = {
      Type = "exec";
      Restart = "on-failure";
      RestartSec = 3;
      KillMode = "mixed";
      TimeoutStopSec = 30;
    };
  };
in
{
  options.services.hennery = lib.recursiveUpdate (common.options {
      defaultPackage = henneryFor pkgs.stdenv.hostPlatform.system;
      adapterPackages = pkgs.callPackage ../adapters.nix { };
      dataDirs = {
        collector = {
          default = "/var/lib/hennery";
          text = "Owned by the `hennery` system user, 0700. `hennery admin` runs as that user: `sudo -u hennery hennery admin --data-dir <this> …`.";
          listen = "The unit keeps no capability: a port below 1024 needs a proxy in front.";
        };
        host = {
          default = "/var/lib/hennery-host";
          text = "Made by systemd-tmpfiles, owned by `host.user`, 0700: keep it outside that user's home, where tmpfiles would make missing parents as root.";
          join = "`sudo -u <user> hennery host join <url> --data-dir <this>`";
          start = "`systemctl start hennery-host`";
        };
      };
      defaultSource = "nix";
      defaultSourceText = lib.literalExpression ''"nix"'';
    }) {
      host.user = lib.mkOption {
        type = lib.types.str;
        example = "alice";
        description = "The login user the host runs as: its agents run with this user's home, credentials and projects.";
      };
    };

  config = lib.mkMerge [
    (lib.mkIf cfg.collector.enable {
      users.users.hennery = {
        isSystemUser = true;
        group = "hennery";
        home = cfg.collector.dataDir;
      };
      users.groups.hennery = { };
      systemd.tmpfiles.settings."10-hennery".${cfg.collector.dataDir}.d = {
        user = "hennery";
        group = "hennery";
        mode = "0700";
      };
      systemd.services.hennery-collector = lib.recursiveUpdate restart {
        description = "hennery collector";
        wantedBy = [ "multi-user.target" ];
        after = [ "network.target" ];
        environment = {
          HENNERY_SERVICE = "systemd";
          HENNERY_LOG_DIR = "/var/log/hennery-collector";
        };
        serviceConfig = {
          ExecStart = common.execStart (common.collectorArgv exe cfg.collector);
          User = "hennery";
          Group = "hennery";
          LogsDirectory = "hennery-collector";
          LogsDirectoryMode = "0700";
          UMask = "0077";
          ReadWritePaths = [ (common.unitPath cfg.collector.dataDir) ];
          # The collector runs no program (agents are the host's): it keeps
          # no capability, so a port below 1024 needs a proxy in front.
          CapabilityBoundingSet = "";
          AmbientCapabilities = "";
          NoNewPrivileges = true;
          PrivateUsers = true;
          PrivateTmp = true;
          PrivateDevices = true;
          ProtectSystem = "strict";
          ProtectHome = true;
          ProtectClock = true;
          ProtectHostname = true;
          ProtectKernelLogs = true;
          ProtectKernelTunables = true;
          ProtectKernelModules = true;
          ProtectControlGroups = true;
          ProtectProc = "invisible";
          ProcSubset = "pid";
          RestrictAddressFamilies = [
            "AF_INET"
            "AF_INET6"
            "AF_UNIX"
          ];
          RestrictNamespaces = true;
          RestrictSUIDSGID = true;
          RestrictRealtime = true;
          RemoveIPC = true;
          MemoryDenyWriteExecute = true;
          LockPersonality = true;
          SystemCallArchitectures = "native";
          SystemCallFilter = [
            "@system-service"
            "~@privileged @resources"
          ];
        };
      };
      environment.systemPackages = [ cfg.package ];
    })

    (lib.mkIf cfg.host.enable {
      assertions = [
        {
          assertion = hostUser.isNormalUser;
          message = "services.hennery.host.user: ${cfg.host.user} is not a normal user of this system; the host runs as the user whose agents it runs.";
        }
        {
          assertion = cfg.host.adapters.source != "managed" || config.programs.nix-ld.enable;
          message = ''services.hennery.host.adapters.source = "managed" downloads glibc programs, which NixOS runs only with programs.nix-ld.enable; or use source = "nix".'';
        }
      ];
      systemd.tmpfiles.settings."10-hennery-host".${cfg.host.dataDir}.d = {
        user = cfg.host.user;
        inherit (hostUser) group;
        mode = "0700";
      };
      systemd.services.hennery-host = lib.recursiveUpdate restart {
        description = "hennery host";
        wantedBy = [ "multi-user.target" ];
        wants = [ "network-online.target" ];
        after = [ "network-online.target" ];
        # The agents' tools: what this unit is given, then the user's and
        # the system's profiles, then a shell and git.
        path =
          cfg.host.path
          ++ [
            "/run/wrappers"
            "/etc/profiles/per-user/${cfg.host.user}"
            "${hostUser.home}/.nix-profile"
            "/run/current-system/sw"
          ]
          ++ (with pkgs; [
            bash
            git
          ]);
        environment = {
          HENNERY_SERVICE = "systemd";
          HENNERY_LOG_DIR = "/var/log/hennery-host";
        };
        unitConfig.ConditionPathExists = common.unitPath "${cfg.host.dataDir}/host.key";
        serviceConfig = {
          ExecStart = common.execStart (common.hostArgv exe cfg.host);
          User = cfg.host.user;
          Group = hostUser.group;
          # Revoked (distribution spec §5.2): pairing again is the operator's.
          RestartPreventExitStatus = 78;
          LogsDirectory = "hennery-host";
          LogsDirectoryMode = "0700";
        };
      };
      environment.systemPackages = [ cfg.package ];
    })
  ];
}
