import { describe, expect, it } from 'vitest'
import { parseWorkflowDefinition } from '@mycopilot/protocol'
import fixture from '../../../../../packages/protocol/fixtures/workflow-definition-v1.json'
import {
  createWorkflowNode,
  createWorkflowUser,
  nameUnnamedWorkflowFlows,
  nameUnnamedWorkflowGates,
  nodeFlows,
  removeWorkflowFlows
} from '../../features/workflows/workflowAuthoring'

describe('workflow graph editing', () => {
  it('names unnamed flows without overwriting custom names or reusing reserved numbers', () => {
    const graph = parseWorkflowDefinition(fixture)
    graph.nextFlowSequence = 24
    graph.flows[0].name = ''
    graph.flows[1].name = 'S25'
    const expected = graph.flows.filter((flow) => !flow.name.trim()).map((flow) => flow.id)
    const named = nameUnnamedWorkflowFlows(graph)
    expect(
      named.flows.filter((flow) => expected.includes(flow.id)).map((flow) => flow.name)
    ).toEqual(expected.map((_, i) => `S${26 + i}`))
    expect(named.flows[1].name).toBe('S25')
    expect(named.nodes).toEqual(graph.nodes)
    expect(named.nextFlowSequence).toBe(26 + expected.length)
    expect(nameUnnamedWorkflowFlows(named)).toBe(named)
    expect(graph.flows[0].name).toBe('')
  })
  it('allows blank nodes independently of a model or saved template', () => {
    const node = createWorkflowNode('Research', 150, 80)
    expect(node.templateId).toBeNull()
    expect(node.name).toBe('Research')
    expect(node.task).toBe('')
  })
  it('removes deleted flow membership without silently relaxing quantity limits', () => {
    const graph = parseWorkflowDefinition(fixture)
    if (graph.nodes[3].kind !== 'outputGate') throw new Error('Expected output gate')
    graph.nodes[3].selection = {
      mode: 'custom',
      min: 1,
      max: 2,
      required: ['deliver'],
      groups: [{ id: 'group', flowIds: ['revise'], min: 1, max: 1 }]
    }
    const edited = removeWorkflowFlows(graph, new Set(['deliver', 'revise']))
    expect(edited.nodes[3]).toMatchObject({
      selection: { required: [], groups: [{ flowIds: [], min: 1 }] }
    })
    expect(edited.nodes[2]).toEqual(graph.nodes[2])
    expect(graph.flows).toHaveLength(6)
    expect(graph.nodes[3].selection.required).toEqual(['deliver'])
  })
})

import {
  createWorkflow,
  createWorkflowGate,
  connectWorkflowFlow,
  workflowConnectionAllowed
} from '../../features/workflows/workflowAuthoring'
import {
  createWorkflowHistory,
  workflowHistoryReducer
} from '../../features/workflows/workflowHistory'
import type { WorkflowDefinition, WorkflowFlow } from '@mycopilot/protocol'

