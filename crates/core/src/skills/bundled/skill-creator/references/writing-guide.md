# Writing guide

Use this reference when drafting or substantially rewriting `SKILL.md`.

## Frontmatter

MyCopilot currently relies on two fields:

```yaml
---
name: concise-skill-name
description: State what the Skill does and the situations in which it should be selected.
---
```

- Always provide both fields. An Installed Skill cannot rely on the Workspace directory name as a
  fallback.
- Keep `name` at most 64 characters. Prefer lowercase letters, digits, and hyphens, and use the same
  value for the directory when practical.
- Keep `description` at most 1024 characters. Make it discriminating: describe the real capability
  and likely user intent without turning it into a catch-all.
- Do not invent compatibility, permission, dependency, or capability frontmatter. Those fields do
  not grant platform authority.

## Instructions

Assume the model already knows general reasoning, writing, and coding practices. Include guidance
that changes its decisions:

- a domain-specific workflow;
- non-obvious constraints or recovery rules;
- exact input and output contracts;
- platform boundaries that would otherwise cause a predictable failure;
- references to reusable resources.

Prefer outcomes and decision criteria over a rigid sequence when several approaches are valid. Use
absolute language only for safety, authorization, compatibility, destructive actions, or fragile
contracts.

Keep the entrypoint concise. Put mode-specific detail in references and link each reference from the
place where the model should read it. Do not duplicate the same rule across several files.

## Trigger quality

A good description answers both questions:

1. What concrete job does this Skill help perform?
2. What wording or task context should cause it to be considered?

Avoid descriptions such as "Helpful utilities" or "Use for many tasks." Also avoid naming one past
failure so narrowly that equivalent requests no longer trigger the Skill.

If trigger behavior changes, run both:

- an explicit capability review, which proves the Skill works when activated;
- a blind trigger review, which proves a fresh Agent can choose it from metadata alone.

## Update discipline

When editing an existing Skill:

- preserve unrelated instructions and resources;
- fix a demonstrated root cause rather than accumulating universal rules;
- remove stale examples and links;
- keep user authorization separate from workflow advice;
- state unsupported behavior instead of inventing a workaround.
