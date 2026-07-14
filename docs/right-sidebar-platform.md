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
