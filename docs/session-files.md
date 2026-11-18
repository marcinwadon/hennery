# Session files

Use the composer's paperclip, paste or drop a file, then send the message.
Images retain their native image handling. Other files, such as Markdown,
PDF, CSV or archives, appear by filename and are copied to the agent host.
The agent receives a local path and reads the file with its available tools.
Support for understanding a particular format depends on those tools.

The session's **Details → Files** panel contains sent attachments and published
outputs. On a desktop, Files is a section of the details sidebar; on a phone it
is a tab. An attachment's original filename is preserved when downloading.

## Publish an output

Ask the agent to publish its completed output using `artifact_publish`. Updated
hosts receive the `hennery_files` MCP server at session start or resume, subject
to the existing hat/runtime MCP isolation policy. File tools work independently
of the Delegation switch.

For example, the logical tool call is:

```json
{
  "path": "reports/analysis.md",
  "request_key": "analysis-first-publication",
  "title": "Analysis"
}
```

The tool returns an `artifact_id` and `revision`. To publish a new version of
that output, pass its `artifact_id`, the last observed `expected_revision`, a
new `request_key`, and the updated file's path. `artifact_list` returns latest
revisions in pages; pass `next_cursor` as `cursor` to continue.

You can also publish an existing file yourself: open Files, enter a path under
the session's working directory and choose **Publish file**. Select an existing
artifact under **Publish as** to create its next version.

## Versions and downloads

Publication saves an immutable copy on the collector. Later edits to the source
file become a new version only after another explicit publication. Existing
versions can be downloaded while the host is offline or after the source
worktree is removed.

A transcript card downloads the version published at that point. Files shows
the latest version of each artifact, with **Version history** for older ones.
Markdown, plain text, CSV and JSON outputs up to 256 KiB offer a safe preview.

Concurrent updates must name the expected latest revision. A stale publication
is refused so the agent or operator can review the newer result. Retrying an
uncertain publication with the same arguments and request key returns the
original result, even if the source file has since changed.

## Bounds and lifecycle

- A prompt takes up to 20 binary attachments, 5 MiB each and 16 MiB in total.
- A published artifact revision takes up to 16 MiB.
- Publication reads regular files below the session's working directory.
  Symlinks and special files are refused.
- These are transfer bounds; there is no total attachment storage quota.
- Deleting a session or purging its hat removes the collector references and
  unshared stored bytes. Host-managed input copies are queued for removal and
  retried when their host reconnects. Workspace originals follow the project's
  own lifecycle.

Both collector and host need a build that supports session files. Older hosts
continue to accept their existing image prompts; other file transfers explain
that the host needs an update. The external session-control MCP's separate
image-staging tools keep their existing image-only contract.
