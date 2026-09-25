import { describe, expect, it } from 'vitest'
import fixture from '../fixtures/workflow-definition-v1.json'
import {
  getDefaultWorkflowBoundaryPositions,
  parseWorkflowDefinition,
  parseWorkflowRequest,
  parseWorkflowResponse
} from './workflows'

describe('workflow authoring contract', () => {
  it('derives legacy root positions without adding worker nodes or changing boundary flows', () => {
    const legacy = structuredClone(fixture)
    Reflect.deleteProperty(legacy, 'boundaryPositions')
    expect(parseWorkflowDefinition(legacy)).toEqual(fixture)
    const empty = parseWorkflowDefinition({ ...legacy, nodes: [], flows: [] })
    expect(empty.boundaryPositions).toEqual({
      input: { x: 80, y: 220 },
      output: { x: 760, y: 220 }
    })
    expect(empty.nodes).toEqual([])
    expect(
      getDefaultWorkflowBoundaryPositions([
        { x: -100000, y: -100000 },
        { x: 100000, y: 100000 }
      ])
    ).toEqual({ input: { x: -100000, y: 0 }, output: { x: 100000, y: 0 } })
  })

  it('round-trips manually positioned boundaries and rejects malformed root layout', () => {
    const definition = {
      ...fixture,
      boundaryPositions: { input: { x: -125.5, y: 80 }, output: { x: 925, y: 410 } }
    }
    expect(parseWorkflowDefinition(definition)).toEqual(definition)
    for (const boundaryPositions of [
      null,
      undefined,
      {},
      { input: { x: 0, y: 0 } },
      { input: { x: 0, y: 0 }, output: { x: 0, y: 0 }, extra: true },
      { input: { x: 0, y: 0, nodeId: 'worker' }, output: { x: 0, y: 0 } },
      { input: { x: Infinity, y: 0 }, output: { x: 0, y: 0 } },
      { input: { x: 0, y: 0 }, output: { x: 100001, y: 0 } },
      { input: { x: 0 }, output: { x: 0, y: 0 } }
    ]) {
      expect(() => parseWorkflowDefinition({ ...fixture, boundaryPositions })).toThrow()
    }
  })
  it('normalizes legacy v1 nodes without a model field while retaining strict fields', () => {
    const legacy = structuredClone(fixture)
    Reflect.deleteProperty(legacy.nodes[0], 'modelConfigId')
    expect(parseWorkflowDefinition(legacy)).toEqual(fixture)
    const output = parseWorkflowResponse({
      records: [{ definition: legacy, revision: 1, updatedAt: 42, issues: [] }],
      issues: []
    })
    expect(output.records[0].definition.nodes[0].modelConfigId).toBeNull()
    Reflect.deleteProperty(legacy.nodes[0], 'templateId')
    expect(() => parseWorkflowDefinition(legacy)).toThrow()
  })

  it('preserves explicit standalone model selection and template-owned model selection', () => {
    const configured = structuredClone(fixture)
    const definition = {
      ...configured,
      nodes: [
        { ...configured.nodes[0], modelConfigId: 'model-implementation' },
        { ...configured.nodes[1], templateId: 'review-template', modelConfigId: null }
      ]
    }
    expect(parseWorkflowDefinition(definition)).toEqual(definition)
    expect(parseWorkflowRequest({ operation: 'save', definition, expectedRevision: 1 })).toEqual({
      operation: 'save',
      definition,
      expectedRevision: 1
    })
  })

  it('rejects malformed model values and template model overrides', () => {
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
    ).toThrow('cannot override')
    expect(() =>
      parseWorkflowDefinition({
        ...fixture,
        nodes: [{ ...fixture.nodes[0], modelId: 'wrong-field' }]
      })
    ).toThrow()
  })

  it('preserves boundary flows, cycles, blank nodes and viewport', () => {
    expect(parseWorkflowDefinition(fixture)).toEqual(fixture)
    const input = { operation: 'save', expectedRevision: 0, definition: fixture }
    expect(parseWorkflowRequest(input)).toEqual(input)
    const output = {
      records: [{ definition: fixture, revision: 1, updatedAt: 42, issues: [] }],
      issues: []
    }
    expect(parseWorkflowResponse(output)).toEqual(output)
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
          { ...fixture.nodes[0], inputRule: { ...fixture.nodes[0].inputRule, mode: ['all'] } }
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
