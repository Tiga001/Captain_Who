# MCP v1 开发与安全边界

状态：stdio tools 第一版

日期：2026-07-30

本文只描述仓库当前实现。HTTP、OAuth、resources、prompts、sampling、elicitation、tasks
和 MCP Apps 尚未接入。

## 1. 模块与依赖方向

```text
Renderer MCP Settings / MCP Approval / MCP Activity
  -> typed Host API
  -> Preload allowlist
  -> trusted Main IPC + native path/authorization dialogs
  -> Core JSON-RPC
  -> core-server MCP management / approval / persistence
  -> mycopilot-mcp-client Registry / Manager / Catalog
  -> controlled stdio + rmcp
  -> MCP Server

Provider tool schema
  -> mycopilot-core ToolRegistry
  -> typed MCP proposal + sealed approval payload
  -> core-server McpRuntimeBridge revalidation
  -> McpConnectionManager
```

- `crates/mcp-client` owns protocol negotiation、Catalog、连接生命周期、调用 certainty、
  limits 和 transport。它不依赖 core、core-server、SQLite、Electron 或 Renderer。
- `crates/core/src/tools/mcp.rs` owns the rmcp-free Agent Tool contract, typed provenance,
  approval summary and bounded model projection.
- `crates/core-server/src/application/mcp` owns SQLite Registry, launch authorization, sealed
  approval envelopes and management use cases.
- `crates/core-server/src/adapters/mcp_runtime.rs` maps the Catalog to the existing ToolRegistry
  path. It does not create a second execution path.
- `packages/protocol`, `packages/host-api`, Main and Preload expose versioned, strict,
  Renderer-safe DTOs. Renderer never receives raw MCP arguments, results, stderr or credentials.

Production/public client types remain inside `mycopilot-mcp-client`; repository-owned dev fixtures
may use rmcp Server APIs directly. Server routing always uses `McpServerId + raw tool name`;
`mcp__...` is only a bounded model-facing name and is never parsed back into authority.

## 2. Lifecycle and supported protocol

The client uses rmcp Auto lifecycle. It attempts `server/discover` for protocol `2026-07-28` and
falls back to `initialize`/`initialized` for `2025-11-25` only when discovery is unsupported.
As of this document date, `2026-07-28` is the MCP draft/RC path while `2025-11-25` is the stable
fallback; supporting the draft through the pinned SDK is not a claim of final-spec conformance.
Protocol differences, modern subscriptions and legacy `tools/list_changed` are normalized behind
`McpPeer`.

Tools are paged into a separate Catalog. Repeated cursors, page/tool/schema/descriptor limits,
invalid schemas and normalized-name collisions produce explicit incomplete/disabled diagnostics.
Only a Complete Catalog can route a call.

Progress notifications are consumed without retention and never extend the Host-owned hard
deadline. Cancellation is best effort. Once a call may have been dispatched, timeout,
cancellation, transport close or process loss becomes `OutcomeUnknown`; it is never retried or
replayed automatically. A JSON-RPC error response and `isError=true` are authoritative responses,
not `OutcomeUnknown`.

## 3. Four independent permissions

The following actions are intentionally separate:

1. Save a non-sensitive Server configuration.
2. Authorize one exact local launch specification.
3. Enable/connect that Server.
4. Approve one frozen MCP Tool invocation.

New user Servers are disabled, untrusted and `Prompt`. There is no Always Allow mode. Tool
annotations and descriptions are untrusted hints and cannot remove approval.

Launch authorization format v2 binds Server identity, config epoch/digest, ordered argv, cwd,
the canonical executable/cwd, and a bounded filesystem metadata identity snapshot for recognized
code entrypoints. On Unix that snapshot includes canonical path, device/inode, ctime/mtime, size,
mode and ownership; it does not hash file contents. Plain code-file argv with a known extension and
explicit local `--import=`, `--require=` or loader paths are bound and rewritten to their canonical
targets in the native confirmation preview and actual launch plan. At most 16 recognized code
inputs are accepted. Package specifiers, URI imports, extensionless positional argv and ordinary
data files remain bound by their exact argv string but are not treated as code-file identities.
Enable/start and the final spawn boundary revalidate the snapshot. A replaced executable, changed
recognized script or retargeted bound symlink makes authorization stale. Records from the previous
authorization format are recovered as disabled/untrusted and require explicit reauthorization.

This check still has a small check-to-spawn race because portable process launch is path-based.
An attacker able to replace the already-canonical path after metadata validation can still race
`spawn`; complete removal requires a descriptor-bound/platform-specific launch design. Windows
has a weaker portable metadata snapshot than Unix.
Unix containment covers the spawned process group, not a malicious descendant that escapes into a
new session. These residual risks are why local Server authorization remains a high-risk native
confirmation rather than a sandbox guarantee.

## 4. Persistence and secrets

The Registry persists an explicit allowlist of non-sensitive fields: Server ID, display name,
scope/source, stdio executable, ordered argv, cwd, enabled/trust/approval mode, config
epoch/digest, monotonic revision, launch authorization identity and timestamps.

Executable, argv and cwd are stored as plaintext configuration. Never place passwords, tokens,
cookies or other credentials in argv. The current stdio UI intentionally has no env, environment
passthrough, header, bearer, OAuth or SecretRef input.

Raw MCP Tool arguments are treated as one secret payload. Durable pending approvals persist only a
safe structural summary plus an authenticated encrypted envelope. The master key belongs in the OS
Credential Store; it is not stored beside SQLite ciphertext. If a protected credential backend is
unavailable, the payload exists only in process memory, no plaintext/ciphertext envelope is
persisted, and the approval cannot resume after restart. Checkpoints, events, trace, IPC and
Renderer state contain only typed identity and safe projections.

