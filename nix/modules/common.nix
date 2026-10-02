# What the NixOS and home-manager modules share (plan 7e-ii-a): the
# options of `services.hennery.{collector,host}`, and the command lines they
# run, written as `hennery service install` writes them (plan 7c) so
# `hennery doctor` reads them back the same way.
{ lib }:
let
  inherit (lib) mkOption types;
in
rec {
  # One word of a systemd command line, as `service/unit.rs`'s
  # `systemd_word`: double-quoted, `\` and `"` escaped, `%` and `$` doubled
  # so neither specifiers nor variables are expanded. Doctor's
  # `systemd_command_line` reads only words written this way.
  systemdWord =
    text:
    "\""
    + lib.replaceStrings
      [ "\\" "\"" "%" "$" ]
      [ "\\\\" "\\\"" "%%" "$$" ]
      text
    + "\"";

  # One `Environment=` assignment, double-quoted: `%` doubled, as systemd
  # expands specifiers there, but not variables.
  envWord =
    text: "\"" + lib.replaceStrings [ "\\" "\"" "%" ] [ "\\\\" "\\\"" "%%" ] text + "\"";

  # A command line as one `ExecStart=` value.
  execStart = argv: lib.concatMapStringsSep " " systemdWord argv;

  # A path in a unit's `Condition…=` or `…Paths=`: specifiers escaped.
  unitPath = lib.replaceStrings [ "%" ] [ "%%" ];

  # The agents a host can be given.
  agentNames = [
    "claude"
    "codex"
  ];

  # `hennery collector`'s command line.
  collectorArgv =
    exe: cfg:
    [
      exe
      "collector"
      "--data-dir"
      cfg.dataDir
    ]
    ++ lib.concatMap (address: [
      "--listen"
      address
    ]) cfg.listen
    ++ lib.optionals (cfg.publicUrl != null) [
      "--public-url"
      cfg.publicUrl
    ];

  # `hennery host run`'s command line. With the Nix adapters, each agent is
  # given with `--agent name=<its wrapper>`: the host then installs and runs
  # no managed set (distribution spec §4.3), and doctor, reading these
  # `--agent` words, judges it as such. With the managed runtime, no
  # `--agent`: the host installs the pinned set at its start.
  hostArgv =
    exe: cfg:
    [
      exe
      "host"
      "run"
      "--data-dir"
      cfg.dataDir
    ]
    ++ lib.optionals (cfg.adapters.source == "nix") (
      lib.concatMap (name: [
        "--agent"
        "${name}=${lib.getExe cfg.adapters.packages.${name}}"
      ]) cfg.adapters.agents
    )
    ++ lib.concatMap (root: [
      "--workspace-root"
      root
    ]) cfg.workspaceRoots;

  # The options both modules declare. `dataDirs` gives each role's default
  # directory and its description; `adapterPackages` the default Nix
  # adapters, built from the operator's own nixpkgs, whose configuration
  # alone accepts the Claude CLI's licence.
  options =
    {
      defaultPackage,
      adapterPackages,
      dataDirs,
      defaultSource,
      defaultSourceText,
    }:
    {
      package = mkOption {
        type = types.package;
        default = defaultPackage;
        defaultText = lib.literalExpression "hennery.packages.\${system}.default";
        description = "The hennery package both services run.";
      };
      collector = {
        enable = lib.mkEnableOption "the hennery collector";
        dataDir = mkOption {
          type = types.str;
          inherit (dataDirs.collector) default;
          description = "The collector's data directory. ${dataDirs.collector.text}";
        };
        listen = mkOption {
          type = types.nonEmptyListOf types.str;
          default = [ "127.0.0.1:7117" ];
          description = "The addresses the collector listens on (`--listen`), as `host:port`. ${dataDirs.collector.listen}";
        };
        publicUrl = mkOption {
          type = types.nullOr types.str;
          default = null;
          example = "https://hennery.example";
          description = "Where browsers reach the collector (`--public-url`), until setup stores its own.";
        };
      };
      host = {
        enable = lib.mkEnableOption "a hennery host";
        dataDir = mkOption {
          type = types.str;
          inherit (dataDirs.host) default;
          description = ''
            The host's data directory. ${dataDirs.host.text} The service is
            skipped until it holds a pairing (`host.key`), and systemd
            checks that only when the unit starts. Pair it as the user the
            host runs as, ${dataDirs.host.join}, adding `--no-runtime` when
            `adapters.source` is `nix`, then start the unit
            (${dataDirs.host.start}). A join run as another user (root)
            leaves files the service cannot read.
          '';
        };
        adapters = {
          source = mkOption {
            type = types.enum [
              "nix"
              "managed"
            ];
            default = defaultSource;
            defaultText = defaultSourceText;
            description = ''
              Where the host's adapters come from. `nix`: the pinned
              adapters built by Nix from the binary's own manifest, given
              with `--agent`. `managed`: the host downloads and installs
              the pinned set itself (distribution spec §3.2), which on
              NixOS needs `programs.nix-ld.enable`.
            '';
          };
          agents = mkOption {
            type = types.listOf (types.enum agentNames);
            default = agentNames;
            description = ''
              With `source = "nix"`, the agents the host runs. Both are in
              the default (the maintainer's decision of 2026-10-02). The
              Claude adapter bundles Anthropic's CLI under an unfree licence,
              so a host with `claude` evaluates only once you accept it in
              your nixpkgs configuration: `nixpkgs.config.allowUnfree = true`,
              or `nixpkgs.config.allowUnfreePredicate = pkg:
              lib.getName pkg == "hennery-claude-acp"` for this one package.
              Or leave `claude` out.
            '';
          };
          packages = mkOption {
            type = types.attrsOf types.package;
            default = adapterPackages;
            defaultText = lib.literalMD "the pinned adapters, built from the manifest with this nixpkgs";
            description = "The package of each agent, with `source = \"nix\"`.";
          };
        };
        workspaceRoots = mkOption {
          type = types.listOf types.str;
          default = [ ];
          example = [ "~/src" ];
          description = "Directories to find projects in (`--workspace-root`): absolute, or `~/…`.";
        };
        path = mkOption {
          type = types.listOf types.package;
          default = [ ];
          description = "Packages added to the PATH the host's agents run with, before the user's profiles.";
        };
      };
    };
}
