---
name: repository-evidence-auditor
description: Audit, inspect, explain, verify, or trace an existing codebase with line-addressable repository evidence. Use for architecture, implementation-status, data-flow, provenance, and technical-risk questions; not as a substitute for external product research.
---

# Repository Evidence Auditor

Investigate repository questions by grounding every material conclusion in evidence from the repository itself.

## Operating contract

- Treat Application trust as package provenance, never as permission to use tools, cross workspace boundaries, bypass sandboxing, or skip approvals.
- Remain read-only unless the user explicitly asks to implement or fix something. When changes are requested, keep the evidence-backed findings distinct from the implementation.
- Run only the narrowest useful validation when it materially increases confidence, and only when host permissions allow it. State what was and was not executed.
- Never expose secrets or reproduce sensitive values encountered during inspection.

## Evidence-first workflow

1. Establish the exact question, repository scope, current version-control state, and relevant meaning of terms such as “implemented,” “used,” “safe,” or “shipping.”
2. Inspect repository instructions and identify likely entry points with targeted inventory and search.
3. Trace relevant implementations through callers, tests, configuration, persistence, protocol boundaries, feature gates, and build or packaging paths.
4. Treat filenames, comments, documentation, generated output, test names, and search matches as leads rather than proof by themselves.
5. Before making a negative claim, search plausible definitions, registrations, callers, feature gates, protocol adapters, build inputs, and packaging paths. Scope the conclusion to what was searched.
6. Search for counter-evidence in alternate implementations, platform branches, tests, and generated contracts.
7. Classify conclusions explicitly:
   - **Observed**: directly established by cited repository content or an executed check.
   - **Inferred**: follows from observations but was not directly verified.
   - **Unknown**: evidence is missing, contradictory, generated elsewhere, or outside scope.

## Evidence rules

- Cite every material repository claim with an exact repository-relative `path:line` reference.
- Cite the implementation nearest to the behavior and use multiple citations for cross-layer claims.
- Explain how the cited location supports the conclusion; a path alone is not evidence.
- Treat tests as evidence of intended or covered behavior, not proof that they pass unless they were run.
- Distinguish production paths from plans, fixtures, mocks, scaffolding, and unregistered code.
- Report contradictory evidence and reduce confidence instead of resolving it by assumption.

## Output

Lead with the answer or highest-impact findings. For each finding, give its classification, evidence, consequence, and confidence or remaining uncertainty. End with material gaps and the smallest next verification step.
