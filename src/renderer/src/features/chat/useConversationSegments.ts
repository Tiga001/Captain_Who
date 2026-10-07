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
  const firstMessageId = messages[0]?.id
  const messageIndices = useMemo(
    () => new Map(messages.map((message, index) => [message.id, index])),
    [messages]
  )
  const [committedBoundaries, setCommittedBoundaries] = useState<{
    conversationId: string
    keys: string[]
  } | null>(null)
  const definitions = useMemo(() => {
    const result: Omit<ConversationMessageSegment, 'mounted'>[] = []
    const previous = committedBoundaries
    const anchors =
      previous?.conversationId === conversationId
        ? previous.keys.flatMap((key) => {
            const index = messageIndices.get(key)
            return index === undefined ? [] : [index]
          })
        : []
    // Preserve already-mounted message groups when older pages are prepended. Paging and segment
    // sizes need not align; repartitioning from index zero would remount live cards and lose the
    // reader's anchor as soon as the list crosses the lazy-mount threshold.
    const ordered = anchors.every((index, order) => order === 0 || index > anchors[order - 1])
    const stops = [...(ordered ? anchors : []), messages.length]
    let start = 0
    for (const stop of stops) {
      while (start < stop) {
        const end = Math.min(stop, start + CONVERSATION_SEGMENT_SIZE)
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
        start = end
      }
    }
    return result
  }, [committedBoundaries, conversationId, messageIndices, messages])
  useLayoutEffect(() => {
    setCommittedBoundaries((previous) =>
      previous?.conversationId === conversationId &&
      previous.keys.length === definitions.length &&
      definitions.every((definition, index) => definition.key === previous.keys[index])
        ? previous
        : { conversationId, keys: definitions.map((definition) => definition.key) }
    )
  }, [conversationId, definitions])
  const messageSegmentIndices = useMemo(() => {
    const indices = new Map<string, number>()
    for (const segment of definitions) {
      for (let index = segment.start; index < segment.end; index++)
        indices.set(messages[index].id, segment.index)
    }
    return indices
  }, [definitions, messages])
  const initialIndices = useMemo(() => {
    if (!segmented) return new Set(definitions.map((definition) => definition.index))
    const last = definitions.length - 1
    const id =
      targetMessageId ?? (typeof initialPosition === 'object' ? initialPosition?.messageId : null)
    const targetIndex = id ? messageSegmentIndices.get(id) : undefined
    const center =
      targetIndex === undefined
        ? initialPosition !== null &&
          !(typeof initialPosition === 'object' && initialPosition.atBottom)
          ? 0
          : last
        : targetIndex
    return new Set(
      [center, center > 0 ? center - 1 : center + 1].filter((index) => index >= 0 && index <= last)
    )
  }, [definitions, initialPosition, messageSegmentIndices, segmented, targetMessageId])
  const initialKeys = useMemo(
    () => new Set([...initialIndices].map((index) => definitions[index].key)),
    [definitions, initialIndices]
  )
  const [state, setState] = useState(() => ({ conversationId, mounted: initialKeys }))
  const [expanding, setExpanding] = useState(false)
  const [visibleMessageIds, setVisibleMessageIds] = useState<ReadonlySet<string>>(() => new Set())
  const mountedKeys = state.conversationId === conversationId ? state.mounted : initialKeys
  const mounted = new Set(
    definitions
      .filter((definition) => mountedKeys.has(definition.key))
      .map((definition) => definition.index)
  )
  const pinnedIndices = new Set<number>()
  for (const id of pinnedMessageIds) {
    const index = messageSegmentIndices.get(id)
    if (index !== undefined) pinnedIndices.add(index)
  }
  const segments = definitions.map((definition) => ({
    ...definition,
    mounted: !segmented || mounted.has(definition.index) || pinnedIndices.has(definition.index)
  }))
  const remaining = segments.filter((segment) => !segment.mounted).length
  const segmentElements = useRef(new Map<number, HTMLElement>())
  const current = useRef({ segments, mounted, mountedKeys, conversationId, messageSegmentIndices })
  useLayoutEffect(() => {
    current.current = { segments, mounted, mountedKeys, conversationId, messageSegmentIndices }
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
        previous.conversationId === snapshot.conversationId
          ? previous.mounted
          : snapshot.mountedKeys
      const next = new Set(base)
      for (const index of indices)
        if (snapshot.segments[index]) next.add(snapshot.segments[index].key)
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
      const index = current.current.messageSegmentIndices.get(id)
      if (index === undefined) return
      const root = containerRef.current
      if (root) setPosition(root.scrollTop, false)
      pending.current = { id, block }
      mountIndices([index])
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
    setState({ conversationId, mounted: initialKeys })
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
    initialKeys,
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
    state,
    firstMessageId
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
