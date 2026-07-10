// Renderer UI for agent todo_update tool activity rows in chat history.
import { CheckCircle2, Circle, ClipboardCheck, ClipboardList, LoaderCircle, XCircle } from 'lucide-react'
import type { AgentTodoItem, AgentTodoState, AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../../config/translationFormat'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import type { SettledToolStatus } from './toolActivityUtils'

interface TodoUpdateToolActivityProps {
  cancelled?: boolean
  call: AgentToolCall
  result?: AgentToolResult
  settledStatus?: SettledToolStatus
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value))
}

function parseTodoItems(value: unknown): AgentTodoItem[] {
  if (!Array.isArray(value)) return []

  return value
    .map((item, index): AgentTodoItem | null => {
      if (!isRecord(item) || typeof item.title !== 'string') return null
      const status = item.status
      if (
        status !== 'pending' &&
        status !== 'in_progress' &&
        status !== 'completed' &&
        status !== 'blocked'
      ) {
        return null
      }

      const now = Date.now()
      return {
        id: typeof item.id === 'string' && item.id.trim() ? item.id : `todo-${index + 1}`,
        title: item.title,
        status,
        note: typeof item.note === 'string' ? item.note : undefined,
        createdAt: typeof item.createdAt === 'number' ? item.createdAt : now,
        updatedAt: typeof item.updatedAt === 'number' ? item.updatedAt : now
      }
    })
    .filter((item): item is AgentTodoItem => Boolean(item && item.title.trim()))
}

function parseTodoState(value: unknown): AgentTodoState | null {
  if (!isRecord(value)) return null
  const items = parseTodoItems(value.items)
  if (items.length === 0) return null

  return {
    revision: typeof value.revision === 'number' ? value.revision : 0,
    items,
    updatedAt: typeof value.updatedAt === 'number' ? value.updatedAt : Date.now()
  }
}

function getTodoState(call: AgentToolCall, result: AgentToolResult | undefined) {
  return parseTodoState(result?.result) ?? parseTodoState(call.args)
}

function getCompletedCount(items: AgentTodoItem[]) {
  return items.filter((item) => item.status === 'completed').length
}

function getFocusedItem(items: AgentTodoItem[], completedCount: number) {
  if (completedCount > 0) {
    return items[Math.min(completedCount - 1, items.length - 1)]
  }

  return (
    items.find((item) => item.status === 'in_progress' || item.status === 'blocked') ??
    items.find((item) => item.status === 'pending') ??
    items[0]
  )
}

function TodoItemIcon({ status }: { status: AgentTodoItem['status'] }) {
  if (status === 'completed') return <CheckCircle2 aria-hidden="true" />
  if (status === 'in_progress') return <LoaderCircle aria-hidden="true" />
  if (status === 'blocked') return <XCircle aria-hidden="true" />
  return <Circle aria-hidden="true" />
}

export function TodoUpdateToolActivity({
  cancelled = false,
  call,
  result,
  settledStatus
}: TodoUpdateToolActivityProps) {
  const { t } = useFrontendConfig()
  const todo = getTodoState(call, result)
  const items = todo?.items.filter((item) => item.title.trim()) ?? []
  const total = items.length
  const completedCount = getCompletedCount(items)
  const isCompletedPlan = total > 0 && completedCount >= total
  const isCreatedPlan = total > 0 && completedCount === 0
  const status =
    result?.ok === false
      ? 'failed'
      : result
        ? 'completed'
        : (settledStatus ?? (cancelled ? 'cancelled' : 'running'))
  const Icon = isCompletedPlan ? ClipboardCheck : ClipboardList

  let label = t('agent.todoActivity.running')
  if (status === 'cancelled') {
    label = t('agent.todoActivity.cancelled')
  } else if (status === 'failed') {
    label = t('agent.todoActivity.failed')
  } else if (status === 'completed') {
    if (isCompletedPlan) {
      label = t('agent.todoActivity.completedPlan')
    } else if (isCreatedPlan) {
      label = formatTranslation(t, 'agent.todoActivity.created', {
        total
      })
    } else if (total > 0) {
      label = formatTranslation(t, 'agent.todoActivity.completedStep', {
        completed: completedCount,
        total
      })
    }
  }

  const focusedItem = getFocusedItem(items, completedCount)
  const visibleItems = isCreatedPlan ? items : focusedItem ? [focusedItem] : []
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
              {visibleItems.map((item, index) => (
                <li className="todo-update-activity__item" data-status={item.status} key={item.id}>
                  {isCreatedPlan ? (
                    <span className="todo-update-activity__item-index" aria-hidden="true">
                      {index + 1}.
                    </span>
                  ) : (
                    <span className="todo-update-activity__item-icon" aria-hidden="true">
                      <TodoItemIcon status={item.status} />
                    </span>
                  )}
                  <span className="todo-update-activity__item-title">{item.title}</span>
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
