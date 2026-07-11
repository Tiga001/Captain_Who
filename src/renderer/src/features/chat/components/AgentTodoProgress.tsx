// Renderer agent todo progress UI.
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  type CSSProperties,
  type FocusEvent,
  type ReactElement,
  useState
} from 'react'
import { createPortal } from 'react-dom'
import { AlertTriangle, CheckCircle2, Circle, LoaderCircle } from 'lucide-react'
import type { AgentTodoItem, AgentTodoState } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../config/translationFormat'
import type { ChatAgentRunView } from '../chatTypes'

interface AgentTodoProgressProps {
  completedAt?: number
  runStatus?: ChatAgentRunView['status']
  todo: AgentTodoState
}

const COMPLETED_TODO_HIDE_DELAY_MS = 1600
const INTERRUPTED_TODO_HIDE_DELAY_MS = 250
const TODO_POPOVER_GAP = 8
const TODO_POPOVER_MARGIN = 16
const TODO_POPOVER_MAX_WIDTH = 520
const TODO_POPOVER_MIN_WIDTH = 220
const TODO_POPOVER_HORIZONTAL_PADDING = 52
const TODO_POPOVER_AVERAGE_CHAR_WIDTH = 14

function getCurrentStepIndex(items: AgentTodoItem[]): number {
  const activeIndex = items.findIndex(
    (item) => item.status === 'in_progress' || item.status === 'blocked'
  )
  if (activeIndex !== -1) return activeIndex + 1

  const firstPendingIndex = items.findIndex((item) => item.status === 'pending')
  if (firstPendingIndex !== -1) return firstPendingIndex + 1

  return items.length
}

function getProgressPercent(items: AgentTodoItem[]): number {
  if (items.length === 0) return 0
  const completedCount = items.filter((item) => item.status === 'completed').length
  return Math.round((completedCount / items.length) * 100)
}

function TodoStatusIcon({ status }: { status: AgentTodoItem['status'] }): ReactElement {
  if (status === 'completed') return <CheckCircle2 aria-hidden="true" />
  if (status === 'in_progress') return <LoaderCircle aria-hidden="true" />
  if (status === 'blocked') return <AlertTriangle aria-hidden="true" />
  return <Circle aria-hidden="true" />
}

