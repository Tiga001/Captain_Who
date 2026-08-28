# MyCopilot platform workflows

Use this reference for concrete Workspace, temporary-resource, child-review, and installation
operations.

## Create a Workspace Skill

The discoverable location is:

```text
.agents/skills/<skill-directory>/SKILL.md
```

Use a normal workspace command to create the directory before writing files. Quote the validated
literal path and do not combine directory creation with unrelated commands. Then use ordinary file
editing for `SKILL.md` and its resources.

Workspace Skill directories and files must be regular, non-symlink paths. `SKILL.md` must use the
exact filename, UTF-8 text, YAML frontmatter, and non-empty Markdown instructions.

The current parent Run will not hot-reload a newly created or changed Skill. Static inspection can
happen in the parent; behavioral review happens in a fresh child Run.

## Use the bundled starter

1. Get the exact revision-bound package root URI from the activated `skill-creator` metadata or
   the package identity returned by `skills_list_resources`. Do not use the child `SKILL.md`
   resource URI as the tree source.
2. Choose a unique task directory such as `skill-creator-tmp-01` and create it first.
3. Confirm that `skill-creator-tmp-01/starter-skill` does not already contain unrelated data.
4. Call `skills_materialize_resource` with this exact shape:

   ```json
   {
     "sourceUri": "<revision-bound package root skill:// URI>",
     "sourcePrefix": "templates/starter-skill",
     "destination": "skill-creator-tmp-01/starter-skill",
     "reason": "Create a temporary starter for the Workspace Skill"
   }
   ```

   The destination itself must be absent, while its parent `skill-creator-tmp-01` must already
   exist.

5. Adapt the starter into `.agents/skills/<skill-directory>/` with ordinary file editing.
6. Delete only the recorded temporary files, then remove the now-empty task directory.

Do not materialize directly into `.agents`: the Host rejects reserved destination components. Do
not respond to `destinationParentNotFound` by repeating the same call; create the parent first.

## Review the latest revision

After all parent writes are complete, record a target-tree mutation guard and stop writing the
target. The guard records the relative regular-file inventory, byte lengths, and byte-content state
needed to detect any later target write; it is not a substitute for the package revision reported by
a fresh Run. Call `spawn_agent` once per review case with a clear task name, the full review message,
and `fork_turns: "none"`. Each child shares the project workspace but starts an independent Run with
fresh Workspace Skill discovery. Reviewer output stays in chat or a task directory outside the
target Skill.

For an explicitly targeted capability review, require exact `source=workspace` selection when
another bundled or installed Skill has the same name. Before spawning, reject or resolve a second
Workspace Skill with the same frontmatter `name`; name plus `source=workspace` is not sufficient to
disambiguate two Workspace entries. The first valid capability review establishes the review
revision from trusted trace evidence when available, or from clearly labeled reviewer-reported
evidence otherwise. Require every later capability reviewer to match it; a trusted positive-blind
trace that shows target activation must also match it. Never add these metadata requests to a blind
child's original user prompt. Compare the target to the mutation guard after every child. Any target
change or revision mismatch invalidates all verdicts from that barrier.

If `spawn_agent` is unavailable in the current Host, do not substitute a same-Run activation or
claim an independent PASS. Complete the static checks that are possible and report the behavioral
review as not run.

## Choose and copy the editable source

Use the source type to choose the workflow:

- **Workspace:** inspect and edit the existing `.agents/skills/<skill-directory>/` package in place.
- **Authorized external source:** copy the exact user-authorized local or checked-out source into a
  task staging directory, verify it, then edit the staged copy. Do not scan unrelated directories,
  follow symlinks, or modify the source.
- **Installed:** treat the application-managed package as immutable. Copy a verified snapshot into
  task staging when permission allows; never edit, delete, rename, or overwrite the managed package
  or its receipt.

Copy-first is a hard gate for every non-Workspace source. It means a real filesystem byte copy, not
reading files and recreating their text with file-writing tools.

