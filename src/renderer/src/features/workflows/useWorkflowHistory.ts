import { useCallback, useReducer } from 'react'
import type { WorkflowDefinition } from '@mycopilot/protocol'
import {
  createWorkflowHistory,
  workflowHistoryReducer,
  type WorkflowChangeOptions,
  type WorkflowUpdate
} from './workflowHistory'

export function useWorkflowHistory() {
  const [history, dispatch] = useReducer(workflowHistoryReducer, null, () =>
    createWorkflowHistory()
  )
  const reset = useCallback(
    (definition: WorkflowDefinition | null) => dispatch({ type: 'reset', definition }),
    []
  )
  const change = useCallback(
    (update: WorkflowUpdate, options?: WorkflowChangeOptions) =>
      dispatch({ type: 'change', update, options, at: Date.now() }),
    []
  )
  const undo = useCallback(() => dispatch({ type: 'undo' }), [])
  const checkpoint = useCallback(() => dispatch({ type: 'checkpoint' }), [])
  const redo = useCallback(() => dispatch({ type: 'redo' }), [])
  return {
    draft: history.present,
    reset,
    change,
    undo,
    redo,
    checkpoint,
    canUndo: history.past.length > 0,
    canRedo: history.future.length > 0
  }
}
