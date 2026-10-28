# Release builds

The read-only `packaging` workflow (`.github/workflows/packaging.yml`) builds
release candidates for packaging-input PRs and main pushes. The signed-tag
`release.yml` calls the same workflow with authenticated release context. It
builds the UI once, reuses native archives for installers/Homebrew, and uses the
same musl binaries in collector images. All channels receive local acceptance,
including channels not selected for publication. PR artifacts are unsigned
qualification evidence and are never promoted to releases.

After packaging succeeds, the tag workflow attests final bytes. The maintainer
reviews its asset/hash summary before one protected `release` environment job
publishes an immutable GitHub release and selected channels. The [release
process](../docs/release-process.md) describes setup and the [shared verification
contract](release/README.md). Alpha.3 has shipped; the approved CI migration is
being applied in stacked steps.

| Artifact | Built by | Checked by |
|---|---|---|
| `hennery-x86_64-unknown-linux-musl.tar.xz`, `hennery-aarch64-unknown-linux-musl.tar.xz` | `dist build`, natively on each architecture, with `musl-gcc` | `check-archive.sh` (the binary, CHANGELOG.md, LICENSE and README.md, nothing else); `check-linkage.sh` (no shared library, no loader); runs on the runner and on Alpine |
| `hennery-aarch64-apple-darwin.tar.xz` | `dist build` on macOS arm64 | `check-archive.sh`; `check-linkage.sh` (system libraries only); runs |
| `hennery-installer.sh` | `dist build --artifacts=global`, then `harden-installer.py` | `test-installer.sh`, on Linux x86_64 and on macOS |
| `hennery.rb` (Homebrew formula) | `dist build --artifacts=global` | `check-formula.sh` |
| `SHA256SUMS` (release only) | final single-run assembly | canonical exact payload closure; tagged-workflow provenance for checksum file and every payload |
| Collector images and a two-platform OCI index | native BuildKit of `Dockerfile --target release` on the accepted archive binaries with release notices/SPDX; preserves SBOM/provenance metadata | OCI payload verification, signing-input extraction, then retained-digest loopback round-trip and `smoke-image.sh` |

[R4's collector-image guide](images.md) describes the release target, labels,
reviewed notices and binary/image SBOMs, native OCI inputs, gated GHCR
publication, digest deployment, volume upgrades and maintainer-only setup.
The shared verifier and registered GHCR acceptance job are integrated; publication
remains disabled pending independent review and maintainer qualification.
PR/main images are never promoted to releases.

Every archive builds with the `vendored-openssl` feature
(`dist-workspace.toml`). OpenSSL, which `webauthn-rs-core` links, is then
compiled in from source. Without it, the macOS binary would load Homebrew's
`libssl` and fail on a Mac without it, and a static musl build would not
link at all (distribution §1.1). Cargo builds now enable vendored OpenSSL by default (R3); Nix opts out with
`--no-default-features` and uses its system-library inputs.

Third-party actions are pinned by commit, every image by its multi-arch
digest, and cargo-dist by the SHA-256 in `install-dist.sh`.

## OpenSSL

The release binaries carry their own OpenSSL: 3.6.3, from `openssl-src`
300.6.1+3.6.3 in `Cargo.lock`. Updating the system's OpenSSL does not
reach them. An OpenSSL fix reaches users only when `openssl-src` is bumped
in `Cargo.lock` and a new release is made. `webauthn-rs` passes what a
browser sends (attestation and assertion data) through it, so OpenSSL's
advisories are this project's to follow.

OpenSSL 3.6 is not a long-term-support line. Check its end of support
against openssl.org's release strategy before the first release, and move
to a supported line if it ends first.

## Locally

The dev shell's toolchain links Nix's `libiconv` on macOS, so a local
release build is for trying out, not for handing on; `check-linkage.sh` says
so. Linux archives and the image are built in CI only.

```sh
nix develop -c cargo build --profile dist -p hennery --features vendored-openssl
sh packaging/check-linkage.sh target/dist/hennery
```

## Pin bumps

`adapters/pins.toml` pins the adapters and their CLI/SDK dependencies separately.
Each adapter's `overrides` table becomes npm's exact-version `overrides`; the
generator rejects missing or differently resolved runtime packages, including
nested copies. This permits a CLI/SDK update when its adapter has not released,
even outside the adapter's dependency range. Compatibility still requires the
live checks below; a version override is not evidence of compatibility.

The hourly `pins-watch` checks both adapters, Claude Agent SDK, Codex CLI and
stable Node releases within the pinned major. It proposes one PR for the entire
set, with a fresh `npm_before` cutoff on every proposal, including CLI-only
changes. Its branch identifies the proposed versions, so unchanged upstream
versions do not open a new PR each hour. It never downgrades pins or selects
prereleases. `nix develop .#release -c python3 packaging/pins_watch.py discover`
prints a read-only proposal without creating branches or contacting GitHub.
Pins CI also qualifies the actual Claude adapter/CLI MCP source isolation and
collision contract on Linux x64 with a synthetic local model provider and the
pinned Node version. This needs no account credentials.
The workflow updates the candidate Codex version automatically; its source and
deletion contract must still be reviewed before merging.

For security fixes confined to compatible transitive dependencies, update only
the affected names in the committed adapter lockfile with `npm update --package-lock-only
--ignore-scripts --no-audit --no-fund <names>`, then run `hennery-pins generate`.
Keep the adapter, bundled CLI and Node pins unchanged, inspect every lockfile
version change, and run the pins CI and `sh packaging/audit-adapters.sh`.
The `npm_before` cutoff describes a full resolution by `hennery-pins lock`;
such a resolution replaces security-only lockfile refreshes, so audit it before
committing. Both pin CI and release preparation audit the committed lockfiles.

A bump of `adapters/pins.toml` is `hennery-pins lock`, then `generate`,
then the pin-bump job (`.github/workflows/pins.yml`). These checks are
live, with logged-in agents, and the operator's to run before it merges:

- the live e2e gate (umbrella spec §14) on every platform;
- all four parent/child delegation matrix rows and the required lifecycle
  scenarios (D4), after the no-inference schema discovery probe;
- **Codex's `thread/delete` still removes everything** (plan 9d
  decision 14). After a turn in a scratch `CODEX_HOME`, delete the session
  from hennery and check that nothing of the thread is left: its rollout
  files in `sessions/` and `archived_sessions/`, its rows in
  `thread_history_1.sqlite` (`thread_items`, `thread_turns`,
  `thread_realtime_items`), its rows in the state DB (`state_5.sqlite`),
  and its entries in `session_index.jsonl`. hennery itself only checks the
  rollout files: the rest is `thread/delete`'s contract. If the bundled
  Codex version changed, read `thread/delete`'s call shape from its source
  first and update `[codex_app_server]` in `pins.toml`.
