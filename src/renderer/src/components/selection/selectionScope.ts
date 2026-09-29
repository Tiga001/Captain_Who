/** Shared document shortcuts select one visible page, never the entire application shell. */
export const SELECTION_ALL_EVENT = 'app:select-all'
export const SELECTION_COPY_EVENT = 'app:copy-all'
export type SelectionCopyAllEvent = CustomEvent<{ clipboardData: DataTransfer }>

const MODAL =
  '[role="dialog"][aria-modal="true"], [role="alertdialog"][aria-modal="true"], dialog[open]'
const REGION = `[data-selection-region], ${MODAL}`
const NATIVE = '[data-selection-native="true"], .xterm, .monaco-editor, .cm-editor, webview, iframe'

interface SelectionState {
  users: number
  interaction: Element | null
  revision: number
  all: { root: HTMLElement; range: Range } | null
  dispose: () => void
}

const states = new WeakMap<Document, SelectionState>()

function elementFor(node: EventTarget | Node | null): Element | null {
  return node instanceof Element ? node : node instanceof Node ? node.parentElement : null
}

function visible(element: Element): boolean {
  return (
    element.isConnected &&
    !element.closest('[hidden], [inert], [aria-hidden="true"], [data-selection-hidden="true"]') &&
    element.getClientRects().length > 0 &&
    element.ownerDocument.defaultView?.getComputedStyle(element).visibility !== 'hidden'
  )
}

function firstVisible(root: ParentNode, selector: string): HTMLElement | null {
  return [...root.querySelectorAll<HTMLElement>(selector)].find(visible) ?? null
}

function interactionElement(doc: Document, target?: EventTarget | null): Element | null {
  const state = states.get(doc)
  const candidates = [state?.interaction, elementFor(target ?? null), doc.activeElement]
  for (const candidate of candidates) {
    if (
      candidate &&
      candidate !== doc.body &&
      candidate !== doc.documentElement &&
      visible(candidate)
    )
      return candidate
  }
  const anchor = elementFor(doc.getSelection()?.anchorNode ?? null)
  return anchor && visible(anchor) ? anchor : null
}

function contentForRegion(region: HTMLElement): HTMLElement {
  return (
    firstVisible(region, '[data-selection-content="primary"]') ??
    firstVisible(region, '[data-selection-content="page"]') ??
    region
  )
}

function resolveScope(doc: Document, target?: EventTarget | null): HTMLElement | null {
  const anchor = interactionElement(doc, target)
  const owner = anchor?.closest<HTMLElement>('[data-selection-owner]')?.dataset.selectionOwner
  if (owner) {
    const ownerRegion = [...doc.querySelectorAll<HTMLElement>('[data-selection-region]')].find(
      (candidate) => candidate.dataset.selectionRegion === owner && visible(candidate)
    )
    if (ownerRegion) return contentForRegion(ownerRegion)
  }
  const region = anchor?.closest<HTMLElement>(REGION)
  if (region && visible(region)) return contentForRegion(region)

  // Toolbars/left navigation belong to the visible main page. Hidden retained workspaces
  // (settings, scheduled pages, inactive sidebar tabs) must never become the fallback.
  const fallback =
    firstVisible(doc, MODAL) ??
    firstVisible(doc, '[data-selection-region="main"]') ??
    firstVisible(doc, '[data-selection-region="conversation"]') ??
    firstVisible(doc, 'main')
  return fallback ? contentForRegion(fallback) : null
}

function nativeSelection(element: Element | null): boolean {
  return Boolean(
    element?.closest(`input, textarea, select, ${NATIVE}`) ||
    (element instanceof HTMLElement && element.isContentEditable)
  )
}

export function getSelectionInteractionRevision(doc: Document = document): number {
  return states.get(doc)?.revision ?? 0
}

export function isSelectionScopeActive(root: HTMLElement): boolean {
  if (!visible(root)) return false
  const current = resolveScope(root.ownerDocument)
  return Boolean(current && (current === root || root.contains(current) || current.contains(root)))
}

