export const MAX_GIT_DIFF_RENDER_CHARACTERS = 512 * 1024
export const MAX_GIT_DIFF_RENDER_LINES = 12_000
export const MAX_GIT_DIFF_RENDER_HUNKS = 1_000
export const MAX_GIT_DIFF_RENDER_LINE_CHARACTERS = 32 * 1024

export const MAX_GIT_DIFF_HYDRATION_CHARACTERS = 512 * 1024
export const MAX_GIT_DIFF_HYDRATION_LINES = 20_000

export type GitDiffRenderBudgetReason = 'characters' | 'hunks' | 'lineCharacters' | 'lines'

export type GitDiffRenderBudgetResult =
  | { ok: true; hunkCount: number; lineCount: number }
  | { ok: false; reason: GitDiffRenderBudgetReason }

interface TextScanLimits {
  countHunks: boolean
  maxCharacters: number
  maxLineCharacters: number
  maxLines: number
}

/**
 * Rejects pathological patches before `split`, parsing, syntax projection, or DOM allocation.
 * The core service enforces the transport budget; this is the renderer's independent last line
 * of defence against stale clients and malformed responses.
 */
export function assessGitDiffRenderBudget(patch: string): GitDiffRenderBudgetResult {
  return scanText(patch, {
    countHunks: true,
    maxCharacters: MAX_GIT_DIFF_RENDER_CHARACTERS,
    maxLineCharacters: MAX_GIT_DIFF_RENDER_LINE_CHARACTERS,
    maxLines: MAX_GIT_DIFF_RENDER_LINES
  })
}

/** Full snapshots are optional enrichment, so oversized inputs fall back to the compact patch. */
export function isGitDiffHydrationTextWithinBudget(text: string | null): boolean {
  if (text === null) return true
  return scanText(text, {
    countHunks: false,
    maxCharacters: MAX_GIT_DIFF_HYDRATION_CHARACTERS,
    maxLineCharacters: MAX_GIT_DIFF_RENDER_LINE_CHARACTERS,
    maxLines: MAX_GIT_DIFF_HYDRATION_LINES
  }).ok
}

function scanText(text: string, limits: TextScanLimits): GitDiffRenderBudgetResult {
  if (text.length > limits.maxCharacters) return { ok: false, reason: 'characters' }
  if (text.length === 0) return { hunkCount: 0, lineCount: 0, ok: true }

  let hunkCount = 0
  let lineCount = 0
  let lineStart = 0

  const finishLine = (lineEnd: number): GitDiffRenderBudgetResult | undefined => {
    lineCount += 1
    if (lineCount > limits.maxLines) return { ok: false, reason: 'lines' }
    if (lineEnd - lineStart > limits.maxLineCharacters) {
      return { ok: false, reason: 'lineCharacters' }
    }
    if (
      limits.countHunks &&
      text.charCodeAt(lineStart) === 64 &&
      text.charCodeAt(lineStart + 1) === 64 &&
      text.charCodeAt(lineStart + 2) === 32 &&
      text.charCodeAt(lineStart + 3) === 45
    ) {
      hunkCount += 1
      if (hunkCount > MAX_GIT_DIFF_RENDER_HUNKS) return { ok: false, reason: 'hunks' }
    }
    return undefined
  }

  for (let index = 0; index < text.length; index += 1) {
    const character = text.charCodeAt(index)
    if (character !== 10 && character !== 13) continue
    const failure = finishLine(index)
    if (failure) return failure
    if (character === 13 && text.charCodeAt(index + 1) === 10) index += 1
    lineStart = index + 1
  }

  if (lineStart < text.length) {
    const failure = finishLine(text.length)
    if (failure) return failure
  }
  return { hunkCount, lineCount, ok: true }
}
