# Plan 7e-ii-a ledger (successor session)

- base main ecc50cd; predecessor's work at 47e0bb9 (scratch/nix-modules-3 from scratch/nix-modules-2)
- #90 macOS flake for 719c79e: waiting (queued at start)
- decisions so far: home-manager service.env writes no SHELL line (user manager gives the account's shell) — to opus
- gap found: adapter-check *:127 with an existing loader had no case -> add library-missing (Linux)
- 2026-10-02 MAINTAINER ANSWERS (binding, via parent): Q1 claude stays in default agents YES (document allowUnfree / allowUnfreePredicate);
  Q2 NoNewPrivileges on host unit NO (operator decision); Q3 CI running Anthropic CLI on GH runners YES (build+check only, never cache/upload/publish);
  Q4 revoked host exit 78 stops restarting YES in the modules (RestartPreventExitStatus=78); 7c's unit.rs/launchd are the parent's — don't touch unit.rs.
- fleet rule extended: probes and tools too run with HOME/XDG_*, npm_config_userconfig, pnpm store/config, cargo/nix config at scratch (wrapper ok).
- 7e-ii-b is PR #104; whoever lands second rebases; flake.nix outputs adjacent conflicts, keep both.
- build branch scratch/7e2a-build: T1 1ab7ecf (adapters), T2 (doctor system unit)
