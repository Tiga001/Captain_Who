import { describe, expect, it } from 'vitest'
import { createWorkflowNode, createWorkflowUser } from '../../features/workflows/workflowAuthoring'
import {
  duplicateWorkflowNode,
  readWorkflowNodeClipboard,
  serializeWorkflowNode
} from '../../features/workflows/workflowNodeClipboard'

describe('workflow node clipboard', () => {
  it('preserves user tasks without adding model configuration', () => {
    const source = { ...createWorkflowUser('用户', 100, 100), task: '检查文案' }
    const copy = duplicateWorkflowNode(readWorkflowNodeClipboard(serializeWorkflowNode(source))!, {
      x: 200,
      y: 300
    })
    expect(copy).toEqual({ ...source, id: expect.any(String), x: 200, y: 300 })
    expect(copy.id).not.toBe(source.id)
    expect(copy).not.toHaveProperty('modelConfigId')
  })
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
