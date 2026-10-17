# Release process

The operator approved the CI/release simplification on 2026-10-17. Alpha.3 has
shipped through the previous pipeline; its genuine offline attestation fixture
is retained under `packaging/release/fixtures/alpha3`. Stable releases publish
all four channels; prereleases publish GitHub only. Merging pipeline changes
never authorizes a tag, publication, credential or settings change. The operator
separately authorizes hosted prerelease qualification before retiring the old
publisher in the final migration step.

## Trust and maintainer setup

The maintainer reviews the source and `.github/workflows/release.yml`, then
creates a YubiKey-signed annotated `v<version>` tag. The signing fingerprint is
`F26C7F594DFE84D131703AFC23E5A318AE8D2861`. A physical touch is not a release
requirement. `tag_signature.py` verifies a freshly fetched annotated object,
its direct commit target and the pinned key in an isolated public-only keyring.
The signing key's custody remains the maintainer's responsibility.

The source signature authorizes source. GitHub/Sigstore provenance binds payload
digests to the exact tag/workflow and source SHA, without binding a unique run or
attempt. Another valid attestation of identical bytes from that signed source is
accepted. Runners, compiler/dependencies, GitHub/OIDC/Sigstore and installed
verifier tools remain trusted. This does not claim reproducible builds or a
later hardware signature over bytes.

The `release` environment, tag rulesets and immutable-release setting are
already configured, according to the parent. The maintainer verifies their
actual settings before enabling publication:

- `release` requires exactly the maintainer's User review, permits only `v*` tag
  deployments and explicitly disables administrator bypass. Self-review is
  permitted for the sole maintainer. No publishing credentials are repository
  secrets. The examples in `.github/release-environment.example.json` and
  `.github/release-branch-policy.json` describe this boundary.
- The creation ruleset permits only the owner's User ID to create `v*`; the
  separate update/deletion ruleset has no bypass actors. Replace placeholder
  owner ID 0 in the checked-in examples. Never move/delete tags or reuse versions.
- Immutable releases are enabled. The approved publisher creates a draft,
  uploads and reads back the complete asset set, publishes and requires GitHub's
  immutable state. An incomplete frozen release requires a new signed version.
- The built-in job `GITHUB_TOKEN` supplies `contents: write` only in the one
  environment-protected `publish` job. There is no stored upload credential,
  upload App, staging environment, held storage or settings-audit approval tuple.

**Accepted limitation:** one approval is guaranteed in the authorized workflow,
not for every possible repository publication. A repository writer, including
someone using the maintainer's SSH push key, can run a branch workflow requesting
`contents: write`. It can publish genuine already-attested bytes early for an
existing tag, or freeze an incomplete immutable release and block that version.
Consumers reject bytes without the signed source/tag and exact tagged-workflow
provenance. This is accepted early-publication/denial-of-service exposure, not an
artifact-tampering guarantee failure. No additional service or App is authorized.
Document the affected release, withhold moving aliases and roll forward with a
new signed version.

## Switches, credentials and acceptance

Switches default off; only the operator changes their platform values:
`RELEASE_PUBLICATION_ENABLED`, `RELEASE_GITHUB_ENABLED`,
`RELEASE_HOMEBREW_ENABLED`, `RELEASE_CRATES_ENABLED`, `RELEASE_GHCR_ENABLED`
and the temporary `RELEASE_CRATES_BOOTSTRAP_ENABLED`.
Channel selection is fixed by tag: prereleases use GitHub only; stable uses
GitHub, Homebrew, crates.io and GHCR. The corresponding switches must also permit
publication. Additional channels require complete immutable GitHub publication.

Long-lived channel credentials exist only as `release` environment secrets and
are passed only to their final channel step, after verification. Homebrew uses
`RELEASE_HOMEBREW_TOKEN`, limited to contents write in the public
`marcinwadon/homebrew-tap` repository's `main` branch, with no workflows/admin
permission or main-repository write authority. The maintainer confirms the scope
of a fine-grained PAT or already supplied tap-only App installation token; this
pipeline never mints/refreshes an App token. `GHCR_PUBLISH_TOKEN` is a dedicated
GHCR credential, with `GHCR_PUBLISH_USER` as a non-secret environment variable.
Confirm the registry-supported credential kind and least required package scope.
The ordinary `GITHUB_TOKEN` must have package Actions access READ, without
inherited write/admin access.