1. This workflow requires an active Workspace and currently supports macOS and Linux. On Windows,
   where ordinary `run_command` execution is unsupported, stop and report the limitation. Do not
   invent a PowerShell fallback. Run every command below from the Workspace root: omit `cwd` so the
   Host selects that root, or use the exact Workspace-root binding when omission is unavailable.
2. Choose and track a unique task directory directly under the Workspace root but outside
   `.agents/skills`. Before creating it, require both `! -e` and `! -L`; then create that one literal
   directory and confirm with an `lstat`-equivalent check that it is a real directory, not a symlink.
   Inspect `.agents` and `.agents/skills` one component at a time. If a component exists, require it
   to be a real directory and not a symlink. If absent by both `! -e` and `! -L`, create only that
   component with a literal `mkdir` command and check it again. Finally require the exact
   `.agents/skills/<skill-directory>` child to be absent by both tests. A fresh project may require
   creating `.agents` and then `.agents/skills`; never create the final Skill child before publish.
3. Confirm that the task directory and `.agents/skills` are on the same filesystem before editing.
   Use the platform's read-only device-id query (`stat -f '%d'` on macOS or `stat -c '%d'` on Linux)
   and require equal device identifiers. If this cannot be established, stop rather than relying on
   a cross-filesystem `mv` that may degrade into copy-and-delete.
4. Inspect only the authorized source. Require a regular exact-case `SKILL.md` and reject symlinks,
   sockets, devices, and other special entries before copying. Enforce the normal Skill package
   count, depth, path, per-file, and total-byte bounds before recursively copying an external tree.
5. Use ordinary `run_command` with correctly shell-quoted literal paths. Do not use
   `run_command.inputs`, `skills_materialize_resource`, a generated copy script, or
   `read_file`/`apply_patch` reconstruction for this copy. The single exception to the literal-path
   rule is the safely joined Installed-store path word described below. Quote only the trusted
   environment prefix with double quotes and encode the validated suffix as a separate POSIX
   single-quoted literal; never interpolate a receipt- or manifest-derived suffix inside double
   quotes or leave it unquoted.
6. A call containing several commands must be fail-fast: either make one command per call or join
   every segment with `&&` (a newline immediately after `&&` is acceptable). Never use unrelated
   plain newline or `;` batches, because `/bin/sh` can hide an earlier failure behind the last
   command's zero exit status. This applies to inspection, `mkdir`, digest checks, `cp`, `cmp`, and
   postcondition batches.
7. Approval may be required for directory creation, external inspection, copy, comparison,
   publication, or cleanup—not only for `cp` and `mv`. Keep the original tool call pending and
   continue from its actual result in the same Run; do not issue a replacement call while approval
   is pending. If approval is denied, execution is unsupported, a destination conflicts, or any
   command fails, remove only task-owned partial staging paths whose creation was confirmed and
   whose exact target was rechecked. If cleanup is not approved or fails, report the retained exact
   task path and stop. Do not silently choose another copying mechanism.
8. Verify the untouched staged bytes before the first edit. Re-scan the staged tree itself and
   require every entry to be a regular file or real directory, with a regular exact-case
   `SKILL.md`; reject any symlink or special entry even if the source passed its earlier scan. For an
   external tree, also use `diff -qr` against the authorized source. For an Installed package,
   require the staged path set to equal the manifest list, validate each staged file against its
   manifest digest, and use `cmp` for every source/destination pair. A nonzero or ambiguous result
   is failure, even if command output is truncated.
9. Apply intentional edits only inside staging. Use patching for existing files and file creation
   only for genuinely new resources; never reconstruct unchanged source files.
10. Immediately before publish, repeat the non-symlink checks for the task directory, staged child,
    `.agents`, and `.agents/skills`; repeat the same-filesystem check; and again require the final
    child to satisfy both `! -e` and `! -L`. Then use the guarded no-clobber command below. If this
    platform's `mv` does not support `-n`, stop instead of falling back to overwrite-capable `mv`.
    Afterward, require the staged child to be absent and the final directory to exist as a real,
    non-symlink directory; otherwise publication failed.
