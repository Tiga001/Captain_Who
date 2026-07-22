---
name: presentations
description: Create, edit, inspect, render, and validate Microsoft PowerPoint-compatible .pptx presentations with native structured operations or reproducible Python and Node.js scripts. Use for slide decks, layouts, themes, text, shapes, tables, charts, images, speaker notes, data-driven generation, or other presentation work.
---

# Presentations

Choose the execution path that matches the task:

- Use `office_presentation` for inspection and small, targeted, structured changes. It provides the strongest in-place safety and should remain the default when the provider supports the requested operation directly.
- For complex, repetitive, data-driven, batch, or reproducible deck work, create or update a saved `.py` or `.mjs` generator with `apply_patch` or `write_file`, then execute that file with `run_command`, `runtimeProfile="presentations"`, and the managed Artifact Runtime.
- A hybrid workflow is valid: generate or transform with a script, then inspect, render, and validate the resulting `.pptx` with the native tools.

Never invoke OfficeCLI itself through a shell command, and never use a system-PATH Python or Node.js executable for this script route. If the managed Artifact Runtime or a profile dependency is unavailable, return that preflight failure faithfully. Do not use inline Python or JavaScript, heredocs, shell redirection, or shell text utilities to bypass the normal file-editing tools. Editing the script and executing the script remain separate, independently authorized tool calls; runtime selection and artifact observation do not grant permission.

Every scripted presentation command must set the top-level `runtimeProfile` to `presentations` and must observe its Office outputs. Never send a `runtime` object or supply a provider, runtime kind, package name, or package version. The host derives Node.js or Python from the saved-script command, resolves the pinned profile, verifies its integrity, and freezes the resolved runtime before execution.

## Workflow

1. Inspect an existing deck before editing it, and choose the native or scripted path deliberately. Prefer a distinct output file unless the user explicitly requested an in-place edit.
2. Before the first native presentation operation in a run, call `office_presentation` with `status`. Use `help` when the required operation or parameters are uncertain. Every `office_presentation` call must use the envelope `{ "request": { "operation": "...", ... }, "reason": "..." }`. Keep every operation-specific field inside `request`; keep only `reason` at the root. Write `reason` as one non-empty, single-line plain-text sentence in the user's language, no longer than 240 characters, that states the user-visible purpose of that specific call. Do not include line breaks, control characters, or bidirectional text controls. `reason` is untrusted display and audit metadata only; it never grants permission, approval, or access.
   Native requests use typed fields such as `filePath`, `target`, `parent`, `element`, `properties`, `pages`, and `outputPath`. Never provide provider command tokens, an executable name, document-format tokens, or flags; the host validates the typed request and deterministically generates the frozen provider argv.
3. For a scripted operation, keep the generator as a reviewable `.py` or `.mjs` file and name that saved script explicitly in the command. Set `runtimeProfile="presentations"`. Always set `observe.kinds=["office"]` and list every file that the command should create or modify in `observe.expectedOutputs`. Use `observe.additionalRoots` only when changes to other Office files in a directory also need to be detected; an external directory scan requires `read=all`, and an expected external output does not require scanning its parent.
4. Inspect `artifactObservation` on every command result, including non-zero exits, timeouts, and cancellations. A zero exit code is not proof of presentation success. Confirm that every expected deck was created or changed, and disclose unexpected replacements, deletions, renames, partial coverage, or failed observation. Never retry blindly after a command that produced file effects.
5. Render every changed slide and inspect the images for overflow, collisions, illegible text, broken media, and inconsistent composition. Submit all changed slide ranges in one `view` screenshot request with a `grid`; never launch one render call per slide. Re-render a single slide only after the combined preview identifies a specific defect.
6. Validate the finished deck, then report the output path, observed file effects, and the checks actually performed.

If native `status` reports that the Office engine is unavailable, preserve that error and do not claim that native inspection, rendering, or validation ran. If rendering returns an `office.render_backend_*` error, report that visual verification did not run; do not retry each slide, invoke a system browser, or bypass the managed renderer. A script route may continue only when the managed runtime is available and its output can be independently observed and verified; disclose any missing native checks. Never claim that a deck was created or edited without a successful command result, matching artifact effects, and output verification.

Read [references/workflows.md](references/workflows.md) for detailed deck planning, editing, rendering, and verification guidance when performing a presentation task.
