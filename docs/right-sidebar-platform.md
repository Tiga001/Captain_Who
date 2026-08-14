# Right Sidebar Platform

The right sidebar is a host for long-lived tool surfaces. Module state belongs to the platform, not
to the toolbar or module picker.

## Core Contracts

- `rightSidebarModules.tsx` is the renderer-side module registry.
- `useRightSidebarPlatform.ts` owns page creation, activation, updates, and close behavior.
- `RightSidebarPageStack.tsx` applies each module's retention policy.
- `WebviewSurface.tsx` is the shared DOM host for isolated external web content.
- `managedWebviewSecurity.ts` is the main-process allowlist for Webview partitions and navigation.

Each module definition declares:

- how its initial page state is created;
- which icon and translated title appear in launch surfaces;
- whether inactive instances stay mounted;
- whether the surface is a React or Webview surface;
- how the module renders without coupling `RightSidebar.tsx` to module-specific props.

## Lifecycle

Pages are created and activated atomically. A `keep-alive` page remains mounted while inactive and
is hidden by the page stack. Closing a page is the disposal boundary. Modules that do not need to
preserve runtime state can use `unmount-when-inactive`.

The browser uses `keep-alive`, so navigation, page state, cookies, and focus history survive tab
switches. The terminal also uses `keep-alive`, preserving its PTY session.

## Root-scoped Agent Center

The Agent Center is a dynamic React module composed by `RightSidebar`; it is deliberately not in
the static default registry used by an empty or legacy Conversation. It is added only when the
currently active root Conversation's authorized tree snapshot contains at least one child Agent.
Consequently, a root with no child preserves the previous sidebar module list and DOM, while a tree
with children gains a translated “Subagents” entry and an active-Agent count.

The module is bound to the active root Conversation, not merely to the project/workspace:

- the home page divides the current tree into active and non-active groups, sorted by latest
  activity, and shows only presentation-safe task, status, model display, and timing fields;
- selecting an Agent pushes the module into a detail state and renders the shared
  `ConversationSurface` in `observer` mode through an application-shell render contract;
- switching to another root in the same project resets an existing Agent Center detail to that
  root's list; a stale snapshot fails closed, and a root with no child removes the module;
- the observer has no composer, send, edit, retry, fork, stop/guide, or approval controls. Its Host
  load also requires the exact root and child Conversation identities, so the UI restriction is
  not the authorization boundary;
- the 280 px sidebar floor uses scoped spacing overrides around the shared Surface rather than a
  second compact chat implementation.

`AppShell` is the sole owner of the active-root collaboration store. It supplies the same snapshot
and per-Agent validated invalidation sequence to the Agent Center, root-chat semantic
activity/approval projections, and observer refreshes. The store replays the durable event log from
sequence zero after reload, resync, or a detected gap, validates every sequence, and atomically
publishes a bounded latest-2,048 semantic activity window. Root chat merges those typed events into
the shared Conversation timeline by durable anchor when present and otherwise by
`occurredAt`/sequence; it never derives status from the Agent Center's current-state rows. A
hydration revision additionally invalidates every observer
after a gap, restart resync, or window reload. An observer change is
identity-scoped so a late response for Agent A cannot appear under Agent B; same-Agent invalidation
refreshes may retain the last authorized snapshot rather than flashing unrelated or empty content.
During a live child Turn, a strictly parsed process-local observer envelope supplies the exact root
Agent, root Conversation, Agent, Conversation, Run, and assistant-message identities. Its nested
`AgentEvent` feeds the existing chat reducer, and a bounded request-local arrival window closes
in-flight hydration races without accumulating a normal long stream. An overflow rejects the stale
snapshot until target invalidation or explicit reload. Managed Command Session events remain a
narrow compatibility stream and are accepted
only with their own exact Conversation/assistant-message/Run owner identity.

The Agent Center's settings button opens the same Agent template settings page that is always
available from Settings, including before the first child exists. That page uses the shared
`ModelConfigPicker`, persists project-scoped template CRUD through the Host API, and saves an exact
`model_config_id`. A deleted or disabled model is shown as unavailable and must be explicitly
replaced before the template can be saved or re-enabled. Template edits affect later Agent
creation; the center continues to display each existing Agent's creation snapshot.

Agent identities, latest statuses, templates, approvals, and child Conversation history recover
from the database. The open Agent Center page/detail, local collapse state, and scroll position are
not persisted across a full renderer reload. The Agent Center remains a current-state index;
root-chat collaboration history instead comes from the durable typed semantic log and appears
inline with ordinary messages. Low-value send, wait, Tool JSON, and duplicate terminal plumbing do
not receive semantic activity projections. Live observer envelopes are an unpersisted latency
overlay, not a second stream store; persisted Conversation snapshots and the collaboration event
log rehydrate and overwrite transient state after gaps, process restart, or window reload. This
module adds no child write path, tree deletion control, graph canvas, or cross-root dashboard.

## Adding A Webview Module

1. Add the module ID and renderer definition to the right-sidebar registry.
2. Render external content through `WebviewSurface`; do not create `<webview>` elements elsewhere.
3. Give the module a dedicated persistent partition when it must not share browser cookies.
4. Register that partition and its allowed navigation protocols in `MANAGED_WEBVIEW_POLICIES`.
5. Pass `openLinksInSameSurface` to `WebviewSurface` and set `newWindowBehavior` to
   `navigate-current` only when `target="_blank"` links should open in the existing surface. The
   host still denies creation of real popup windows.
6. Add only narrowly scoped host APIs. Guest content must never receive the main application API.
7. Verify overlays, guest focus, tab retention, crash recovery, permissions, and cleanup.

Webview creation is denied unless its partition is registered. The main process strips preload and
normalizes popup attributes from its policy, enforces sandboxing and context isolation, denies
permissions by default, and blocks main-frame navigation outside the policy's protocol allowlist.
