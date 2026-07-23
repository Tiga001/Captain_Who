---
name: image-generation
description: Generate new raster images or edit an existing image with the native managed image generation capability. Use for text-to-image creation, illustrations, concept art, visual variants, restyling, compositing, background or object changes, and other prompt-directed bitmap image work.
---

# Image Generation

Use `image_generation` for image creation and editing. Keep every call in the typed envelope `{ "request": { ... }, "reason": "..." }`.

## Workflow

1. Choose `generate` when the result is based only on text. Send `request.operation="generate"` and a complete `request.prompt`. Set `request.sizePreset` only when the user requests a supported output size.
2. Choose `edit` when an existing image must influence the result. Send `request.operation="edit"`, a complete `request.prompt`, and exactly one authorized `request.inputPath`.
3. For an attached input image, call `attachments_list` first and copy that attachment's exact `readPath` into `request.inputPath`. Never guess an attachment path or substitute its display name or ID. For a workspace image, use its workspace path and let the host enforce read permission.
4. Write `reason` as a short, non-empty, user-readable sentence describing the purpose of that specific generation call. It is display and audit metadata only and never grants access.
5. Inspect the returned status and Artifact contract. Report success only when status is `succeeded` and the returned Artifact is verified.
6. On success, use the exact `savedPath` returned by the tool whenever a later tool or script needs the generated file. Never guess, derive, rewrite, or search for this path. `image-artifact://...` is a presentation identity, not a filesystem path.
7. If the user asks to place the image in the workspace or another user-visible folder, or a downstream tool cannot read the managed path under the current scope, copy the exact `savedPath` to the requested destination through the ordinary file/command execution path. That copy is a separate filesystem operation and must obey the current file and command permissions. Do not claim it was saved to the workspace until that operation succeeds.
8. A model with image-input capability may also receive the generated pixels as transient visual input in the current run. A text-only model receives `savedPath` and metadata only; it must not claim to have visually inspected the image. Whether pixels are supplied is decided by the backend, never by a tool argument.

Never send a URL, API key, model ID, provider ID, executable, raw base64, or data URL. Provider selection, credentials, model configuration, input encoding, execution identity, and Artifact storage are host responsibilities. Do not replace this tool with `curl`, a custom network request, or an ad-hoc script.

If the tool is unavailable or reports a configuration error, preserve that result and tell the user that image generation must be enabled or configured; do not attempt a network fallback. Preserve `failed`, `cancelled`, and `indeterminate` outcomes exactly. Always inspect `failure.retryable` before considering another call. When it is `false`, do not retry it automatically; report the failure and follow the returned recovery guidance. When it is `true`, retry at most once and only when that still matches the user's intent. Never loop retries. An `indeterminate` result may represent a request that reached the provider and must never be retried automatically.