After setup, the maintainer separately authorizes GHCR's disposable branch-job
negative acceptance, using only its ordinary `GITHUB_TOKEN` with requested
`packages: write`. `post_setup.py` first requires an authenticated 200 read of a
known digest, then an authoritative 403 from the attempted canary write.
Authentication/network errors or a failed read do not prove denial. If this
creates a canary, stop and repair access before enabling GHCR. The test itself
can write if setup is wrong; it was not run during implementation.

`settings_audit.py` is a maintainer-local inspection helper, never called by a
publisher. It checks the signed tag before using the maintainer's own settings
access and records API settings plus explicitly confirmed registry
observations. Its `--payload`, `--observations` and create-only `--output` inputs
produce a local acceptance record. This report is neither publication authority
nor an approval digest. Start from
`packaging/release/example-observations.json`; replace observations with verified
facts, never merely copy them. No CI administration credential is needed.

The offline verifier test runs the pinned CLI against genuine alpha.3 checksums,
metadata and provenance, with network access disabled by proxy. Altered bytes,
wrong source, repository, workflow, tag, issuer or predicate must fail. It creates
no signatures or transparency-log entries. The fixture does not prove the new
workflow's hosted OIDC issuance, approval handling or remote publication.

GitHub's protected environment supplies one approval per publishing attempt,
including publish-only retries. Self-approval is permitted for the sole
maintainer. No API review-history interpretation, special comment, age limit or
rerun count grants authority. Confirm actual first-run and retry behavior during
the separately authorized hosted qualification.

## crates.io bootstrap, then trusted publishing

Normal operation exchanges OIDC only after verification. Every one of the seven
production crates must trust repository `marcinwadon/hennery`, workflow filename
`release.yml` and environment `release`; repository/workflow alone is insufficient.
A failed exchange never falls back to a stored token.

For the first claim of unregistered crate names, the maintainer:

1. Confirms ownership/availability of all seven names and corresponding source,
   reviews the frozen packages and authorizes this channel explicitly. Creates
   one short-lived crates.io API token scoped to publishing new crates/versions
   for this exact seven-crate closure, without owner/admin/yank privileges.
   Stores it only as `RELEASE_CRATES_BOOTSTRAP_TOKEN` in `release` and records
   the expiry/revocation owner locally. No token is committed or printed.
2. Reviews the stable all-channel release and separately enables the
   global/GitHub/crates switches and temporary bootstrap switch only after the
   relevant acceptance above. Approves the same protected `publish` job. The
   explicit bootstrap mode uses the existing frozen-byte API uploader; it never
   invokes `cargo publish` or rebuilds packages.
3. The uploader re-verifies immutable GitHub bytes and exact provenance before
   credentials and each write, then publishes in dependency order:
   `hennery-proto`, `hennery-kernel`, `hennery-view`, `hennery-host`,
   `hennery-gateway`, `hennery-sessions`, `hennery`.
   Existing identical crate versions are idempotent; mismatching bytes,
   ambiguous state or missing source refuse. Registry visibility is checked
   before dependent crates proceed.
4. Once names exist, configures trusted publishing for **each** crate with that
   exact repository, `release.yml` and `release` environment. Confirms all seven
   settings and the ordinary branch/environment-less OIDC refusal. A partial
   configuration cannot qualify normal publication.
5. Revokes the bootstrap token at crates.io, confirms revocation, removes the
   environment secret and disables `RELEASE_CRATES_BOOTSTRAP_ENABLED` on both
   success **and abandonment**, including partial or cancelled bootstrap. This
   is maintainer cleanup, not a claim that the workflow can revoke a long-lived
   token. Then qualifies normal OIDC publication; no stale secret fallback.

If bootstrap stops after some crates, keep the dependency order and reconcile
already-existing identical versions under renewed approval while the original artifacts remain available. If abandoning, revoke immediately. Restart later with a new explicitly
scoped bootstrap token and, when required, a new signed version. Never replace a
published crate or treat a registry error as absence.

## One tagged run, one protected publisher

The tag-only `release.yml` calls read-only reusable `packaging.yml`. Prepare
requires successful main-push core CI for the exact signed commit, builds the UI
and audits dependencies once. Native builders share their archives with image,
installer and Homebrew acceptance; Linux qualifies all seven crate tarballs.
Every local channel acceptance runs for packaging PRs and releases, regardless
of publication selection. No restored project build cache is used in releases.
Actions remain commit-pinned. Native YAML dependencies and actionlint replace
custom job-name, DAG and command-order policing.

