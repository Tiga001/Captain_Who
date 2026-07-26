// Renderer chat navigation: shows a read-only rail for completed user-message turns.
import {
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type PointerEvent as ReactPointerEvent,
  type RefObject
} from 'react'
import { createPortal } from 'react-dom'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import type { ConversationTurnNavigationItem } from '../conversationTurnNavigation'
import './ConversationTurnNavigationRail.css'

const MINIMUM_TURN_COUNT = 4
const WAVE_PROGRESS = [1, 0.7, 0.4, 0.2] as const
const SMOOTH_SCROLL_DISTANCE_IN_VIEWPORTS = 1.5
const SCRUB_ACTIVATION_DISTANCE_PX = 4

interface ConversationTurnNavigationRailProps {
  items: ConversationTurnNavigationItem[]
  scrollContainerRef: RefObject<HTMLDivElement | null>
}

interface TooltipPosition {
  left: number
  top: number
  ready: boolean
}

interface PointerScrubState {
  hasMoved: boolean
  initialTurnId: string
  pointerId: number
  startY: number
  stops: PointerScrubStop[]
}

interface PointerScrubStop {
  clientY: number
  scrollTop: number
}

interface NearestTurnTarget {
  anchor: HTMLButtonElement
  turnId: string
}

function findUserMessageElement(root: HTMLElement, messageId: string) {
  return Array.from(
    root.querySelectorAll<HTMLElement>('.chat-message--user[data-message-id]')
  ).find((element) => element.dataset.messageId === messageId)
}

function findNearestTurnTarget(list: HTMLDivElement, clientY: number): NearestTurnTarget | null {
  let nearestTarget: NearestTurnTarget | null = null
  let nearestDistance = Number.POSITIVE_INFINITY

  for (const anchor of list.querySelectorAll<HTMLButtonElement>(
    '.conversation-turn-navigation__row[data-turn-id]'
  )) {
    const turnId = anchor.dataset.turnId
    if (!turnId) continue

    const rect = anchor.getBoundingClientRect()
    const distance = Math.abs(clientY - (rect.top + rect.height / 2))
    if (distance >= nearestDistance) continue

    nearestDistance = distance
    nearestTarget = { anchor, turnId }
  }

  return nearestTarget
}

function createPointerScrubStops(
  list: HTMLDivElement,
  root: HTMLDivElement,
  items: ConversationTurnNavigationItem[]
) {
  const itemById = new Map(items.map((item) => [item.id, item]))
  const rootRect = root.getBoundingClientRect()
  const maximumScrollTop = Math.max(0, root.scrollHeight - root.clientHeight)
  const stops: PointerScrubStop[] = []

  for (const anchor of list.querySelectorAll<HTMLButtonElement>(
    '.conversation-turn-navigation__row[data-turn-id]'
  )) {
    const turnId = anchor.dataset.turnId
    const item = turnId ? itemById.get(turnId) : undefined
    if (!item) continue

    const target = findUserMessageElement(root, item.userMessageId)
    if (!target) continue

    const anchorRect = anchor.getBoundingClientRect()
    const targetRect = target.getBoundingClientRect()
    stops.push({
      clientY: anchorRect.top + anchorRect.height / 2,
      scrollTop: Math.min(
        maximumScrollTop,
        Math.max(0, root.scrollTop + targetRect.top - rootRect.top)
      )
    })
  }

  return stops.sort((left, right) => left.clientY - right.clientY)
}

function scrollConversationToPointer(
  root: HTMLDivElement,
  stops: PointerScrubStop[],
  clientY: number
) {
  const first = stops[0]
  const last = stops.at(-1)
  if (!first || !last) return

  if (clientY <= first.clientY) {
    root.scrollTo({ behavior: 'auto', top: first.scrollTop })
    return
  }
  if (clientY >= last.clientY) {
    root.scrollTo({ behavior: 'auto', top: last.scrollTop })
    return
  }

  for (let index = 1; index < stops.length; index += 1) {
    const next = stops[index]
    const previous = stops[index - 1]
    if (!next || !previous || clientY > next.clientY) continue

    const distance = next.clientY - previous.clientY
    const progress = distance <= 0 ? 0 : (clientY - previous.clientY) / distance
    root.scrollTo({
      behavior: 'auto',
      top: previous.scrollTop + (next.scrollTop - previous.scrollTop) * progress
    })
    return
  }
}