11. Check the published file inventory, then perform any required behavioral review against the
    now discoverable Workspace package. Record its reviewed package revision. Clean only the
    tracked task files and empty task directory; retain nothing accidental in the Skill package.

For an authorized external directory, copy the whole validated tree into an absent staging child:

```sh
cp -R -- '<authorized-source-directory>' '<task-directory>/<absent-staging-child>'
```

The quoted placeholders stand for already validated literal paths; escape any embedded shell
metacharacters correctly. Do not edit until the recursive comparison succeeds.

Publish with literal absence and no-clobber checks:

```sh
test ! -e '.agents/skills/<skill-directory>' &&
test ! -L '.agents/skills/<skill-directory>' &&
mv -n -- '<task-directory>/<staged-skill>' '.agents/skills/<skill-directory>'
```

`mv -n` prevents the ordinary overwrite that bare `mv` permits, while the `-L` check also rejects a
dangling destination symlink that `test ! -e` alone would miss. This remains a best-effort shell
guard, not a Host-level atomic no-replace primitive. A concurrent conflict can still make `mv -n`
perform no move and report success on some implementations, which is why the source-absent and
destination-present postconditions are mandatory. Any conflict or ambiguous result is a hard stop.

### Copy an Installed package

1. Identify one exact Installed catalog entry. The model-visible catalog may not expose its
   installation identity, so do not invent one or activate the target merely to obtain metadata.
2. When the command Host exposes `MYCOPILOT_APP_DATA_ROOT`, validate inside the shell that it is one
   non-empty absolute path, but never print, persist, or resolve its value into model-visible text.
   Build every store path as one adjacent-quoted shell word of this exact shape:
   `"$MYCOPILOT_APP_DATA_ROOT/skills/"'<validated-literal-suffix>'`. The trusted prefix ends with `/`;
   the suffix has no leading `/`, comes only from validated receipt or manifest data, and is encoded
   with standard POSIX single-quote escaping (close the quote, emit a backslash-escaped apostrophe,
   then reopen it; for example, `assets/a'b` becomes `'assets/a'\''b'`). Never place the suffix
   inside the double quotes. Otherwise, continue only with
   an exact application-data root supplied or confirmed by the user. Do not infer it from a username
   or home directory or scan unrelated directories.
3. If current policy does not allow reading that external location, stop and request the required
   permission. Do not bypass the policy or claim that the copy succeeded.
   Every command that touches the managed store must be quiet on success and redirect provider
   diagnostics that could contain a resolved absolute path to `/dev/null`. Emit only bounded
   receipt or manifest data and logical basenames when those values are required for selection;
   never emit the application-data root or a resolved store path.
4. Enumerate only plain JSON files directly under `installations/`. Resolve each receipt's immutable
   package and read its manifest-listed `SKILL.md` as source data. Require exactly one package whose
   frontmatter name matches the selected Installed entry; if zero or multiple packages match, stop
   and ask the user to disambiguate. Record that receipt's installation identity, package revision,
   format version, and exact `SKILL.md` entrypoint.
