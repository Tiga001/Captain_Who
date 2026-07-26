// Renderer chat navigation: shows a read-only rail for completed user-message turns.
import {
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type RefObject
} from 'react'
import { createPortal } from 'react-dom'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import type { ConversationTurnNavigationItem } from '../conversationTurnNavigation'
import './ConversationTurnNavigationRail.css'

const MINIMUM_TURN_COUNT = 4
const WAVE_PROGRESS = [1, 0.7, 0.4, 0.2] as const
const SMOOTH_SCROLL_DISTANCE_IN_VIEWPORTS = 1.5

interface ConversationTurnNavigationRailProps {
  items: ConversationTurnNavigationItem[]
  scrollContainerRef: RefObject<HTMLDivElement | null>
}

interface TooltipPosition {
  left: number
  top: number
  ready: boolean
}

function findUserMessageElement(root: HTMLElement, messageId: string) {
  return Array.from(
    root.querySelectorAll<HTMLElement>('.chat-message--user[data-message-id]')
  ).find((element) => element.dataset.messageId === messageId)
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
  const [tooltipAnchor, setTooltipAnchor] = useState<HTMLButtonElement | null>(null)
  const [tooltipPosition, setTooltipPosition] = useState<TooltipPosition>({
    left: 0,
    top: 0,
    ready: false
  })
  const tooltipRef = useRef<HTMLDivElement>(null)
  const previewTurnId = hoveredTurnId ?? focusedTurnId
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
    setTooltipAnchor(null)
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

  const revealTurn = (item: ConversationTurnNavigationItem) => {
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
      !reduceMotion && distance <= viewportHeight * SMOOTH_SCROLL_DISTANCE_IN_VIEWPORTS

    target.scrollIntoView({
      behavior: shouldScrollSmoothly ? 'smooth' : 'auto',
      block: 'start'
    })
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
      <nav aria-label={t('chat.turnNavigationLabel')} className="conversation-turn-navigation">
        <div className="conversation-turn-navigation__list">
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
                data-turn-id={item.id}
                data-visible={isVisible ? 'true' : undefined}
                key={item.id}
                onBlur={() => {
                  setFocusedTurnId((current) => (current === item.id ? null : current))
                }}
                onClick={() => revealTurn(item)}
                onFocus={(event) => {
                  setFocusedTurnId(item.id)
                  setTooltipAnchor(event.currentTarget)
                }}
                onKeyDown={(event) => {
                  if (event.key !== 'Escape') return
                  event.currentTarget.blur()
                }}
                onPointerEnter={(event) => {
                  setHoveredTurnId(item.id)
                  setTooltipAnchor(event.currentTarget)
                }}
                onPointerLeave={() => {
                  setHoveredTurnId((current) => (current === item.id ? null : current))
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
