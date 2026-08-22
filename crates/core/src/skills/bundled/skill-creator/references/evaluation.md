# Independent evaluation

Use this reference for a new Skill, a substantial revision, or a smaller change whose behavior is
risky or uncertain.

## Choose the review depth

- New Skill: capability review by default.
- Substantial instruction or resource change: capability review by default.
- Description or trigger change: capability review plus a separate blind trigger review.
- Script, permission, or destructive workflow change: capability review with explicit failure and
  authorization cases.
- Typo-only or explanation-only edit: review is normally unnecessary.

## Capability review

Finish all parent writes first. Then spawn a new direct child with `fork_turns: "none"`. Give it a
realistic user request and enough file context to perform the task, but do not give the expected
answer or suspected defect.

Before spawning, ensure the target's frontmatter `name` is unique among Workspace Skills. If a
second Workspace Skill uses the same name, resolve the ambiguity first; `source=workspace` alone
does not identify the intended package.

The message should require the child to:

1. find the catalog entry whose name matches and whose source is `workspace`;
2. activate that exact entry;
3. report the activated source and package revision;
4. complete the realistic task;
5. avoid editing `.agents/skills/<target>`;
6. report evidence, limitations, and PASS or FAIL.

If evaluation needs output files, give the child a separate task-owned directory. Do not let review
artifacts enter the Skill package unless the user later chooses to keep them.

If child-Agent collaboration is unavailable, stop at static checks and state that independent
behavioral review was not run. Do not activate the changed Skill in the frozen parent Run or invent
a PASS result.

## Blind trigger review

Use a separate child. Provide only the realistic user request; do not mention the Skill name, path,
or expected activation. Ask it to report which Skill, if any, it selected naturally and why; the
reporting requirement is not permission to activate an otherwise irrelevant Skill. This tests the
catalog description rather than the already-loaded instructions.

Include at least one nearby request that should not trigger when over-triggering is a material risk.

## Review report

Require a compact report containing:

- task and review type;
- activated Skill id, source, and package revision, or `none` for a blind test;
- observable result and produced artifacts;
- expectation-by-expectation evidence;
- resource, platform, or authorization failures;
- PASS or FAIL;
- narrow recommended changes.

Do not treat fluent prose as proof. Inspect files and deterministic outputs when they matter. Do not
force quantitative checks onto visual taste, writing style, or another genuinely subjective result.

## Iterate

The parent reviews the report and changes only demonstrated weaknesses. Because each child Run
freezes its own Skill revision, create a new child after every revision that needs retesting. Never
ask an earlier reviewer to "reload" a modified Skill.

Version 1 intentionally uses chat reports rather than an HTML viewer or benchmark aggregation.
