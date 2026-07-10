// Renderer agent todo progress UI.
import { useEffect, type CSSProperties, useState } from 'react'
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
const CANCELLED_TODO_HIDE_DELAY_MS = 250

function getCurrentStepIndex(items: AgentTodoItem[]) {
  const activeIndex = items.findIndex(
    (item) => item.status === 'in_progress' || item.status === 'blocked'
  )
  if (activeIndex !== -1) return activeIndex + 1

  const firstPendingIndex = items.findIndex((item) => item.status === 'pending')
  if (firstPendingIndex !== -1) return firstPendingIndex + 1

  return items.length
}

function getProgressPercent(items: AgentTodoItem[]) {
  if (items.length === 0) return 0
  const completedCount = items.filter((item) => item.status === 'completed').length
  return Math.round((completedCount / items.length) * 100)
}

function TodoStatusIcon({ status }: { status: AgentTodoItem['status'] }) {
  if (status === 'completed') return <CheckCircle2 aria-hidden="true" />
  if (status === 'in_progress') return <LoaderCircle aria-hidden="true" />
  if (status === 'blocked') return <AlertTriangle aria-hidden="true" />
  return <Circle aria-hidden="true" />
}

export function AgentTodoProgress({ completedAt, runStatus, todo }: AgentTodoProgressProps) {
  const { t } = useFrontendConfig()
  const [visibilityTick, setVisibilityTick] = useState(Date.now())
  const items = todo.items.filter((item) => item.title.trim().length > 0)
  const hasItems = items.length > 0
  const currentStep = getCurrentStepIndex(items)
  const activeStatus = items[currentStep - 1]?.status ?? 'pending'
  const allCompleted = hasItems && items.every((item) => item.status === 'completed')
  const hideAt =
    runStatus === 'cancelled'
      ? (completedAt ?? todo.updatedAt) + CANCELLED_TODO_HIDE_DELAY_MS
      : runStatus === 'completed' && allCompleted
        ? todo.updatedAt + COMPLETED_TODO_HIDE_DELAY_MS
        : null
  const progressStyle = {
    '--agent-todo-progress': `${getProgressPercent(items)}%`
  } as CSSProperties & Record<'--agent-todo-progress', string>
  const progressLabel = formatTranslation(t, 'agent.todo.progress', {
    current: currentStep,
    total: items.length
  })

  useEffect(() => {
    setVisibilityTick(Date.now())
    if (!hideAt) return undefined

    const timerId = window.setTimeout(
      () => setVisibilityTick(Date.now()),
      Math.max(0, hideAt - Date.now())
    )

    return () => window.clearTimeout(timerId)
  }, [hideAt])

  if (!hasItems) return null
  if (hideAt && visibilityTick >= hideAt) return null

  return (
    <div
      className="agent-todo-progress"
      aria-label={progressLabel}
      style={progressStyle}
      tabIndex={0}
      data-status={activeStatus}
    >
      <div className="agent-todo-progress__popover" role="status">
        <ul className="agent-todo-progress__list" aria-label={t('agent.todo.title')}>
          {items.map((item) => (
            <li className="agent-todo-progress__item" data-status={item.status} key={item.id}>
              <span className="agent-todo-progress__item-icon" aria-hidden="true">
                <TodoStatusIcon status={item.status} />
              </span>
              <span className="agent-todo-progress__item-copy">
                <span className="agent-todo-progress__item-title">{item.title}</span>
                {item.note && <span className="agent-todo-progress__item-note">{item.note}</span>}
              </span>
            </li>
          ))}
        </ul>
      </div>

      <span className="agent-todo-progress__ring" aria-hidden="true" />
      <span className="agent-todo-progress__label">{progressLabel}</span>
    </div>
  )
}
