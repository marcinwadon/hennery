# Selecting CI work

Core CI always starts and its result job requires success for every selected
job. Unknown non-documentation paths conservatively select Rust, web and the
main-branch macOS smoke test. Application and shared toolchain changes retain
those checks; PRs keep their existing Linux-only platform policy.

The Nix package job lives in `.github/workflows/nix-package.yml`, called by
`ci.yml`. A change confined to that reusable workflow selects Nix on both Linux
architectures and, on main, macOS. It does not select source tests or README
recipes. Changes to the shared caller `ci.yml` still select the full checks.
The macOS buffer/output fix and error propagation remain in the reusable job.

The selector, standalone CI/runtime drivers and their regression tests are an exact infrastructure allowlist:
`checks` executes their tests, without building the application. A mixed change
with application code still selects application checks. Documentation-only main
pushes no longer build a duplicate Nix package; README edits retain recipe checks.

Packaging reacts to its own workflows (`packaging.yml`, `release.yml`) and
existing package/build inputs. An unrelated workflow edit does not rebuild
archives or repeat installer acceptance. The standalone CI/runtime drivers are
also excluded by exact path; their Python regressions run in `checks`.
Package workflows, Nix inputs, locks,
adapter inputs and package metadata retain packaging qualification.

Run `nix develop .#release -c python3 -m unittest discover -s packaging -p
test_ci_paths.py` and actionlint when editing this selection. The tests exercise
actual Git changes, including deleted/renamed files and mixed changes. A missing
Git base fails rather than silently reporting unselected jobs.

These changes are prepared independently of the 0.5.1 release. They neither
reuse evidence from a different source commit nor change release publication,
signed tags or the exact-main-CI requirement.
