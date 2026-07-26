import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  type CSSProperties,
  type ReactElement,
  useState
} from 'react'
import { createPortal } from 'react-dom'
import { AlertTriangle, BadgeCheck, Circle } from 'lucide-react'
import type { AgentTodoItem, AgentTodoState } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import type { ChatAgentRunView } from '../chatTypes'
import {
  formatTodoCompactProgressLabel,
  getTodoCounts,
  getTodoDisplayStatus,
  getTodoProgressPercent,
  getValidTodoItems
} from './todoProgress'

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

function TodoStatusIcon({ status }: { status: AgentTodoItem['status'] }): ReactElement {
  if (status === 'completed') return <BadgeCheck aria-hidden="true" />
  if (status === 'in_progress') return <span className="mc-processing-spinner" />
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
  const popoverId = useId()
  const rootRef = useRef<HTMLButtonElement>(null)
  const popoverRef = useRef<HTMLDivElement>(null)
  const items = useMemo(() => getValidTodoItems(todo.items), [todo.items])
  const counts = useMemo(() => getTodoCounts(items), [items])
  const longestTitleLength = useMemo(
    () => items.reduce((max, item) => Math.max(max, item.title.length), 0),
    [items]
  )
  const hasItems = items.length > 0
  const displayStatus = getTodoDisplayStatus(counts)
  const hideAt =
    runStatus === 'cancelled' || runStatus === 'failed'
      ? (completedAt ?? todo.updatedAt) + INTERRUPTED_TODO_HIDE_DELAY_MS
      : runStatus === 'completed'
        ? (completedAt ?? todo.updatedAt) + COMPLETED_TODO_HIDE_DELAY_MS
        : null
  const hideDeadlineKey = hideAt === null ? null : `${todo.revision}:${runStatus}:${hideAt}`
  const isHidden = hideDeadlineKey !== null && hiddenDeadlineKey === hideDeadlineKey
  const progressStyle = {
    '--agent-todo-progress': `${getTodoProgressPercent(counts)}%`
  } as CSSProperties & Record<'--agent-todo-progress', string>
  const progressLabel = formatTodoCompactProgressLabel(t, counts)
  const updatePopoverPosition = useCallback(() => {
    const root = rootRef.current
    const popover = popoverRef.current
    if (!root || !popover) return

    const rect = root.getBoundingClientRect()
    const viewportWidth = window.innerWidth
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
    const availableHeight = Math.max(0, rect.top - TODO_POPOVER_GAP - TODO_POPOVER_MARGIN)
    const rawLeft = rect.left + rect.width / 2 - width / 2
    const left = Math.min(
      Math.max(rawLeft, TODO_POPOVER_MARGIN),
      Math.max(TODO_POPOVER_MARGIN, viewportWidth - width - TODO_POPOVER_MARGIN)
    )

    popover.style.left = `${left}px`
    popover.style.width = `${width}px`
    popover.style.maxHeight = `${availableHeight}px`
    const top = Math.max(TODO_POPOVER_MARGIN, rect.top - popover.offsetHeight - TODO_POPOVER_GAP)
    popover.style.top = `${top}px`
    popover.style.visibility = 'visible'
  }, [longestTitleLength])

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
    if (!isPopoverOpen) return undefined

    const isWithinDisclosure = (target: EventTarget | null): boolean =>
      target instanceof Node &&
      (Boolean(rootRef.current?.contains(target)) || Boolean(popoverRef.current?.contains(target)))
    const handlePointerDown = (event: PointerEvent): void => {
      if (!isWithinDisclosure(event.target)) setPopoverOpen(false)
    }
    const handleFocusIn = (event: FocusEvent): void => {
      if (!isWithinDisclosure(event.target)) setPopoverOpen(false)
    }
    const handleKeyDown = (event: KeyboardEvent): void => {
      if (event.key !== 'Escape') return
      event.preventDefault()
      setPopoverOpen(false)
      rootRef.current?.focus()
    }

    document.addEventListener('pointerdown', handlePointerDown, true)
    document.addEventListener('focusin', handleFocusIn)
    document.addEventListener('keydown', handleKeyDown)
    return () => {
      document.removeEventListener('pointerdown', handlePointerDown, true)
      document.removeEventListener('focusin', handleFocusIn)
      document.removeEventListener('keydown', handleKeyDown)
    }
  }, [isPopoverOpen])

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
    <>
      <button
        aria-controls={isPopoverOpen ? popoverId : undefined}
        aria-expanded={isPopoverOpen}
        aria-label={progressLabel}
        className="agent-todo-progress"
        data-status={displayStatus}
        onClick={() => setPopoverOpen((current) => !current)}
        ref={rootRef}
        style={progressStyle}
        type="button"
      >
        <span className="agent-todo-progress__ring" aria-hidden="true" />
        <span className="agent-todo-progress__label">{progressLabel}</span>
      </button>

      {isPopoverOpen &&
        createPortal(
          <div
            aria-label={t('agent.todo.title')}
            className="agent-todo-progress__popover"
            id={popoverId}
            ref={popoverRef}
            role="region"
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
    </>
  )
}