function sameStringSet(left: Set<string>, right: Set<string>) {
  if (left.size !== right.size) return false
  for (const value of left) {
    if (!right.has(value)) return false
  }
  return true
}

export function ConversationTurnNavigationRail({
  items,
  scrollContainerRef
}: ConversationTurnNavigationRailProps) {
  const { t } = useFrontendConfig()
  const tooltipId = useId()
  const itemIdsKey = useMemo(() => items.map((item) => item.id).join('\0'), [items])
  const itemIds = useMemo(() => new Set(itemIdsKey ? itemIdsKey.split('\0') : []), [itemIdsKey])
  const [visibleTurnIds, setVisibleTurnIds] = useState<Set<string>>(() => new Set())
  const [hoveredTurnId, setHoveredTurnId] = useState<string | null>(null)
  const [focusedTurnId, setFocusedTurnId] = useState<string | null>(null)
  const [scrubbedTurnId, setScrubbedTurnId] = useState<string | null>(null)
  const [pointerPreviewSuppressed, setPointerPreviewSuppressed] = useState(false)
  const [tooltipAnchor, setTooltipAnchor] = useState<HTMLButtonElement | null>(null)
  const [tooltipPosition, setTooltipPosition] = useState<TooltipPosition>({
    left: 0,
    top: 0,
    ready: false
  })
  const tooltipRef = useRef<HTMLDivElement>(null)
  const pointerScrubRef = useRef<PointerScrubState | null>(null)
  const pointerPreviewSuppressedRef = useRef(false)
  const suppressClickRef = useRef(false)
  const previewTurnId = scrubbedTurnId ?? hoveredTurnId ?? focusedTurnId
  const previewItem = items.find((item) => item.id === previewTurnId) ?? null
  const waveTurnId = previewTurnId
  const waveIndex = items.findIndex((item) => item.id === waveTurnId)

  useEffect(() => {
    if (items.length < MINIMUM_TURN_COUNT) {
      setVisibleTurnIds((current) => (current.size === 0 ? current : new Set()))
      return
    }

    const root = scrollContainerRef.current
    if (!root || typeof IntersectionObserver === 'undefined') return

    const observedElements = new Map<Element, string>()
    const visibleElements = new Set<Element>()
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (entry.isIntersecting && entry.intersectionRatio > 0) {
            visibleElements.add(entry.target)
          } else {
            visibleElements.delete(entry.target)
          }
        }

        const nextVisibleIds = new Set<string>()
        for (const element of visibleElements) {
          const turnId = observedElements.get(element)
          if (turnId) nextVisibleIds.add(turnId)
        }

        setVisibleTurnIds((current) =>
          sameStringSet(current, nextVisibleIds) ? current : new Set(nextVisibleIds)
        )
      },
      {
        root,
        threshold: 0
      }
    )

    let activeTurnId: string | null = null
    for (const element of root.querySelectorAll<HTMLElement>('.chat-message[data-message-id]')) {
      const messageId = element.dataset.messageId
      if (!messageId) continue

      if (element.classList.contains('chat-message--user')) {
        activeTurnId = itemIds.has(messageId) ? messageId : null
      }
      if (!activeTurnId) continue

      observedElements.set(element, activeTurnId)
      observer.observe(element)
    }

    return () => observer.disconnect()
  }, [itemIds, itemIdsKey, items.length, scrollContainerRef])

  useEffect(() => {
    if (previewTurnId && itemIds.has(previewTurnId)) return
    setHoveredTurnId(null)
    setFocusedTurnId(null)
    setScrubbedTurnId(null)
    setTooltipAnchor(null)
    pointerScrubRef.current = null
  }, [itemIds, previewTurnId])

  useLayoutEffect(() => {
    if (!previewItem || !tooltipAnchor) {
      setTooltipPosition((current) => (current.ready ? { ...current, ready: false } : current))
      return
    }

    const updatePosition = () => {
      const anchorRect = tooltipAnchor.getBoundingClientRect()
      const tooltipRect = tooltipRef.current?.getBoundingClientRect()
      const tooltipWidth = tooltipRect?.width ?? Math.min(320, window.innerWidth - 16)
      const tooltipHeight = tooltipRect?.height ?? 120
      const viewportPadding = 8
      const gap = 10

      let left = anchorRect.right + gap
      if (left + tooltipWidth > window.innerWidth - viewportPadding) {
        left = Math.max(viewportPadding, anchorRect.left - gap - tooltipWidth)
      }

      const top = Math.min(
        Math.max(viewportPadding, anchorRect.top + anchorRect.height / 2 - tooltipHeight / 2),
        Math.max(viewportPadding, window.innerHeight - tooltipHeight - viewportPadding)
      )

      setTooltipPosition({ left, top, ready: true })
    }

    updatePosition()
    const animationFrameId = window.requestAnimationFrame(updatePosition)
    const scrollElement = scrollContainerRef.current
    window.addEventListener('resize', updatePosition)
    scrollElement?.addEventListener('scroll', updatePosition)

    return () => {
      window.cancelAnimationFrame(animationFrameId)
      window.removeEventListener('resize', updatePosition)
      scrollElement?.removeEventListener('scroll', updatePosition)
    }
  }, [previewItem, scrollContainerRef, tooltipAnchor])

  if (items.length < MINIMUM_TURN_COUNT) return null

  const revealTurn = (
    item: ConversationTurnNavigationItem,
    interaction: 'click' | 'scrub' = 'click'
  ) => {
    const root = scrollContainerRef.current
    if (!root) return
    const target = findUserMessageElement(root, item.userMessageId)
    if (!target) return

    const reduceMotion = window.matchMedia?.('(prefers-reduced-motion: reduce)').matches
    const rootRect = root.getBoundingClientRect()
    const targetRect = target.getBoundingClientRect()
    const viewportHeight = root.clientHeight || window.innerHeight
    const distance = Math.abs(targetRect.top - rootRect.top)
    const shouldScrollSmoothly =
      interaction === 'click' &&
      !reduceMotion &&
      distance <= viewportHeight * SMOOTH_SCROLL_DISTANCE_IN_VIEWPORTS

    target.scrollIntoView({
      behavior: shouldScrollSmoothly ? 'smooth' : 'auto',
      block: 'start'
    })
  }

  const handlePointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || !event.isPrimary) return

    const nearestTarget = findNearestTurnTarget(event.currentTarget, event.clientY)
    if (!nearestTarget) return

    pointerPreviewSuppressedRef.current = false
    setPointerPreviewSuppressed(false)
    pointerScrubRef.current = {
      hasMoved: false,
      initialTurnId: nearestTarget.turnId,
      pointerId: event.pointerId,
      startY: event.clientY,
      stops: []
    }
    setHoveredTurnId(nearestTarget.turnId)
    setTooltipAnchor(nearestTarget.anchor)
  }

  const handlePointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const scrubState = pointerScrubRef.current
    if (!scrubState || scrubState.pointerId !== event.pointerId) return

    if (
      !scrubState.hasMoved &&
      Math.abs(event.clientY - scrubState.startY) < SCRUB_ACTIVATION_DISTANCE_PX
    ) {
      return
    }

    const isFirstScrubMove = !scrubState.hasMoved
    if (isFirstScrubMove) {
      scrubState.hasMoved = true
      const root = scrollContainerRef.current
      if (root) {
        scrubState.stops = createPointerScrubStops(event.currentTarget, root, items)
      }
      try {
        event.currentTarget.setPointerCapture(event.pointerId)
      } catch {
        // Pointer capture can fail for synthetic events; in-window dragging still works.
      }
    }
    suppressClickRef.current = true

    const nearestTarget = findNearestTurnTarget(event.currentTarget, event.clientY)
    if (!nearestTarget) return

    setScrubbedTurnId(nearestTarget.turnId)
    setTooltipAnchor(nearestTarget.anchor)

    if (isFirstScrubMove) {
      const initialItem = items.find((candidate) => candidate.id === scrubState.initialTurnId)
      if (initialItem) revealTurn(initialItem, 'scrub')
    } else {
      const root = scrollContainerRef.current
      if (root) scrollConversationToPointer(root, scrubState.stops, event.clientY)
    }

    event.preventDefault()
  }

  const finishPointerScrub = (event: ReactPointerEvent<HTMLDivElement>) => {
    const scrubState = pointerScrubRef.current
    if (!scrubState || scrubState.pointerId !== event.pointerId) return

    pointerScrubRef.current = null
    pointerPreviewSuppressedRef.current = true
    setPointerPreviewSuppressed(true)
    setHoveredTurnId(null)
    setFocusedTurnId(null)
    setScrubbedTurnId(null)
    setTooltipAnchor(null)

    const activeElement = document.activeElement
    if (activeElement instanceof HTMLElement && event.currentTarget.contains(activeElement)) {
      activeElement.blur()
    }

    if (scrubState.hasMoved) {
      suppressClickRef.current = true
      window.setTimeout(() => {
        suppressClickRef.current = false
      }, 0)
    }

    try {
      if (event.currentTarget.hasPointerCapture(event.pointerId)) {
        event.currentTarget.releasePointerCapture(event.pointerId)
      }
    } catch {
      // Pointer capture may already have been released by the browser.
    }
  }

  const tooltip =
    previewItem && typeof document !== 'undefined'
      ? createPortal(
          <div
            className="conversation-turn-navigation__tooltip"
            data-ready={tooltipPosition.ready ? 'true' : undefined}
            id={tooltipId}
            ref={tooltipRef}
            role="tooltip"
            style={{
              left: tooltipPosition.left,
              top: tooltipPosition.top
            }}
          >
            <strong>
              {previewItem.userPreview || t('chat.turnNavigationAttachmentOnlyMessage')}
            </strong>
            {previewItem.assistantPreview && <p>{previewItem.assistantPreview}</p>}
          </div>,
          document.body
        )
      : null

  return (
    <>
      <nav
        aria-label={t('chat.turnNavigationLabel')}
        className="conversation-turn-navigation"
        data-pointer-preview-suppressed={pointerPreviewSuppressed ? 'true' : undefined}
        data-scrubbing={scrubbedTurnId ? 'true' : undefined}
      >
        <div
          className="conversation-turn-navigation__list"
          onLostPointerCapture={finishPointerScrub}
          onPointerCancel={finishPointerScrub}
          onPointerDown={handlePointerDown}
          onPointerMove={handlePointerMove}
          onPointerUp={finishPointerScrub}
        >
          {items.map((item, index) => {
            const distance = waveIndex < 0 ? -1 : Math.abs(index - waveIndex)
            const waveProgress =
              distance >= 0 && distance < WAVE_PROGRESS.length ? WAVE_PROGRESS[distance] : 0
            const isVisible = visibleTurnIds.has(item.id)
            const isPreviewed = previewTurnId === item.id

            return (
              <button
                aria-current={isVisible ? 'location' : undefined}
                aria-describedby={isPreviewed ? tooltipId : undefined}
                aria-label={`${t('chat.turnNavigationJumpToTurn')} ${index + 1}`}
                className="conversation-turn-navigation__row"
                data-favorited={item.favorited ? 'true' : undefined}
                data-turn-id={item.id}
                data-visible={isVisible ? 'true' : undefined}
                key={item.id}
                onBlur={() => {
                  setFocusedTurnId((current) => (current === item.id ? null : current))
                }}
                onClick={(event) => {
                  if (suppressClickRef.current) {
                    suppressClickRef.current = false
                    event.preventDefault()
                    event.stopPropagation()
                    return
                  }
                  revealTurn(item)
                }}
                onFocus={(event) => {
                  setFocusedTurnId(item.id)
                  setTooltipAnchor(event.currentTarget)
                }}
                onKeyDown={(event) => {
                  if (event.key !== 'Escape') return
                  event.currentTarget.blur()
                }}
                onPointerEnter={(event) => {
                  if (pointerPreviewSuppressedRef.current) return
                  setHoveredTurnId(item.id)
                  setTooltipAnchor(event.currentTarget)
                }}
                onPointerLeave={() => {
                  setHoveredTurnId((current) => (current === item.id ? null : current))
                  if (!pointerScrubRef.current) {
                    pointerPreviewSuppressedRef.current = false
                    setPointerPreviewSuppressed(false)
                  }
                }}
                style={
                  {
                    '--conversation-turn-wave-progress': waveProgress
                  } as CSSProperties
                }
                type="button"
              >
                <span aria-hidden="true" />
              </button>
            )
          })}
        </div>
      </nav>
      {tooltip}
    </>
  )
}
