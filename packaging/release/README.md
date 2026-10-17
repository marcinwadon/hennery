# Signed-source release contract

The authorized publisher is `.github/workflows/release.yml`, triggered only by
annotated `v*` tags signed with the pinned maintainer key. Read the [maintainer
runbook](../../docs/release-process.md) for setup, recovery and accepted risk.
Source signing, hosted builders and GitHub/Sigstore are trust roots; this does
not claim reproducible builds or a maintainer signature over final payloads.

## Assets and verification

GitHub Releases contain seven assets: the three native archives,
`hennery-installer.sh`, `SHA256SUMS`, `hennery-source.tar.xz` and
`hennery.spdx.json` (one combined SPDX document). Each native archive contains
`THIRD-PARTY-NOTICES.txt`; it is not a separate download. The source archive
contains this tagged tree and deduplicated dependency sources under
`hennery-source/third-party/`. Cargo-dist's additional source archive, per-file
checksum sidecars, formula, manifests, advisories and attestation bundles are
not public release assets. Release notes are the release body; the formula
is delivered to the tap.

The five shipped MPL-2.0 crates at 0.5.5 are `base64urlsafedata`,
`webauthn-attestation-ca`, `webauthn-rs`, `webauthn-rs-core` and
`webauthn-rs-proto`. Their source must remain available even when unmodified
([Mozilla's distribution guidance](https://www.mozilla.org/en-US/MPL/2.0/FAQ/)).
`webauthn-authenticator-rs` is test-only. `r-efi` permits MIT or Apache-2.0 as
alternatives to LGPL; the production frontend dependencies are MIT, ISC or
BSD-3-Clause. No GPL/LGPL-only dependency was found. Hennery's own
[AGPL corresponding-source obligations](../../LICENSE) also cover the linked
work: do not discard permissively licensed linked source merely because the
dependency's own licence permits binary-only distribution. Keep the shipped
CLI's normal/build Cargo dependency closure and production frontend packages,
with vendored native source and licence texts. Exclude unrelated workspace/test
dependencies and frontend development tools. Identical files from the three
native builders are stored once; conflicting source bytes refuse assembly.

Public `SHA256SUMS` covers exactly the other six public files. Internally the
same bytes are frozen and attested as `public-SHA256SUMS`, then copied under
`SHA256SUMS` for upload. Official provenance verifies subject digests, independent
of that filename. The full private qualification payload retains its own
`SHA256SUMS`, metadata, sidecars and per-target materials as an immutable Actions
artifact; publisher/channel acceptance still checks that complete closure.

`verify.py` accepts the seven-file download directory. It freshly authenticates
the annotated signed tag and invokes supported `gh attestation verify` for
public checksums and every payload. README/installer authenticate checksums
and each consumed payload before execution/extraction. They retrieve bundles
by digest through GitHub's [anonymous public attestation endpoint](https://docs.github.com/en/rest/users/attestations#list-attestations),
then use the CLI with `--bundle`; no login or token is required. An optional
cached bundle may be supplied outside the public download directory. Bundle
transport is untrusted: cryptography, repository, exact
`release.yml@refs/tags/<tag>` certificate identity, source ref/digest, signer
digest, GitHub OIDC issuer, SLSA predicate and hosted runners are all enforced.
No certificate JSON layout or invocation URI is parsed. Another valid
attestation of identical bytes from that signed source is acceptable. Legacy
schema-2 alpha.3 and private schema-3 qualification directories remain readable;
producing-run fields convey no authority.

The real pinned-CLI test uses genuine alpha.3 checksums, metadata and bundle,
with the existing Sigstore trust-root snapshot and no network or account.
Wrong identity/source/tag/predicate and altered bytes fail. Disposable signed
tag tests independently exercise unsigned/lightweight/wrong-key tag refusal,
bootstrap verification before execution and installer verification before
extraction. Tests never mint provenance or contact a transparency log.

Archives contain only the target directory and `hennery`, changelog, licence,
README and third-party notices. Executable hashes bind the OCI payloads to the
accepted native archive binaries. UI, linkage, source, licence/SBOM and advisory
checks are retained. Native cargo-binstall remains disabled pending a reviewed
verification bridge. Authenticate source before evaluating a release flake.

## Workflow and credentials

Read-only `packaging.yml` shares construction between packaging-input PRs,
main and tags. PRs use Linux only, including the Nix and README recipes in core
CI. Main retains macOS smoke and native packaging/recipes; scheduled maintenance
runs the full macOS/ARM suites weekly. Prepare builds UI/audits once; native
builders reuse musl binaries for images. The expensive seven-crate acceptance
runs on release tags and PRs changing package metadata/version, dist configuration
or crate packaging scripts. Dependency-only Cargo changes and unrelated
`packaging/**` changes do not select it; there is no duplicate main-push crates
acceptance. Assembly still accepts installers/formula and OCI using the archives
that actually ran, with native installer/Homebrew checks for those platforms.
Selected failures stop consumers; intentional crates skips permit assembly.
Actionlint and a small static test cover tag-only publication, permissions,
one protected publisher and pinned actions.

Tags select channels directly: prereleases use GitHub only, stable uses all four.
Local acceptance follows the platform/crate selection above. Unselected crate/OCI
materials are removed from the publishable payload before freezing. PRs have no
publication secrets, OIDC or attestation authority. Release prepare authenticates
the signed tag/version/source and waits for main-push CI for the exact commit.
No restored project build caches are used. A build or attestation rerun cannot
rebuild an old version; retries target only the publisher.

The separate tag-only attester records final digests and immutable artifact IDs.
Only one `publish` job enters environment `release` and receives contents write,
Actions/attestations read and OIDC write. Self-approval is allowed; signing-key
touch is not required. Native environment approval replaces API review-history,
magic approval-comment, 30-day and 50-retry rules. No script claims unique
producing-run provenance. Artifacts still come from the same workflow's immutable
output IDs, not a user-selected name or unrelated run.

The publisher verifies tag/source/digests before writes, publishes complete
immutable GitHub assets, then invokes channel writers with step-scoped
credentials. Homebrew uses the tap-only token, GHCR its dedicated credential,
and crates either explicit temporary bootstrap authority or normal trusted
publishing OIDC, with no fallback. Crate uploads retain the exact seven frozen
tarballs and dependency order. Revoke bootstrap authority on success or
abandonment. Verifier children receive isolated configuration and no publication
or OIDC credentials; the crates writer revalidates through private parent pipes.

`hosting.py` wraps the pinned `gh release` commands. It creates a draft with
`--verify-tag` and `--latest=false`, uploads missing frozen files without
`--clobber`, downloads all existing assets and compares actual byte hashes,
then publishes and requires `isImmutable` in the CLI's JSON. A failed command
stops; only an explicitly missing release permits draft creation. The wrapper
does not implement HTTP routing, redirect authentication or pagination. Each
write command revalidates authority and local bytes; uploads use one CLI batch.
An interrupted batch can leave a partial draft that the next approved attempt
reconciles. No moving GitHub/image aliases are written.

Official pinned provenance/SBOM actions sign final files and OCI digests. The
attester receives no registry credential and never pushes there; it materializes
the accepted OCI layout once. The protected publisher later copies the accepted
image digest and attaches these exact bundles with standard Skopeo/ORAS transport.

The former standalone `release-tests` workflow is consolidated: the exact README
bootstrap runs in `ci/checks`, while real cargo-dist staging/advisory regressions
run on the selected native archive builders (Linux on PRs; all three on main/tags). Native installer acceptance
already consumes the assembled archives on all three targets. There are four
core workflows (`ci`, packaging, release, scheduled maintenance) plus `pins`.

## OCI and recovery

Local provenance and image/binary SPDX bundles are checked against accepted
index/platform manifests before registry login. Standard `oras attach` delivers
bundles by immutable subject digest. Publisher remote readback uses the supported verifier
with `--bundle-from-oci`, exact source/tag/workflow identity and provenance/SPDX
predicates. Public `verify.py --image` instead retrieves provenance/SPDX
bundles by image digest from GitHub, with the same signed-tag/source constraints;
private qualification also binds the image to the accepted native binary inventory.
A deterministic seven-referrer layout and fixed timestamps are not
required; duplicate valid attachments on retry are acceptable.

Retry only publication with renewed environment approval and original artifact
IDs/bytes/bundles. Identical remote bytes are reusable; only missing bytes may
upload. Different or ambiguous state stops, and moving aliases wait for all
channels. Missing originals or a conflicting/incomplete immutable release require
a new signed version. See the runbook for per-channel partial-release recovery.

Local fixtures cannot prove hosted OIDC, protected approvals, immutable-release
settings or remote transport. An operator-authorized hosted qualification remains
necessary before retiring the old publisher. A repository writer may publish
genuine attested bytes early or freeze an incomplete release: the accepted
availability risk is handled by a new signed version, without adding a service.
`publication.py` and `publish.py` are retained only for that proof-gated cleanup;
the active runtime does not import them and has no fallback to them. Their mock
transport tests are replaced by real pinned-CLI hosting tests using genuine
alpha.3 assets and a loopback TLS service. That service proves client integration,
never GitHub's hosted authorization or immutability behavior.

Generated with Claude AI — please review before distribution.
