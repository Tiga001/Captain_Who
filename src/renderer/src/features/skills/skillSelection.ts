import type { SkillDescriptor, SkillSelection } from '@mycopilot/protocol'

export const MAX_SELECTED_SKILLS = 8
const MAX_SKILL_ID_BYTES = 16 * 1024
const MAX_SKILL_REVISION_BYTES = 256

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value))
}

function normalizeOpaqueAscii(value: unknown, maximumLength: number): string | null {
  if (typeof value !== 'string' || value.length === 0 || value.length > maximumLength) return null
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index)
    if (code < 0x21 || code > 0x7e) return null
  }
  return value
}

function normalizeSkillId(value: unknown): string | null {
  const normalized = normalizeOpaqueAscii(value, MAX_SKILL_ID_BYTES)
  if (!normalized) return null

  const localSeparator = normalized.lastIndexOf(':')
  if (localSeparator <= 0 || localSeparator === normalized.length - 1) return null
  const sourceId = normalized.slice(0, localSeparator)
  if (!sourceId.includes(':') || sourceId.endsWith(':')) return null
  return normalized
}

/**
 * Treats a Skill id as the stable selection key. A second revision for the same id must never be
 * activated alongside the first one, and stored input cannot bypass the composer selection limit.
 */
export function normalizeSkillSelections(value: unknown): SkillSelection[] {
  if (!Array.isArray(value)) return []

  const selections: SkillSelection[] = []
  const seenIds = new Set<string>()

  for (const candidate of value) {
    if (selections.length >= MAX_SELECTED_SKILLS) break
    if (!isRecord(candidate)) continue

    const id = normalizeSkillId(candidate.id)
    const revision = normalizeOpaqueAscii(candidate.revision, MAX_SKILL_REVISION_BYTES)
    if (!id || !revision || seenIds.has(id)) continue

    seenIds.add(id)
    selections.push({ id, revision })
  }

  return selections
}

export function parseStoredSkillSelections(value: string | null | undefined): SkillSelection[] {
  if (!value) return []
  try {
    return normalizeSkillSelections(JSON.parse(value) as unknown)
  } catch {
    return []
  }
}

export function mergeSkillSelections(
  preferred: readonly SkillSelection[],
  fallback: readonly SkillSelection[]
): SkillSelection[] {
  return normalizeSkillSelections([...preferred, ...fallback])
}

/**
 * Keeps only selections whose source is valid without a workspace. Unknown source prefixes are
 * dropped fail-closed so changing project scope can never leak a workspace-bound selection into a
 * project-less run.
 */
export function retainGlobalSkillSelections(
  selections: readonly SkillSelection[]
): SkillSelection[] {
  return selections.filter(
    (selection) => selection.id.startsWith('bundled:') || selection.id.startsWith('installed:')
  )
}

export function toggleSkillSelection(
  selections: readonly SkillSelection[],
  descriptor: SkillDescriptor
): SkillSelection[] {
  const selectedIndex = selections.findIndex((selection) => selection.id === descriptor.id)
  if (selectedIndex >= 0) {
    return selections.filter((_, index) => index !== selectedIndex)
  }
  if (selections.length >= MAX_SELECTED_SKILLS) return [...selections]

  return [...selections, { id: descriptor.id, revision: descriptor.revision }]
}

export function updateSkillSelectionRevision(
  selections: readonly SkillSelection[],
  descriptor: SkillDescriptor
): SkillSelection[] {
  return selections.map((selection) =>
    selection.id === descriptor.id
      ? { id: descriptor.id, revision: descriptor.revision }
      : selection
  )
}

export function removeSkillSelection(
  selections: readonly SkillSelection[],
  skillId: string
): SkillSelection[] {
  return selections.filter((selection) => selection.id !== skillId)
}

export type SkillSelectionStatus = 'current' | 'stale' | 'unavailable'

export interface SkillSelectionMatch {
  descriptor?: SkillDescriptor
  status: SkillSelectionStatus
}

export function matchSkillSelection(
  selection: SkillSelection,
  descriptors: readonly SkillDescriptor[]
): SkillSelectionMatch {
  const descriptor = descriptors.find((candidate) => candidate.id === selection.id)
  if (!descriptor) return { status: 'unavailable' }

  return {
    descriptor,
    status: descriptor.revision === selection.revision ? 'current' : 'stale'
  }
}

export function filterSkillDescriptors(
  descriptors: readonly SkillDescriptor[],
  search: string
): SkillDescriptor[] {
  const query = search.trim().toLocaleLowerCase()
  return descriptors.filter((descriptor) => {
    if (!query) return true

    return `${descriptor.name}\n${descriptor.description}`.toLocaleLowerCase().includes(query)
  })
}

export function getSkillFallbackName(skillId: string): string {
  const encodedName = skillId.split(':').at(-1) ?? skillId
  let name = encodedName
  try {
    name = decodeURIComponent(encodedName)
  } catch {
    // Skill ids are opaque. A malformed display suffix must not affect the stored selection.
  }

  const normalized = name.trim() || skillId
  return normalized.length > 42 ? `${normalized.slice(0, 39)}...` : normalized
}
