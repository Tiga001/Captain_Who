import type { AgentTodoItem, AgentTodoState, AgentToolResult } from '@mycopilot/protocol'

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

      return {
        id: typeof item.id === 'string' && item.id.trim() ? item.id : `todo-${index + 1}`,
        title: item.title,
        status,
        note: typeof item.note === 'string' ? item.note : undefined,
        createdAt: typeof item.createdAt === 'number' ? item.createdAt : 0,
        updatedAt: typeof item.updatedAt === 'number' ? item.updatedAt : 0
      }
    })
    .filter((item): item is AgentTodoItem => Boolean(item && item.title.trim()))
}

function parseTodoState(value: unknown): AgentTodoState | null {
  if (!isRecord(value) || !Array.isArray(value.items)) return null

  return {
    revision: typeof value.revision === 'number' ? value.revision : 0,
    items: parseTodoItems(value.items),
    updatedAt: typeof value.updatedAt === 'number' ? value.updatedAt : 0
  }
}

export function getAcceptedTodoState(result: AgentToolResult | undefined): AgentTodoState | null {
  return result?.ok === true ? parseTodoState(result.result) : null
}

export function getChangedTodoItems(
  previousTodo: AgentTodoState | null,
  items: AgentTodoItem[]
): AgentTodoItem[] {
  if (!previousTodo) return items
  const previousItems = new Map(previousTodo.items.map((item) => [item.id, item]))
  return items.filter((item) => {
    const previousItem = previousItems.get(item.id)
    return (
      !previousItem ||
      previousItem.status !== item.status ||
      previousItem.title !== item.title ||
      (previousItem.note ?? '') !== (item.note ?? '')
    )
  })
}
