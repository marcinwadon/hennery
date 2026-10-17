# Offline Sigstore trust root

`trusted_root.json` is the unmodified public-good trusted-root snapshot from
`cli/cli` commit `b62671151b7c98852b7023dd672f5092d1c9dab0`, directory
`pkg/cmd/attestation/test/data`. `LICENSE.cli` retains the upstream licence.
SHA-256: `455b3fe53e2678889f093d66ebbfda5f12ebb9d682b146c3535fa04e97689787`.

The pinned CLI uses this root only in offline tests of the genuine Hennery
alpha.3 release in `../alpha3`. Production uses the supported verifier's normal
trust-root handling. No certificate JSON layout is an authorization input.
The former substituted upstream payload fixture was retired when the genuine
Hennery producer output became available. Tests never mint signatures or logs.

Generated with Claude AI — please review before distribution.
