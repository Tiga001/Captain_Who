import type { ActivatedSkillSummary } from '@mycopilot/protocol'

/**
 * Merges backend-authoritative activated Skill summaries into one run-scoped inventory.
 *
 * Existing order is stable so a late activation cannot reshuffle the activity UI. Historical
 * duplicates are collapsed, while an incoming record for the same Skill replaces the prior
 * revision and presentation metadata in place. The helper is intentionally presentation-only:
 * it never participates in activation or permission decisions.
 */
export function mergeActivatedSkillSummaries(
  current: readonly ActivatedSkillSummary[] | undefined,
  incoming: readonly ActivatedSkillSummary[]
): ActivatedSkillSummary[] {
  const merged: ActivatedSkillSummary[] = []
  const indexById = new Map<string, number>()

  for (const skill of current ?? []) {
    if (indexById.has(skill.id)) continue
    indexById.set(skill.id, merged.length)
    merged.push(skill)
  }

  for (const skill of incoming) {
    const existingIndex = indexById.get(skill.id)
    if (existingIndex === undefined) {
      indexById.set(skill.id, merged.length)
      merged.push(skill)
    } else {
      merged[existingIndex] = skill
    }
  }

  return merged
}