Raw MCP results are not exact-archived. The transport rejects results above 4 MiB; core projects at
most 16 KiB of text and 8 KiB of structured JSON. Binary/image/audio/resource blocks are omitted
from model context and checkpoints with bounded diagnostics.

## 5. Central limits

Default hard ceilings are:

- protocol frame 8 MiB; safe error 4 KiB;
- stderr line 8 KiB, retained total 64 KiB, rate 16 KiB/s;
- 32 Catalog pages, 1,024 tools, 4 KiB cursor;
- 1 MiB schema/page, 8 MiB schema total, 2 MiB descriptors/page, 16 MiB descriptors total;
- 256 KiB per schema, depth 32, 4,096 nodes, 256 properties, 256 enum items;
- 64 KiB canonical arguments, depth 32, 4,096 nodes, 256 object properties;
- raw result 4 MiB, 128 content blocks, encoded media 1 MiB/block and 2 MiB total;
- model text 16 KiB and structured JSON 8 KiB;
- default Tool timeout 60 seconds, hard maximum 300 seconds;
- 32 active calls per Server and 256 active calls per Manager.

Transports may be stricter but may not exceed `McpSecurityLimits`.

## 6. Adding a repository-owned fixture

Fixtures live in `crates/mcp-client/tests/stdio_integration.rs`. This integration target has
`harness = false`, so the same repository binary acts as both the test driver and child Server.

To add a fixture:

1. Add a fixed `--fixture-*` mode and a Server implementation using only deterministic test data.
2. Do not access network, user files, inherited environment values or real credential stores.
3. Add the mode to the explicit connector allowlist by continuing to launch only
   `current_exe()`.
4. Assert negotiation, bounded output, cancellation and close/reap behavior from the parent.
5. Keep untrusted error/result canaries out of public errors, events and persistence.

Do not use `npx`, an installed Server or a user MCP configuration as a test fixture.

## 7. Adding another transport

Add a tagged `McpTransportConfig` variant and an `McpConnector` implementation. Keep
Registry/Manager/Catalog and Agent adapters transport-neutral. A transport is not complete until it
provides:

- Auto lifecycle and capability snapshot normalization;
- bounded framing, metadata and result handling;
- typed dispatch certainty, cancellation and an immutable Host deadline;
- reconnect/close semantics with bounded task cleanup;
- a threat model and secret boundary appropriate to the transport;
- repository-owned integration fixtures and negative tests.

Streamable HTTP must not reuse stdio launch authorization. It needs separate URL/SSRF policy,
redirect and DNS-rebinding controls, OAuth/PKCE/resource-indicator handling and a secret-backed
header store.

## 8. Extending resources and prompts

Resources and prompts must have independent Catalogs and adapters. Do not disguise them as Tools.
Resources require URI/MIME/pagination/subscription and read-policy controls. Prompts require typed
argument validation and an explicit rule for whether users or the model may select them. Neither
feature may inherit Tool invocation authority merely because it comes from an already-connected
Server.

## 9. Tests

Core validation:

```sh
cargo fmt --all -- --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm typecheck
pnpm lint
pnpm test:web
pnpm build
```

Owned stdio protocol and pressure suites:

```sh
cargo test -p mycopilot-mcp-client --test stdio_integration
cargo test -p mycopilot-mcp-client --test stdio_integration -- --stress-suite
cargo test -p mycopilot-core-server --test mcp_stdio_runtime_e2e
```

The stress suite executes 100 connect/close cycles, repeated restart, multiple concurrent fixture
Servers and refreshes, stop/remove during calls, crash recovery, and the 1,024/1,025 Tool boundary.
On Unix, the stdio and core-server E2E fixtures also record their own repository-owned PIDs and
assert OS-level disappearance after stop, restart, delete and application shutdown.
The normal stdio suite exercises raw-result, structured-content, content-block and encoded-media
limits over the real rmcp wire rather than only through in-memory peers.

The official [MCP Conformance Test Framework](https://github.com/modelcontextprotocol/conformance)
starts a scenario Server and appends its HTTP URL to a client-under-test command. This stdio-only
release has no compatible conformance command adapter, and the repository does not bundle a pinned
runner, so the official suite was not executed. Passing rmcp or local fixture tests must not be
described as passing the official conformance suite. A future integration must pin an exact
framework release and run only the transport/scenarios the client actually implements.

## 10. Platform status and current limits

- macOS is the currently exercised platform.
- Linux shares the Unix process-group implementation but still requires target-platform CI and
  release validation.
- Windows currently terminates only the direct child. Until a Job Object backend is implemented
  and tested, Windows stdio MCP cannot claim process-tree isolation or release acceptance.

Registry startup validates authorized launch paths outside the SQLite write transaction and then
rebinds the result to the exact row identity before recovery. A slow filesystem path can still
delay startup, but it cannot hold the Registry write transaction while that I/O is in progress.
Manager lifecycle events currently use a process-local unbounded channel; Server notification
storms are coalesced before publication, but a future high-volume Host should replace this with a
bounded/lag-aware event queue.

Current v1 is local stdio Tools only. It does not install Servers and does not support HTTP, OAuth,
env/SecretRef, resources, prompts, sampling, elicitation, tasks, MCP Apps, Plugin/Skill dependency
installation, raw logs/schema browsing or automatic retry of an unknown outcome.
