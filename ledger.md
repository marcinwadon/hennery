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
- #90 macOS flake at 719c79e: PASS (12m37s, run 37054209788); ubuntu flake PASS (10m20s)
- opus security review (deltas) 2026-10-02: APPROVE WITH AMENDMENTS A1 (spec §7: checks 1,2,3-4,9,12 read the agents), A2 (spec §4.3 unfree/cache wording);
  O1 (check 3 spawns system-unit agents with doctor's PATH: note), O2 (missing loader fatal on patched adapters), N1 (offline parity: XDG_CACHE_HOME, CLAUDE_CONFIG_DIR, CODEX_HOME, CODEX_SQLITE_HOME, HENNERY_DEV_TOKEN), N2 (comment). Taking all.
- trial merges: #102 (c9c1b25e) and #96 (1c912153) clean, together compile + doctor tests 39 pass; #104 (cc2047e1) conflicts flake.nix only
- mac probes run 1: unpack-links caught by backstop (expectation fixed + new unpack-backstop), unpack-strip caught other msg (fixed), rust-user-first didn't compile (fixed)
- scoped re-confirmation (same opus reviewer) 2026-10-02: A1, A2, O1, O2, N1, N2 all CONFIRMED.
- build branch rebuilt: 38d69d2a 795cb7b9 79588ebe 0f5f72f1 c8d0a2f9 (v1 kept as scratch/7e2a-build-v1); replay MATCH x5
- #90 pushed 32480c7 (build + scratch probes job)
- #90 run 37062042499 at 32480c7: flake ubuntu PASS 11m48s, macOS PASS 18m20s; probes job: 12 Linux probes, all failed their check;
  2 expectation regexes were wrong (hm-doctor-dollar: new nix "Cannot build" wording; vm-doctor: assertion text not in output) -> fixed; vm-host-log/mode regexes tightened to observed output.
  helper-cannot-start's status is in the 126-255 arm (check-cannot-start caught); the VM without the system-unit read: check 1 "no adapter set", check 2 FAIL nix-ld (confirms A1).
- #90 run 37064429485 at f7df7d1: flake ubuntu+macOS SUCCESS; probes job SUCCESS (4 re-patterned Linux probes)
