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

Finish all parent writes first. Record the target's file inventory and byte-content state as a
mutation guard, then enter a review barrier: neither the parent nor any reviewer may write the
target until the complete review set finishes. A later target write invalidates every verdict from
the barrier, even if the edit seems unrelated.

Spawn one new direct child with `fork_turns: "none"` for each review case. Never reuse a child Run,
send it a follow-up after an edit, or combine cases to save Runs. Give it one realistic user request
and enough file context to perform that case, but do not give the expected answer or suspected
defect.

Before spawning, ensure the target's frontmatter `name` is unique among Workspace Skills. If a
second Workspace Skill uses the same name, resolve the ambiguity first; `source=workspace` alone
does not identify the intended package.

The capability-review message may explicitly identify the target and should require the child to:

1. find the catalog entry whose name matches and whose source is `workspace`;
2. activate that exact entry;
3. complete the realistic task;
4. treat `.agents/skills/<target>` as read-only;
5. report the activated source and package revision in its final chat report;
6. report task-result evidence, limitations, and PASS or FAIL in that final report.

Explicit target and activation instructions make this a capability review, not a blind trigger
review. Prefer Host-provided or otherwise trusted child execution traces for activation source and
revision. If that trace is unavailable, explicitly label the final-report values as
`reviewer-reported` evidence, verify that the mutation guard is unchanged, and require every
capability reviewer in the set to report the same revision. Reviewer-reported evidence is accepted
only for an explicitly targeted capability review; it is never valid blind-trigger evidence.

If evaluation needs output files, give the child a separate task-owned directory outside the target
package. Never put reports, fixtures, generated artifacts, or `test-outputs` in the Skill being
reviewed. After every child finishes, compare the target with the barrier mutation guard before
accepting its report.

The first otherwise-valid capability review establishes the exact review package revision from a
Host-provided or trusted execution trace when available, or from its explicitly labeled
reviewer-reported evidence otherwise. Every later capability reviewer must report the same
revision, and any available trusted trace must agree with it. A positive blind case whose trusted
trace shows target activation must show that established revision; a negative blind case whose
trusted trace shows no target is revision-neutral. For a blind case, unavailable or incomplete
trace evidence makes the case `UNVERIFIED`; do not add meta-diagnostic instructions to the child
prompt to recover it. A missing capability source or revision, a trusted-trace conflict, or any
revision mismatch means the children did not review one frozen package and invalidates the review
set; finish any needed edits, establish a new barrier, and rerun every required case with fresh
children.

A child result cannot PASS if it is cancelled, interrupted, missing its final report or task result,
missing the source or revision evidence required for its review type, modifies the target, has a
mismatched revision, conflicts with an available trusted trace, or lacks evidence for a required
expectation. Do not infer activation or success from partial commentary or fluent prose.

If child-Agent collaboration is unavailable, stop at static checks and state that independent
behavioral review was not run. Do not activate the changed Skill in the frozen parent Run or invent
a PASS result.

## Blind trigger review

Use a separate fresh child for every blind case. Its input message must be exactly the original
natural-language user request for that case. Do not append review instructions, ask which Skill it
selected, request activation, source, revision, rationale, or PASS/FAIL details, or mention the
Skill name, path, expected activation result, earlier verdicts, or implementation. This tests the
catalog description rather than compliance with a meta-evaluation prompt.

Determine natural selection, activation, and revision only from Host-provided or otherwise trusted
child execution traces, never from self-authored child prose. If the Host does not expose sufficient
trace evidence, mark the blind case `UNVERIFIED`; do not modify or supplement the user request to
make the child disclose it. A capability review may explicitly name and activate the target, but it
cannot serve as blind-trigger evidence.

Run a positive case and each nearby request that should not trigger in independent child Runs when
over-triggering is a material risk.

## Parent review report

After reviewing the child task result, trusted execution trace, and mutation guard, produce a
compact parent report containing:

- task and review type;
- activated Skill id, source, and package revision with its evidence source (`trusted-trace` or
  `reviewer-reported`) for a capability review, `none` when a trusted negative-blind trace shows no
  selection, or `UNVERIFIED` when blind trace evidence is unavailable or incomplete;
- observable result and produced artifacts;
- expectation-by-expectation evidence;
- resource, platform, or authorization failures;
- PASS or FAIL;
- narrow recommended changes.

Do not treat fluent prose as proof. Inspect files and deterministic outputs when they matter. Do not
force quantitative checks onto visual taste, writing style, or another genuinely subjective result.

## Iterate

The parent reviews the reports and changes only demonstrated weaknesses. Any edit ends the current
barrier and invalidates all of its verdicts. Finish the edit, record a new mutation guard, and rerun
the complete required set with new children; never ask an earlier reviewer to "reload" a modified
Skill.

Version 1 intentionally uses chat reports rather than an HTML viewer or benchmark aggregation.
