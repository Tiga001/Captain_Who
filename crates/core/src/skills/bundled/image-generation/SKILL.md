---
name: image-generation
description: Generate new raster images or edit an existing image with the native managed image generation capability. Use for text-to-image creation, illustrations, concept art, visual variants, restyling, compositing, background or object changes, and other prompt-directed bitmap image work.
---

# Image Generation

Use `image_generation` for image creation and editing. Keep every call in the typed envelope `{ "request": { ... }, "reason": "..." }`.

## Workflow

1. Choose `generate` when the result is based only on text. Send `request.operation="generate"` and a complete `request.prompt`. Set `request.sizePreset` only when the user requests a supported output size.
2. Choose `edit` when an existing image must influence the result. Send `request.operation="edit"`, a complete `request.prompt`, and exactly one authorized `request.inputPath`.
3. Copy the exact image path returned by the producing tool or supplied by the user into `request.inputPath`. This includes workspace paths, authorized absolute/system paths, attachment `readPath` values, generated `image-artifact://...` paths, and revision-bound `skill://...` paths. For an attachment, call `attachments_list` first and copy its exact `readPath`. Never build a source object, guess an attachment path, or substitute a display name, ID, or private saved path.
4. Write `reason` as a short, non-empty, user-readable sentence describing the purpose of that specific generation call. It is display and audit metadata only and never grants access.
5. Inspect the returned status and Artifact contract. Report success only when status is `succeeded` and the returned Artifact is verified.
6. On success, the model result returns one top-level `path`, normally an application-owned `image-artifact://...` reference. Copy that same path into `read_image.path` to inspect it, or into a later `image_generation` edit request's `inputPath` to edit it again. Do not build a `source` object, combine URI and filesystem fields, search attachments, or generate the image again merely to inspect it.
7. The Artifact `path` is a stable `read_image` reference, not a filesystem destination. If the user asks to place the image in the workspace or another user-visible folder, use the separately returned exact `savedPath` with the ordinary authorized file/command path. Never derive `savedPath` from the Artifact URI, and do not claim the image was exported until that separate operation succeeds.
8. A model with image-input capability may also receive the generated pixels transiently during the generation execution. `visualInputDelivery` uses stable values such as `attachedDuringGeneration`; it does not mean pixels remain attached in later model requests. In a later turn, call `read_image` with the exact returned `path` before claiming to have visually re-inspected the image.

Never send a URL, API key, model ID, provider ID, executable, raw base64, or data URL. Provider selection, credentials, model configuration, input encoding, execution identity, and Artifact storage are host responsibilities. Do not replace this tool with `curl`, a custom network request, or an ad-hoc script.

If the tool is unavailable or reports a configuration error, preserve that result and tell the user to review the image-generation Skill switch in Settings → Skills and the API/model configuration in Settings → Configuration → Image Generation; do not attempt a network fallback. Preserve `failed`, `cancelled`, and `indeterminate` outcomes exactly. Always inspect `failure.retryable` before considering another call. When it is `false`, do not retry it automatically; report the failure and follow the returned recovery guidance. When it is `true`, retry at most once and only when that still matches the user's intent. Never loop retries. An `indeterminate` result may represent a request that reached the provider and must never be retried automatically.

## Product watermark

Treat a platform-added watermark such as “AI生成” as a product configuration result, not an image quality defect. Do not change that configuration or make the watermark invisible by cropping, covering, repainting, editing, or regenerating. If the user does not want it, direct them to turn off “添加水印” in “设置 → 配置 → 图片生成”, save, and generate again. This rule concerns the product's generated-image watermark, not third-party copyright watermarks or marks of unknown origin.
