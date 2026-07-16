export const DEFAULT_GIT_DIFF_EXPANSION_STEP = 20

export interface GitDiffGapExpansion {
  fromEnd: number
  fromStart: number
}

export type GitDiffExpansionState = ReadonlyMap<string, GitDiffGapExpansion>
export type GitDiffExpansionDirection = 'up' | 'down' | 'both'

export type GitDiffExpansionAction =
  | {
      direction: GitDiffExpansionDirection
      gapId: string
      lineCount: number
      step?: number
      type: 'expand'
    }
  | { gapId: string; type: 'collapse' }
  | { type: 'reset' }

export interface GitDiffExpandedSlices {
  collapsedCount: number
  end: { end: number; start: number } | null
  start: { end: number; start: number } | null
}

function normalizeNonNegativeInteger(value: number): number {
  if (!Number.isFinite(value)) return 0
  return Math.max(0, Math.floor(value))
}

function clampExpansion(expansion: GitDiffGapExpansion, lineCount: number): GitDiffGapExpansion {
  const count = normalizeNonNegativeInteger(lineCount)
  const fromStart = Math.min(normalizeNonNegativeInteger(expansion.fromStart), count)
  const fromEnd = Math.min(normalizeNonNegativeInteger(expansion.fromEnd), count - fromStart)
  return { fromEnd, fromStart }
}

export function reduceGitDiffExpansion(
  state: GitDiffExpansionState,
  action: GitDiffExpansionAction
): GitDiffExpansionState {
  if (action.type === 'reset') return new Map()
  if (action.type === 'collapse') {
    if (!state.has(action.gapId)) return state
    const next = new Map(state)
    next.delete(action.gapId)
    return next
  }

  const lineCount = normalizeNonNegativeInteger(action.lineCount)
  const step = normalizeNonNegativeInteger(action.step ?? DEFAULT_GIT_DIFF_EXPANSION_STEP)
  if (lineCount === 0 || step === 0) return state
  const current = clampExpansion(state.get(action.gapId) ?? { fromEnd: 0, fromStart: 0 }, lineCount)
  let fromStart = current.fromStart
  let fromEnd = current.fromEnd

  // A down arrow opens from the beginning of the gap; an up arrow opens from its end.
  if (action.direction === 'down' || action.direction === 'both') {
    fromStart = Math.min(fromStart + step, lineCount - fromEnd)
  }
  if (action.direction === 'up' || action.direction === 'both') {
    fromEnd = Math.min(fromEnd + step, lineCount - fromStart)
  }

  const nextValue = clampExpansion({ fromEnd, fromStart }, lineCount)
  if (
    current.fromStart === nextValue.fromStart &&
    current.fromEnd === nextValue.fromEnd &&
    state.has(action.gapId)
  ) {
    return state
  }
  const next = new Map(state)
  next.set(action.gapId, nextValue)
  return next
}

export function getGitDiffExpandedSlices(
  lineCount: number,
  expansion: GitDiffGapExpansion | undefined
): GitDiffExpandedSlices {
  const count = normalizeNonNegativeInteger(lineCount)
  const normalized = clampExpansion(expansion ?? { fromEnd: 0, fromStart: 0 }, count)
  return {
    collapsedCount: count - normalized.fromStart - normalized.fromEnd,
    end: normalized.fromEnd > 0 ? { end: count, start: count - normalized.fromEnd } : null,
    start: normalized.fromStart > 0 ? { end: normalized.fromStart, start: 0 } : null
  }
}
