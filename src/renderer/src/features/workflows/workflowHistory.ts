import type { WorkflowDefinition } from '@mycopilot/protocol'

export interface WorkflowChangeOptions {
  group?: string
  transient?: boolean
}
export interface WorkflowHistory {
  present: WorkflowDefinition | null
  past: WorkflowDefinition[]
  future: WorkflowDefinition[]
  group?: string
  changedAt: number
}
export type WorkflowUpdate =
  WorkflowDefinition | ((current: WorkflowDefinition) => WorkflowDefinition)
export type WorkflowHistoryAction =
  | { type: 'reset'; definition: WorkflowDefinition | null }
  | { type: 'change'; update: WorkflowUpdate; options?: WorkflowChangeOptions; at: number }
  | { type: 'undo' }
  | { type: 'redo' }
  | { type: 'checkpoint' }

/** Moving the viewport is persisted on save, but does not edit the workflow itself. */
export function workflowContentKey(definition: WorkflowDefinition): string {
  return JSON.stringify({ ...definition, viewport: undefined })
}
export function createWorkflowHistory(
  definition: WorkflowDefinition | null = null
): WorkflowHistory {
  return { present: definition, past: [], future: [], changedAt: 0 }
}
export function workflowHistoryReducer(
  state: WorkflowHistory,
  action: WorkflowHistoryAction
): WorkflowHistory {
  if (action.type === 'reset') return createWorkflowHistory(action.definition)
  if (action.type === 'checkpoint') return { ...state, group: undefined, changedAt: 0 }
  if (!state.present) return state
  if (action.type === 'undo' || action.type === 'redo') {
    const source = action.type === 'undo' ? state.past : state.future
    const restored = source.at(-1)
    if (!restored) return state
    const present = { ...restored, viewport: state.present.viewport }
    return action.type === 'undo'
      ? {
          present,
          past: source.slice(0, -1),
          future: [...state.future, state.present],
          changedAt: 0
        }
      : { present, past: [...state.past, state.present], future: source.slice(0, -1), changedAt: 0 }
  }
  const next = typeof action.update === 'function' ? action.update(state.present) : action.update
  if (next === state.present) return state
  if (workflowContentKey(next) === workflowContentKey(state.present)) {
    return { ...state, present: next }
  }
  // A viewport caller must never accidentally bypass history for a content edit.
  const merge = Boolean(
    action.options?.group &&
    action.options.group === state.group &&
    action.at - state.changedAt < 1000
  )
  return {
    present: next,
    past: merge ? state.past : [...state.past, state.present].slice(-50),
    future: [],
    group: action.options?.group,
    changedAt: action.at
  }
}
