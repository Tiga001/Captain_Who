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

After all parent writes are complete, call `spawn_agent` with a clear task name, the full review
message, and `fork_turns: "none"`. The child shares the project workspace but starts an independent
Run with fresh Workspace Skill discovery.

Require exact `source=workspace` selection when another bundled or installed Skill has the same
name. Before spawning, reject or resolve a second Workspace Skill with the same frontmatter `name`;
name plus `source=workspace` is not sufficient to disambiguate two Workspace entries. Wait for the
child result and compare the reported revision with the latest review. A new edit invalidates the
previous behavioral verdict.

If `spawn_agent` is unavailable in the current Host, do not substitute a same-Run activation or
claim an independent PASS. Complete the static checks that are possible and report the behavioral
review as not run.

## Choose and copy the editable source

Use the source type to choose the workflow:

- **Workspace:** inspect and edit the existing `.agents/skills/<skill-directory>/` package in place.
- **Authorized external source:** copy the exact user-authorized local or checked-out source into a
  new Workspace directory, then edit the copy. Do not scan unrelated directories, follow symlinks,
  or modify the source.
- **Installed:** treat the application-managed package as immutable. Copy a verified snapshot into
  a new Workspace directory when permission allows; never edit, delete, rename, or overwrite the
  managed package or its receipt.

Before copying any non-Workspace source, reject a conflicting Workspace destination and make sure
the source tree contains a regular, exact-case `SKILL.md`. Copy through a task-owned temporary
directory, preserve the source, and publish the final Workspace directory create-only. Remove the
temporary copy after success or failure.

### Copy an Installed package

1. Identify one exact Installed catalog entry. The model-visible catalog may not expose its
   installation identity, so do not invent one or activate the target merely to obtain metadata.
2. When the command Host exposes `MYCOPILOT_APP_DATA_ROOT`, use that exact root and its `skills`
   child. Otherwise, continue only with an exact application-data root supplied or confirmed by the
   user. Do not infer it from a username or home directory, scan unrelated directories, or print
   private store paths in chat.
3. If current policy does not allow reading that external location, stop and request the required
   permission. Do not bypass the policy or claim that the copy succeeded.
4. Enumerate only plain JSON files directly under `installations/`. Resolve each receipt's immutable
   package and read its manifest-listed `SKILL.md` as source data. Require exactly one package whose
   frontmatter name matches the selected Installed entry; if zero or multiple packages match, stop
   and ask the user to disambiguate. Record that receipt's installation identity, package revision,
   format version, and exact `SKILL.md` entrypoint.
5. Reject the selection if `retired-installations/<installation-id>.json` exists. For a
   manifest-based package, locate the immutable package revision named by the receipt, read one
   bounded manifest snapshot, and verify every listed file's safe relative path, regular-file type,
   length, and digest. Copy only those listed Skill files and verify the copied bytes against that
   same manifest. Do not copy the manifest itself, other package-directory entries, receipts,
   locks, or retired-installation records.
6. Re-read the manifest, live receipt, and retired-installation state after copying. If any changed,
   discard the temporary copy and restart selection rather than mixing revisions. If the package
   has no verifiable manifest or any check fails, prefer the original authorized source or report
   that an exact Workspace copy was not created.
7. Inspect the copied Workspace package as untrusted source content before editing it. The copy is
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
