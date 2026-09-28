import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type RefObject
} from 'react'
import type { ChatMessage } from './chatTypes'

export interface ConversationScrollAnchor {
  messageId: string
  offset: number
  scrollTop: number
  atBottom: boolean
}
/** Numbers remain accepted for positions saved by older navigation callers. */
export type ConversationScrollPosition = number | ConversationScrollAnchor
export const CONVERSATION_SEGMENT_SIZE = 16
export const CONVERSATION_SEGMENT_THRESHOLD = 64
const PRELOAD_PX = 500

export interface ConversationMessageSegment {
  index: number
  start: number
  end: number
  key: string
  estimatedHeight: number
  mounted: boolean
}

interface Options {
  conversationId: string
  messages: readonly ChatMessage[]
  containerRef: RefObject<HTMLDivElement | null>
  initialPosition: ConversationScrollPosition | null
  targetMessageId?: string | null
  pinnedMessageIds: ReadonlySet<string>
  setPosition: (top: number, follow?: boolean) => void
  followsBottom: () => boolean
}

export function findConversationMessage(root: HTMLElement, id: string) {
  return root.querySelector<HTMLElement>(`[data-message-id="${CSS.escape(id)}"]`)
}

export function useConversationSegments({
  conversationId,
  messages,
  containerRef,
  initialPosition,
  targetMessageId,
  pinnedMessageIds,
  setPosition,
  followsBottom
}: Options) {
  const segmented = messages.length > CONVERSATION_SEGMENT_THRESHOLD
  const messageIndices = useMemo(
    () => new Map(messages.map((message, index) => [message.id, index])),
    [messages]
  )
  const definitions = useMemo(() => {
    const result: Omit<ConversationMessageSegment, 'mounted'>[] = []
    for (let start = 0; start < messages.length; start += CONVERSATION_SEGMENT_SIZE) {
      const end = Math.min(messages.length, start + CONVERSATION_SEGMENT_SIZE)
      let estimatedHeight = 0
      for (let index = start; index < end; index += 1) {
        const message = messages[index]
        estimatedHeight +=
          40 +
          (message.role === 'user' ? 72 : 120) +
          Math.min(1200, Math.ceil(message.content.length / 65) * 24) +
          (message.attachments?.length ? 126 : 0) +
          (message.agentRun ? 48 : 0)
      }
      result.push({ index: result.length, start, end, key: messages[start].id, estimatedHeight })
    }
    return result
  }, [messages])
  const initialIndices = useMemo(() => {
    if (!segmented) return new Set(definitions.map((definition) => definition.index))
    const last = definitions.length - 1
    const id =
      targetMessageId ?? (typeof initialPosition === 'object' ? initialPosition?.messageId : null)
    const targetIndex = id ? messageIndices.get(id) : undefined
    const center =
      targetIndex === undefined
        ? initialPosition !== null &&
          !(typeof initialPosition === 'object' && initialPosition.atBottom)
          ? 0
          : last
        : Math.floor(targetIndex / CONVERSATION_SEGMENT_SIZE)
    return new Set(
      [center, center > 0 ? center - 1 : center + 1].filter((index) => index >= 0 && index <= last)
    )
  }, [definitions, initialPosition, messageIndices, segmented, targetMessageId])
  const [state, setState] = useState(() => ({ conversationId, mounted: initialIndices }))
  const [expanding, setExpanding] = useState(false)
  const [visibleMessageIds, setVisibleMessageIds] = useState<ReadonlySet<string>>(() => new Set())
  const mounted = state.conversationId === conversationId ? state.mounted : initialIndices
  const pinnedIndices = new Set<number>()
  for (const id of pinnedMessageIds) {
    const index = messageIndices.get(id)
    if (index !== undefined) pinnedIndices.add(Math.floor(index / CONVERSATION_SEGMENT_SIZE))
  }
  const segments = definitions.map((definition) => ({
    ...definition,
    mounted: !segmented || mounted.has(definition.index) || pinnedIndices.has(definition.index)
  }))
  const remaining = segments.filter((segment) => !segment.mounted).length
  const segmentElements = useRef(new Map<number, HTMLElement>())
  const current = useRef({ segments, mounted, conversationId, messageIndices })
  useLayoutEffect(() => {
    current.current = { segments, mounted, conversationId, messageIndices }
  })
  const anchor = useRef<ConversationScrollAnchor | null>(null)
  const placeholderAnchor = useRef<{ index: number; fraction: number; scrollTop: number } | null>(
    null
  )
  const pending = useRef<{ id: string; offset?: number; block?: ScrollLogicalPosition } | null>(
    null
  )
  const [navigationRevision, setNavigationRevision] = useState(0)
  const scanFrame = useRef<number | null>(null)
  const initializedConversation = useRef<string | null>(null)

  const mountIndices = useCallback((indices: number[]) => {
    const snapshot = current.current
    if (!indices.some((index) => snapshot.segments[index] && !snapshot.mounted.has(index))) return
    setState((previous) => {
      const base =
        previous.conversationId === snapshot.conversationId ? previous.mounted : snapshot.mounted
      const next = new Set(base)
      for (const index of indices) if (snapshot.segments[index]) next.add(index)
      return next.size === base.size
        ? previous
        : { conversationId: snapshot.conversationId, mounted: next }
    })
  }, [])

  const visibleSegments = useCallback(
    (margin = 0) => {
      const root = containerRef.current
      if (!root) return []
      const rootRect = root.getBoundingClientRect()
      const count = current.current.segments.length
      let low = 0
      let high = count
      // Segment wrappers exist even when their expensive message bodies have not mounted.
      while (low < high) {
        const mid = Math.floor((low + high) / 2)
        const bottom = segmentElements.current.get(mid)?.getBoundingClientRect().bottom ?? 0
        if (bottom < rootRect.top - margin) low = mid + 1
        else high = mid
      }
      const visible: number[] = []
      for (let index = low; index < count; index += 1) {
        const element = segmentElements.current.get(index)
        if (!element || element.getBoundingClientRect().top > rootRect.bottom + margin) break
        visible.push(index)
      }
      return visible
    },
    [containerRef]
  )

  const captureAnchor = useCallback(() => {
    const root = containerRef.current
    if (!root) return null
    const rootRect = root.getBoundingClientRect()
    let first: HTMLElement | undefined
    const visible = new Set<string>()
    const candidates = segmented
      ? visibleSegments().flatMap((index) => [
          ...(segmentElements.current
            .get(index)
            ?.querySelectorAll<HTMLElement>('[data-message-id]') ?? [])
        ])
      : [...root.querySelectorAll<HTMLElement>('[data-message-id]')]
    for (const element of candidates) {
      const rect = element.getBoundingClientRect()
      if (rect.bottom <= rootRect.top || rect.top >= rootRect.bottom) continue
      first ??= element
      if (element.dataset.messageId) visible.add(element.dataset.messageId)
    }
    setVisibleMessageIds((previous) =>
      previous.size === visible.size && [...visible].every((id) => previous.has(id))
        ? previous
        : visible
    )
    if (first?.dataset.messageId) {
      placeholderAnchor.current = null
      anchor.current = {
        messageId: first.dataset.messageId,
        offset: first.getBoundingClientRect().top - rootRect.top,
        scrollTop: root.scrollTop,
        atBottom: root.scrollHeight - root.scrollTop - root.clientHeight <= 20
      }
    } else {
      // A scrollbar jump can land wholly inside an unvisited section. The old message
      // anchor is no longer the reader's position; retain progress within this section
      // until its real messages mount and provide a precise message anchor.
      anchor.current = null
      const index = visibleSegments()[0]
      const element = index === undefined ? undefined : segmentElements.current.get(index)
      if (element) {
        const rect = element.getBoundingClientRect()
        placeholderAnchor.current = {
          index,
          fraction: Math.max(0, Math.min(1, (rootRect.top - rect.top) / Math.max(1, rect.height))),
          scrollTop: root.scrollTop
        }
      }
    }
    return anchor.current
  }, [containerRef, segmented, visibleSegments])

  const restoreAnchor = useCallback(() => {
    const root = containerRef.current
    const saved = anchor.current
    if (!root || followsBottom() || pending.current) return
    if (!saved) {
      const gap = placeholderAnchor.current
      if (!gap || !current.current.segments[gap.index]?.mounted) return
      if (Math.abs(root.scrollTop - gap.scrollTop) > 0.5) {
        captureAnchor()
        return
      }
      const element = segmentElements.current.get(gap.index)
      if (element) {
        const rect = element.getBoundingClientRect()
        setPosition(
          root.scrollTop + rect.top - root.getBoundingClientRect().top + gap.fraction * rect.height,
          false
        )
        placeholderAnchor.current = null
      }
      return
    }
    // A real scroll may precede its event. Do not undo the reader's new position while
    // ResizeObserver is delivering a simultaneous content resize.
    if (Math.abs(root.scrollTop - saved.scrollTop) > 0.5) {
      captureAnchor()
      return
    }
    const element = findConversationMessage(root, saved.messageId)
    if (!element) return
    const delta =
      element.getBoundingClientRect().top - root.getBoundingClientRect().top - saved.offset
    if (Math.abs(delta) > 0.5) setPosition(root.scrollTop + delta, false)
  }, [captureAnchor, containerRef, followsBottom, setPosition])

  const scan = useCallback(() => {
    scanFrame.current = null
    if (initializedConversation.current !== current.current.conversationId) return
    restoreAnchor()
    captureAnchor()
    const nearby = visibleSegments(PRELOAD_PX)
    mountIndices(nearby.filter((index) => !current.current.segments[index]?.mounted).slice(0, 1))
  }, [captureAnchor, mountIndices, restoreAnchor, visibleSegments])
  const scheduleScan = useCallback(() => {
    if (scanFrame.current === null) scanFrame.current = requestAnimationFrame(scan)
  }, [scan])

  const onReadLayoutChange = useCallback(() => {
    restoreAnchor()
    scheduleScan()
  }, [restoreAnchor, scheduleScan])

  const revealMessage = useCallback(
    (id: string, block: ScrollLogicalPosition = 'start') => {
      const index = current.current.messageIndices.get(id)
      if (index === undefined) return
      const root = containerRef.current
      if (root) setPosition(root.scrollTop, false)
      pending.current = { id, block }
      mountIndices([Math.floor(index / CONVERSATION_SEGMENT_SIZE)])
      setNavigationRevision((value) => value + 1)
    },
    [containerRef, mountIndices, setPosition]
  )

  useLayoutEffect(() => {
    if (initializedConversation.current === conversationId) return
    initializedConversation.current = conversationId
    anchor.current = null
    placeholderAnchor.current = null
    setExpanding(false)
    setState({ conversationId, mounted: initialIndices })
    if (targetMessageId && messageIndices.has(targetMessageId)) {
      pending.current = { id: targetMessageId, block: 'center' }
    } else if (
      typeof initialPosition === 'object' &&
      initialPosition &&
      !initialPosition.atBottom &&
      messageIndices.has(initialPosition.messageId)
    ) {
      pending.current = { id: initialPosition.messageId, offset: initialPosition.offset }
    } else {
      pending.current = null
      const root = containerRef.current
      if (root)
        setPosition(
          typeof initialPosition === 'number' ? initialPosition : root.scrollHeight,
          initialPosition === null ||
            (typeof initialPosition === 'object' && initialPosition.atBottom)
        )
    }
    scheduleScan()
  }, [
    containerRef,
    conversationId,
    initialIndices,
    initialPosition,
    messageIndices,
    scheduleScan,
    setPosition,
    targetMessageId
  ])

  useLayoutEffect(() => {
    const root = containerRef.current
    const request = pending.current
    if (!root) return
    if (request) {
      const element = findConversationMessage(root, request.id)
      if (element) {
        const rect = element.getBoundingClientRect()
        const offset =
          request.offset ??
          (request.block === 'center' ? Math.max(0, (root.clientHeight - rect.height) / 2) : 0)
        setPosition(root.scrollTop + rect.top - root.getBoundingClientRect().top - offset, false)
        pending.current = null
      }
    } else {
      restoreAnchor()
    }
    captureAnchor()
    scheduleScan()
  }, [
    captureAnchor,
    containerRef,
    navigationRevision,
    restoreAnchor,
    scheduleScan,
    setPosition,
    state
  ])

  useLayoutEffect(() => {
    // Anything rendered once remains mounted, including short conversations that grow past
    // the segmentation threshold and historical requests whose pending status later clears.
    mountIndices(segments.filter((segment) => segment.mounted).map((segment) => segment.index))
  })

  useEffect(() => {
    const root = containerRef.current
    if (!root) return
    root.addEventListener('scroll', scheduleScan, { passive: true })
    return () => {
      root.removeEventListener('scroll', scheduleScan)
      if (scanFrame.current !== null) cancelAnimationFrame(scanFrame.current)
      scanFrame.current = null
    }
  }, [containerRef, conversationId, scheduleScan])

  useEffect(() => {
    if (!expanding) return
    if (!remaining) {
      setExpanding(false)
      return
    }
    const frame = requestAnimationFrame(() => {
      mountIndices(
        current.current.segments
          .filter((segment) => !segment.mounted)
          .slice(0, 1)
          .map((segment) => segment.index)
      )
    })
    return () => cancelAnimationFrame(frame)
  }, [expanding, mountIndices, remaining, state])

  const registerSegment = useCallback((index: number, element: HTMLDivElement | null) => {
    if (element) segmentElements.current.set(index, element)
    else segmentElements.current.delete(index)
  }, [])

  return {
    segmented,
    segments,
    visibleMessageIds,
    remaining,
    expanding,
    registerSegment,
    revealMessage,
    captureAnchor,
    onReadLayoutChange,
    expandAll: useCallback(() => setExpanding(true), []),
    cancelExpansion: useCallback(() => setExpanding(false), [])
  }
}

export type ConversationSegmentsController = ReturnType<typeof useConversationSegments>