export function selectScopeContents(root: HTMLElement): void {
  if (!visible(root)) return
  const doc = root.ownerDocument
  const selection = doc.getSelection()
  if (!selection) return
  const range = doc.createRange()
  range.selectNodeContents(root)
  selection.removeAllRanges()
  selection.addRange(range)
  const state = states.get(doc)
  if (state) state.all = { root, range: range.cloneRange() }
}

function isFullSelection(doc: Document, all: NonNullable<SelectionState['all']>): boolean {
  const selection = doc.getSelection()
  if (!selection || selection.rangeCount !== 1 || selection.isCollapsed) return false
  const current = selection.getRangeAt(0)
  return (
    current.startContainer === all.range.startContainer &&
    current.startOffset === all.range.startOffset &&
    current.endContainer === all.range.endContainer &&
    current.endOffset === all.range.endOffset
  )
}

/** Native Edit > Select All uses this same path; false means the focused control owns it. */
export function requestScopedSelectAll(
  doc: Document = document,
  target?: EventTarget | null
): boolean {
  const anchor = interactionElement(doc, target)
  if (nativeSelection(anchor)) {
    const nativeRoot = anchor?.closest<HTMLElement>('[data-selection-native="true"]')
    if (nativeRoot) {
      return !nativeRoot.dispatchEvent(new Event(SELECTION_ALL_EVENT, { cancelable: true }))
    }
    return false
  }
  const root = resolveScope(doc, target)
  if (!root) return false
  const state = states.get(doc)
  if (state) state.all = null
  doc.getSelection()?.removeAllRanges()
  const request = new Event(SELECTION_ALL_EVENT, { cancelable: true })
  if (root.dispatchEvent(request)) selectScopeContents(root)
  return true
}

/** Ref-counted so isolated conversation surfaces can use the same shortcuts in tests/popouts. */
export function installSelectionScopes(doc: Document = document): () => void {
  let state = states.get(doc)
  if (!state) {
    state = { users: 0, interaction: null, revision: 0, all: null, dispose: () => {} }
    const current = state
    const remember = (event: Event) => {
      current.interaction = elementFor(event.composedPath()[0] ?? event.target)
      current.revision += 1
      current.all = null
    }
    const keydown = (event: KeyboardEvent) => {
      if (
        event.defaultPrevented ||
        event.isComposing ||
        event.altKey ||
        event.shiftKey ||
        !(event.metaKey || event.ctrlKey) ||
        event.key.toLowerCase() !== 'a'
      )
        return
      if (nativeSelection(interactionElement(doc, event.target))) return
      if (requestScopedSelectAll(doc, event.target)) event.preventDefault()
    }
    const copy = (event: ClipboardEvent) => {
      const all = current.all
      if (
        event.defaultPrevented ||
        !event.clipboardData ||
        !all ||
        nativeSelection(interactionElement(doc, event.target)) ||
        !isSelectionScopeActive(all.root) ||
        !isFullSelection(doc, all)
      )
        return
      const request: SelectionCopyAllEvent = new CustomEvent(SELECTION_COPY_EVENT, {
        cancelable: true,
        detail: { clipboardData: event.clipboardData }
      })
      if (!all.root.dispatchEvent(request)) event.preventDefault()
    }
    const selectionChanged = () => {
      if (current.all && !isFullSelection(doc, current.all)) current.all = null
    }
    doc.addEventListener('pointerdown', remember, true)
    doc.addEventListener('focusin', remember, true)
    doc.addEventListener('keydown', keydown, true)
    doc.addEventListener('copy', copy, true)
    doc.addEventListener('selectionchange', selectionChanged)
    current.dispose = () => {
      doc.removeEventListener('pointerdown', remember, true)
      doc.removeEventListener('focusin', remember, true)
      doc.removeEventListener('keydown', keydown, true)
      doc.removeEventListener('copy', copy, true)
      doc.removeEventListener('selectionchange', selectionChanged)
    }
    states.set(doc, current)
  }
  state.users += 1
  let released = false
  return () => {
    if (released) return
    released = true
    if (--state.users === 0) {
      state.dispose()
      states.delete(doc)
    }
  }
}
