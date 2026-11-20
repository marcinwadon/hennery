# External MCP prompt delivery implementation plan

Implement inline against the [design](../specs/2026-11-19-mcp-default-steering-design.md) using existing SQLite, Rust, MCP and host/fake ACP harnesses.

- [x] Add authorized external_control integration coverage for a blocked active turn, FIFO ordering, explicit queue, admissions/receipts, fallback, late facts, uncertain delivery, scoped uploads and immutable origin. Run on the baseline to observe missing behavior.
- [x] Extend the external tool schema, capability metadata, grant and revision guards and migration. Preserve existing operations on upgrade. Keep ordinary turn ownership separate from shared steering targets.
- [x] Add a scoped external_steering module. Select and claim the queued head atomically, validate its current content and authority and call the shared attempt/transport primitive. Preserve the established idle/parked worker flows.
- [x] Correlate outcomes, timeout, detach and restart to the external operation. Publish canonical input on confirmed admission and retain the original attachment references and target turn. Verify actual delivery across status, wait, replay, queue editing and deletion.
- [x] Update public documentation and owner SQL audit. Run focused tests, generators, formatting, strict workspace and binary Clippy, full workspace tests and doctests. Check generated web changes and verify any affected frontend. Record commands, evidence and limits in the implementation report.
