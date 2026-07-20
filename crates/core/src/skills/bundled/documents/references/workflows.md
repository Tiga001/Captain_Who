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

Call `office_document` with one JSON object per operation. `arguments` is an array of literal OfficeCLI tokens, not a shell command: keep every flag and every flag value in separate array entries. Never add shell quoting, redirects, pipes, or an executable name.

Before the first document operation in a run, check the managed engine:

```json
{ "operation": "status" }
```

When an element, property, or DOM path is uncertain, disclose only the relevant provider schema. For example:

```json
{ "operation": "help", "arguments": ["docx", "add", "paragraph", "--json"] }
```

Do not use a raw batch command. Submit each supported native operation separately and inspect its result before depending on it.

The managed runtime currently rejects the provider's ambiguous `data` property because it can be
interpreted as either inline content or a local file. Build tables through explicit table, row, and
cell operations instead of passing `--prop data=...`.

## Script authoring and observation contract

The managed Artifact Runtime exposes these document libraries:

| Runtime | Script | Package requirement   | Use                                                                        |
| ------- | ------ | --------------------- | -------------------------------------------------------------------------- |
| Node.js | `.mjs` | `docx` `9.6.1`        | Create data-driven `.docx` documents with reusable JavaScript layout code. |
| Python  | `.py`  | `python-docx` `1.2.0` | Create or transform `.docx` content with Python.                           |

Declare only the packages the selected script actually imports. Package names and versions belong
in `runtime.requiredPackages`; never install them from the script.

1. Put substantial program logic in a saved `.py` or `.mjs` file. Do not pass artifact-producing
   code through `python -c`, `node -e`, a heredoc, shell redirection, or another opaque inline form.
2. Make inputs and outputs explicit script arguments. Avoid hard-coded machine-specific paths, and
   make a rerun deterministic where practical.
3. Execute the saved file through `run_command` with the managed Artifact Runtime. Declare every
   runtime package the script requires; dependency resolution is preflight, never implicit package
   installation.
4. Set `observe.kinds` to `["office"]`. List every `.docx` file that should be created or modified
   in `observe.expectedOutputs`. Paths are resolved relative to the command `cwd` and each expected
   output is observed directly. Do not add its parent merely because the output is outside the
   workspace. Use `observe.additionalRoots` only to scan for other Office changes in an otherwise
   uncovered directory; recursively observing an external directory requires `read=all`.
5. Treat observation as evidence, not authorization or transactionality. It never expands what the
   command may do and does not give an arbitrary script the native tool's staging or rollback
   guarantees.

For example, after creating `scripts/build_report.mjs` with a file-editing tool, execute that exact
file with the managed Node.js runtime and observe its declared output:

```json
{
  "command": "node scripts/build_report.mjs --output outputs/report.docx",
  "cwd": ".",
  "runtime": {
    "provider": "managedArtifact",
    "kind": "node",
    "requiredPackages": [{ "name": "docx", "version": "9.6.1" }]
  },
  "observe": {
    "kinds": ["office"],
    "expectedOutputs": ["outputs/report.docx"]
  },
  "reason": "Generate the requested report reproducibly"
}
```

Keep `node` as the logical first command token. The host binds it to the selected managed runtime;
never discover or persist the runtime's private executable path in the script or command.

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
package, and render every page whose layout matters. Do not equate a valid ZIP/package with a
visually correct document.

## Core recipes

Create a real Word document:

```json
{
  "operation": "create",
  "path": "report.docx",
  "arguments": ["--locale", "zh-CN", "--json"],
  "reason": "Create the requested Word document"
}
```

Add a structurally styled heading, then a normal paragraph:

```json
{
  "operation": "add",
  "path": "report.docx",
  "arguments": [
    "/body",
    "--type",
    "paragraph",
    "--prop",
    "text=Quarterly Report",
    "--prop",
    "style=Heading1",
    "--json"
  ],
  "reason": "Add the document heading"
}
```

```json
{
  "operation": "add",
  "path": "report.docx",
  "arguments": [
    "/body",
    "--type",
    "paragraph",
    "--prop",
    "text=Revenue and operating results are summarized below.",
    "--prop",
    "style=Normal",
    "--json"
  ],
  "reason": "Add the introductory paragraph"
}
```

Read back the body structure and content:

```json
{ "operation": "get", "path": "report.docx", "arguments": ["/body", "--depth", "2", "--json"] }
```

To preserve an existing source while editing, set `destinationPath` on the first mutation. The
engine copies the frozen source into destination-local staging and atomically publishes only the
new file. Apply later mutations to `revised.docx` itself so earlier changes are preserved:

```json
{
  "operation": "set",
  "path": "source.docx",
  "destinationPath": "revised.docx",
  "arguments": ["/body/p[1]", "--prop", "text=Revised heading", "--json"],
  "reason": "Create a revised copy without overwriting the source"
}
```

Render the first page to a workspace PNG. Rendering writes `outputPath`, so it follows the same file-write approval policy as other write tools:

```json
{
  "operation": "view",
  "path": "report.docx",
  "arguments": ["screenshot", "--page", "1", "--json"],
  "outputPath": "report-preview.png",
  "reason": "Render the finished document for visual inspection"
}
```

Validate the OOXML package:

```json
{ "operation": "validate", "path": "report.docx", "arguments": ["--json"] }
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
   input and output arguments. Omit the distinct destination only when the user clearly requested
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