- **What the host reads from Codex is still what it says** (the 9d-ii
  review). Re-read, in the bumped version's source and against a live run
  in a scratch `CODEX_HOME`:
  - the refusal texts `classify_delete` matches (`forked history still
    references it`, `thread is not persisted`, `live internal threads`,
    `no rollout found for thread id`, `thread not found:`, and the
    `Invalid request: unknown variant` / `missing field` / `invalid type`
    prefixes);
  - the `codexHome` field of `initialize`'s answer, canonical;
  - `--version`'s `codex-cli X.Y.Z` line.

The opt-in `pinned_codex_delete_removes_rollouts_and_private_database_rows`
test in `hennery-testkit/tests/forget_codex.rs` automates the deletion check on
the current platform. Set `HENNERY_PIN_CODEX` to the installed set's native
Codex binary and `HENNERY_D4_CODEX_AUTH` to a source credential file, then run:

```sh
nix develop -c cargo test -p hennery-testkit --test forget_codex --locked \
  pinned_codex_delete_removes_rollouts_and_private_database_rows -- \
  --ignored --exact --nocapture
```

It copies credentials into a disposable home, makes one real provider turn,
calls the host's forget implementation and checks rollouts, state/history
databases and the session index. It never hands the source home to Codex.

## Crates.io and cargo-binstall (R3)

The seven production crates opt into crates.io at one exact version. Publish
in this order: hennery-proto, hennery-kernel, hennery-view, hennery-host,
hennery-gateway, hennery-sessions, hennery. The workspace default, testkit and
pin generator remain private. Library compatibility is pre-1.0 application
support, not a separately stable SDK. Protocol/view generators are build tools,
not advertised install products.

The shared workflow prepares all seven crates on Linux from a fresh git-archive
snapshot outside the checkout. `packaging/prepare-crates.mjs` reuses the UI built
in prepare (or builds it for standalone local use), copies only its distributable
output into the kernel, gathers licence texts for production
dependencies and configured build tools (including emitted helpers and CSS), and
writes its source/UI inventories.
The canonical host manifest is `crates/hennery-host/manifest.json`; pin generation,
CI and Nix read that same file. No agent or managed Node payload is packaged.
Each final tarball is inspected for actual AGPL text, normalized registry
metadata and its exact production dependency versions, and the 10 MB size cap.
The kernel also carries FRONTEND-NOTICES.txt and SOURCE.json, and its README
links to the immutable corresponding-source archive and SHA-256. The complete
source archive includes Rust/web sources, lockfiles, build scripts, repository
tests and fixtures. Individual crates intentionally omit the workspace's tests.

