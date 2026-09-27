import { describe, expect, it } from 'vitest'
import {
  createWorkflowGate,
  createWorkflowNode,
  createWorkflowUser
} from '../../features/workflows/workflowAuthoring'
import {
  duplicateWorkflowNode,
  readWorkflowNodeClipboard,
  serializeWorkflowNode
} from '../../features/workflows/workflowNodeClipboard'

describe('workflow node clipboard', () => {
  it('preserves all agent settings while assigning independent identities and positions', () => {
    const source = {
      ...createWorkflowNode('文书1号', 100, 100, 'model-b'),
      permissionMode: 'full' as const,
      receives: '原始文案',
      task: '润色文案',
      delivers: '交付文案'
    }
    const decoded = readWorkflowNodeClipboard(serializeWorkflowNode(source))!
    const first = duplicateWorkflowNode(decoded, { x: 300, y: 200 })
    const second = duplicateWorkflowNode(decoded, { x: 500, y: 200 })
    expect(first).toEqual({ ...source, id: first.id, x: 300, y: 200 })
    expect(new Set([source.id, first.id, second.id]).size).toBe(3)
    expect(decoded).toEqual(source)
  })

  it('preserves user and input gate settings and detaches output rules from old connections', () => {
    for (const source of [
      { ...createWorkflowUser('用户', 100, 100), task: '检查文案' },
      {
        ...createWorkflowGate('inputGate', 100, 100),
        processingMode: 'individual' as const,
        busyPolicy: 'inject' as const
      }
    ]) {
      const node = readWorkflowNodeClipboard(serializeWorkflowNode(source))!
      expect(duplicateWorkflowNode(node, source)).toEqual({ ...source, id: expect.any(String) })
    }
    const gate = createWorkflowGate('outputGate', 100, 100)
    if (gate.kind !== 'outputGate') throw new Error('Expected output gate')
    gate.selection = {
      mode: 'custom',
      min: 1,
      max: 2,
      required: ['old-flow'],
      groups: [{ id: 'group', flowIds: ['another-flow'], min: 1, max: 2 }]
    }
    const copied = duplicateWorkflowNode(
      readWorkflowNodeClipboard(serializeWorkflowNode(gate))!,
      gate
    )
    expect(copied).toMatchObject({
      selection: {
        mode: 'custom',
        min: 1,
        max: 2,
        required: [],
        groups: [{ flowIds: [], min: 1, max: 2 }]
      }
    })
    expect(gate.selection.required).toEqual(['old-flow'])
    expect(gate.selection.groups[0].flowIds).toEqual(['another-flow'])
  })

  it('ignores unrelated, malformed and unsupported clipboard content', () => {
    const node = createWorkflowNode('test', 100, 100)
    const envelope = JSON.parse(serializeWorkflowNode(node))
    for (const value of [
      '',
      'plain text',
      'null',
      '{}',
      '{',
      JSON.stringify({ ...envelope, version: 2 }),
      JSON.stringify({ ...envelope, node: { ...node, kind: 'unknown' } }),
      JSON.stringify({ ...envelope, node: { ...node, x: null } }),
      'x'.repeat(2_000_001)
    ]) {
      expect(readWorkflowNodeClipboard(value)).toBeNull()
    }
  })
})
