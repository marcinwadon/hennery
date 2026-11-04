# OpenSSL supply for 0.5.1

The release review found that archives and default Cargo installs used
`openssl-src 300.6.1+3.6.3`. OpenSSL 3.6 reached end of support on
1 November 2026; that source also omits subsequent security fixes.
The separate Nix input was 3.6.4.

The [upstream download inventory](https://openssl-library.org/source/) names
3.5.9 as the current 3.5 LTS release, supported until 8 April 2030.
Its [release notes](https://openssl-library.org/news/openssl-3.5-notes/)
describe the security fixes. Source SHA-256
`603f5602e2eef00d77fbd429d34dcd5822bb301757a1bc9cdb24c670f1eb859a`
matches the downloaded archive, upstream's checksum asset and
[Homebrew's maintained openssl@3 formula](https://formulae.brew.sh/formula/openssl@3).

## Supply decision

- Build static release OpenSSL 3.5.9 from the checksum-pinned upstream archive
  with the native compiler on each supported release host. Cargo-dist receives
  `OPENSSL_DIR` and `OPENSSL_STATIC=1`; the existing linkage checks still reject
  external libraries. `hennery --version` reports the actual native library
  through OpenSSL's version API; archive acceptance requires the pinned version
  in that output. Together these detect accidental fallback to runner libraries;
  the version string alone is not proof of source provenance.
  The macOS C build takes its deployment minimum from the selected Rust
  compiler, preserving the application's baseline rather than the runner OS.
- Keep the pin in `packaging/openssl.json`, shared by the native builder and the
  Nix application/development-shell input. Nix retains system-library linking;
  there is no global replacement of libraries used by Nix build tools.
- Remove the default `vendored-openssl` Cargo feature. Source consumers provide
  a maintained platform OpenSSL development package. Update both source-build
  recipes and the published CLI README. A workspace-only Cargo patch would not
  protect registry consumers, so it is not used.
- Carry the native library version, upstream source URL, SHA-256 and licence in
  notices and SPDX. Refuse release builds after the recorded support end date,
  source checksum mismatches, missing native notices and source-record drift.
  Continue reviewing upstream security patch releases before every publication.

Two disposable probes ruled out smaller dependency-only changes. Even
`openssl-sys 0.9.118` requires `openssl-src ^300.2.0`: adding the available
400.0.2+4.0.3 source crate resolves both major versions and leaves 3.6.3 linked.
The registry's 3.5 source stops at unpatched 3.5.5. AWS-LC builds, but the current
WebAuthn crates reject its missing OpenSSL version metadata; changing backend
would require a broader upstream compatibility migration.

## Verification

The upstream archive hash was checked and the native macOS static build passed,
including executing the resulting OpenSSL 3.5.9 binary. Four focused tests cover
corrupted source, expired support, wrong/missing binary version, and native
SPDX/notices with missing-licence/source-drift refusals. The existing supply
suite passed 11 tests. Full packaging, published-crate installation, WebAuthn,
Web Push and current-head CI qualification are required before signing the tag;
their final results are recorded on the release preparation PR.

Local follow-up passed against the native static prefix: 80 kernel integration
tests (passkeys, passkey log, auth sessions, push, delivery and HTTP delivery),
two CLI identity tests and one diagnostic unit test, all exit 0. The CLI reports
the linked OpenSSL 3.5.9 through the native API. Cargo's production tree contains
no `openssl-src`; formatting and 83 crate/publication regression tests passed.
The complete release Python suite ran 170 tests with six skipped (exit 0);
after the runtime-version check amendment, its four focused crypto and four
source-identity tests passed again. The native Nix OpenSSL 3.5.9 input also built
successfully with its upstream test suite enabled.

The first native Linux x64 archive passed the pinned-library query, static
linkage checks and Alpine execution. ARM64 exposed Ubuntu's musl-gcc/glibc
libgcc mismatch when linking OpenSSL's own CLI: `libgcc.a(lse-init.o)` required
glibc's `__getauxval`. The previous vendored builder made only libraries, while
the new builder also executes OpenSSL itself. The ARM64-musl configuration now
uses `-mno-outline-atomics`, retaining baseline ARMv8 load/store-exclusive
instructions without the incompatible runtime helper. Native qualification
must pass after this amendment; the failed archive is not a release candidate.

The corrected native ARM64 archive job completed successfully at `d47d32bc`,
including the OpenSSL 3.5.9 API query and Alpine execution. The macOS builder
follow-up derives its deployment minimum from Rust: with the pinned compiler,
the rebuilt OpenSSL CLI records Mach-O `minos 14.0`. All 80 crypto/auth
integration tests and three CLI/identity tests passed again with that rebuilt
static prefix. Portable intake regressions use the existing release tooling
shell, so the checks job does not build an unused OpenSSL before dispatching
the Rust/web/Nix qualification jobs; their crypto coverage remains enabled.

This amends the original distribution plan's `vendored-openssl` implementation;
the self-contained, statically linked release archive contract is preserved.

## Hosted macOS Nix qualification

The merged source `f1abef18` passed the full Rust/web suites, 47 native macOS
smoke tests, both README build recipes, all three native archives and their
installer/Homebrew checks. Its Nix package, source-input and embedded-UI checks
also passed locally on macOS, including five source-input tests; the installed
binary reports that source revision and OpenSSL 3.5.9.

Hosted Nix builds twice stopped with `No buffer space available` while OpenSSL
compilation and web dependency fetching emitted interleaved progress
([first failure](https://github.com/marcinwadon/hennery/actions/runs/41891162469/job/126814071082),
[second failure](https://github.com/marcinwadon/hennery/actions/runs/41891162469/job/126837634942)).
The local Nix client is 2.31.3+1; the hosted client is 2.35.4, so the local pass
does not qualify that hosted logging path. CI now limits macOS to one Nix builder,
saves complete build output to an artifact, preserves the build's exit status
and gives that serial build 60 minutes. All package, source, UI and README checks
remain required. Hosted acceptance after this change is required before tagging.
