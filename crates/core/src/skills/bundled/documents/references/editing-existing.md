# Editing an existing Word document

Use this workflow for every change to an existing `.docx`. The fixed Editor opens the frozen
source with `python-docx`, executes normal Python in one marked edit region, and saves a distinct
candidate. It is not a restricted JSON or AST operation language, and it is not a second document
generation workflow.

## Contents

- [Fixed workflow](#fixed-workflow)
- [Materialize and run one Editor](#materialize-and-run-one-editor)
- [Use normal Python in the edit region](#use-normal-python-in-the-edit-region)
- [Host-owned transaction](#host-owned-transaction)
- [Fidelity boundaries](#fidelity-boundaries)
- [Completion checks](#completion-checks)

## Fixed workflow

1. Inspect the source before writing. Record its paragraph, table, section, header/footer, media,
   field, comment, and tracked-change structure where available.
2. Render and read every known page that may change to establish a visual baseline. The current
   semantic surface does not return an authoritative final page count, so do not infer exhaustive
   coverage from a contact sheet.
3. Choose one workspace-relative script directory. Reuse it when it is already a plain directory;
   otherwise create it first with a separate idempotent `mkdir -p <script-directory>` command.
4. Locate the active revision's `templates/editor.py` with `skills_list_resources`, materialize it
   once to a new path in that directory, and patch only `BEGIN EDIT REGION` / `END EDIT REGION`.
5. Execute one direct command with exactly one static `--source` and one static, distinct
   `--output`. Bind the source and every auxiliary input through `run_command.inputs`.
6. The Host automatically syntax-checks the frozen Python revision before execution. If it fails,
   patch the same Editor and submit the normal edit command again; do not run a separate model-made
   `python -m py_compile`, `python -c`, or combined shell command.
7. If the result is `status: "running"`, follow the returned `continueWith` receipt and wait with
   `command_session` until the authoritative terminal result. Only terminal success plus
   `artifactObservation` for the declared destination proves publication.
8. The Host's pinned OfficeCLI schema gate runs before publication; do not call native `validate`.
   Inspect the result, then render and read every known or affected final page. Compare it with the
   source baseline and check that unrelated content remains intact.
9. Delete the exact Editor and task-created temporary files after the final checks. If this task
   created the script directory and it is then empty, remove it with `rmdir`. Preserve pre-existing
   directories and unrelated files; never use recursive deletion.

The gates are independent:

`syntax-valid != Python-runtime-valid != DOCX-valid != fidelity-verified != visually-verified`

## Materialize and run one Editor

Create or reuse the directory before materialization:

```json
{
  "command": "mkdir -p word-work",
  "cwd": ".",
  "reason": "Prepare a workspace directory for the Word Editor"
}
```

Materialize the exact revision-bound resource URI returned by the active Skill; this URI is only
illustrative:

```json
{
  "sourceUri": "skill://package/<exact-revision>/templates/editor.py",
  "destination": "word-work/edit_document.py",
  "reason": "Create the fixed Word editor for the requested existing-document changes"
}
```

Patch the marked region, then run that same materialized file. `source.docx` is a logical mount
name below `MYCOPILOT_INPUT_ROOT`, not a private attachment or original filesystem path:

```json
{
  "command": "python word-work/edit_document.py --source source.docx --output report-edited.docx",
  "cwd": ".",
  "inputs": [
    {
      "mountPath": "source.docx",
      "path": "<exact source path or readPath>"
    },
    {
      "mountPath": "media/replacement.png",
      "path": "<exact auxiliary input readPath or artifact URI>"
    }
  ],
  "reason": "Edit the frozen Word source and publish a Host-gated save-as result"
}
```

Keep `--source` equal to one declared `inputs[].mountPath`. Use one workspace-relative `.docx`
destination that differs from the source. Do not request in-place overwrite, delete or rename the
source, or reconstruct a private input path. The Host may replace the process-visible output with a
private candidate path; the declared destination is published only after the Host schema gate
succeeds.

Do not add `runtimeProfile` or `observe`. The Host derives the pinned `documents` profile from the
materialized Editor receipt, freezes its source and inputs, performs Python syntax preflight, and
observes the static output automatically.

## Use normal Python in the edit region

The Editor is real Python, not a serialized edit plan. Inside the marked region you may use:

- functions, imports from the pinned runtime, loops, conditions, comprehensions, and data
  transformations;
- the complete `python-docx` object model available in the managed profile;
- `mounted_input("media/replacement.png")` for each separately declared auxiliary input;
- standard-library helpers when needed for the document transformation.

The fixed wrapper already provides `document`, the resolved read-only `source_path`,
`mounted_input`, argument parsing, and one direct save to the Host-provided private candidate. The
Host alone validates and atomically publishes that candidate. The unmodified edit region fails
closed instead of publishing an unchanged copy. Keep that wrapper unchanged. Restrict the script's
effects to the requested document and
declared mounted inputs. Do not install packages, invoke system Python, start child processes,
access the network, or discover Host-private paths.

Use run-level edits when formatting must be preserved. Assigning `paragraph.text` replaces all runs
and can destroy character-level styling, hyperlinks, fields, bookmarks, and other inline markup.
For an expected textual replacement, traverse the relevant runs or implement a style-aware
replacement and verify the result. Likewise, never assume the visible text uniquely identifies a
paragraph or table cell; inspect surrounding structure and assert expected preconditions in Python.

## Host-owned transaction

The Python process executes against frozen, read-only source and input mounts. The Host freezes the
materialized script bytes, runtime profile, inputs, destination state, conversation, and approval
before launch. It directs the Editor's output to a private candidate, then requires a valid bounded
DOCX package and a successful pinned OfficeCLI schema validation before atomically publishing the
declared destination. Cancellation and destination preconditions are rechecked immediately before
publish. A gate failure returns bounded stable OfficeCLI `type`, `description`, `path`, `part`,
`code`, `error`, and `message` details plus the Host error code/message when available, so patch the
same Editor instead of retrying unchanged.

Python remains normal Python within the existing managed-command permission and approval boundary;
the fixed template is not an operating-system sandbox and does not make unrelated side effects
transactional. The Office guarantee is narrower: the Host supplies a private source snapshot and
does not publish its private candidate to the declared destination before every gate passes.
Python code that independently writes some other path remains an ordinary `run_command` side
effect and is not rolled back.

## Fidelity boundaries

`python-docx` is appropriate for ordinary paragraphs, runs, styles, lists, tables, images, sections,
and headers/footers that it can represent and that the final checks can verify. Loading and saving a
file is not proof that every OOXML feature survived semantically.

Fail closed instead of silently flattening, rebuilding, or approximating when the requested file or
edit depends on features without reliable support and verification, including:

- digital signatures, rights management, encryption, protection, or password requirements;
- macros or `.docm` behavior;
- tracked-change semantics, comments, content controls, live fields, custom XML, or complex
  numbering/section inheritance that must remain exact;
- embedded OLE objects, ActiveX, SmartArt, advanced charts, equations, or externally linked content;
- precise preservation of unsupported relationships, extension lists, or application-specific
  metadata.

Some such parts may happen to survive a round trip, but incidental preservation is not a supported
claim. Use raw OOXML only when the user explicitly requests that route and a feature-specific
structural verifier is available; never use it as an unverified fallback. Preserve the original and
explain the unsupported boundary when proof is unavailable.

## Completion checks

- Confirm the source bytes did not change and the output is a distinct file.
- Re-inspect headings, paragraph and table order, sections, headers/footers, media, and every
  requested content change.
- Require terminal success from the Host prepublish OfficeCLI schema gate after the last edit; do
  not call native `validate`.
- Run feature-specific structural checks for comments, tracked changes, fields, content controls,
  or other fidelity-sensitive parts. Rendering is not structural proof.
- Render the whole document once for overview, then render and read every known or affected page
  individually. Keep a numbered ledger of pages actually seen and disclose that exhaustive coverage
  is unavailable without a trusted final page count.
- Compare typography, spacing, pagination, tables, images, and unchanged regions with the baseline.

Any correction invalidates earlier inspection, Host publication, structural, and visual evidence.
Repeat the affected checks on the new final output before delivery.
