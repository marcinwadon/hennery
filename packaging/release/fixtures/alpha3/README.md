# Genuine alpha.3 producer fixture

These files are unmodified public release assets from
`https://github.com/marcinwadon/hennery/releases/tag/v0.1.0-alpha.3`.
They were downloaded over public HTTPS without an account/token on 2026-10-17.
Source: `881f46f5d9a86c66bdd9bff4cef95acfc9cd751a`.
The annotated signed tag object is `a24048ddfe09b0bf9c28e6c3cc7c9cce92e0d819`.

`test_verify_cli.py` pins these file hashes, uses the existing independently
retained Sigstore trusted-root snapshot, verifies the actual Hennery tag/workflow
and source/digest policy with pinned gh 2.101.0 offline, and rejects altered bytes
and wrong identities. This is authentic positive cryptographic evidence; no
certificate field substitution or test signature is used. Legacy producing-run
metadata is preserved verbatim but is no longer an authorization input.

SHA-256:

- `SHA256SUMS`: `044729948b43f415d4ebf71cbe4469d8521c68ad14c2d663907a34a52c3bb08b`
- `release-metadata.json`: `10c3ff9e53b31b95dae543ddbd08d6a86d8b97363e6a1508e0d4f527216feb64`
- `release.sigstore.jsonl`: `a3f768af3af38ef2655bde54891dbdefa60d5b9e96a0e39454d947fd54d41f31`

Generated with Claude AI — please review before distribution.
