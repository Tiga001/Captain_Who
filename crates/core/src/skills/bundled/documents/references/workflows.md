# Word document workflows

## Choosing an execution path

Use the native path for inspection, isolated paragraph or property changes, and other bounded
operations that `office_document` represents directly. It provides a frozen structured action,
conflict checks, staging, and atomic publication for writes.

Use a reproducible script when the task is substantially clearer as code: data-driven reports,
mail-merge-like generation, repeated sections or tables, large batches, reusable layouts, or a
transformation that requires coordinated changes throughout the package. Keep the generator in a
reviewable `.py` or `.mjs` file. A script is not a shortcut around file-edit or command
authorization; create or update it with `apply_patch` or `write_file`, then execute that exact saved
file with `run_command` and the managed Artifact Runtime.

The two paths compose. A script may produce the document, after which the native read, render, and
validate operations provide structural and visual verification. Prefer a new output document for
scripted transformations unless the user explicitly requested an in-place edit.

## Native tool contract

Call `office_document` once per operation with exactly this root envelope: `{ "request": { "operation": "...", ... }, "reason": "..." }`. Put every operation and operation-specific field inside `request`. Keep `reason` as the only root field. Every call, including `status`, `help`, `get`, `query`, `validate`, `view`, and every mutation, must include it. Write `reason` as one non-empty, single-line plain-text sentence in the user's language, no longer than 240 characters, describing the user-visible purpose of this specific call. Do not include line breaks, control characters, or bidirectional text controls. Do not use it to assert success or authorization: it is untrusted display and audit metadata only and never grants permission, approval, or access.

The native request is typed and operation-specific. Put the document in `request.filePath`; use `request.target` for an exact DOM path, `request.parent` plus `request.element` for insertion, `request.properties` for provider-neutral property values, and `request.outputPath` only for a rendering operation. Never send provider command tokens, document-format tokens, output flags, or shell syntax. The host validates the typed request, generates deterministic provider argv, freezes it for approval, and regenerates it before execution.

Before the first document operation in a run, check the managed engine:

```json
{ "request": { "operation": "status" }, "reason": "Check whether document tools are available" }
```

When an element, property, or DOM path is uncertain, disclose only the relevant provider schema. For example:

```json
{
  "request": {
    "operation": "help",
    "verb": "add",
    "element": "paragraph"
  },
  "reason": "Check how to add the requested document paragraph"
}
```

Do not use a raw batch command. Submit each supported native operation separately and inspect its result before depending on it.

The managed runtime rejects the provider's ambiguous `data` property because it can be interpreted
as either inline content or a local file. Build tables through explicit typed table, row, and cell
operations instead.

## Script authoring and observation contract

Select the `documents` runtime profile. The logical saved-script command selects its runtime
family; the model does not select a provider, runtime kind, dependency set, or version:

| Command  | Script | Profile library               | Use                                                                        |
| -------- | ------ | ----------------------------- | -------------------------------------------------------------------------- |
| `node`   | `.mjs` | `docx`                        | Create data-driven `.docx` documents with reusable JavaScript layout code. |
| `python` | `.py`  | `python-docx` (`docx` import) | Create or transform `.docx` content with Python.                           |

Set the top-level `runtimeProfile` field to `documents`. Do not send a `runtime` object and do not
copy a provider, kind, package name, or package version into the tool call. The host maps `node`
or `python` to the matching profile entry, resolves the pinned dependencies, verifies the managed
runtime, and freezes the exact resolution and integrity identity before execution.

1. Put substantial program logic in a saved `.py` or `.mjs` file. Do not pass artifact-producing
   code through `python -c`, `node -e`, a heredoc, shell redirection, or another opaque inline form.
2. Make inputs and outputs explicit script parameters. Avoid hard-coded machine-specific paths, and
   make a rerun deterministic where practical.
3. Execute the saved file through `run_command` with `runtimeProfile="documents"`. Dependency and
   runtime resolution is host-owned preflight, never implicit package installation.
4. Set `observe.kinds` to `["office"]`. List every `.docx` file that should be created or modified
   in `observe.expectedOutputs`. Paths are resolved relative to the command `cwd` and each expected
   output is observed directly. Do not add its parent merely because the output is outside the
   workspace. Use `observe.additionalRoots` only to scan for other Office changes in an otherwise
   uncovered directory; recursively observing an external directory requires `read=all`.
5. Treat observation as evidence, not authorization or transactionality. It never expands what the
   command may do and does not give an arbitrary script the native tool's staging or rollback
   guarantees.

For example, after creating `scripts/build_report.mjs` with a file-editing tool, run the Node.js
entrypoint and observe its declared output:

```json
{
  "command": "node scripts/build_report.mjs --output outputs/report.docx",
  "cwd": ".",
  "runtimeProfile": "documents",
  "observe": {
    "kinds": ["office"],
    "expectedOutputs": ["outputs/report.docx"]
  },
  "reason": "Generate the requested report reproducibly"
}
```

The equivalent Python route keeps the same profile and changes only the reviewed script and its
logical entrypoint:

```json
{
  "command": "python scripts/build_report.py --output outputs/report.docx",
  "cwd": ".",
  "runtimeProfile": "documents",
  "observe": {
    "kinds": ["office"],
    "expectedOutputs": ["outputs/report.docx"]
  },
  "reason": "Generate the requested report reproducibly"
}
```

