# Signed-source release contract

The authorized publisher is `.github/workflows/release.yml`, triggered only by
annotated `v*` tags signed with the pinned maintainer key. Read the [maintainer
runbook](../../docs/release-process.md) for setup, recovery and accepted risk.
Source signing, hosted builders and GitHub/Sigstore are trust roots; this does
not claim reproducible builds or a maintainer signature over final payloads.

## Assets and verification

GitHub Releases contain six uploaded assets: the three native archives,
`hennery-installer.sh`, `SHA256SUMS` and `hennery.spdx.json` (one combined SPDX
document). Each archive contains `THIRD-PARTY-NOTICES.txt`; it is not a separate
download. There are no custom application/dependency source archives. GitHub
provides its automatic **Source code** downloads for the signed release tag;
hennery's AGPL source is also public at that tag and its immutable source commit.
Those automatic downloads are not uploaded assets and are outside `SHA256SUMS`.
Authenticate the signed tag/source commit before using source. Release notes are
the release body and the formula is delivered to the tap.

Per the operator's source-distribution decision of 2026-10-18, embedded notices
identify each MPL crate's name, exact version, crates.io download URL and upstream
repository. The five shipped MPL-2.0 crates at 0.5.5 are `base64urlsafedata`,
`webauthn-attestation-ca`, `webauthn-rs`, `webauthn-rs-core` and
`webauthn-rs-proto`, from <https://github.com/kanidm/webauthn-rs>. Each source
archive is publicly available at
`https://crates.io/api/v1/crates/<name>/<version>/download`; source is not copied
into the release. Conservative build/dev inventories may additionally name the
test-only `webauthn-authenticator-rs`. Missing upstream MPL source locations are
licence findings that prevent freezing. Notices retain the licence texts and
link hennery's signed tag, source commit and GitHub's automatic source archives.

Crate packages likewise point to the immutable public source commit rather
than a custom release asset. Crates preparation retains a local source snapshot
only to create/check its isolated packaging workspace; it is not staged into
the frozen upload artifact. Frozen crate bytes, UI/source markers, source-tree
completeness acceptance and dependency-order upload checks remain enforced.

Public `SHA256SUMS` covers exactly the other five public files. Internally the
same bytes are frozen and attested as `public-SHA256SUMS`, then copied under
`SHA256SUMS` for upload. Official provenance verifies subject digests, independent
of that filename. The full private qualification payload retains its own
`SHA256SUMS`, metadata, sidecars and per-target materials as an immutable Actions
artifact. The publish job verifies the signed source, complete payload closure and
all local OCI provenance once, in its first verification step. That step writes a
job-local manifest of verified digests. GitHub, Homebrew, crates and GHCR then
check the frozen files they send against that manifest, without repeating full
qualification or cryptographic verification. Writers run only after verification
succeeds; retries rerun verification against the original downloaded artifact.
Final remote readbacks remain: public immutable GitHub assets, tap bytes, crate
bytes/index checks, and GHCR subjects and named tags.

`verify.py` accepts the six-file download directory. It freshly authenticates
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

OCI provenance uses `--predicate-type https://slsa.dev/provenance/v1`. The image
and binary SBOM attestations produced by `actions/attest-sbom` require the exact
`--predicate-type https://spdx.dev/Document/v2.3`, also accepted by `verify.py`.
BuildKit's retained in-toto SBOM statements use the separate unversioned
`https://spdx.dev/Document` predicate; those are inputs to the official action,
not its signed output.

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

Read-only `packaging.yml` constructs packaging-input PR and main candidates.
PRs use Linux only. Main qualifies all three native archives and installers;
scheduled maintenance runs the full macOS/ARM suites weekly. Prepare builds
UI/audits once; native builders reuse musl binaries for images. Stable main
candidates qualify all seven crates and retain all four channels' payloads.
PRs select crate acceptance for package metadata/version, dist configuration or
crate packaging script changes. Assembly accepts installers/formula and OCI
using the archives that actually ran, with native Homebrew checks on every target.
Selected failures stop consumers; intentional crates skips permit assembly.
Actionlint and a small static test cover tag-only publication, permissions,
one protected publisher and pinned actions.

Tags select channels directly: prereleases use GitHub only, stable uses all four.
Local acceptance follows the platform/crate selection above. Unselected crate/OCI
materials are removed from the publishable payload before freezing. PRs have no
publication secrets, OIDC or attestation authority. Release prepare authenticates
the signed tag/version/source and waits for main-push CI for the exact commit.
It performs fresh audits and promotes a successful main packaging run for that
exact SHA, downloading the payload and all three native acceptance proofs by
immutable IDs and verifying their ZIP digests. Native proofs must bind the
original payload's private checksum list. Public archives/crates/images are
unchanged; new private metadata binds the authenticated tag object, and the
attested payload records the main input IDs/digests and fresh audit reports.
Tags require no second macOS build. A build or attestation rerun cannot
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
or OIDC credentials. The crates parent verifies the full release once before
choosing the registry token, then passes only that token and the verified inventory
digest to the isolated uploader. Before each upload the uploader checks the local
inventory digest and the SHA-256 of the exact crate bytes it sends. Registry polling
does not repeat release verification. API/CDN and index propagation share an
eight-minute window per crate (48 polls at ten seconds, including request time).
The writer deadline derives from those same limits and seven crates: 75.5 minutes,
including request bounds and startup/local hashing allowance.

