# The modules' evaluation checks (plan 7e-ii-a), Linux only:
#
# - `modules-eval`: the NixOS module's units for each `adapters.source`, the
#   consent the Claude adapter needs, and the assertions, and the
#   home-manager module's unit and PATH file, all evaluated without building
#   a system;
# - `home-manager-doctor`: the home-manager module evaluated against stand-ins
#   for home-manager's own options (this flake takes no home-manager
#   input), its host unit written where `hennery service install` writes
#   one, and the real `hennery doctor` reading its `--agent` back, with a data
#   directory holding every character the unit must escape.
{
  pkgs,
  nixpkgs,
  hennery,
  nixosModule,
  homeManagerModule,
}:
let
  inherit (pkgs) lib;
  inherit (pkgs.stdenv.hostPlatform) system;

  nixos =
    config:
    (import "${nixpkgs}/nixos/lib/eval-config.nix" {
      inherit system;
      modules = [
        nixosModule
        {
          users.users.alice.isNormalUser = true;
          system.stateVersion = "26.05";
          boot.loader.grub.enable = false;
          fileSystems."/" = {
            device = "none";
            fsType = "tmpfs";
          };
        }
        config
      ];
    }).config;
  failedAssertions = config: map (a: a.message) (lib.filter (a: !a.assertion) config.assertions);
  hostExec = config: config.systemd.services.hennery-host.serviceConfig.ExecStart;
  host = extra: {
    services.hennery.host = {
      enable = true;
      user = "alice";
    } // extra;
  };

  nixNoClaude = nixos (host { adapters.agents = [ "codex" ]; });
  nixBoth = nixos (host { } // { nixpkgs.config.allowUnfree = true; });
  nixDefault = nixos (host { });
  managed = nixos (host { adapters.source = "managed"; });
  managedLd = nixos (host { adapters.source = "managed"; } // { programs.nix-ld.enable = true; });
  stranger = nixos {
    services.hennery.host = {
      enable = true;
      user = "nobody-here";
    };
  };
  collector = nixos {
    services.hennery.collector = {
      enable = true;
      listen = [ "127.0.0.1:7117" "[::1]:7117" ];
      publicUrl = "https://hennery.example";
    };
  };

  # home-manager's options this module uses, as stand-ins.
  homeStubs =
    { lib, ... }:
    {
      options = {
        assertions = lib.mkOption {
          type = lib.types.listOf lib.types.anything;
          default = [ ];
        };
        home.packages = lib.mkOption {
          type = lib.types.listOf lib.types.package;
          default = [ ];
        };
        home.username = lib.mkOption { type = lib.types.str; };
        home.profileDirectory = lib.mkOption { type = lib.types.str; };
        xdg.dataHome = lib.mkOption { type = lib.types.str; };
        xdg.configHome = lib.mkOption { type = lib.types.str; };
        xdg.configFile = lib.mkOption {
          type = lib.types.attrsOf (lib.types.submodule { options.text = lib.mkOption { type = lib.types.lines; }; });
          default = { };
        };
        systemd.user.services = lib.mkOption {
          type = lib.types.attrsOf lib.types.anything;
          default = { };
        };
      };
    };
  # `@DATA@` is the check's own temporary directory, put in at build time.
  data = "@DATA@/a dir %h $HOME \"quoted\" \\back";
  home =
    (lib.evalModules {
      modules = [
        homeStubs
        homeManagerModule
        {
          _module.args.pkgs = pkgs;
          home.username = "alice";
          home.profileDirectory = "/home/alice/.nix-profile";
          xdg.dataHome = "/home/alice/.local/share";
          xdg.configHome = "/home/alice/.config";
          services.hennery.package = hennery;
          services.hennery.host = {
            enable = true;
            dataDir = data;
            adapters.agents = [ "codex" ];
            # The sandbox has no profile: the shell and git the agents need.
            path = [
              pkgs.bash
              pkgs.git
            ];
          };
        }
      ];
    }).config;
  homeHost = home.systemd.user.services.hennery-host;
  codex = (pkgs.callPackage ../adapters.nix { }).codex;
  has = needle: haystack: lib.hasInfix needle haystack;
  # Each check, its name and whether it holds.
  results = {
    "nix: the agents given with --agent" =
      has ''"--agent" "codex=/nix/store/'' (hostExec nixNoClaude)
      && !has "claude=" (hostExec nixNoClaude)
      && has ''"host" "run" "--data-dir" "/var/lib/hennery-host"'' (hostExec nixNoClaude);
    "nix: claude given once its licence is accepted" =
      has ''"claude=/nix/store/'' (hostExec nixBoth) && has ''"codex=/nix/store/'' (hostExec nixBoth);
    "nix: claude refused without the licence accepted" =
      !(builtins.tryEval (builtins.seq (hostExec nixDefault) true)).success;
    "managed: no --agent" = !has "--agent" (hostExec managed);
    "managed: refused without nix-ld" = lib.any (has "nix-ld") (failedAssertions managed);
    "managed: accepted with nix-ld" = failedAssertions managedLd == [ ];
    "nix: no assertion fails" = failedAssertions nixNoClaude == [ ];
    "a user who is not there is refused" = lib.any (has "not a normal user") (failedAssertions stranger);
    "host: skipped until paired" =
      nixNoClaude.systemd.services.hennery-host.unitConfig.ConditionPathExists == "/var/lib/hennery-host/host.key";
    "host: a revoked host is not restarted" =
      nixNoClaude.systemd.services.hennery-host.serviceConfig.RestartPreventExitStatus == 78;
    "host: runs as its user" = nixNoClaude.systemd.services.hennery-host.serviceConfig.User == "alice";
    "collector: its command line" =
      collector.systemd.services.hennery-collector.serviceConfig.ExecStart
      == ''"${lib.getExe hennery}" "collector" "--data-dir" "/var/lib/hennery" "--listen" "127.0.0.1:7117" "--listen" "[::1]:7117" "--public-url" "https://hennery.example"'';
    "collector: its own system user" =
      collector.systemd.services.hennery-collector.serviceConfig.User == "hennery"
      && collector.users.users.hennery.isSystemUser;
    "home-manager: every word quoted as unit.rs quotes it" =
      homeHost.Service.ExecStart
      == ''"${lib.getExe hennery}" "host" "run" "--data-dir" "@DATA@/a dir %%h $$HOME \"quoted\" \\back" "--agent" "codex=${lib.getExe codex}"'';
    "home-manager: the PATH in service.env, in its quoting" =
      home.xdg.configFile."hennery/service.env".text
      == "# Written by the home-manager module of hennery: the services' PATH.\nPATH=\"${pkgs.bash}/bin:${pkgs.git}/bin:/home/alice/.nix-profile/bin:/etc/profiles/per-user/alice/bin:/run/wrappers/bin:/run/current-system/sw/bin:/nix/var/nix/profiles/default/bin:/usr/bin:/bin\"\n";
    "home-manager: the unit reads it" =
      homeHost.Service.EnvironmentFile == "/home/alice/.config/hennery/service.env";
    "home-manager: skipped until paired, not restarted once revoked" =
      homeHost.Unit.ConditionPathExists == "@DATA@/a dir %%h $HOME \"quoted\" \\back/host.key"
      && homeHost.Service.RestartPreventExitStatus == 78;
    "home-manager: no assertion fails on Linux" = failedAssertions home == [ ];
  };
  failed = lib.attrNames (lib.filterAttrs (_: ok: !ok) results);

  # The unit as home-manager writes it: one `key=value` line per value.
  ini =
    unit:
    lib.concatStrings (
      lib.mapAttrsToList (
        section: keys:
        "[${section}]\n"
        + lib.concatStrings (
          lib.mapAttrsToList (
            key: value: lib.concatMapStrings (v: "${key}=${toString v}\n") (lib.toList value)
          ) keys
        )
        + "\n"
      ) unit
    );
  homeUnit = pkgs.writeText "hennery-host.service" (ini home.systemd.user.services.hennery-host);
  homeEnv = pkgs.writeText "service.env" home.xdg.configFile."hennery/service.env".text;
in
{
  modules-eval = pkgs.runCommand "hennery-modules-eval" { } ''
    ${lib.concatStrings (lib.mapAttrsToList (name: ok: "echo '${if ok then "ok  " else "FAIL"} ${name}'\n") results)}
    ${lib.optionalString (failed != [ ]) "exit 1"}
    touch "$out"
  '';

  home-manager-doctor =
    pkgs.runCommand "hennery-home-manager-doctor"
      {
        nativeBuildInputs = [ hennery ];
      }
      ''
        export HOME="$TMPDIR/home" XDG_CONFIG_HOME="$TMPDIR/config"
        data="$TMPDIR/a dir %h \$HOME \"quoted\" \\back"
        mkdir -p "$HOME" "$data" "$XDG_CONFIG_HOME/systemd/user" "$XDG_CONFIG_HOME/hennery"
        # A host's directory, by its names alone.
        touch "$data/host.key"
        sed "s|@DATA@|$TMPDIR|g" ${homeUnit} > "$XDG_CONFIG_HOME/systemd/user/hennery-host.service"
        cp ${homeEnv} "$XDG_CONFIG_HOME/hennery/service.env"
        cat "$XDG_CONFIG_HOME/systemd/user/hennery-host.service" "$XDG_CONFIG_HOME/hennery/service.env"
        grep -q 'ExecStart=".*" "host" "run" "--data-dir" ".*/a dir %%h $$HOME \\"quoted\\" \\\\back" "--agent" "codex=' \
          "$XDG_CONFIG_HOME/systemd/user/hennery-host.service"
        hennery doctor --data-dir "$data" > report 2>&1 || true
        cat report
        grep -E '^ok +1 .*; the service gives its agents with --agent$' report
        grep -E '^ok +3 .*codex answers .*given by --agent' report
        # Check 5 reads the module's PATH, and finds sh and git on it.
        grep -E '^(ok|warn) +5 ' report
        if grep -E '^(ok|warn|fail) +5 .*(cannot be read|(sh|git) is not on the service)' report; then
          exit 1
        fi
        touch "$out"
      '';
}
