# Claude v2 steering audit

Inspected the published @agentclientprotocol/claude-agent-acp 0.109.0 npm
archive without installing dependencies or starting real agents.

Findings:

- README explicitly says steering is v1 only.
- dist/v1/steering.js defines _session/steering, parser and runtime priorities.
- dist/v1/acp-agent.js registers the route and advertises _meta.steering.
  steer sets priority now, or later while permission/elicitation is open;
  it also records steered UUIDs to avoid ending the owning turn on the
  intermediate aborted result.
- dist/v2 has neither the steering route nor the priority injection path.
  V2Session.submit constructs an ordinary message and submits it to the input
  queue. Another session/prompt therefore is not a steering replacement.

Upstream evidence:

- https://github.com/agentclientprotocol/claude-agent-acp/issues/1498
  reports the missing route/advertisement and ordinary prompt queueing.
- https://github.com/agentclientprotocol/claude-agent-acp/pull/1499
  is an open steering PR from October 31. It adds the old v1 handler to the
  former v2 bridge and preserves the initialize advertisement.
- https://github.com/agentclientprotocol/claude-agent-acp/pull/1560
  merged November 10, replacing that bridge with a separate native v2 agent.
  Its description explicitly lists steering as not yet served on v2.

Conclusion: an unported adapter extension, not a restriction imposed by ACP v2
or Hennery's Rust SDK. The managed-adapter compatibility gate is justified.
PR1499 is useful behavioral guidance, but its bridge-based implementation is
not a ready unchanged patch for the current native v2 core.

A future patch needs a v2 route and advertisement, native-session injection,
idleBehavior handling, appropriate priorities during user input, and correct
foreground/cancel lifecycle. Cover live steering, idle promptRequired, pending
permission/elicitation survival and interrupted intermediate results before
removing Hennery's gate. No product code changed in this audit.
