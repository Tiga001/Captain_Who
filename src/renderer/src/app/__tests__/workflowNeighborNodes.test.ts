import type { WorkflowDefinition, WorkflowNode } from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import { workflowNeighborNodes } from '../../features/workflows/workflowNeighborNodes'

const agent = (id: string, name = id): WorkflowNode => ({
  id,
  name,
  kind: 'agent',
  x: 0,
  y: 0,
  permissionMode: 'default',
  modelConfigId: 'model',
  receives: '',
  task: 'task',
  delivers: ''
})

const flow = (id: string, source: string | null, target: string) => ({
  id,
  name: id,
  source: source ? { kind: 'node' as const, nodeId: source } : { kind: 'boundary' as const },
  target: { kind: 'node' as const, nodeId: target }
})

const definition = (
  nodes: WorkflowNode[],
  flows: ReturnType<typeof flow>[]
): WorkflowDefinition => ({
  schemaVersion: 1,
  id: 'template',
  name: 'Template',
  description: '',
  background: '',
  nodes,
  flows,
  viewport: { x: 0, y: 0, zoom: 1 },
  boundaryPositions: { input: { x: 0, y: 0 } }
})

const bind = (...nodeIds: string[]) =>
  nodeIds.map((nodeId) => ({ nodeId, conversationId: `conversation-${nodeId}` }))

describe('workflow neighbor nodes', () => {
  it('walks through logic gates to the bound agents on either side of a node', () => {
    const graph = definition(
      [
        agent('planner', ' Planner '),
        agent('researcher', 'Researcher'),
        {
          id: 'input-gate',
          name: 'Input gate',
          kind: 'inputGate',
          x: 0,
          y: 0,
          processingMode: 'batch',
          busyPolicy: 'queue'
        },
        agent('developer', 'Developer'),
        {
          id: 'output-gate',
          name: 'Output gate',
          kind: 'outputGate',
          x: 0,
          y: 0,
          selection: { mode: 'one', min: 1, max: 1, required: [], groups: [] }
        },
        agent('reviewer', 'Reviewer'),
        { id: 'user', name: 'Approver', kind: 'user', x: 0, y: 0, task: 'Approve' }
      ],
      [
        flow('entry', null, 'planner'),
        flow('planner-gate', 'planner', 'input-gate'),
        flow('researcher-gate', 'researcher', 'input-gate'),
        flow('gate-developer', 'input-gate', 'developer'),
        flow('developer-output', 'developer', 'output-gate'),
        flow('output-reviewer', 'output-gate', 'reviewer'),
        flow('output-user', 'output-gate', 'user')
      ]
    )
    const bindings = bind('planner', 'researcher', 'developer', 'reviewer')

    expect(workflowNeighborNodes(graph, bindings, 'developer')).toEqual({
      upstream: [
        { nodeId: 'planner', name: 'Planner', conversationId: 'conversation-planner' },
        { nodeId: 'researcher', name: 'Researcher', conversationId: 'conversation-researcher' }
      ],
      downstream: [
        { nodeId: 'reviewer', name: 'Reviewer', conversationId: 'conversation-reviewer' }
      ]
    })
    expect(workflowNeighborNodes(graph, bindings, 'planner')).toEqual({
      upstream: [],
      downstream: [
        { nodeId: 'developer', name: 'Developer', conversationId: 'conversation-developer' }
      ]
    })
  })

  it('stops at direct agent neighbors, keeps loop partners and skips unbound agents', () => {
    const graph = definition(
      [agent('writer'), agent('editor'), agent('publisher'), agent('archivist')],
      [
        flow('writer-editor', 'writer', 'editor'),
        flow('editor-writer', 'editor', 'writer'),
        flow('editor-publisher', 'editor', 'publisher'),
        flow('publisher-archivist', 'publisher', 'archivist')
      ]
    )

    const { upstream, downstream } = workflowNeighborNodes(
      graph,
      bind('writer', 'editor', 'publisher'),
      'editor'
    )
    expect(upstream.map((node) => node.nodeId)).toEqual(['writer'])
    expect(downstream.map((node) => node.nodeId)).toEqual(['writer', 'publisher'])
    expect(
      workflowNeighborNodes(graph, bind('writer', 'editor', 'publisher'), 'publisher').downstream
    ).toEqual([])
  })
})
