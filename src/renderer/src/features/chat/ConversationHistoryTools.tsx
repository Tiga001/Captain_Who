import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ClipboardEvent,
  type RefObject
} from 'react'
import { Search, X, ChevronUp, ChevronDown, History } from 'lucide-react'
import { copyTextToClipboard } from '../../components/clipboard'
import {
  getSelectionInteractionRevision,
  installSelectionScopes,
  isSelectionScopeActive,
  SELECTION_ALL_EVENT,
  selectScopeContents
} from '../../components/selection/selectionScope'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { copyCollaborationTimelineSelection } from '../agentCollaboration/CollaborationTimelineActivity'
import {
  findConversationMessage,
  type ConversationSegmentsController
} from './useConversationSegments'

interface Point {
  messageId: string
  offset: number
}
interface SelectionSnapshot {
  start: Point
  end: Point
}
interface Match {
  messageId: string
  start: number
  end: number
}

function canHandleHistoryShortcut(root: HTMLElement | null, event: Event) {
  if (
    !root ||
    event.defaultPrevented ||
    root.closest('[inert],[hidden],[aria-hidden="true"]') ||
    !root.getClientRects().length
  )
    return false
  const target = event.target instanceof HTMLElement ? event.target : null
  return (
    !target ||
    target === document.body ||
    Boolean(root.closest('.conversation-surface')?.contains(target))
  )
}

function point(root: HTMLElement, node: Node, offset: number): Point | null {
  const message = (node instanceof Element ? node : node.parentElement)?.closest<HTMLElement>(
    '[data-message-id]'
  )
  if (!message?.dataset.messageId || !root.contains(message)) return null
  const range = document.createRange()
  range.selectNodeContents(message)
  range.setEnd(node, offset)
  return { messageId: message.dataset.messageId, offset: range.toString().length }
}

function textPoint(root: HTMLElement, offset: number): [Node, number] {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT)
  let node: Node | null
  while ((node = walker.nextNode())) {
    const length = node.textContent?.length ?? 0
    if (offset <= length) return [node, offset]
    offset -= length
  }
  return [root, root.childNodes.length]
}

function restoreSelection(root: HTMLElement, snapshot?: SelectionSnapshot) {
  if (!snapshot) {
    selectScopeContents(root)
    return true
  }
  const range = document.createRange()
  const start = findConversationMessage(root, snapshot.start.messageId)
  const end = findConversationMessage(root, snapshot.end.messageId)
  if (!start || !end) return false
  range.setStart(...textPoint(start, snapshot.start.offset))
  range.setEnd(...textPoint(end, snapshot.end.offset))
  const selection = window.getSelection()
  selection?.removeAllRanges()
  selection?.addRange(range)
  return true
}

