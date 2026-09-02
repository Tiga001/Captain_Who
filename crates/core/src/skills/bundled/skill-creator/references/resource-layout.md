# Resource layout

Read this reference when a Skill needs supporting files.

## Package structure

```text
<skill-name>/
├── SKILL.md
├── references/   # Text guidance loaded only when needed
├── assets/       # Files used in generated output
├── templates/    # Reusable starter trees or files
└── scripts/      # Optional executable Python helpers
```

Captain Who packages the complete safe directory tree into an immutable revision. Resource kinds are
derived from paths:

- `references/**` → `reference`
- `assets/**` → `asset`
- `scripts/**` → `script`
- `templates/**` and other safe package files → `other`

Do not add empty directories, placeholders, a README, or a manifest without a concrete consumer.
Every reference must be linked from `SKILL.md` or another reachable reference with guidance about
when to read it.

## References

Use references for schemas, examples, domain rules, APIs, and conditional workflows. Keep one source
of truth for each rule. For a large reference, add a short contents list or useful search terms.

## Assets and templates

Assets belong in generated output and normally should not be loaded as instructions. Templates are
classified as `other` resources but may be materialized through the existing template subtree
contract.

`skills_materialize_resource` is create-only and approval-gated. Its destination must be a new
workspace-relative path whose parent already exists. It cannot write into `.agents`, `.git`, `.hg`,
or `.svn`. Materialize a starter into a task-owned temporary directory, then use ordinary file
editing to create the final Workspace Skill.

## Scripts

Only add a script when deterministic repeated logic materially improves reliability. Current generic
Skill script execution supports Python files under `scripts/`; it requires the existing preflight,
Full Access, and per-call approval boundaries.

Keep each script standalone. The runtime executes one frozen script with isolated Python behavior,
so do not depend on sibling imports, package installation, a user shell profile, a local server, or a
vendor-specific CLI. Declare third-party requirements only when they are genuinely necessary and
available through the platform contract. If the workflow works clearly without a script, omit it.

## Package safety

Use UTF-8 text, portable relative paths, and regular files. Do not use symlinks, path traversal,
special files, or generated dependency trees. Never treat a package revision as publisher identity
or permission; it proves content identity only.
