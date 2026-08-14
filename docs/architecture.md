# Architecture boundaries

This repository uses explicit ownership rules so process boundaries do not become dependency
shortcuts.

## Renderer

Dependencies flow toward shared UI, never back toward the application shell:

```text
app (composition and navigation)
  -> features (domain UI and controllers)
    -> components (domain-neutral UI)
      -> config / host / protocol
```

- `src/renderer/src/app` composes pages, shell layout, and cross-feature navigation.
- `src/renderer/src/features` owns domain behavior such as chat, Agent runs, Git review, files, and
  sidebars with domain state.
- `src/renderer/src/components` contains reusable UI primitives only.
- Features must not import `app`; shared components must not import `app` or `features`. ESLint
  enforces these rules.

### Conversation surfaces and collaboration composition

`ConversationSurface` is the single renderer boundary for a Conversation timeline. Its
discriminated `mode` is a capability contract rather than a presentation hint:

- `interactive` receives the composer and root-only mutation callbacks for send, edit, retry,
  fork, stop/guide, approval, model transition, and persisted message UI state. The existing
  `ChatConversationPage` is a thin adapter that can select only this branch.
- `observer` receives an exact `rootConversationId` plus an authorized child Conversation. It
  reuses the same message, Markdown, Tool/MCP/Skill, Artifact, error, usage, copy, and disclosure
  rendering, but its type and DOM omit the composer and every Conversation mutation or approval
  callback. Agent-origin input remains a `user` role projection for the model while the renderer
  labels it from its persisted origin and parent Agent identity.

The application shell owns one optional `CollaborationStore` for the currently selected root
Conversation. It passes that root-scoped snapshot to root-chat collaboration projections and the
right sidebar; those consumers do not create parallel event subscriptions or Conversation
reducers. Child messages remain owned by the existing Conversation projection and
`ConversationSurface`, not by the collaboration store.

Collaboration notifications are invalidations, not renderer state truth. The store hydrates the
authoritative tree from the Host, ignores duplicate or foreign-root notifications, validates the
root-local persistent sequence through the event log, and rehydrates on a gap or Host resync.
Observer Conversation refreshes use the selected Agent's latest validated sequence plus a
process-hydration revision; unrelated child events therefore do not replace an active stream,
while a gap or Core resync invalidates every selected observer.
Changing root or child identity hides the previous scope immediately and rejects late responses;
a refresh within the same scope can retain the last authorized snapshot while the replacement is
loaded. While a child is live, Core additionally emits a process-local observer envelope with the
exact root Agent/root Conversation/Agent/Conversation/Run/assistant-message identity. The observer
applies its nested ordinary `AgentEvent` through the existing Conversation reducer. A bounded
request-local arrival window replays events which race an in-flight hydration; normal long-running
streams are never accumulated in another store. If that request window overflows, the observer
keeps its live overlay and refuses the potentially stale response until a target invalidation or
explicit reload can obtain a durable snapshot.

The durable boundary in this round is the Agent tree, Conversation history, approval projection,
template records, and root-local collaboration event log. Root chat renders the log's typed,
backend-authored semantic activity (`started`, `updated`, `waiting_approval`, and terminal states)
inside the one Conversation timeline. It never reconstructs activity from model prose, Tool JSON,
Mailbox details, or the latest Agent summary. Reload and gap recovery replay the monotonic log from
sequence zero; the Renderer retains the latest 2,048 semantic items as a documented UI-history
window while still advancing across every non-display invalidation event. The observer envelope is
only a low-latency overlay and is not persisted; durable Conversation snapshots and the
collaboration event log remain recovery truth after gaps, Core restart, and window reload. Both
live and restored child chat use the one Conversation reducer rather than a second token/chat
store. The right-sidebar navigation stack and local disclosure/scroll UI state are renderer state
and are not presented as persisted collaboration facts.

An interactive root that has been materialized as an Agent still uses the existing Conversation
fork service. Its collaboration-owned fork mode commits the target Conversation, an independent
root Agent, the ordinary idempotency receipt, and provenance-preserving history in one SQLite
transaction. Agent-origin transport projections remain model/audit context but are filtered from
the user-facing target history and search. Child Conversations and roots with an active Turn remain
non-forkable.

## Core server

`crates/core-server` is an application boundary around `mycopilot-core`, not an unstructured
collection of JSON-RPC handlers:

- `application/` owns Agent use-case orchestration and run lifecycle.
- `transport/` owns line-delimited JSON-RPC parsing, routing, responses, bootstrap, and shutdown.
- `adapters/` owns Git, image-generation, and Skills integrations and bounded dispatch queues.

Allowed dependencies are `transport -> application -> core` and
`transport/application -> adapters -> core/protocol`. Adapters must not depend on transport.

## Protocol ownership

Protocol has two intentionally different levels:

- `packages/protocol` and `crates/protocol-rs` own cross-process method names and transport DTOs.
- `crates/core/src/protocol.rs` owns Agent runtime models that depend on core concepts such as
  context, traces, permissions, checkpoints, and world state.

Runtime models are not copied into `protocol-rs`, because doing so would either create a core
dependency cycle or turn the transport crate into a second domain model. Cross-language Agent
method names and representative event wire shapes are locked by
`packages/protocol/fixtures/agent-contract-v1.json`; TypeScript, `protocol-rs`, and `core` tests all
consume that same fixture.

## Electron host IPC

Renderer-facing channel names are owned by `packages/host-api`. Main-process registrars and preload
bridges import those constants. Main IPC registration is split by domain; preload exposes the same
domains as narrow bridge modules.

## MCP

`mycopilot-mcp-client` is a transport-neutral protocol boundary below core-server. It owns rmcp,
stdio process supervision, lifecycle negotiation, Registry/Manager/Catalog state and bounded Tool
results, but does not depend on Agent Runtime or Electron.

Core owns the rmcp-free Tool identity, approval and model projection contract. Core-server composes
the persistent Registry, exact launch authorization, sealed approval payload store and
`McpRuntimeBridge`. Renderer management and approval surfaces can use only the versioned Host API
through Main/Preload allowlists.

The complete implementation and extension contract is documented in
[MCP v1 development and security boundaries](mcp.md).

## Multi-Agent coordination

Multi-Agent coordination is a persistent Agent parent-child tree outside the existing Agent Loop;
it is not a generic workflow graph. Its domain, Mailbox projection, dependency, lifecycle, deletion
and rollout contracts are documented in [Multi-Agent Architecture](multi-agent-architecture.md).