export function AgentTodoProgress({
  completedAt,
  runStatus,
  todo
}: AgentTodoProgressProps): ReactElement | null {
  const { t } = useFrontendConfig()
  const [hiddenDeadlineKey, setHiddenDeadlineKey] = useState<string | null>(null)
  const [isPopoverOpen, setPopoverOpen] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)
  const popoverRef = useRef<HTMLDivElement>(null)
  const closeTimerRef = useRef<number | null>(null)
  const items = useMemo(
    () => todo.items.filter((item) => item.title.trim().length > 0),
    [todo.items]
  )
  const longestTitleLength = useMemo(
    () => items.reduce((max, item) => Math.max(max, item.title.length), 0),
    [items]
  )
  const hasItems = items.length > 0
  const currentStep = getCurrentStepIndex(items)
  const activeStatus = items[currentStep - 1]?.status ?? 'pending'
  const allCompleted = hasItems && items.every((item) => item.status === 'completed')
  const hideAt =
    runStatus === 'cancelled' || runStatus === 'failed'
      ? (completedAt ?? todo.updatedAt) + INTERRUPTED_TODO_HIDE_DELAY_MS
      : runStatus === 'completed' && allCompleted
        ? todo.updatedAt + COMPLETED_TODO_HIDE_DELAY_MS
        : null
  const hideDeadlineKey = hideAt === null ? null : `${todo.revision}:${runStatus}:${hideAt}`
  const isHidden = hideDeadlineKey !== null && hiddenDeadlineKey === hideDeadlineKey
  const progressStyle = {
    '--agent-todo-progress': `${getProgressPercent(items)}%`
  } as CSSProperties & Record<'--agent-todo-progress', string>
  const progressLabel = formatTranslation(t, 'agent.todo.progress', {
    current: currentStep,
    total: items.length
  })
  const closePopoverLater = useCallback(() => {
    if (closeTimerRef.current !== null) {
      window.clearTimeout(closeTimerRef.current)
    }
    closeTimerRef.current = window.setTimeout(() => {
      setPopoverOpen(false)
      closeTimerRef.current = null
    }, 80)
  }, [])
  const openPopover = useCallback(() => {
    if (closeTimerRef.current !== null) {
      window.clearTimeout(closeTimerRef.current)
      closeTimerRef.current = null
    }
    setPopoverOpen(true)
  }, [])
  const updatePopoverPosition = useCallback(() => {
    const root = rootRef.current
    const popover = popoverRef.current
    if (!root || !popover) return

    const rect = root.getBoundingClientRect()
    const viewportWidth = window.innerWidth
    const viewportHeight = window.innerHeight
    const preferredWidth = Math.ceil(
      longestTitleLength * TODO_POPOVER_AVERAGE_CHAR_WIDTH + TODO_POPOVER_HORIZONTAL_PADDING
    )
    const width = Math.min(
      TODO_POPOVER_MAX_WIDTH,
      Math.max(
        TODO_POPOVER_MIN_WIDTH,
        Math.min(preferredWidth, viewportWidth - TODO_POPOVER_MARGIN * 2)
      )
    )
    const measuredHeight = popover.offsetHeight
    const rawLeft = rect.left + rect.width / 2 - width / 2
    const left = Math.min(
      Math.max(rawLeft, TODO_POPOVER_MARGIN),
      Math.max(TODO_POPOVER_MARGIN, viewportWidth - width - TODO_POPOVER_MARGIN)
    )
    const rawTop = rect.top - measuredHeight - TODO_POPOVER_GAP
    const top = Math.min(
      Math.max(rawTop, TODO_POPOVER_MARGIN),
      Math.max(TODO_POPOVER_MARGIN, viewportHeight - measuredHeight - TODO_POPOVER_MARGIN)
    )

    popover.style.left = `${left}px`
    popover.style.top = `${top}px`
    popover.style.width = `${width}px`
    popover.style.visibility = 'visible'
  }, [longestTitleLength])
  const handleBlur = useCallback(
    (event: FocusEvent<HTMLDivElement>) => {
      const nextTarget = event.relatedTarget
      if (nextTarget instanceof Node && popoverRef.current?.contains(nextTarget)) return
      closePopoverLater()
    },
    [closePopoverLater]
  )

  useEffect(() => {
    if (hideAt === null || hideDeadlineKey === null) return undefined

    const timerId = window.setTimeout(
      () => {
        setHiddenDeadlineKey(hideDeadlineKey)
        setPopoverOpen(false)
      },
      Math.max(0, hideAt - Date.now())
    )

    return () => window.clearTimeout(timerId)
  }, [hideAt, hideDeadlineKey])

  useEffect(() => {
    return () => {
      if (closeTimerRef.current !== null) {
        window.clearTimeout(closeTimerRef.current)
      }
    }
  }, [])

  useLayoutEffect(() => {
    if (!isPopoverOpen) return undefined

    updatePopoverPosition()
    const animationFrame = window.requestAnimationFrame(updatePopoverPosition)
    const resizeObserver = new ResizeObserver(updatePopoverPosition)
    if (popoverRef.current) resizeObserver.observe(popoverRef.current)
    window.addEventListener('resize', updatePopoverPosition)
    document.addEventListener('scroll', updatePopoverPosition, true)
    return () => {
      window.cancelAnimationFrame(animationFrame)
      resizeObserver.disconnect()
      window.removeEventListener('resize', updatePopoverPosition)
      document.removeEventListener('scroll', updatePopoverPosition, true)
    }
  }, [isPopoverOpen, updatePopoverPosition])

  if (!hasItems) return null
  if (isHidden) return null

  return (
    <div
      className="agent-todo-progress"
      aria-label={progressLabel}
      ref={rootRef}
      style={progressStyle}
      tabIndex={0}
      data-status={activeStatus}
      onBlur={handleBlur}
      onFocus={openPopover}
      onPointerEnter={openPopover}
      onPointerLeave={closePopoverLater}
    >
      {isPopoverOpen &&
        createPortal(
          <div
            className="agent-todo-progress__popover"
            onPointerEnter={openPopover}
            onPointerLeave={closePopoverLater}
            ref={popoverRef}
            role="status"
            style={{ visibility: 'hidden' }}
          >
            <ul className="agent-todo-progress__list" aria-label={t('agent.todo.title')}>
              {items.map((item) => (
                <li className="agent-todo-progress__item" data-status={item.status} key={item.id}>
                  <span className="agent-todo-progress__item-icon" aria-hidden="true">
                    <TodoStatusIcon status={item.status} />
                  </span>
                  <span className="agent-todo-progress__item-copy">
                    <span className="agent-todo-progress__item-title">{item.title}</span>
                    {item.note && (
                      <span className="agent-todo-progress__item-note">{item.note}</span>
                    )}
                  </span>
                </li>
              ))}
            </ul>
          </div>,
          document.body
        )}

      <span className="agent-todo-progress__ring" aria-hidden="true" />
      <span className="agent-todo-progress__label">{progressLabel}</span>
    </div>
  )
}
