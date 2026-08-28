# Optional evaluation records

Version 1 normally reports reviews directly in chat. Persist these small JSON files only when the
user wants reusable evaluation cases or machine-readable review history.

## `evals/evals.json`

```json
{
  "skill_name": "example-skill",
  "evals": [
    {
      "id": 1,
      "prompt": "A realistic user request",
      "expected_output": "A short description of successful behavior",
      "files": [],
      "expectations": ["The requested artifact is created", "The source file remains unchanged"]
    }
  ]
}
```

Rules:

- `skill_name` matches the Skill frontmatter.
- `id` is a unique positive integer.
- `prompt` is realistic and must not reveal the intended implementation.
- `expected_output` is a human-readable outcome, not a hidden answer supplied to the reviewer.
- `files` contains project-relative input paths only.
- `expectations` contains observable, individually reviewable statements.

Use the field name `expectations` consistently. Do not alternate between `assertions` and
`expectations`.

## `review.json`

```json
{
  "eval_id": 1,
  "review_type": "capability",
  "skill": {
    "id": "workspace:project-id:example-skill",
    "source": "workspace:project-id",
    "revision": "skill-package-sha256-v3:..."
  },
  "expectations": [
    {
      "text": "The requested artifact is created",
      "passed": true,
      "evidence": "The output file exists and was reopened successfully."
    }
  ],
  "verdict": "pass",
  "limitations": []
}
```

For a blind trigger review, `skill` may be `null` when no Skill was activated. `review_type` is
`capability` or `blind_trigger`; `verdict` is `pass` or `fail`.
