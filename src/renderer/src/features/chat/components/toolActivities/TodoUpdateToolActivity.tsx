import {
  BadgeCheck,
  Circle,
  ClipboardList,
  Clock3,
  ListChecks,
  ListTodo,
  XCircle
} from 'lucide-react'
import type { AgentTodoItem, AgentToolResult } from '@mycopilot/protocol'
import type { ReactElement } from 'react'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../../config/translationFormat'
import { formatTodoProgressLabel, getTodoCounts, getValidTodoItems } from '../todoProgress'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import { getAcceptedTodoState, getChangedTodoItems } from './todoUpdateState'
import type { SettledToolStatus } from './toolActivityUtils'

interface TodoUpdateToolActivityProps {
  cancelled?: boolean
  previousResult?: AgentToolResult
  result?: AgentToolResult
  settledStatus?: SettledToolStatus
}

function TodoItemIcon({ status }: { status: AgentTodoItem['status'] }): ReactElement {
  if (status === 'completed') return <BadgeCheck aria-hidden="true" />
  if (status === 'in_progress') return <Clock3 aria-hidden="true" />
  if (status === 'blocked') return <XCircle aria-hidden="true" />
  return <Circle aria-hidden="true" />
}

export function TodoUpdateToolActivity({
  cancelled = false,
  previousResult,
  result,
  settledStatus
}: TodoUpdateToolActivityProps): ReactElement {
  const { t } = useFrontendConfig()
  const todo = getAcceptedTodoState(result)
  const previousTodo = getAcceptedTodoState(previousResult)
  const items = todo ? getValidTodoItems(todo.items) : []
  const counts = getTodoCounts(items)
  const status =
    result?.ok === false
      ? 'failed'
      : result
        ? 'completed'
        : (settledStatus ?? (cancelled ? 'cancelled' : 'running'))
  const isCreatedPlan = status === 'completed' && Boolean(todo) && !previousTodo
  const isCompletedPlan = counts.total > 0 && counts.completed === counts.total
  const Icon =
    status === 'completed' && !isCreatedPlan
      ? isCompletedPlan
        ? ListChecks
        : ListTodo
      : ClipboardList

  let label = t('agent.todoActivity.running')
  if (status === 'cancelled') {
    label = t('agent.todoActivity.cancelled')
  } else if (status === 'failed') {
    label = t('agent.todoActivity.failed')
  } else if (status === 'completed' && todo) {
    if (isCreatedPlan) {
      label = formatTranslation(t, 'agent.todoActivity.created', {
        total: counts.total
      })
    } else {
      label = formatTodoProgressLabel(t, counts)
    }
  }

  const visibleItems =
    status === 'completed' && todo ? getChangedTodoItems(previousTodo, items) : []
  const hasDetails = visibleItems.length > 0 || Boolean(result?.error)

  return (
    <AgentActivityDisclosure
      className="agent-activity--todo-update"
      hasDetails={hasDetails}
      icon={Icon}
      isPending={status === 'running'}
      label={label}
    >
      {hasDetails && (
        <div className="agent-activity__details todo-update-activity__details">
          {visibleItems.length > 0 && (
            <ul className="todo-update-activity__list" aria-label={t('agent.todo.title')}>
              {visibleItems.map((item) => (
                <li className="todo-update-activity__item" data-status={item.status} key={item.id}>
                  <span className="todo-update-activity__item-icon" aria-hidden="true">
                    <TodoItemIcon status={item.status} />
                  </span>
                  <span className="todo-update-activity__item-copy">
                    <span className="todo-update-activity__item-title">{item.title}</span>
                    {item.note && (
                      <span className="todo-update-activity__item-note">{item.note}</span>
                    )}
                  </span>
                </li>
              ))}
            </ul>
          )}
          {result?.error && <pre>{result.error}</pre>}
        </div>
      )}
    </AgentActivityDisclosure>
  )
}
