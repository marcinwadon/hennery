# Watching CI without repeated model turns

`packaging/ci_watch.py` watches explicit GitHub Actions run IDs for one full
source SHA. It is read-only: it never reruns jobs, changes branches, merges,
publishes or contacts another session. GitHub authentication comes from `gh`.

Run from the release dev shell on the host that owns the work:

```sh
nix develop .#release -c python3 packaging/ci_watch.py \
  --repo marcinwadon/hennery --sha <full-source-SHA> \
  --run <ci-run-id> --run <packaging-run-id> \
  --state .superpowers/ci-watch/state.json
```

The process polls internally, prints only changed job/run states and atomically
saves the latest state. Jobs are paginated and tied to the current run attempt.
A failed job returns immediately, even while another job is still running.
Cancelled, skipped or timed-out workflows never count as successful runs.
Workflow success remains authoritative for deliberately skipped optional jobs.

Exit codes: 0 all requested runs succeeded; 1 a job/run failed; 2 three consecutive
API errors or invalid source/run responses; 3 overall timeout or no job/run
transition for `--stall-after` seconds (default 30 minutes); 4 callback failure.
A stall requests investigation; it does not assert that compilation has failed.
The overall default deadline is two hours. Polling defaults to 30 seconds.

For an existing external job supervisor, append `--notify /absolute/callback`.
On exit the watcher invokes that executable once, appending the absolute state
file path. Arguments are passed directly, without shell evaluation. The callback
has 30 seconds and should enqueue a notification, not perform the next release.
Only configure a callback for the owning session. Read the saved outcome before
deciding whether to proceed; receiving a notification alone is not success.

The watcher does not provide a durable supervisor or automatically wake a parked
Hennery session. That integration belongs to the host's job service. A shell
background process may die when its owning session parks; do not use `nohup` as
a substitute for supervised ownership. In a foreground tool session, wait for
its completion rather than repeatedly asking the model to interpret unchanged
logs. Leave publication to the original authorized release workflow.

Run offline tests with `nix develop .#release -c python3 -m unittest discover -s
packaging/tests -p test_ci_watch.py`. Core CI already discovers that directory.
