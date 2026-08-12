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