export function useConversationHistoryTools(
  conversationId: string,
  segments: ConversationSegmentsController,
  containerRef: RefObject<HTMLDivElement | null>,
  contentRef: RefObject<HTMLDivElement | null>
) {
  const { t } = useFrontendConfig()
  const { revealMessage } = segments
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [matches, setMatches] = useState<Match[]>([])
  const [matchIndex, setMatchIndex] = useState(0)
  const [notice, setNotice] = useState<string | null>(null)
  const input = useRef<HTMLInputElement>(null)
  const searchInteractionRevision = useRef<number | null>(null)
  const intent = useRef<{
    conversationId: string
    interactionRevision: number
    copy: boolean
    selection?: SelectionSnapshot
    selectAgain?: boolean
  } | null>(null)

  useEffect(() => installSelectionScopes(contentRef.current?.ownerDocument), [contentRef])

  const hasCurrentSelectionIntent = useCallback(() => {
    const root = contentRef.current
    const pending = intent.current
    return Boolean(
      root &&
      pending?.conversationId === conversationId &&
      pending.interactionRevision === getSelectionInteractionRevision(root.ownerDocument) &&
      isSelectionScopeActive(root)
    )
  }, [contentRef, conversationId])

  useLayoutEffect(() => {
    const root = contentRef.current
    const field = input.current
    return () => {
      if (field === document.activeElement) field?.blur()
      const selection = window.getSelection()
      if (
        root &&
        selection &&
        (root.contains(selection.anchorNode) || root.contains(selection.focusNode))
      ) {
        selection.collapse(document.body, 0)
        selection.removeAllRanges()
      }
    }
  }, [contentRef, conversationId, open])

  useEffect(() => {
    setOpen(false)
    setQuery('')
    setMatches([])
    setNotice(null)
    intent.current = null
    searchInteractionRevision.current = null
  }, [conversationId])

  const rememberSearchIntent = useCallback(() => {
    const root = contentRef.current
    searchInteractionRevision.current = root
      ? getSelectionInteractionRevision(root.ownerDocument)
      : null
  }, [contentRef])

  const showSearch = useCallback(() => {
    const root = contentRef.current
    rememberSearchIntent()
    const revision = searchInteractionRevision.current
    setOpen(true)
    if (segments.remaining) segments.expandAll()
    requestAnimationFrame(() => {
      if (
        root &&
        isSelectionScopeActive(root) &&
        revision === getSelectionInteractionRevision(root.ownerDocument)
      )
        input.current?.focus()
    })
  }, [contentRef, rememberSearchIntent, segments])
  const closeSearch = useCallback(() => {
    // Release focus before removing the input; React's selection plugin tracks it globally.
    input.current?.blur()
    setOpen(false)
    setMatches([])
    searchInteractionRevision.current = null
  }, [])

  useEffect(() => {
    const root = contentRef.current
    if (!root) return
    const selectAll = (event: Event) => {
      if (event.target !== root || event.defaultPrevented || !isSelectionScopeActive(root)) return
      event.preventDefault()
      intent.current = {
        conversationId,
        interactionRevision: getSelectionInteractionRevision(root.ownerDocument),
        copy: false
      }
      if (segments.remaining) segments.expandAll()
      else {
        intent.current = null
        selectScopeContents(root)
      }
    }
    root.addEventListener(SELECTION_ALL_EVENT, selectAll)
    return () => root.removeEventListener(SELECTION_ALL_EVENT, selectAll)
  }, [contentRef, conversationId, segments])

  useEffect(() => {
    if (!segments.segmented) return
    const handle = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.altKey) return
      const root = containerRef.current
      if (!canHandleHistoryShortcut(root, event)) return
      const target = event.target instanceof HTMLElement ? event.target : null
      const editing = target?.closest('input,textarea,[contenteditable="true"]')
      if (event.key.toLowerCase() === 'f' && (editing || (root && isSelectionScopeActive(root)))) {
        event.preventDefault()
        showSearch()
      }
    }
    document.addEventListener('keydown', handle)
    return () => document.removeEventListener('keydown', handle)
  }, [containerRef, segments.segmented, showSearch])

  const onCopy = useCallback(
    (event: ClipboardEvent<HTMLDivElement>) => {
      const root = contentRef.current
      const selection = window.getSelection()
      if (segments.remaining && hasCurrentSelectionIntent()) {
        event.preventDefault()
        intent.current!.copy = true
        setNotice(t('chat.history.copyNeedsHistory'))
        segments.expandAll()
        return
      }
      if (
        root &&
        isSelectionScopeActive(root) &&
        segments.remaining &&
        selection?.rangeCount &&
        !selection.isCollapsed
      ) {
        const range = selection.getRangeAt(0)
        const crossesGap = [...root.querySelectorAll('[data-segment-placeholder]')].some((gap) =>
          range.intersectsNode(gap)
        )
        if (crossesGap) {
          event.preventDefault()
          const start = point(root, range.startContainer, range.startOffset)
          const end = point(root, range.endContainer, range.endOffset)
          intent.current = {
            conversationId,
            interactionRevision: getSelectionInteractionRevision(root.ownerDocument),
            copy: true,
            selection: start && end ? { start, end } : undefined,
            selectAgain: !(start && end)
          }
          setNotice(t('chat.history.copyNeedsHistory'))
          segments.expandAll()
          return
        }
      }
      copyCollaborationTimelineSelection(event)
    },
    [contentRef, conversationId, hasCurrentSelectionIntent, segments, t]
  )

  useEffect(() => {
    // Cmd+A can start asynchronous expansion before a selection exists. Its immediately
    // following Cmd+C may target the document rather than the message container.
    const copyPendingSelection = (event: globalThis.ClipboardEvent) => {
      if (
        !segments.remaining ||
        !hasCurrentSelectionIntent() ||
        !canHandleHistoryShortcut(containerRef.current, event)
      )
        return
      if (
        event.target instanceof Element &&
        event.target.closest('input,textarea,[contenteditable="true"]')
      )
        return
      event.preventDefault()
      intent.current!.copy = true
      setNotice(t('chat.history.copyNeedsHistory'))
      segments.expandAll()
    }
    document.addEventListener('copy', copyPendingSelection)
    return () => document.removeEventListener('copy', copyPendingSelection)
  }, [containerRef, hasCurrentSelectionIntent, segments, t])

  useEffect(() => {
    if (segments.remaining) return
    const pending = intent.current
    const root = contentRef.current
    if (!pending || pending.conversationId !== conversationId || !root) return
    const isCurrent = hasCurrentSelectionIntent()
    intent.current = null
    if (!isCurrent) return
    if (pending.selectAgain) {
      setNotice(t('chat.history.selectAgain'))
      return
    }
    if (!restoreSelection(root, pending.selection)) return
    if (!pending.copy) return
    // Reuse the normal semantic-copy handler after filling the gaps, then perform the user's
    // original copy. Never put a silently truncated selection on the clipboard.
    const clipboardData = new DataTransfer()
    containerRef.current?.dispatchEvent(
      new window.ClipboardEvent('copy', {
        bubbles: true,
        cancelable: true,
        clipboardData
      })
    )
    const text = clipboardData.getData('text/plain') || window.getSelection()?.toString() || ''
    void copyTextToClipboard(text).then(
      () => setNotice(null),
      () => setNotice(t('chat.history.selectAgain'))
    )
  }, [containerRef, contentRef, conversationId, hasCurrentSelectionIntent, segments.remaining, t])

  useEffect(() => {
    if (!open || segments.remaining || !query.trim()) {
      setMatches([])
      return
    }
    const timeout = setTimeout(() => {
      const found: Match[] = []
      const needle = query.toLocaleLowerCase()
      for (const element of contentRef.current?.querySelectorAll<HTMLElement>(
        '[data-message-id]'
      ) ?? []) {
        const text = (element.textContent ?? '').toLocaleLowerCase()
        let from = 0
        let offset: number
        while ((offset = text.indexOf(needle, from)) !== -1) {
          found.push({
            messageId: element.dataset.messageId!,
            start: offset,
            end: offset + needle.length
          })
          from = offset + Math.max(1, needle.length)
        }
      }
      setMatches(found)
      setMatchIndex(0)
    }, 120)
    return () => clearTimeout(timeout)
  }, [contentRef, open, query, segments.remaining])

  useEffect(() => {
    const match = matches[matchIndex]
    const root = contentRef.current
    if (
      !match ||
      !root ||
      !isSelectionScopeActive(root) ||
      searchInteractionRevision.current !== getSelectionInteractionRevision(root.ownerDocument)
    )
      return
    revealMessage(match.messageId, 'center')
    restoreSelection(root, {
      start: { messageId: match.messageId, offset: match.start },
      end: { messageId: match.messageId, offset: match.end }
    })
  }, [contentRef, matchIndex, matches, revealMessage])

  const cancel = () => {
    segments.cancelExpansion()
    intent.current = null
    setNotice(null)
  }
  const controls = segments.segmented ? (
    <div className="conversation-history-tools">
      <div className="conversation-history-tools__actions">
        {segments.remaining > 0 && (
          <button type="button" onClick={segments.expanding ? cancel : segments.expandAll}>
            <History aria-hidden="true" />
            {t(segments.expanding ? 'chat.history.cancel' : 'chat.history.expandAll')}
          </button>
        )}
        <button type="button" aria-label={t('chat.history.find')} onClick={showSearch}>
          <Search aria-hidden="true" />
        </button>
      </div>
      {segments.expanding && <div role="status">{t('chat.history.expanding')}</div>}
      {notice && <div role="status">{notice}</div>}
      {open && (
        <div className="conversation-history-tools__search">
          <input
            ref={input}
            aria-label={t('chat.history.searchPlaceholder')}
            placeholder={t('chat.history.searchPlaceholder')}
            value={query}
            onFocus={rememberSearchIntent}
            onChange={(event) => {
              rememberSearchIntent()
              setQuery(event.target.value)
              if (segments.remaining) segments.expandAll()
            }}
            onKeyDown={(event) => {
              if (event.key === 'Escape') {
                closeSearch()
              }
              if (event.key === 'Enter' && matches.length) {
                rememberSearchIntent()
                setMatchIndex(
                  (index) => (index + (event.shiftKey ? matches.length - 1 : 1)) % matches.length
                )
              }
            }}
          />
          <span role="status">
            {matches.length
              ? `${matchIndex + 1} / ${matches.length}`
              : query && !segments.remaining
                ? t('chat.history.noMatches')
                : ''}
          </span>
          <button
            type="button"
            aria-label={t('chat.history.previous')}
            disabled={!matches.length}
            onClick={() => {
              rememberSearchIntent()
              setMatchIndex((index) => (index + matches.length - 1) % matches.length)
            }}
          >
            <ChevronUp aria-hidden="true" />
          </button>
          <button
            type="button"
            aria-label={t('chat.history.next')}
            disabled={!matches.length}
            onClick={() => {
              rememberSearchIntent()
              setMatchIndex((index) => (index + 1) % matches.length)
            }}
          >
            <ChevronDown aria-hidden="true" />
          </button>
          <button type="button" aria-label={t('chat.history.close')} onClick={closeSearch}>
            <X aria-hidden="true" />
          </button>
        </div>
      )}
    </div>
  ) : null
  return { controls, onCopy }
}