`packaging/check-crates.mjs` creates a temporary loopback sparse registry
containing these seven exact candidates and prefetched upstream dependencies.
The overlay only exists in a scratch CARGO_HOME; it is never a real publishing
registry or user configuration. Each tarball is inspected and its production
feature graph checked, then one locked install runs with Cargo offline and
OS network access denied to subprocesses. The OS restriction includes build
scripts and is tested with a reachable loopback positive control. The locked
install runs from an empty directory; unlocked installation and a Rust 1.88 build
run in weekly maintenance. CI caches compilation between candidates. The installed collectors
run in disposable homes with Node/pnpm absent from PATH, serve all HTML-referenced
assets and render the app in the pinned Chromium. Native macOS acceptance uses
the release archive already built for that platform.
Source installs need Rust, a C compiler, make and Perl (macOS: Xcode Command
Line Tools); default vendored OpenSSL removes the system OpenSSL prerequisite.

Native cargo-binstall is disabled. Pinned 1.24.0 refusal tests offer real loopback
archives, but all seven attempts must refuse without fetching or installing an
archive. All native strategies (`crate-meta-data`, `quick-install`, `compile`)
are disabled in the final CLI metadata. Native binstall cannot enforce the
required signed-source and digest Sigstore policy. The `hennery-release` metadata namespace
records the maintainer fingerprint and concrete version's exact tagged workflow
identity for an external verifier. A native bridge needs separate review;
archive URL mappings remain dormant data for that future integration.