`hosting.py` wraps the pinned `gh release` commands. It creates a draft with
`--verify-tag` and `--latest=false`, uploads missing frozen files without
`--clobber`, downloads all existing assets and compares actual byte hashes,
then publishes and requires `isImmutable` in the CLI's JSON. A failed command
stops; only an explicitly missing release permits draft creation. The wrapper
does not implement HTTP routing, redirect authentication or pagination. Each
write command revalidates authority and local bytes; uploads use one CLI batch.
An interrupted batch can leave a partial draft that the next approved attempt
reconciles. No moving GitHub aliases are written. Starting with 0.1.4, GHCR
version tags and the stable-only `latest` tag follow successful image readback.

Official pinned provenance/SBOM actions sign final files and OCI digests. The
attester receives no registry credential and never pushes there; it materializes
the accepted OCI layout once. The protected publisher later copies the accepted
image digest and attaches these exact bundles with standard Skopeo/ORAS transport.

The former standalone `release-tests` workflow is consolidated: the exact README
bootstrap runs in `ci/checks`, while real cargo-dist staging/advisory regressions
run on the selected native archive builders (Linux on PRs; all three on main/tags). Native installer acceptance
already consumes the assembled archives on all three targets. There are four
core workflows (`ci`, packaging, release, scheduled maintenance) plus `pins`.

## Pinned action compatibility (2026-10-19)

The action upgrade has no contract change requiring a publisher or verifier
migration. The pinned
[`attest-sbom` v4.1.0 wrapper](https://github.com/actions/attest-sbom/blob/c604332985a26aa8cf1bdc465b92731239ec6b9e/action.yml)
forwards to `actions/attest` v4.1.0. Its
[SBOM producer](https://github.com/actions/attest/blob/59d89421af93a897026c735860bf21b6eb4f7b26/src/sbom.ts)
derives `https://spdx.dev/Document/v2.3` from `SPDX-2.3`, matching `verify.py`.
The pinned
[`attest-build-provenance` v4.2.2 wrapper](https://github.com/actions/attest-build-provenance/blob/4d101475d8b20a2381f78447822ac1eab6504dd8/action.yml)
forwards to `actions/attest` v4.2.1, whose bundled SDK still produces SLSA v1
with the source ref, commit and workflow identity consumed by the verifier.
Both wrappers retain `bundle-path`; their producers write the SDK's Sigstore
v0.3 DSSE bundle as newline-terminated JSON, the same media type as our genuine
alpha.3 fixture (`application/vnd.dev.sigstore.bundle.v0.3+json`). Our consumer
delegates bundle decoding and cryptographic verification to the pinned CLI.
The new storage-record feature
requires registry push, which this attester disables; no extra permission is
needed. The SBOM wrapper is deprecated upstream; migration to `actions/attest`
can be a separate update.

[`upload-artifact` v7.0.2](https://github.com/actions/upload-artifact/blob/cf430e030ddbb5b0abf93d22962f4752f3646cd9/action.yml)
keeps ZIP upload as the default (`archive: true`), retaining our explicit names
and `artifact-id` output. Its new direct-file mode would ignore `name`, but is
unused. Hidden files remain excluded, overwrite remains disabled, and extracted
file/directory modes remain 0644/0755; executable binaries stay inside tarballs.
[`download-artifact` v8.0.2](https://github.com/actions/download-artifact/blob/9000827ccba6bdab643e8b6fd33ac0654aef8333/src/download-artifact.ts)
preserves named downloads and our pattern grouping. The single-artifact ID path
change introduced in v5 is already covered by our explicit `merge-multiple:
true`; the candidate/image patterns select multiple artifacts. v8 now fails on
digest mismatch and still decompresses ZIPs by default. Same-run downloads use
the Actions runtime credential; explicit token downloads retain `actions: read`.
The [upload v7](https://github.com/actions/upload-artifact/releases/tag/v7.0.0),
[download v8](https://github.com/actions/download-artifact/releases/tag/v8.0.0)
and [SBOM](https://github.com/actions/attest-sbom/releases/tag/v4.1.0) and
[provenance](https://github.com/actions/attest-build-provenance/releases/tag/v4.2.2)
release notes were checked alongside these pinned sources.
The actions use Node 24 on GitHub-hosted runners. Hosted signing/transport still
requires the next authorized release; local tests cannot exercise OIDC.

## OCI and recovery

Local provenance and image/binary SPDX bundles are checked against accepted
index/platform manifests before registry login. Standard `oras attach` delivers
bundles by immutable subject digest. Publisher remote readback uses the supported verifier
with `--bundle-from-oci`, exact source/tag/workflow identity and provenance/SPDX
predicates. Only that readback call receives the step's `GH_TOKEN` for GitHub
authentication, in an isolated environment without registry publication authority.
After all remote subjects verify, Skopeo copies the verified index digest to
the version tag and, for stable releases, `latest`, preserving all digests.
Matching version tags are reusable; conflicting version tags refuse. Every
written tag is read back and hashed against the verified index digest.
Public `verify.py --image` instead retrieves provenance/SPDX
bundles by image digest from GitHub, with the same signed-tag/source constraints;
private qualification also binds the image to the accepted native binary inventory.
A deterministic seven-referrer layout and fixed timestamps are not
required; duplicate valid attachments on retry are acceptable.

Retry only publication with renewed environment approval and original artifact
IDs/bytes/bundles. Identical remote bytes are reusable; only missing bytes may
upload. Different or ambiguous state stops; GHCR tags wait for successful OCI
upload and readback. Missing originals or a conflicting/incomplete immutable release require
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
