# Collector images (R4)

The collector image is published at
`ghcr.io/marcinwadon/hennery`, for `linux/amd64` and `linux/arm64`.
The 0.1.3 image is published without tags at
`ghcr.io/marcinwadon/hennery@sha256:48de562c392a47c0273b901330601c67d89766af6bf0f637d0fec0df441f9aad`.
From 0.1.4 onward, pull stable releases as
`ghcr.io/marcinwadon/hennery:<version>` (for example `:0.1.4`) or
`ghcr.io/marcinwadon/hennery:latest`. Tags are written only after upload and
remote attestation verification succeed. GHCR currently selects stable releases
only; if enabled for a prerelease, it receives only its version tag.

## What runs here

The image runs `hennery collector`, including its embedded web UI. It copies
the release's musl executable unchanged into digest-pinned distroless static,
with CA roots and user/group **65532:65532**. It contains neither a shell nor
Node, adapters, agent CLIs, credentials or repositories.

Pair separate hennery hosts running beside the repositories. Although the
executable also contains host subcommands, this is not a supported agent-host
image. The managed Node/agent runtime requires **glibc >= 2.28** and Linux
kernel >= 4.18 (and the pinned agents may require more). Distroless **static**
has no glibc loader; a static collector running here does not qualify managed
agents here or on Alpine. Installing glibc/Node/agents in a custom derivative
would require separate qualification and changes this image's contents and
supply-chain record.

## Deployment and storage

Use an independently trusted checkout of the [shared verifier](release/verify.py)
and the full release payload directory plus its separate Sigstore bundles. Supply
an independently selected exact tag and source commit. The verifier freshly authenticates the annotated tag with the pinned
YubiKey fingerprint, checks `SHA256SUMS` and every payload's provenance, then
checks the index or platform digest against authenticated `ghcr-index.json`.
A verifier or key downloaded from an unverified release is not a trust root.
Follow the [trusted bootstrap and release process](../docs/release-process.md).

Verify the index, selected platform manifest and both image/base and binary SPDX
attestations. The exact certificate identity is
`https://github.com/marcinwadon/hennery/.github/workflows/release.yml@refs/tags/<exact-release-tag>`.
The verifier also constrains the GitHub OIDC issuer, source ref and source/signer
commit digests, without requiring a unique producing run or attempt. It never combines mutually
exclusive `--cert-identity` and `--signer-workflow`. Local manifests must hash to
the authenticated descriptor digest; remote subjects use only immutable OCI
references. Keep bundles outside the payload directory to avoid checksum cycles.
BuildKit's embedded provenance is unsigned build metadata; exact tagged-workflow
Sigstore attestations authenticate the final subjects.

Set your deployment's image to `ghcr.io/marcinwadon/hennery@sha256:<verified-index>`;
the angle-bracket value is a placeholder, not an existing release digest.
Use a release's immutable version tag only when selecting the digest. A base
image security rebuild gets a new immutable revision/version and provenance;
never overwrite an existing version tag. Starting with 0.1.4, the publisher
checks an existing version tag against the verified index digest and refuses
different bytes. Stable releases update `latest` after both architectures and
their attestations verify; prereleases never update it. No minor aliases are
created. Pin the verified digest for deployment so later `latest` updates do
not change the selected image.

Publish the collector port on loopback, for example Docker's
`-p 127.0.0.1:8080:8080`, behind a TLS reverse proxy with the intended
`HENNERY_PUBLIC_URL`. Persist `/var/lib/hennery` on **local storage** (SQLite
locking rules exclude network filesystems). A newly created Docker named
volume inherits the image's UID/GID and mode **0700**. A bind mount retains its
host owner and permissions: prepare it for **65532:65532, 0700** before start.
Never mount an agent home into the collector. Protect backups as credentials.

Bound container stdout logs as well as storage. For Docker, use
`--log-driver local --log-opt max-size=10m --log-opt max-file=5`, or equivalent
explicit rotation in your deployment. The collector's service-file rotation
does not apply to container stdout; Docker's default `json-file` driver does
not rotate unless configured. Set memory and process limits appropriate to the
deployment and keep the proxy's TLS admission limits in front of the backend.

Change the listening address/port with `HENNERY_LISTEN`, keeping the container
port mapping in sync. The image's healthcheck reads that environment variable;
changing only a command-line `--listen` would break the healthcheck.
Read setup using `docker exec <container> hennery admin setup-url`; avoid
allocating a TTY or obtaining setup credentials through logs. The admin socket
is local to the data volume. Pair hosts using the normal collector setup.

For an upgrade, record the running digest, stop the collector and make a
consistent backup of the **entire** local volume (database/WAL, configuration,
keys and attachments). Start the new verified digest on that volume, then
check health and load the UI before resuming use. Retain the previous digest
and its matching pre-upgrade backup. A schema upgrade is not necessarily
backward compatible: rollback means restoring that backup with its matching
old digest, with the associated loss of writes since the backup. Do not put
an older executable against an upgraded database without explicit support.

## Signed-source release contract

The single tag-triggered `.github/workflows/release.yml` builds each native image
beside its fresh Linux archive, with the target's unchanged musl executable,
reviewed notices and binary SPDX. It uses the Dockerfile's `release` target,
pinned BuildKit/scanner/bases, native amd64/arm64 runners and no restored cache.
It checks labels, executable bytes, legal files, SBOM/provenance and signing
inputs, then exercises the retained digest through a loopback registry. Smoke
checks health, private writable storage, actual UID and the embedded UI.

