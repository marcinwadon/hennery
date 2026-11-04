# hennery

Self-hosted cockpit for ACP coding agents. Part of [hennery](https://github.com/marcinwadon/hennery),
a self-hosted ACP cockpit, licensed under AGPL-3.0-only (see LICENSE).

The internal libraries support the application and have pre-1.0 compatibility;
they are not a separately stable SDK. All seven production crates use one version.

Source installation requires Rust at the workspace rust-version, a C compiler
and a maintained OpenSSL development package. On Debian/Ubuntu install
`build-essential pkg-config libssl-dev`; on macOS use Xcode Command Line Tools
and Homebrew `openssl@3`, setting `OPENSSL_DIR="$(brew --prefix openssl@3)"`.
Since 0.5.1, Cargo builds use platform OpenSSL; the old `vendored-openssl`
feature is removed because its source crate supplies an unsupported library.
Node and pnpm are not required: the kernel package carries the compiled UI.

Use an explicitly published version and `--locked` for source installation.
The release pipeline qualifies installation from the actual packaged crates.

Native cargo-binstall stays disabled pending a reviewed verification bridge.
Release consumers first authenticate the exact inventory with the independently
confirmed maintainer OpenPGP fingerprint, then verify the exact tagged Sigstore
identity using trusted source. Follow the repository's reviewed verify-then-run
bootstrap before executing an installer or release-contained verifier.

Generated with Claude AI — please review before distribution.
