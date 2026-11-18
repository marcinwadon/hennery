# Session files

The operator approved generic prompt attachments and explicitly published,
versioned output files. This extends the image-only content contract in the
umbrella, ACP core and frontend specs.

- Prompt files retain their original name, MIME type and immutable bytes in the
  collector's content-addressed attachment store. The host materializes them in
  a private, session-specific staging directory and passes the local path as
  text context. Images retain their existing native ACP representation.
- File transfer uses the existing bounded prompt envelope: at most 20 binary
  blocks, 5 MiB each and 16 MiB together. These are transfer bounds, not a
  lifetime storage quota. Arbitrary bytes are accepted; readability depends on
  the agent's tools. No implicit archive extraction or document conversion.
- An artifact has a stable ID scoped to one owner and session. Explicit
  publication creates an immutable revision containing its original filename,
  title, MIME type, digest, size and publication time. File changes alone never
  publish a revision. A retry key returns its original revision; updating an
  artifact requires its ID and expected current revision to prevent lost updates.
- The host reads regular files beneath the session's canonical working directory
  only, without following symlinks. Publication captures bytes on the collector,
  so downloads survive host disconnection, file changes and worktree removal.
  A read is bounded to 16 MiB and shares bounded host probe admission.
- The transcript pins the revision published at that point. The Files panel
  lists latest outputs and revision history, plus prompt attachments. The
  operator can publish an existing host file manually, or the agent can use
  session-scoped file MCP tools. These tools do not grant delegation authority.
- Downloads require operator authentication, use attachment disposition and
  nosniff, and are not rendered as active documents. Text/Markdown previews use
  the existing safe renderer. Session deletion and hat purge remove artifact
  references and bytes no remaining owner/reference needs. In-flight publishing
  rechecks session/token/hat authority at commit.
- Host file capability is negotiated; older hosts reject generic attachments
  and publication visibly. Existing image delivery remains compatible.

Real-agent acceptance belongs to the operator's separate automation. Local
verification uses synthetic files and fake agents, never real Hennery data.