function edge(id: string, from: string | null, to: string | null): WorkflowFlow {
  return {
    id,
    name: id,
    source: from ? { kind: 'node', nodeId: from } : { kind: 'boundary' },
    target: to ? { kind: 'node', nodeId: to } : { kind: 'boundary' }
  }
}
function simpleGraph(): WorkflowDefinition {
  return {
    ...createWorkflow(),
    nodes: ['a', 'b', 'c'].map((id, i) => ({ ...createWorkflowNode(id, 300 + i * 300, 100), id })),
    flows: [edge('entry', null, 'a'), edge('ab', 'a', 'b'), edge('exit', 'b', null)]
  }
}
describe('explicit logic gates', () => {
  it('inserts only input gates for users, preserves free outputs and protects target agents', () => {
    const graph = simpleGraph()
    graph.nodes.push({ ...createWorkflowUser('User', 900, 350), id: 'user' })
    graph.flows = [edge('au', 'a', 'user')]
    const next = connectWorkflowFlow(graph, edge('bu', 'b', 'user'))
    const gate = next.nodes.find((n) => n.kind === 'inputGate')!
    expect(nodeFlows(next, 'user', 'input')).toHaveLength(1)
    expect(nodeFlows(next, gate.id, 'input').map((f) => f.id)).toEqual(['au', 'bu'])
    expect(next.nodes.some((n) => n.kind === 'outputGate')).toBe(false)
    const withOutputs = connectWorkflowFlow(
      connectWorkflowFlow(next, edge('ua', 'user', 'a')),
      edge('uc', 'user', 'c')
    )
    expect(nodeFlows(withOutputs, 'user', 'output')).toHaveLength(2)
    expect(withOutputs.nodes.some((n) => n.kind === 'outputGate')).toBe(false)
    const withTargetGate = connectWorkflowFlow(withOutputs, edge('entry', null, 'c'))
    const cInput = nodeFlows(withTargetGate, 'c', 'input')[0].source
    expect(
      cInput.kind === 'node' && withTargetGate.nodes.find((n) => n.id === cInput.nodeId)?.kind
    ).toBe('inputGate')
    const out = createWorkflowGate('outputGate', 1100, 350)
    const withOutputGate = { ...withTargetGate, nodes: [...withTargetGate.nodes, out] }
    const forbidden = edge('user-output-gate', 'user', out.id)
    expect(workflowConnectionAllowed(withOutputGate, forbidden.source, forbidden.target)).toBe(
      false
    )
    expect(connectWorkflowFlow(withOutputGate, forbidden)).toBe(withOutputGate)
    expect(nodeFlows(withTargetGate, 'user', 'output')).toHaveLength(2)
    const state = workflowHistoryReducer(createWorkflowHistory(graph), {
      type: 'change',
      at: 1,
      update: next
    })
    expect(workflowHistoryReducer(state, { type: 'undo' }).present).toEqual({
      ...graph,
      nextFlowSequence: next.nextFlowSequence
    })
  })

  it('assigns distinct names per gate type and preserves custom names when filling unnamed gates', () => {
    const graph = createWorkflow()
    for (const kind of ['inputGate', 'inputGate', 'outputGate'] as const)
      graph.nodes.push(createWorkflowGate(kind, 100, 100, graph.nodes))
    expect(graph.nodes.map((node) => node.name)).toEqual(['输入门1', '输入门2', '输出门1'])
    graph.nodes[0].name = '资料汇总'
    graph.nodes[2].name = ''
    const named = nameUnnamedWorkflowGates(graph)
    expect(named.nodes.map((node) => node.name)).toEqual(['资料汇总', '输入门2', '输出门1'])
    expect(createWorkflowGate('inputGate', 100, 100, named.nodes).name).toBe('输入门3')
    expect(nameUnnamedWorkflowGates(named)).toBe(named)
    expect(parseWorkflowDefinition(named)).toEqual(named)
  })
  it('rejects self-connections and retargeting to self before allocating names or gates', () => {
    const graph = simpleGraph()
    graph.nodes.push(
      createWorkflowGate('inputGate', 100, 100),
      createWorkflowGate('outputGate', 900, 100)
    )
    for (const node of graph.nodes) {
      const selfFlow = { ...edge('self', node.id, node.id), name: '' }
      expect(workflowConnectionAllowed(graph, selfFlow.source, selfFlow.target)).toBe(false)
      expect(connectWorkflowFlow(graph, selfFlow)).toBe(graph)
    }
    const existing = graph.flows.find((flow) => flow.id === 'ab')!
    expect(connectWorkflowFlow(graph, { ...existing, target: existing.source })).toBe(graph)
    expect(connectWorkflowFlow(graph, { ...existing, source: existing.target })).toBe(graph)
    expect(
      workflowConnectionAllowed(graph, { kind: 'node', nodeId: 'b' }, { kind: 'node', nodeId: 'a' })
    ).toBe(true)
  })
  it('does not recycle flow numbers after deleting, renaming, reopening or undoing', () => {
    let graph: WorkflowDefinition = {
      ...createWorkflow(),
      nodes: [createWorkflowGate('inputGate', 100, 100)]
    }
    const target = graph.nodes[0].id
    for (let i = 1; i <= 23; i++)
      graph = connectWorkflowFlow(graph, { ...edge(`flow-${i}`, null, target), name: '' })
    expect(graph.flows.map((flow) => flow.name)).toEqual(
      Array.from({ length: 23 }, (_, i) => `S${i + 1}`)
    )
    graph = removeWorkflowFlows(graph, new Set(['flow-2', 'flow-23']))
    graph.flows = graph.flows.map((flow) => ({ ...flow, name: 'Custom' }))
    graph = parseWorkflowDefinition(JSON.parse(JSON.stringify(graph)))
    let history = workflowHistoryReducer(createWorkflowHistory(graph), {
      type: 'change',
      at: 1,
      update: (g) => connectWorkflowFlow(g, { ...edge('new', null, target), name: '' })
    })
    expect(history.present?.flows.at(-1)?.name).toBe('S24')
    history = workflowHistoryReducer(history, { type: 'undo' })
    const next = connectWorkflowFlow(history.present!, {
      ...edge('after-undo', null, target),
      name: ''
    })
    expect(next.flows.at(-1)?.name).toBe('S25')
  })

  it('numbers automatic gate bindings without renaming existing flows on retarget', () => {
    const next = connectWorkflowFlow(simpleGraph(), { ...edge('loop', 'b', 'a'), name: '' })
    expect(next.flows.slice(3).map((flow) => flow.name)).toEqual(['S1', 'S2', 'S3'])
    const loop = next.flows.find((flow) => flow.id === 'loop')!
    const retargeted = connectWorkflowFlow(next, { ...loop, target: { kind: 'node', nodeId: 'c' } })
    expect(retargeted.flows.find((flow) => flow.id === 'loop')?.name).toBe('S1')
    expect(retargeted.nextFlowSequence).toBe(4)
  })
  it('creates both gates atomically for a loop and preserves existing flow IDs, labels and agent fields', () => {
    const graph = simpleGraph()
    graph.flows[0].targetAnchor = { side: 'bottom', offset: 0.3 }
    graph.flows[2].sourceAnchor = { side: 'top', offset: 0.7 }
    const loop = edge('loop', 'b', 'a')
    let state = workflowHistoryReducer(createWorkflowHistory(graph), {
      type: 'change',
      at: 1,
      update: (g) => connectWorkflowFlow(g, loop)
    })
    const next = state.present!
    const input = next.nodes.find((n) => n.kind === 'inputGate')!
    const output = next.nodes.find((n) => n.kind === 'outputGate')!
    expect(input.name).toBe('输入门1')
    expect(output.name).toBe('输出门1')
    expect(next.nodes).toHaveLength(5)
    expect(next.flows).toHaveLength(6)
    expect(nodeFlows(next, 'a', 'input')).toHaveLength(1)
    expect(nodeFlows(next, 'b', 'output')).toHaveLength(1)
    expect(nodeFlows(next, 'a', 'input')[0].targetAnchor).toEqual({ side: 'bottom', offset: 0.3 })
    expect(nodeFlows(next, 'b', 'output')[0].sourceAnchor).toEqual({ side: 'top', offset: 0.7 })
    expect(next.flows.find((f) => f.id === 'entry')?.targetAnchor).toBeUndefined()
    expect(next.flows.find((f) => f.id === 'exit')?.sourceAnchor).toBeUndefined()
    expect(nodeFlows(next, input.id, 'input').map((f) => f.id)).toEqual(['entry', 'loop'])
    expect(nodeFlows(next, output.id, 'output').map((f) => f.id)).toEqual(['exit', 'loop'])
    expect(next.flows.find((f) => f.id === 'entry')?.name).toBe('entry')
    expect(next.nodes.slice(0, 3)).toEqual(graph.nodes)
    expect(parseWorkflowDefinition(next)).toEqual(next)
    state = workflowHistoryReducer(state, { type: 'undo' })
    expect(state.present).toEqual({ ...graph, nextFlowSequence: next.nextFlowSequence })
    state = workflowHistoryReducer(state, { type: 'redo' })
    expect(state.present).toEqual(next)
  })

  it('reuses attached gates on subsequent agent connections without losing their settings', () => {
    let graph = connectWorkflowFlow(simpleGraph(), edge('loop', 'b', 'a'))
    const gate = graph.nodes.find((n) => n.kind === 'inputGate')!
    gate.processingMode = 'individual'
    gate.busyPolicy = 'inject'
    graph = connectWorkflowFlow(graph, edge('ca', 'c', 'a'))
    expect(graph.nodes.filter((n) => n.kind === 'inputGate')).toHaveLength(1)
    expect(graph.nodes.find((n) => n.id === gate.id)).toMatchObject({
      processingMode: 'individual',
      busyPolicy: 'inject'
    })
    expect(nodeFlows(graph, gate.id, 'input')).toHaveLength(3)
    expect(nodeFlows(graph, 'a', 'input')).toHaveLength(1)
  })

  it('inserts a manually added gate on the agent side and transfers the existing input', () => {
    const graph = simpleGraph()
    const gate = createWorkflowGate('inputGate', 100, 100)
    graph.nodes.push(gate)
    const next = connectWorkflowFlow(graph, edge('binding', gate.id, 'a'))
    expect(next.nodes).toHaveLength(4)
    expect(next.flows.find((f) => f.id === 'entry')?.target).toEqual({
      kind: 'node',
      nodeId: gate.id
    })
    expect(nodeFlows(next, 'a', 'input').map((f) => f.id)).toEqual(['binding'])
    // A second agent owner, direct root output, and gate-only cycle are forbidden.
    expect(
      workflowConnectionAllowed(
        next,
        { kind: 'node', nodeId: gate.id },
        { kind: 'node', nodeId: 'b' }
      )
    ).toBe(false)
    expect(
      workflowConnectionAllowed(next, { kind: 'node', nodeId: gate.id }, { kind: 'boundary' })
    ).toBe(false)
    expect(
      workflowConnectionAllowed(
        next,
        { kind: 'node', nodeId: gate.id },
        { kind: 'node', nodeId: gate.id }
      )
    ).toBe(false)
  })

  it('retargets a flow through automatic gates and removes membership only from its previous gate', () => {
    let graph = connectWorkflowFlow(simpleGraph(), edge('loop', 'b', 'a'))
    const output = graph.nodes.find((n) => n.kind === 'outputGate')!
    output.selection = {
      mode: 'custom',
      min: 1,
      max: 1,
      required: [],
      groups: [{ id: 'needed', flowIds: ['loop'], min: 1, max: 1 }]
    }
    const loop = graph.flows.find((f) => f.id === 'loop')!
    graph = connectWorkflowFlow(graph, { ...loop, source: { kind: 'node', nodeId: 'c' } })
    expect(graph.nodes.find((n) => n.id === output.id)).toMatchObject({
      selection: { groups: [{ flowIds: [], min: 1 }] }
    })
    expect(graph.flows.find((f) => f.id === 'loop')?.name).toBe('loop')
  })

  it('refuses a connection atomically when automatic gates would exceed the graph limits', () => {
    const graph = simpleGraph()
    while (graph.nodes.length < 128) graph.nodes.push(createWorkflowNode('Unused', 100, 100))
    const snapshot = structuredClone(graph)
    expect(connectWorkflowFlow(graph, edge('loop', 'b', 'a'))).toBe(graph)
    expect(graph).toEqual(snapshot)
  })
})