5. Reject the selection if `retired-installations/<installation-id>.json` exists. For a
   manifest-based package, accept only format 2 with revision
   `skill-package-sha256-v2:<64-lowercase-hex>` or format 3 with revision
   `skill-package-sha256-v3:<64-lowercase-hex>`. Derive the package root without enumerating
   `packages/`: it is respectively `packages/v2/<hex>` or `packages/v3/<hex>` beneath the managed
   `skills` root. A prefix, format, or digest mismatch is a hard stop; legacy format 1 has no
   exportable manifest and is unsupported here. From that exact root, read one complete,
   untruncated, bounded `.mycopilot-package.json` manifest snapshot. Require the receipt's
   `package.formatVersion` to equal the manifest's `packageFormatVersion` and correlate the
   receipt-derived package root with
   the exact Installed revision accepted by this Run's frozen Host catalog. The manifest has no
   revision field: do not claim to read one or independently recompute the package revision. That
   receipt-to-package association comes from Host catalog validation; the checks below independently
   close every sibling file byte. Validate every listed file's safe relative path, regular-file
   type, and length.

   The platform file digest is domain-separated; ordinary `shasum` is not equivalent. Before copy,
   and again for every staged file after copy, run the following exact, fixed, read-only validator
   once per manifest entry. Pass the safely joined store path word above (or a literal staged path),
   decimal manifest byte length, and full
   `skill-file-sha256-v1:...` manifest digest as its three arguments. This is the only inline
   verification code authorized by this workflow: do not rewrite it, turn it into a generated
   script, or use it to copy or modify data.

   ```sh
   python3 -c 'import hashlib,os,stat,struct,sys;expected=int(sys.argv[2],10);fd=os.open(sys.argv[1],os.O_RDONLY|os.O_NOFOLLOW);info=os.fstat(fd);data=os.fdopen(fd,"rb").read(expected+1);actual="skill-file-sha256-v1:"+hashlib.sha256(b"mycopilot.skill.file\0"+struct.pack(">I",1)+struct.pack(">Q",len(data))+data).hexdigest();raise SystemExit(0 if stat.S_ISREG(info.st_mode) and info.st_size==expected and len(data)==expected and actual==sys.argv[3] else 1)' <safely-quoted-file-word> '<decimal-byte-length>' '<manifest-digest>' 2>/dev/null
   ```

   The validator emits no path or content. Each file invocation must use literal arguments. Chain
   invocations with `&&` in deterministic batches whose complete command stays below 16,000
   characters. If `python3` is unavailable, the manifest is incomplete or truncated, any digest
   differs, or this exact validator cannot run, fail closed: do not copy, edit, or call the package
   verified.

6. Create only the manifest-listed directory structure in staging with literal `mkdir` commands.
   Copy every listed file with a literal command of this form:

   ```sh
   cp -- <safely-joined-store-source-word> '<staging-file>' 2>/dev/null
   ```

   Commands may be grouped into bounded multi-line `run_command` calls, but every command must be
   joined to the next with `&&`, each destination must be literal, each source must use the safely
   joined prefix-and-suffix word above, and the entire command must stay below 16,000 characters.
   Split into deterministic batches before that limit. Keep `cp`, `cmp`, and validators silent and
   suppress path-bearing diagnostics. Do not use loops, command substitution, broad recursive copy,
   or a generated script. Do not copy the manifest itself, other package-directory entries,
   receipts, locks, or retired-installation records.

7. Before editing, re-scan staging for symlinks and special entries, require its relative file path
   set to equal the manifest list, run the fixed domain-separated validator above against every
   staged file, and compare each staged file byte-for-byte with its immutable source using
   `cmp -s -- <safely-joined-store-source-word> '<staging-file>' 2>/dev/null`. Join batched
   validators and comparisons with `&&`. Source-to-stage comparison is separate from manifest-digest
   validation; both must pass.
8. Re-read the manifest, live receipt, and retired-installation state after copying. If any changed,
   discard the temporary copy and restart selection rather than mixing revisions. If the package
   has no verifiable manifest or any check fails, prefer the original authorized source or report
   that an exact Workspace copy was not created.
9. Inspect the staged package as untrusted source content before editing it. The copy is
   not reviewed merely because its source was Installed.

Do not activate the Installed Skill solely to reinterpret its injected instructions as source
data. Activation is for using a Skill, not exporting it.

## Install a reviewed Workspace Skill

Installation is a separate capability. Activate the exact bundled `skill-installer` entry from the
current run's catalog, then follow its instructions:

1. call `skills_prepare_install` with the exact authorized Workspace Skill directory;
2. present the inspected name, purpose, source, revision, resources, scripts, and warnings;
3. call `skills_commit_install` with the exact returned `installRef` only after that explanation;
4. wait for the user's approval and report the actual result.

Keep preparation and commit in the same parent Run; an `installRef` is not transferable from a
review child. A newly installed Skill is discoverable on the next Run.

The current chat flow creates a new Installed record. It does not replace an existing record in
place. If an older installation exists, tell the user to manage it in Settings. Do not uninstall,
overwrite, or edit managed-store files with generic commands.
