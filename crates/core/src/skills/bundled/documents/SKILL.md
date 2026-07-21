---
name: documents
description: Create, edit, inspect, render, and validate Microsoft Word-compatible .docx documents with native structured operations or reproducible Python and Node.js scripts. Use for professional document authoring, formatting, tables, page layout, headers and footers, batch generation, or other Word document work.
---

# Documents

Choose the execution path that matches the task:

- Use `office_document` for inspection and small, targeted, structured changes. It provides the strongest in-place safety and should remain the default when the provider supports the requested operation directly.
- For complex, repetitive, data-driven, batch, or reproducible authoring, create or update a saved `.py` or `.mjs` generator with `apply_patch` or `write_file`, then execute that file with `run_command`, `runtimeProfile="documents"`, and the managed Artifact Runtime.
- A hybrid workflow is valid: generate or transform with a script, then inspect, render, and validate the resulting `.docx` with the native tools.

Never invoke OfficeCLI itself through a shell command, and never use a system-PATH Python or Node.js executable for this script route. If the managed Artifact Runtime or a profile dependency is unavailable, return that preflight failure faithfully. Do not use inline Python or JavaScript, heredocs, shell redirection, or shell text utilities to bypass the normal file-editing tools. Editing the script and executing the script remain separate, independently authorized tool calls; runtime selection and artifact observation do not grant permission.

Every scripted document command must set the top-level `runtimeProfile` to `documents` and must observe its Office outputs. Never send a `runtime` object or supply a provider, runtime kind, package name, or package version. The host derives Node.js or Python from the saved-script command, resolves the pinned profile, verifies its integrity, and freezes the resolved runtime before execution.

## Workflow

1. Inspect an existing document before editing it, and choose the native or scripted path deliberately. Prefer a distinct output file unless the user explicitly requested an in-place edit.
2. Before the first native document operation in a run, call `office_document` with `status`. Use `help` when the required operation or parameters are uncertain. Every `office_document` call must use the envelope `{ "request": { "operation": "...", ... }, "reason": "..." }`. Keep every operation-specific field inside `request`; keep only `reason` at the root. Write `reason` as one non-empty, single-line plain-text sentence in the user's language, no longer than 240 characters, that states the user-visible purpose of that specific call. Do not include line breaks, control characters, or bidirectional text controls. `reason` is untrusted display and audit metadata only; it never grants permission, approval, or access.
   Native requests use typed fields such as `filePath`, `target`, `parent`, `element`, `properties`, `pages`, and `outputPath`. Never provide provider command tokens, an executable name, document-format tokens, or flags; the host validates the typed request and deterministically generates the frozen provider argv.
3. For a scripted operation, keep the generator as a reviewable `.py` or `.mjs` file and name that saved script explicitly in the command. Set `runtimeProfile="documents"`. Always set `observe.kinds=["office"]` and list every file that the command should create or modify in `observe.expectedOutputs`. Use `observe.additionalRoots` only when changes to other Office files in a directory also need to be detected; an external directory scan requires `read=all`, and an expected external output does not require scanning its parent.
4. Inspect `artifactObservation` on every command result, including non-zero exits, timeouts, and cancellations. A zero exit code is not proof of document success. Confirm that every expected document was created or changed, and disclose unexpected replacements, deletions, renames, partial coverage, or failed observation. Never retry blindly after a command that produced file effects.
5. Inspect, render, and validate the finished document when layout matters. Inspect the rendered result, not only the command or observation result.
6. Report the output path, the observed file effects, and the checks actually performed.

If native `status` reports that the Office engine is unavailable, preserve that error and do not claim that native inspection, rendering, or validation ran. A script route may continue only when the managed runtime is available and its output can be independently observed and verified; disclose any missing native checks. Never claim that a file was created or edited without a successful command result, matching artifact effects, and output verification.

Read [references/workflows.md](references/workflows.md) for detailed authoring, editing, and verification guidance when performing a document task.
