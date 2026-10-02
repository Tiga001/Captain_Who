import { describe, expect, it } from 'vitest'
import { parseWorkflowDefinition } from '@mycopilot/protocol'
import fixture from '../../../../../packages/protocol/fixtures/workflow-definition-v1.json'
import {
  createWorkflowHistory,
  workflowContentKey,
  workflowHistoryReducer
} from '../../features/workflows/workflowHistory'

describe('workflow editing history', () => {
  it('restores removed member settings while preserving the current viewport', () => {
    const graph = parseWorkflowDefinition(fixture)
    let state = workflowHistoryReducer(createWorkflowHistory(graph), {
      type: 'change',
      at: 1,
      update: (g) => ({ ...g, nodes: g.nodes.slice(1) })
    })
    state = workflowHistoryReducer(state, {
      type: 'change',
      at: 2,
      update: (g) => ({ ...g, viewport: { x: 120, y: 50, zoom: 0.75 } }),
      options: { transient: true }
    })
    expect(state.past).toHaveLength(1)
    state = workflowHistoryReducer(state, { type: 'undo' })
    expect(state.present?.nodes).toEqual(graph.nodes)
    expect(state.present?.viewport).toEqual({ x: 120, y: 50, zoom: 0.75 })
    expect(workflowContentKey(state.present!)).toBe(workflowContentKey(graph))
    state = workflowHistoryReducer(state, { type: 'redo' })
    expect(state.present?.nodes).toEqual(graph.nodes.slice(1))
  })
  it('coalesces one drag, separates gestures, and clears redo after new edits', () => {
    let state = createWorkflowHistory(parseWorkflowDefinition(fixture))
    for (const [at, x, group] of [
      [1, 100, 'drag-1'],
      [2, 120, 'drag-1'],
      [3, 160, 'drag-2']
    ] as const)
      state = workflowHistoryReducer(state, {
        type: 'change',
        at,
        options: { group },
        update: (g) => ({ ...g, nodes: g.nodes.map((n, i) => (i === 0 ? { ...n, x } : n)) })
      })
    expect(state.past).toHaveLength(2)
    state = workflowHistoryReducer(state, { type: 'undo' })
    expect(state.present?.nodes[0].x).toBe(120)
    state = workflowHistoryReducer(state, {
      type: 'change',
      at: 5,
      update: (g) => ({ ...g, name: 'New name' })
    })
    expect(state.future).toHaveLength(0)
    expect(workflowHistoryReducer(state, { type: 'redo' })).toBe(state)
  })
})
