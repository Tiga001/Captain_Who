import type { ElementHandle, Frame, Page } from 'playwright'

const MAX_FRAME_DEPTH = 8
const MAX_FRAMES = 32
const MAX_EDITORS_PER_FRAME = 16
const MAX_EDITOR_CANDIDATES = 64

// Keep the selector intentionally small. Playwright serializes its parsed selector AST across the
// private CDP relay, where the ordinary command node/depth budgets must remain effective. Element
// state is filtered below without returning a value or any page-provided string.
const EDITOR_SELECTOR = 'input, textarea, [contenteditable]'

export type BrowserFrameEditorKind = 'input' | 'textarea' | 'contenteditable'

/**
 * Safe recovery metadata for an editable control hidden by an opaque iframe accessibility tree.
 *
 * The object deliberately contains no URL, accessible name, DOM text, value, selector supplied by
 * the page, or Target identity. The exact ElementHandle is retained only inside Main so a later
 * sibling reorder cannot redirect text to a different editor. It is never serialized.
 */
export interface BrowserFrameEditorCandidate {
  framePath: readonly number[]
  frameTypes: readonly ('frame' | 'iframe')[]
  editorIndex: number
  kind: BrowserFrameEditorKind
  frame: Frame
  element: ElementHandle<HTMLElement>
}

/**
 * Enumerates only safe, bounded editor metadata from child frames of the managed page.
 * Detached/navigating frames are skipped. A failure in one frame never widens the query to DOM
 * content or falls back to browser_evaluate.
 */
export async function probeBrowserFrameEditors(
  page: Page
): Promise<readonly BrowserFrameEditorCandidate[]> {
  const candidates: BrowserFrameEditorCandidate[] = []
  const state = { frames: 0 }
  await probeChildren(page.mainFrame(), [], [], candidates, state)
  return candidates
}

export function formatBrowserFrameEditorCandidates(
  candidates: readonly BrowserFrameEditorCandidate[],
  targetForCandidate?: (candidate: BrowserFrameEditorCandidate, index: number) => string
): string {
  if (candidates.length === 0) return ''
  return [
    'Managed iframe editor candidates (safe metadata; page content and values omitted):',
    ...candidates.map((candidate, index) => {
      const target = targetForCandidate?.(candidate, index)
      return (
        `- frame ${candidate.framePath.join('.')} (${candidate.frameTypes.join(' > ')}), ` +
        `${candidate.kind} ${candidate.editorIndex}${target ? `: ${target}` : ''}`
      )
    })
  ].join('\n')
}

async function probeChildren(
  parent: Frame,
  parentPath: readonly number[],
  parentTypes: readonly ('frame' | 'iframe')[],
  candidates: BrowserFrameEditorCandidate[],
  state: { frames: number }
): Promise<void> {
  if (
    parentPath.length >= MAX_FRAME_DEPTH ||
    state.frames >= MAX_FRAMES ||
    candidates.length >= MAX_EDITOR_CANDIDATES
  ) {
    return
  }

  for (const child of parent.childFrames()) {
    if (state.frames >= MAX_FRAMES || candidates.length >= MAX_EDITOR_CANDIDATES) return
    state.frames += 1
    const identity = await safeFrameIdentity(child)
    if (!identity) continue
    const framePath = [...parentPath, identity.index]
    const frameTypes = [...parentTypes, identity.type]
    await probeEditors(child, framePath, frameTypes, candidates)
    await probeChildren(child, framePath, frameTypes, candidates, state)
  }
}

async function safeFrameIdentity(
  frame: Frame
): Promise<{ index: number; type: 'frame' | 'iframe' } | null> {
  try {
    const frameElement = await frame.frameElement()
    try {
      return await frameElement.evaluate((element) => {
        if (!(element instanceof Element) || !element.ownerDocument) return null
        const tagName = element.tagName.toLowerCase()
        if (tagName !== 'iframe' && tagName !== 'frame') return null
        const index = [...element.ownerDocument.querySelectorAll('iframe, frame')].indexOf(element)
        return index >= 0 ? { index, type: tagName } : null
      })
    } finally {
      await frameElement.dispose()
    }
  } catch {
    return null
  }
}

async function probeEditors(
  frame: Frame,
  framePath: readonly number[],
  frameTypes: readonly ('frame' | 'iframe')[],
  candidates: BrowserFrameEditorCandidate[]
): Promise<void> {
  try {
    const editors = frame.locator(EDITOR_SELECTOR)
    const metadata = await editors.evaluateAll((elements, maximum) => {
      return elements
        .slice(0, maximum)
        .reduce<Array<{ index: number; kind: 'input' | 'textarea' | 'contenteditable' }>>(
          (result, element, index) => {
            if (!(element instanceof HTMLElement) || element.getClientRects().length === 0) {
              return result
            }
            const tagName = element.tagName.toLowerCase()
            if (tagName === 'input') {
              const input = element as HTMLInputElement
              if (
                ['email', 'number', 'password', 'search', 'tel', 'text', 'url'].includes(
                  input.type
                ) &&
                !input.disabled &&
                !input.readOnly
              ) {
                result.push({ index, kind: 'input' })
              }
              return result
            }
            if (tagName === 'textarea') {
              const textarea = element as HTMLTextAreaElement
              if (!textarea.disabled && !textarea.readOnly) {
                result.push({ index, kind: 'textarea' })
              }
              return result
            }
            if (element.isContentEditable) {
              result.push({ index, kind: 'contenteditable' })
            }
            return result
          },
          []
        )
    }, MAX_EDITORS_PER_FRAME)
    for (const { index, kind } of metadata) {
      if (candidates.length >= MAX_EDITOR_CANDIDATES) return
      const element = await editors.nth(index).elementHandle()
      if (!element) continue
      candidates.push({
        framePath,
        frameTypes,
        editorIndex: index,
        kind,
        frame,
        element: element as ElementHandle<HTMLElement>
      })
    }
  } catch {
    // A frame may navigate or detach between enumeration and inspection. Fail closed for it.
  }
}