For selected `ghcr`, `ghcr-candidate.py` assembles before the global checksum
freeze and materializes afterward for unprivileged data acceptance. The historical
filename conveys no separate candidate stage or publication authority. It emits:

- `ghcr-oci-layout.tar`: the exact index/platform/BuildKit blobs;
- `ghcr-index.json`: index digest, both `platforms.amd64`/`platforms.arm64`
  manifest digests, source SHA and version.

`SHA256SUMS` binds both. `release-metadata.json` records the exact archive
executable hashes in `binaries[target]`; OCI executable bytes must match them.
Notices/SPDX cover Rust, vendored OpenSSL/SQLite and embedded frontend, with
corresponding source and licence/advisory clearance. Image scanner SBOMs also
cover the base filesystem; binary SPDX is attested separately because scanning
cannot discover all static/frontend code.

The read-only shared `packaging.yml` builds archives once, reuses the musl
binaries for images and tests retained OCI tarballs through loopback transport.
Required YAML dependencies gate attestation on all native acceptance. PRs test
the same graph without signing/publication authority; their bytes are not promoted.

The dedicated `attest` job produces seven OCI bundles without registry credentials
or pushes: index provenance, and provenance/image SPDX/binary SPDX for each of
amd64 and arm64. Every subject binds the signed tag, source and accepted digest.
It also attests the global checksum file and every release payload, then writes
the complete asset/hash run summary for maintainer approval.

### Protected publication and recovery

One job in environment `release` verifies the original run's artifacts, creates
a draft, reads back and re-verifies uploaded bytes, and publishes an immutable
GitHub release. After that, folded channel steps require `ghcr` selection plus
`RELEASE_GHCR_ENABLED`, the global switch and GitHub switch. The common gate and
all seven local OCI attestations verify before dedicated registry authentication.
Only the GHCR step receives `GHCR_PUBLISH_TOKEN` and `GHCR_PUBLISH_USER`; there is
no ordinary-job-token fallback. Credentials stay in a temporary restricted config.

The publisher rechecks exact root/layout/bundles at each boundary, reconciles the
remote version/digest and copies only absent content with `--preserve-digests`.
A matching digest skips image upload. Only an authenticated structured absence
permits a new write; differing bytes and auth/network/ambiguous responses refuse.
The initial upload targets the immutable digest. Pinned ORAS attaches the seven
preverified bundles by digest, with fresh checks before each attachment, then
all remote subjects verify. Only this remote readback verifier receives the
job's `GH_TOKEN` for GitHub authentication; its fresh environment excludes
registry credentials and the GHCR publish token. The publisher then rechecks
the version tag, reuses a matching tag or creates an absent one, and updates
`latest` for stable releases. Skopeo preserves the verified index digest and
all platform manifests; each written tag is read back against that exact digest.
An existing version tag with different bytes refuses. No minor aliases or
announcement is written.

After an ambiguous successful image write or any failed bundle attachment, keep
aliases withheld and obtain renewed approval for a publish-only retry. Reconcile
matching image content, reusing original artifacts and attestations. Standard
attachments may duplicate valid provenance/SBOM referrers. Fetched subjects must
verify under the signed-source/tag/digest policy; exact referrer count, creation
timestamp and byte-for-byte referrer manifests are not required. Never rerun all
or rebuild old-version payloads. Missing originals or conflicting image digests
require a new signed version and documentation of the incomplete release.

## Maintainer-only setup

1. Approve `ghcr.io/marcinwadon/hennery`, link it to this repository, explicitly
   set intended public visibility and qualify anonymous digest pulls after the
   authorized first publication. Keep needed image/attestation digests for rollback;
   exclude them from automatic deletion. No external storage or cleanup service.
2. Set package Actions access to READ, with inherited write/admin disabled.
   Qualify the dedicated credential's minimum capability and the branch-push
   negative platform test only with explicit maintainer authorization. Store
   `GHCR_PUBLISH_TOKEN` and `GHCR_PUBLISH_USER` only in `release`.
3. The `release` environment, owner-only tag creation/update-delete rules and
   immutable GitHub releases are already configured. Confirm enforcement,
   signed-tag verification and independent fingerprint bootstrap. The owner
   signs source, not final bytes; runners remain trusted. Repository writers can
   publish genuine attested bytes early or freeze an incomplete immutable release.
   This accepted early-publication/denial-of-service risk rolls forward to a new
   signed version, as documented in the [design amendment](../docs/specs/2026-10-15-distribution-release-design.md#simplified-release-model-operator-decision-2026-10-16).
4. The genuine alpha.3 fixture verifies offline under the source/tag/digest
   policy with pinned `gh 2.101.0`; it does not qualify GHCR's hosted services.
   Builders never mint provenance/log entries. Separately authorize a hosted
   prerelease qualification and all-channel stable qualification before claiming
   that the new publisher's OIDC, approval and remote transport work.
5. Qualify native Linux/Docker and first-run/retry approval behavior. Review the
   run summary before the one `release` approval; no draft exists before approval.
   Retries need renewed environment approval and original immutable artifact IDs.
   Self-approval is allowed. The operator controls publication switches; merging
   code never enables a channel or authorizes publication.
6. After authorized publication, independently verify index/platform/SBOM subject,
   source/tag/workflow identities, anonymous pulls and non-root health/UI/storage
   behavior, then record the immutable deployment digest before announcing success.

Generated with Claude AI — please review before distribution.