Before running any release-contained installer or verifier, follow the
[verify-then-run bootstrap](../README.md#verify-a-release-installer-before-running-it).
Installed GnuPG authenticates the exact annotated tag from the pinned signing
fingerprint. Installed `gh` authenticates `SHA256SUMS` and the installer against
`release.yml@refs/tags/<exact-tag>`, with source/signer digest binding. Verify hashes before executing downloaded
code. A trusted
checkout's [common verifier](release/verify.py) applies the same policy to all
six public assets; private qualification also checks archive executable metadata. There is no inventory signature.
Bare Cargo installation waits for stable publication and public-path qualification;
the first alpha remains GitHub-only.

### Crates integration and maintainer bootstrap

The crate step is folded into the single `release` environment job, after
immutable GitHub publication. The stable all-channel tag policy,
`RELEASE_PUBLICATION_ENABLED`, `RELEASE_GITHUB_ENABLED` and `RELEASE_CRATES_ENABLED`
must all permit it. Their values remain operator-controlled. All seven frozen `.crate`
files and `crates-inventory.json` are private qualification payloads bound by
`SHA256SUMS` and source/tag attestations. Corresponding source is public at the
immutable commit recorded in `SOURCE.json`; the local packaging workspace
snapshot is not staged as a release payload. The publisher
never invokes Cargo to rebuild/repackage accepted tarballs.

Native environment approval protects the publisher; its verifier checks signed tag, checksums,
attestations and complete immutable public release bytes before any channel
credential. Node repeats frozen-byte checks at every credential/upload/index boundary and
requires the exact public source commit URL. It does not fetch a custom source
asset. Hennery source is public at the signed tag and in GitHub's automatic Source
code downloads; embedded notices link exact MPL dependency sources. Preserve source availability for the applicable AGPL period. Missing
originals cause refusal. A matching registry digest is reused; differing, yanked,
auth-failed or ambiguous remote state stops, with no aliases or announcement.
Existence comes from the crates.io version API: 404 permits upload, 200 requires
downloading and matching the frozen digest; any other status refuses. All registry
requests identify the release client. After upload, up to 48 polls spaced 10 seconds
apart wait for API visibility and CDN bytes (403/404 can mean propagation), with
per-request timeouts. An already-present version with unavailable bytes refuses.
After each crate, the index must resolve the exact version/checksum before any
consumer uploads. Partial or ambiguously successful publication is recovered by
a newly approved publish-only retry with the same artifacts/attestations, while
the original artifacts remain available. Reconcile completed work, never
rebuild. Otherwise document
the incomplete version, withhold aliases and roll forward to a new signed version.

Maintainer steps (never performed by builders):

1. Confirm release controls and signed-tag verification, source/licence/advisory,
   all platform/MSRV gates and the public immutable source URL. Before **any**
   channel switch is enabled, obtain an explicitly authorized genuine Hennery
   tagged attestation as the authentic positive fixture and verify the full policy
   with pinned `gh 2.101.0`. Upstream authentic crypto and substituted structural
   tests do not replace this pre-enablement platform obligation.
2. Create a short-lived crates.io token narrowly scoped to first-publishing the
   exact seven crate names in the topological order above. Put it only in the
   `release` secret `RELEASE_CRATES_BOOTSTRAP_TOKEN`. Set temporary
   `RELEASE_CRATES_BOOTSTRAP_ENABLED=true` only for the approved stable release,
   together with explicit channel selection and enablement. Approval authorizes
   the same frozen-byte uploader and verification policy, with no OIDC fallback.
   Missing, expired or insufficiently scoped tokens fail; never add standing
   all-crate authority. Do not grant repository administration to the job.
3. First-claim the seven names through that protected job using accepted tarballs.
   On partial failure reconcile registry digests before retry; do not obtain a new
   token or rebuild bytes while the previous bootstrap authority remains live.
   On abandonment or success immediately revoke the bootstrap token at crates.io,
   confirm revocation, remove the environment secret and disable bootstrap mode.
   This is an operator action; a printed cleanup instruction is not revocation.
4. Once each crate exists, configure its own trusted publisher: repository
   `marcinwadon/hennery`, workflow filename **`release.yml`**, environment **`release`**.
   Inspect all seven settings; trusting only the final CLI is insufficient. Revoke
   bootstrap authority even if configuring trusted publishing is abandoned.
5. Normal runs leave bootstrap mode off and have no bootstrap secret. After the
   gate/public-source preflight, the pinned official auth action exchanges OIDC
   for a short-lived token; its post step revokes it. Missing per-crate trust or
   OIDC refusal stops without falling back to the bootstrap token. Test actual
   first-run and renewed retry approval with explicit maintainer authorization.
6. Independently verify public installs and the embedded UI from clean scratch
   homes before advertising channel success. Keep early-testing migration/data
   guarantees unchanged.

Cargo's [registry API](https://doc.rust-lang.org/cargo/reference/registry-web-api.html#publish),
[source replacement](https://doc.rust-lang.org/cargo/reference/source-replacement.html),
[binstall metadata](https://github.com/cargo-bins/cargo-binstall/blob/main/SUPPORT.md)
and [trusted publishing bootstrap](https://blog.rust-lang.org/2025/07/11/crates-io-development-update-2025-07/)
are the implementation contracts to recheck on changes.

## Publishing is the operator's

- `release` environment, owner-only `v*` tag rulesets (updates/deletes blocked),
  and immutable releases are configured. Signed source must authenticate the
  pinned maintainer key; touch is not required. Merging a workflow does not
  authorize publication; every switch remains
  disabled pending independent review and platform acceptance.
- The maintainer signs source, while exact tagged-workflow attestations bind
  bytes. Runners/toolchains/GitHub/Sigstore remain trusted. One approval in this
  authorized workflow does not mean all publication needs approval: a repository
  writer can publish genuine attested bytes early or freeze an incomplete release.
  Integrity checks reject untrusted bytes; availability damage needs a new signed
  version. See the accepted [threat model](../docs/specs/2026-10-15-distribution-release-design.md#simplified-release-model-operator-decision-2026-10-16).
- `contents: write` exists only in the single `release` job. Build/test jobs have
  no environment or publication secrets. The separate `attest` job has OIDC and
  attestation write authority and does not upload to registries. No AWS, upload
  App, held-storage stage or maintainer byte-inventory signature remains.
- The 2026-10-17 decision selects GitHub only for prereleases and all four
  channels for stable. The genuine alpha.3 attestation is the offline positive
  fixture. Consumer verification binds digest/source/tag, not a unique run.
  Builders never create transparency-log entries during tests.
- Homebrew uses `RELEASE_HOMEBREW_TOKEN`; GHCR uses `GHCR_PUBLISH_TOKEN` plus
  `GHCR_PUBLISH_USER`, all only in `release`, after verification. Configure tap
  permissions, GHCR namespace/linkage/visibility/retention and package Actions
  READ access explicitly. The [image guide](images.md) covers storage and upgrades.
- Final release archives/images include reviewed notices, SPDX and corresponding
  source; native CI fixtures do not constitute licence permission or production
  clearance. Review vendored OpenSSL/SQLite, Rust and frontend assets/fonts.
  Third-party agent binaries and managed Node are never redistributed here.
- Dependency/action/base-image updates and approval of fork workflows remain
  maintainer responsibilities. Generated cargo-dist workflows must not replace
  the explicit orchestrator or bypass archive/linkage/installer/formula checks,
  hardening-before-attestation, pinned actions/toolchain and cache-free builds.

## Homebrew (R2)

The generated formula is hardened with version, embedded-UI and notice checks
before final checksum assembly. The secret-free native proof and tag-run
acceptance exercise a scratch file-based tap; the folded protected publisher
checks the exact frozen formula after GitHub's immutable release bytes match. The channel
and global publication switches remain disabled. See the [operator guide](../docs/homebrew.md)
and [maintainer setup](../docs/release-process.md#homebrew-channel-setup-r2).

Generated with Claude AI — please review before distribution.
