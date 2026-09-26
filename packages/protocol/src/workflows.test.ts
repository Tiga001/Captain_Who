import { describe, expect, it } from 'vitest'
import fixture from '../fixtures/workflow-definition-v1.json'
import { parseWorkflowDefinition, parseWorkflowRequest, parseWorkflowResponse } from './workflows'

describe('workflow authoring contract', () => {
  it('round-trips user tasks and rejects model or gate configuration on users', () => {
    const node = {
      kind: 'user',
      id: 'user',
      name: 'User',
      task: 'Review the proposal',
      x: 320,
      y: 160
    }
    const graph = { ...fixture, nodes: [node], flows: [] }
    expect(parseWorkflowDefinition(graph)).toEqual(graph)
    expect(
      parseWorkflowDefinition({ ...graph, nodes: [{ ...node, task: undefined }] }).nodes[0]
    ).toEqual({ ...node, task: '' })
    for (const extra of [
      { task: null },
      { modelConfigId: 'model' },
      { busyPolicy: 'queue' },
      { selection: {} }
    ])
      expect(() => parseWorkflowDefinition({ ...graph, nodes: [{ ...node, ...extra }] })).toThrow()
  })
  it('persists the next flow sequence and rejects invalid counters', () => {
    const graph = { ...fixture, nextFlowSequence: 24 }
    expect(parseWorkflowDefinition(graph)).toEqual(graph)
    for (const nextFlowSequence of [null, 0, -1, 1.5, Infinity, Number.MAX_SAFE_INTEGER + 1, '24'])
      expect(() => parseWorkflowDefinition({ ...fixture, nextFlowSequence })).toThrow()
  })
  it('round-trips per-flow anchors and rejects invalid coordinates or sides', () => {
    const flow = {
      ...fixture.flows[0],
      sourceAnchor: { side: 'top', offset: 0.35 },
      targetAnchor: { side: 'left', offset: 0.8 }
    }
    const graph = { ...fixture, flows: [flow] }
    expect(parseWorkflowDefinition(graph)).toEqual(graph)
    for (const sourceAnchor of [
      null,
      {},
      { side: 'diagonal', offset: 0.5 },
      { side: 'top', offset: -0.1 },
      { side: 'top', offset: 1.1 },
      { side: 'top', offset: NaN },
      { side: 'top', offset: Infinity }
    ])
      expect(() =>
        parseWorkflowDefinition({ ...graph, flows: [{ ...flow, sourceAnchor }] })
      ).toThrow()
  })

  it('round-trips manually positioned boundaries and rejects malformed root layout', () => {
    const definition = {
      ...fixture,
      boundaryPositions: { input: { x: -125.5, y: 80 } }
    }
    expect(parseWorkflowDefinition(definition)).toEqual(definition)
    for (const boundaryPositions of [
      null,
      undefined,
      {},
      { input: { x: 0, y: 0 }, output: { x: 0, y: 0 } },
      { input: { x: 0, y: 0 }, output: { x: 0, y: 0 }, extra: true },
      { input: { x: 0, y: 0, nodeId: 'worker' }, output: { x: 0, y: 0 } },
      { input: { x: Infinity, y: 0 }, output: { x: 0, y: 0 } },
      { input: { x: 0, y: 0 }, output: { x: 100001, y: 0 } },
      { input: { x: 0 }, output: { x: 0, y: 0 } }
    ]) {
      expect(() => parseWorkflowDefinition({ ...fixture, boundaryPositions })).toThrow()
    }
  })
  it('preserves independent conversation models and permissions', () => {
    const configured = structuredClone(fixture)
    const definition = {
      ...configured,
      nodes: [
        { ...configured.nodes[0], modelConfigId: 'model-implementation' },
        { ...configured.nodes[1], permissionMode: 'custom', modelConfigId: 'model-review' }
      ]
    }
    expect(parseWorkflowDefinition(definition)).toEqual(definition)
    expect(parseWorkflowRequest({ operation: 'save', definition, expectedRevision: 1 })).toEqual({
      operation: 'save',
      definition,
      expectedRevision: 1
    })
  })

  it('rejects malformed models and removed subagent template references', () => {
    for (const modelConfigId of [undefined, 12, {}, ['model-a'], true]) {
      expect(() =>
        parseWorkflowDefinition({
          ...fixture,
          nodes: [{ ...fixture.nodes[0], modelConfigId }]
        })
      ).toThrow()
    }
    expect(() =>
      parseWorkflowDefinition({
        ...fixture,
        nodes: [{ ...fixture.nodes[0], templateId: 'review-template', modelConfigId: 'override' }]
      })
    ).toThrow()
    expect(() =>
      parseWorkflowDefinition({
        ...fixture,
        nodes: [{ ...fixture.nodes[0], modelId: 'wrong-field' }]
      })
    ).toThrow()
  })

  it('round trips every permission mode and rejects missing or unknown modes', () => {
    for (const permissionMode of ['default', 'custom', 'full']) {
      const definition = { ...fixture, nodes: [{ ...fixture.nodes[0], permissionMode }] }
      expect(parseWorkflowDefinition(definition)).toEqual(definition)
    }
    for (const permissionMode of [undefined, null, '', 'auto', true, 1, {}]) {
      expect(() =>
        parseWorkflowDefinition({
          ...fixture,
          nodes: [{ ...fixture.nodes[0], permissionMode }]
        })
      ).toThrow()
    }
  })

  it('rejects flows targeting the fixed user entry', () => {
    expect(() =>
      parseWorkflowDefinition({
        ...fixture,
        flows: [
          {
            ...fixture.flows[0],
            source: { kind: 'node', nodeId: 'review' },
            target: { kind: 'boundary' }
          }
        ]
      })
    ).toThrow('cannot receive')
  })

  it('preserves boundary flows, cycles, blank nodes and viewport', () => {
    expect(parseWorkflowDefinition(fixture)).toEqual(fixture)
    const input = { operation: 'save', expectedRevision: 0, definition: fixture }
    expect(parseWorkflowRequest(input)).toEqual(input)
    const output = {
      records: [{ definition: fixture, enabled: false, revision: 1, updatedAt: 42, issues: [] }],
      issues: []
    }
    expect(parseWorkflowResponse(output)).toEqual(output)
  })

  it('sets record availability through a revision-checked request without changing the definition', () => {
    for (const enabled of [true, false]) {
      const request = { operation: 'setEnabled', id: fixture.id, enabled, expectedRevision: 2 }
      expect(parseWorkflowRequest(request)).toEqual(request)
      const record = { definition: fixture, enabled, revision: 3, updatedAt: 42, issues: [] }
      expect(parseWorkflowResponse({ records: [record], issues: [] }).records).toEqual([record])
    }
    expect(() => parseWorkflowDefinition({ ...fixture, enabled: true })).toThrow()
  })

  it('rejects malformed availability commands before they can reach storage', () => {
    const request = { operation: 'setEnabled', id: fixture.id, enabled: true, expectedRevision: 1 }
    for (const enabled of [null, undefined, 0, 1, 'true', {}, []]) {
      expect(() => parseWorkflowRequest({ ...request, enabled })).toThrow()
    }
    for (const expectedRevision of [0, -1, 1.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1, '1']) {
      expect(() => parseWorkflowRequest({ ...request, expectedRevision })).toThrow()
    }
    for (const key of ['id', 'enabled', 'expectedRevision']) {
      const missing = { ...request }
      Reflect.deleteProperty(missing, key)
      expect(() => parseWorkflowRequest(missing)).toThrow()
    }
    expect(() => parseWorkflowRequest({ ...request, id: null })).toThrow()
    expect(() => parseWorkflowRequest({ ...request, definition: fixture })).toThrow()
  })

  it('defaults legacy availability to false and rejects invalid or contradictory effective states', () => {
    const record = { definition: fixture, revision: 1, updatedAt: 42, issues: [] }
    expect(parseWorkflowResponse({ records: [record], issues: [] }).records[0]).toEqual({
      ...record,
      enabled: false
    })
    for (const enabled of [null, undefined, 0, 1, 'false', {}, []]) {
      expect(() =>
        parseWorkflowResponse({ records: [{ ...record, enabled }], issues: [] })
      ).toThrow()
    }
    const invalid = { ...record, issues: [{ code: 'node_task', subject: fixture.nodes[0].id }] }
    expect(parseWorkflowResponse({ records: [invalid], issues: [] }).records[0].enabled).toBe(false)
    expect(
      parseWorkflowResponse({ records: [{ ...invalid, enabled: false }], issues: [] }).records[0]
        .issues
    ).toEqual(invalid.issues)
    expect(() =>
      parseWorkflowResponse({ records: [{ ...invalid, enabled: true }], issues: [] })
    ).toThrow('cannot have validation issues')
    expect(() =>
      parseWorkflowResponse({ records: [{ ...record, enabled: false, active: true }], issues: [] })
    ).toThrow()
  })
  it('rejects protocol drift and unexpected execution authority', () => {
    expect(() => parseWorkflowDefinition({ ...fixture, schemaVersion: 2 })).toThrow()
    expect(() =>
      parseWorkflowRequest({
        operation: 'save',
        expectedRevision: 0,
        definition: fixture,
        enabled: true
      })
    ).toThrow()
    expect(() => parseWorkflowRequest({ operation: 'start', id: fixture.id })).toThrow()
    expect(() =>
      parseWorkflowRequest({ operation: 'delete', id: fixture.id, expectedRevision: 0 })
    ).toThrow()
    expect(() =>
      parseWorkflowRequest({ operation: 'save', expectedRevision: NaN, definition: fixture })
    ).toThrow()
  })
  it('validates nested objects without rejecting incomplete drafts', () => {
    expect(parseWorkflowDefinition({ ...fixture, name: '', nodes: [], flows: [] }).nodes).toEqual(
      []
    )
    expect(() =>
      parseWorkflowDefinition({ ...fixture, viewport: { x: Infinity, y: 0, zoom: 1 } })
    ).toThrow()
    expect(() =>
      parseWorkflowDefinition({
        ...fixture,
        flows: [{ ...fixture.flows[0], source: { kind: 'boundary', nodeId: 'fake' } }]
      })
    ).toThrow()
    expect(() =>
      parseWorkflowDefinition({ ...fixture, nodes: [{ ...fixture.nodes[0], templateId: 12 }] })
    ).toThrow()
    expect(() =>
      parseWorkflowDefinition({
        ...fixture,
        nodes: [
          { ...fixture.nodes[2], trigger: { mode: ['all'], count: 1, required: [], groups: [] } }
        ]
      })
    ).toThrow()
    expect(() =>
      parseWorkflowResponse({
        records: [{ definition: fixture, revision: 1, updatedAt: 0, issues: [{ code: 'task' }] }],
        issues: []
      })
    ).toThrow()
  })
})

