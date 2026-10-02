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
    const graph = { ...fixture, nodes: [node] }
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

  it('preserves independent members and viewport', () => {
    expect(parseWorkflowDefinition(fixture)).toEqual(fixture)
    const input = { operation: 'save', expectedRevision: 0, definition: fixture }
    expect(parseWorkflowRequest(input)).toEqual(input)
    const output = {
      records: [{ definition: fixture, enabled: false, revision: 1, updatedAt: 42, issues: [] }],
      issues: []
    }
    expect(parseWorkflowResponse(output)).toEqual(output)
  })

  it('sets instance activation through a revision-checked request without changing the template', () => {
    for (const enabled of [true, false]) {
      const request = {
        operation: 'setInstanceEnabled',
        id: fixture.id,
        enabled,
        expectedRevision: 2
      }
      expect(parseWorkflowRequest(request)).toEqual(request)
      const instance = {
        id: fixture.id,
        templateId: fixture.id,
        templateRevision: 1,
        name: 'Review',
        color: '#4A82E8',
        bindings: [],
        revision: 3,
        updatedAt: 42,
        needsReview: false,
        enabled,
        running: false
      }
      expect(
        parseWorkflowResponse({ records: [], issues: [], instances: [instance] }).instances
      ).toEqual([instance])
    }
    expect(() => parseWorkflowDefinition({ ...fixture, enabled: true })).toThrow()
    expect(() =>
      parseWorkflowRequest({
        operation: 'setEnabled',
        id: fixture.id,
        enabled: true,
        expectedRevision: 1
      })
    ).toThrow()
  })

  it('rejects malformed availability commands before they can reach storage', () => {
    const request = {
      operation: 'setInstanceEnabled',
      id: fixture.id,
      enabled: true,
      expectedRevision: 1
    }
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
    expect(parseWorkflowDefinition({ ...fixture, name: '', nodes: [] }).nodes).toEqual([])
    expect(() =>
      parseWorkflowDefinition({ ...fixture, viewport: { x: Infinity, y: 0, zoom: 1 } })
    ).toThrow()
    expect(() =>
      parseWorkflowDefinition({
        ...fixture,
        flows: [{ id: 'removed-flow', source: { kind: 'boundary', nodeId: 'fake' } }]
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

describe('free mail network wire contract', () => {
  it('rejects removed graph and delivery policies rather than migrating them', () => {
    for (const extra of [
      { flows: [] },
      { boundaryPositions: { input: { x: 0, y: 0 } } },
      { nextFlowSequence: 1 }
    ])
      expect(() => parseWorkflowDefinition({ ...fixture, ...extra })).toThrow()
    for (const extra of [
      { kind: 'inputGate' },
      { kind: 'outputGate' },
      { busyPolicy: 'queue' },
      { processingMode: 'batch' },
      { inputRule: {} },
      { selection: {} }
    ])
      expect(() =>
        parseWorkflowDefinition({ ...fixture, nodes: [{ ...fixture.nodes[0], ...extra }] })
      ).toThrow()
  })
})

describe('global workflow instance contract', () => {
  const save = {
    operation: 'saveInstance',
    id: 'instance',
    templateId: 'template',
    name: 'Review',
    color: '#4A82E8',
    bindings: [{ nodeId: 'agent', conversationId: 'chat' }],
    expectedRevision: 0,
    expectedTemplateRevision: 2
  }
  it('accepts global cross-project binding requests without a project owner', () => {
    expect(parseWorkflowRequest(save)).toEqual(save)
    expect(parseWorkflowRequest({ operation: 'listInstances' })).toEqual({
      operation: 'listInstances'
    })
    expect(
      parseWorkflowRequest({ ...save, bindings: [{ nodeId: 'agent', conversationId: null }] })
    ).toMatchObject({ bindings: [{ nodeId: 'agent', conversationId: null }] })
    for (const extra of [
      { projectId: 42 },
      { running: true },
      { expectedRevision: -1 },
      { bindings: [{ nodeId: 'agent' }] }
    ])
      expect(() => parseWorkflowRequest({ ...save, ...extra })).toThrow()
  })
  it('accepts a nullable default project without restricting bound conversation projects', () => {
    for (const projectId of ['project', null]) {
      expect(parseWorkflowRequest({ ...save, projectId })).toEqual({ ...save, projectId })
    }
  })
  it('round-trips usage guards and isolated editing drafts', () => {
    for (const request of [
      {
        operation: 'save',
        definition: fixture,
        expectedRevision: 2,
        expectedUsageRevision: 'usage',
        expectedDraftRevision: 2
      },
      {
        operation: 'saveDraft',
        definition: fixture,
        expectedRevision: 2,
        expectedDraftRevision: 0
      },
      { operation: 'deleteDraft', id: 'template', expectedDraftRevision: 1 },
      { operation: 'duplicate', id: 'template', expectedRevision: 2, newId: 'copy', name: 'Copy' },
      { operation: 'deleteInstance', id: 'instance', expectedRevision: 1 }
    ])
      expect(parseWorkflowRequest(request)).toEqual(request)
    const response = {
      records: [],
      issues: [],
      instances: [
        {
          id: 'instance',
          templateId: 'template',
          templateRevision: 2,
          name: 'Review',
          color: '#4A82E8',
          bindings: [{ nodeId: 'agent', conversationId: 'chat' }],
          revision: 1,
          updatedAt: 1,
          needsReview: false,
          enabled: true,
          running: false
        }
      ],
      usages: [
        {
          templateId: 'template',
          usageRevision: 'usage',
          instances: [
            {
              id: 'instance',
              name: 'Review',
              running: false,
              projectNames: ['Project A', 'Project B']
            }
          ]
        }
      ],
      drafts: [{ definition: fixture, baseRevision: 2, revision: 1, updatedAt: 1 }],
      affectedConversationIds: ['chat']
    }
    expect(parseWorkflowResponse(response)).toEqual(response)
    for (const projectId of ['project', null]) {
      const withProject = { ...response, instances: [{ ...response.instances[0], projectId }] }
      expect(parseWorkflowResponse(withProject)).toEqual(withProject)
    }
    for (const enabled of [undefined, null, 'true', 1]) {
      expect(() =>
        parseWorkflowResponse({ ...response, instances: [{ ...response.instances[0], enabled }] })
      ).toThrow()
    }
    const missingEnabled = { ...response.instances[0] } as Record<string, unknown>
    delete missingEnabled.enabled
    expect(() => parseWorkflowResponse({ ...response, instances: [missingEnabled] })).toThrow()
    expect(() =>
      parseWorkflowResponse({
        ...response,
        instances: [{ ...response.instances[0], running: 'yes' }]
      })
    ).toThrow()
  })
})

describe('workflow catalog recovery contract', () => {
  const invalidRecord = {
    id: 'old-template',
    name: 'Legacy template',
    revision: 2,
    updatedAt: 123,
    reason: 'incompatible_definition'
  }
  it('keeps incompatible templates and drafts separate from usable definitions', () => {
    const response = {
      records: [{ definition: fixture, enabled: false, revision: 1, updatedAt: 123, issues: [] }],
      issues: [],
      invalidRecords: [invalidRecord],
      invalidDrafts: [
        { ...invalidRecord, id: fixture.id, baseRevision: 1, reason: 'invalid_definition' }
      ]
    }
    expect(parseWorkflowResponse(response)).toEqual(response)
    expect(parseWorkflowResponse({ records: [], issues: [] })).toEqual({ records: [], issues: [] })
  })
  it('rejects malformed recovery metadata without accepting legacy graph fields', () => {
    for (const invalid of [
      { ...invalidRecord, reason: 'unknown' },
      { ...invalidRecord, revision: 0 },
      { ...invalidRecord, updatedAt: -1 },
      { ...invalidRecord, name: 123 },
      { ...invalidRecord, definition: fixture }
    ]) {
      expect(() =>
        parseWorkflowResponse({ records: [], issues: [], invalidRecords: [invalid] })
      ).toThrow()
    }
    expect(() =>
      parseWorkflowResponse({ records: [], issues: [], invalidDrafts: [invalidRecord] })
    ).toThrow()
    expect(() =>
      parseWorkflowResponse({
        records: [],
        issues: [],
        invalidRecords: [invalidRecord, invalidRecord]
      })
    ).toThrow()
    expect(() =>
      parseWorkflowResponse({
        records: [{ definition: fixture, enabled: false, revision: 1, updatedAt: 123, issues: [] }],
        issues: [],
        invalidRecords: [{ ...invalidRecord, id: fixture.id }]
      })
    ).toThrow()
    expect(() =>
      parseWorkflowDefinition({
        ...fixture,
        boundaryPositions: { output: { x: 0, y: 0 } }
      })
    ).toThrow()
  })
})