Build/acceptance jobs have read-only contents and no publishing secrets. The
separate `attest` job alone adds OIDC/attestation write. It attests `SHA256SUMS`
and every payload, then writes a run summary showing every asset/hash, bundle
hash, tag/source, channels/targets and immutable artifact IDs.
The maintainer approves that summary. There is no draft before approval; this
is the deliberate trade-off for keeping all authorized draft mutation protected.

The single `publish` job enters `release`, downloads the immutable artifact ID
from the original producing attempt, verifies the checksum closure, tag and
attestations, creates/uploads the draft, reads back/re-verifies uploaded bytes,
publishes and requires immutable state. It then serially runs selected/enabled
Homebrew, crates and GHCR steps. Each re-verifies the immutable public bytes and
provenance before its credential/login/OIDC exchange and at write boundaries.
The job has contents write, Actions/attestations read and OIDC write; other
permissions are absent. Job permissions exist at job start; scripts do not mint
the built-in token only after verification.

Authenticated GitHub transport stays in the publisher parent. Verifier children
receive only an explicit credential-free allow-list and fresh private Git/GPG,
GitHub CLI and Docker/ORAS configuration, including no GitHub or Actions/OIDC
tokens. The crates writer asks the parent to revalidate over private pipes; it
does not spawn a verifier with its upload token.

Consumers independently verify the signed tag, the attested canonical
`SHA256SUMS` and each consumed payload. The exact certificate identity is
`https://github.com/marcinwadon/hennery/.github/workflows/release.yml@refs/tags/<exact-tag>`.
Use exact `--cert-identity`, source ref and source/signer digest constraints,
GitHub's OIDC issuer and hosted-runner requirement. Do not combine the mutually
exclusive `--cert-identity` and `--signer-workflow` options. No certificate JSON field parsing or invocation-URI matching is
required. The
supported CLI enforces these identity and digest constraints. New schema-3
metadata omits producing-run identity; legacy schema-2 alpha.3 metadata remains
readable, with its historical `attesting` field ignored as authority.

Use independently trusted installed verification tools/key and reviewed source;
never execute a downloaded installer/verifier to authenticate itself. Native
cargo-binstall remains disabled without a reviewed bridge enforcing this policy.
Authenticate source before evaluating a release flake, and use its verified full
commit SHA. Preserve corresponding source, licence notices and rollback backups.

## Failure and recovery

Retry **only `publish`**, with renewed environment approval and the original
immutable artifact IDs, accepted payloads, checksum list and bundles. No special
approval comment, recovery-age limit or retry-count budget applies. Never rerun
all or rebuild under an old version. Producers/attesters refuse attempts beyond
the initial build; a later publisher is not a new producer. Missing, deleted or
expired originals, conflicting remote bytes or an incomplete immutable release
require a new signed version. Withhold aliases and document the incomplete
release until recovery is verified.

- Before immutable GitHub publication, reconcile the draft's exact asset set.
  Upload only verified missing assets; extra/different bytes refuse. If the
  publish response is lost or ambiguous, first inspect/re-verify the public
  immutable release. A matching complete immutable release is an idempotent
  success on retry; an incomplete/conflicting frozen release needs a new version.
- After Homebrew, read back the exact frozen formula and confirm its atomic CAS
  update. Identical content is a no-op. Same-version differences, downgrade,
  parse failure, concurrent update or ambiguity stop; reconcile before retry.
- After each crate, reconcile registry/index/download digests before continuing.
  A partial graph resumes only with identical bytes; wrong digests/ambiguous
  authentication stop. Apply bootstrap revocation on abandonment even if some
  names/versions already exist. Published crates cannot be replaced.
- After GHCR, preserve and reconcile the accepted index/platform digests. Attach
  preverified provenance and SBOM bundles with standard ORAS transport, then use
  `gh attestation verify --bundle-from-oci` on fetched immutable subjects with the
  same tag/source/workflow policy. No canonical seven-referrer manifest set or
  fixed creation timestamp is required. Duplicate valid attachments on retry are
  acceptable; different image digests, invalid provenance and ambiguous writes
  stop publication. Do not move aliases until every selected channel succeeds.

Public install, Homebrew service/PATH, crate UI/source and image runtime
acceptance remain maintainer-owned after authorized publication. Stop services,
back up complete data/config/keys and check schema compatibility before rollback.
Restore a compatible backup or ship a forward fix rather than overwriting
immutable versions. Retain corresponding source and document remediation.

Generated with Claude AI — please review before distribution.