describe('logic gate wire contract', () => {
  it('round-trips distinct gates without template, model or agent prompts', () => {
    const graph = parseWorkflowDefinition(fixture)
    const input = graph.nodes.find((n) => n.kind === 'inputGate')!
    const output = graph.nodes.find((n) => n.kind === 'outputGate')!
    expect(input).not.toHaveProperty('modelConfigId')
    expect(input).not.toHaveProperty('task')
    input.processingMode = 'batch'
    input.busyPolicy = 'inject'
    output.selection.mode = 'exact'
    output.selection.min = 2
    expect(parseWorkflowDefinition(graph)).toEqual(graph)
  })

  it('round-trips all four input modes and rejects missing policies', () => {
    for (const processingMode of ['individual', 'batch'])
      for (const busyPolicy of ['queue', 'inject']) {
        const graph = { ...fixture, nodes: [{ ...fixture.nodes[2], processingMode, busyPolicy }] }
        expect(parseWorkflowDefinition(graph)).toEqual(graph)
      }
    for (const key of ['processingMode', 'busyPolicy']) {
      const node = { ...fixture.nodes[2] }
      Reflect.deleteProperty(node, key)
      expect(() => parseWorkflowDefinition({ ...fixture, nodes: [node] })).toThrow()
    }
  })

  it('rejects agent/gate field mixing and unsupported arrival modes', () => {
    const input = fixture.nodes[2]
    const output = fixture.nodes[3]
    for (const node of [
      { ...input, task: 'Do work' },
      { ...input, modelConfigId: null },
      { ...input, busyPolicy: 'interrupt' },
      { ...input, processingMode: 'any' },
      { ...input, processingMode: null },
      { ...input, busyPolicy: null },
      { ...input, processingMode: ['individual'] },
      { ...input, trigger: { mode: 'one', count: 1, required: [], groups: [] } },
      { ...input, trigger: { mode: 'atLeast', count: 1.5, required: [], groups: [] } },
      {
        ...input,
        trigger: {
          mode: 'custom',
          count: 1,
          required: [],
          groups: [{ id: 'g', flowIds: [], min: 1, max: 2 }]
        }
      },
      { ...output, lateArrivals: 'queue' },
      { ...fixture.nodes[0], inputRule: { mode: 'all' } }
    ]) {
      expect(() => parseWorkflowDefinition({ ...fixture, nodes: [node] })).toThrow()
    }
    const old = { ...fixture.nodes[0] }
    Reflect.deleteProperty(old, 'kind')
    expect(() => parseWorkflowDefinition({ ...fixture, nodes: [old] })).toThrow()
  })
})
