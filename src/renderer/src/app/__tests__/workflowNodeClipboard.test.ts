import { describe, expect, it } from 'vitest'
import { createWorkflowNode } from '../../features/workflows/workflowAuthoring'
import {
  duplicateWorkflowNode,
  readWorkflowNodeClipboard,
  serializeWorkflowNode
} from '../../features/workflows/workflowNodeClipboard'

describe('organization node clipboard', () => {
  it('rejects removed user nodes from the clipboard', () => {
    const envelope = JSON.parse(serializeWorkflowNode(createWorkflowNode('Agent', 100, 100)))
    envelope.node = { id: 'user', kind: 'user', name: 'User', task: 'Review', x: 100, y: 100 }
    expect(readWorkflowNodeClipboard(JSON.stringify(envelope))).toBeNull()
  })
  it('preserves all agent settings while assigning independent identities and positions', () => {
    const source = {
      ...createWorkflowNode('文书1号', 100, 100, 'model-b'),
      permissionMode: 'full' as const,
      rank: 8,
      managementRole: 'organization_admin' as const,
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

  it('preserves settings but chooses an available name when pasting into a team', () => {
    const source = { ...createWorkflowNode('Boss', 0, 0), task: 'Manage', rank: 99 }
    const first = duplicateWorkflowNode(source, { x: 10, y: 20 }, [source])
    const second = duplicateWorkflowNode(source, { x: 20, y: 30 }, [source, first])
    expect(first).toMatchObject({ name: 'Boss (2)', task: 'Manage', rank: 99 })
    expect(second.name).toBe('Boss (3)')
    expect(duplicateWorkflowNode(source, { x: 0, y: 0 }, []).name).toBe('Boss')
    expect(source.name).toBe('Boss')
  })

  it('decodes department administrator settings without requiring the source department', () => {
    const source = {
      ...createWorkflowNode('部门负责人', 100, 100),
      rank: 5,
      managementRole: 'department_admin' as const,
      departmentId: 'source-template-department'
    }
    expect(readWorkflowNodeClipboard(serializeWorkflowNode(source))).toEqual(source)
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
