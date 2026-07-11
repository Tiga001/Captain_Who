import type { AgentTodoItem, AgentTodoStatus } from '@mycopilot/protocol'
import type { Translate } from '../../../config/translationFormat'
import { formatTranslation } from '../../../config/translationFormat'

export interface TodoCounts {
  total: number
  completed: number
  inProgress: number
  blocked: number
  pending: number
}

function isTodoStatus(status: unknown): status is AgentTodoStatus {
  return (
    status === 'pending' ||
    status === 'in_progress' ||
    status === 'completed' ||
    status === 'blocked'
  )
}

export function getValidTodoItems(items: readonly AgentTodoItem[]): AgentTodoItem[] {
  return items.filter(
    (item) =>
      typeof item.title === 'string' && item.title.trim().length > 0 && isTodoStatus(item.status)
  )
}

export function getTodoCounts(items: readonly AgentTodoItem[]): TodoCounts {
  const validItems = getValidTodoItems(items)
  let completed = 0
  let inProgress = 0
  let blocked = 0

  for (const item of validItems) {
    if (item.status === 'completed') completed += 1
    if (item.status === 'in_progress') inProgress += 1
    if (item.status === 'blocked') blocked += 1
  }

  const total = validItems.length
  return {
    total,
    completed,
    inProgress,
    blocked,
    pending: total - completed - inProgress - blocked
  }
}

export function getTodoProgressPercent(counts: TodoCounts): number {
  if (counts.total === 0) return 0
  return Math.round((counts.completed / counts.total) * 100)
}

export function getTodoDisplayStatus(counts: TodoCounts): AgentTodoStatus {
  if (counts.blocked > 0) return 'blocked'
  if (counts.inProgress > 0) return 'in_progress'
  if (counts.total > 0 && counts.completed === counts.total) return 'completed'
  return 'pending'
}

export function formatTodoProgressLabel(t: Translate, counts: TodoCounts): string {
  const parts = [
    formatTranslation(t, 'agent.todo.progress', {
      completed: counts.completed,
      total: counts.total
    })
  ]

  if (counts.inProgress > 0) {
    parts.push(
      formatTranslation(t, 'agent.todo.progressInProgress', {
        inProgress: counts.inProgress
      })
    )
  }
  if (counts.blocked > 0) {
    parts.push(
      formatTranslation(t, 'agent.todo.progressBlocked', {
        blocked: counts.blocked
      })
    )
  }

  return parts.join(t('agent.separator'))
}

export function formatTodoCompactProgressLabel(t: Translate, counts: TodoCounts): string {
  return formatTranslation(t, 'agent.todo.compactProgress', {
    completed: counts.completed,
    total: counts.total
  })
}
