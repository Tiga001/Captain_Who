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

## Adapt an Installed Skill

Never edit the managed Installed Skill store. Prefer an original authorized local directory or
GitHub source and copy that source into a new Workspace Skill directory.

If only the installed package is available, the platform cannot export every package byte through
model tools. Do not activate the Installed Skill solely to reinterpret its injected instructions as
source data. With user confirmation, create a new Workspace Skill from the user's stated behavior
and any resources that can be recovered safely; explain the limitation and do not call the result
an exact copy.

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