Keep `node` or `python` as the logical first command token. The host binds it to the profile's
managed executable; never discover or persist a private executable path. If profile preflight
fails, preserve that error. Do not remove `runtimeProfile`, use a system executable, install a
package, or guess another version. Use the native Office path only when it supports the requested
work, or rewrite and review a script for the other profile entrypoint before retrying with the same
profile. Never run a `.mjs` file as Python or a `.py` file as Node.js.

After every observed command, inspect `artifactObservation` even if the command failed, timed out,
or was cancelled:

- `status=complete` means the scan finished within its reported coverage policy, not that every
  filesystem entry was inspected. Always inspect excluded directories, warnings, and
  `changesTruncated`; `partial` or `failed` must be disclosed and cannot establish that no other
  Office file changed.
- Match every requested output in `artifactObservation.expectedOutputs`. Only `created`, `modified`,
  `replaced`, or `renamed` establish a file effect. `unchanged`, `missing`, `unobserved`, or
  `invalid` do not satisfy a requested edit.
- Inspect `changes` for unexpected document creation, replacement, deletion, or rename, and retain
  every warning. A non-zero exit can still leave file effects; a zero exit does not prove that the
  expected document exists or is valid.
- Do not blindly retry after any observed side effect. First inspect the resulting file and decide
  whether to continue from it, overwrite it deliberately, or report the partial outcome.

Observation is followed by document verification. Read the relevant structure, validate the OOXML
package, and render every page whose layout matters. Put all required page ranges into one
browser-backed screenshot request and use a contact-sheet grid; never loop over pages. A focused
single-page render is appropriate only after that combined preview exposes a defect. Do not equate
a valid ZIP/package with a visually correct document. If the managed renderer returns an
`office.render_backend_*` error, preserve it and report the visual check as unavailable instead of
falling back to a user browser.

## Core recipes

Create a real Word document:

```json
{
  "request": {
    "operation": "create",
    "filePath": "report.docx",
    "locale": "zh-CN"
  },
  "reason": "Create the requested Word document"
}
```

Add a structurally styled heading, then a normal paragraph:

```json
{
  "request": {
    "operation": "add",
    "filePath": "report.docx",
    "parent": "/body",
    "element": "paragraph",
    "properties": {
      "text": "Quarterly Report",
      "style": "Heading1"
    }
  },
  "reason": "Add the document heading"
}
```

```json
{
  "request": {
    "operation": "add",
    "filePath": "report.docx",
    "parent": "/body",
    "element": "paragraph",
    "properties": {
      "text": "Revenue and operating results are summarized below.",
      "style": "Normal"
    }
  },
  "reason": "Add the introductory paragraph"
}
```

Read back the body structure and content:

```json
{
  "request": {
    "operation": "get",
    "filePath": "report.docx",
    "target": "/body",
    "depth": 2
  },
  "reason": "Verify the document body structure and content"
}
```

To preserve an existing source while editing, set `destinationPath` on the first mutation. The
engine copies the frozen source into destination-local staging and atomically publishes only the
new file. Apply later mutations to `revised.docx` itself so earlier changes are preserved:

```json
{
  "request": {
    "operation": "set",
    "filePath": "source.docx",
    "destinationPath": "revised.docx",
    "target": "/body/p[1]",
    "properties": { "text": "Revised heading" }
  },
  "reason": "Create a revised copy without overwriting the source"
}
```

Render all pages that need review into one workspace contact sheet. Rendering writes `outputPath`, so it follows the same file-write approval policy as other write tools:

```json
{
  "request": {
    "operation": "view",
    "filePath": "report.docx",
    "mode": "screenshot",
    "pages": [{ "start": 1, "end": 3 }],
    "grid": { "mode": "auto" },
    "outputPath": "report-preview.png"
  },
  "reason": "Render the finished document pages for visual inspection"
}
```

Validate the OOXML package:

```json
{
  "request": {
    "operation": "validate",
    "filePath": "report.docx"
  },
  "reason": "Validate the finished Word document"
}
```

## Create

1. Confirm the requested output name, content hierarchy, page size, and any visual requirements.
2. Choose native operations for bounded authoring or a saved generator for coordinated,
   repetitive authoring. Never substitute a plain-text file with a `.docx` extension.
3. Apply named styles consistently. Prefer real headings, lists, tables, page breaks, headers, and footers over visual approximations.
4. Render the result and inspect every page for overflow, clipped content, accidental blank pages, weak hierarchy, and inconsistent spacing.
5. Validate the final package and confirm that the requested path exists.

## Edit

1. Inspect the document structure and relevant content before changing it.
2. Preserve styles, relationships, media, sections, and unrelated content unless the user asks to replace them.
3. Prefer targeted native operations for local changes. Use a saved script only when its coordinated
   transformation is materially clearer or more reproducible than many isolated calls.
4. With the native path, use `destinationPath` for save-as edits. With the script path, pass distinct
   input and output parameters. Omit the distinct destination only when the user clearly requested
   in-place editing.
5. Re-inspect and render the result. Verify both the intended change and preservation of surrounding content, and reconcile those checks with the observed file effects.

## Quality checks

- Verify headings and reading order, table widths, pagination, margins, headers and footers, and image placement.
- Check that important semantics are represented structurally rather than with repeated spaces or manual line breaks.
- Treat a successful engine exit as necessary but insufficient when visual layout matters.
- Treat a successful script exit as necessary but insufficient; require matching complete artifact
  observation plus structural, package, and visual checks appropriate to the task.
- If a requested feature is not reported by `help` or validation, describe the limitation instead of simulating support.
- Report only checks that actually ran, including any warnings returned by the tool.
