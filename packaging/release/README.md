# Signed-source release contract

The authorized publisher is `.github/workflows/release.yml`, triggered only by
annotated `v*` tags signed with the pinned maintainer key. Read the [maintainer
runbook](../../docs/release-process.md) for setup, recovery and accepted risk.
Source signing, hosted builders and GitHub/Sigstore are trust roots; this does
not claim reproducible builds or a maintainer signature over final payloads.

## Assets and verification

`SHA256SUMS` is the canonical sorted checksum closure of the flat payload.
Missing, extra, duplicate, case-ambiguous or linked files refuse. Bundles are
kept separately to avoid checksum cycles. Metadata binds repository, signed tag
object, source SHA, channels/targets and executable inventories. New schema 3
has no producing-run fields. Alpha.3 schema 2 remains readable; its `attesting`
field is historical data and conveys no authority.

`contract.py` freshly authenticates the annotated signed tag and invokes the
supported `gh attestation verify` for the checksum list and every payload. It
requires repository, exact `release.yml@refs/tags/<tag>` certificate identity,
source ref/digest, signer digest, GitHub OIDC issuer, SLSA predicate and hosted
runners. The CLI validates subject digests and cryptography. No certificate JSON
layout or `runInvocationURI` is parsed; another valid attestation of identical
bytes from that signed source is acceptable. Standalone consumers no longer
accept `--run-id` or `--attesting-attempt`.

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

Read-only `packaging.yml` runs the same nine packaging jobs for packaging-input
PRs/main and tags. Prepare builds UI/audits once; three native builders reuse
musl binaries for images. Linux qualifies all seven crates. Assembly reuses
archives for installers/formula, then three native jobs accept installer and
Homebrew, with one packaged CLI install on macOS. Required native YAML `needs`
and failure propagation govern acceptance. Actionlint and a small static test
cover syntax/dependencies, tag-only publication, permissions, one protected
publisher and pinned actions. The channel/name registry, rehearsal compiler and
bespoke workflow/DAG policy are removed.

Tags select channels directly: prereleases use GitHub only, stable uses all four.
All channels receive local acceptance on packaging PRs. Unselected crate/OCI
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
run on all three existing native archive builders. Native installer acceptance
already consumes the assembled archives on all three targets. There are four
core workflows (`ci`, packaging, release, scheduled maintenance) plus `pins`.

## OCI and recovery

Local provenance and image/binary SPDX bundles are checked against accepted
index/platform manifests before registry login. Standard `oras attach` delivers
bundles by immutable subject digest. Remote readback uses the supported verifier
with `--bundle-from-oci`, exact source/tag/workflow identity and provenance/SPDX
predicates. A deterministic seven-referrer layout and fixed timestamps are not
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
