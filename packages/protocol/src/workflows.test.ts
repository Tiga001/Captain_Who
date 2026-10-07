import { describe, expect, it } from 'vitest'
import fixture from '../fixtures/workflow-definition-v1.json'
import hierarchy from '../fixtures/organization-hierarchy-v1.json'
import memberNames from '../fixtures/organization-member-names-v1.json'
import {
  workflowMemberNameKey,
  parseWorkflowDefinition,
  parseWorkflowNode,
  parseWorkflowRequest,
  parseWorkflowResponse
} from './workflows'

describe('organization hierarchy authoring contract', () => {
  it('accepts targeted board reads and validates optional activity enrichment', () => {
    for (const request of [
      { operation: 'getInstance', instanceId: 'board' },
      { operation: 'getInstance', instanceId: 'board', includeActivity: true },
      { operation: 'getInstance', instanceId: 'board', includeActivity: false }
    ])
      expect(parseWorkflowRequest(request)).toEqual(request)
    for (const extra of [
      { includeActivity: 1 },
      { includeActivity: null },
      { projectId: 'spoof' },
      { instanceId: '' }
    ])
      expect(() =>
        parseWorkflowRequest({ operation: 'getInstance', instanceId: 'board', ...extra })
      ).toThrow()
  })
  it('round trips nested departments, independent rank and management identity', () => {
    expect(parseWorkflowDefinition(hierarchy)).toEqual(hierarchy)
    for (const operation of ['save', 'saveDraft']) {
      const request = {
        operation,
        definition: hierarchy,
        expectedRevision: 1,
        ...(operation === 'saveDraft' ? { expectedDraftRevision: 0 } : {})
      }
      expect(parseWorkflowRequest(request)).toEqual(request)
    }
    expect(parseWorkflowNode(hierarchy.nodes[1])).toEqual(hierarchy.nodes[1])
  })

  it('rejects invalid rank and management role without accepting execution authority', () => {
    for (const rank of [0, 100, -1, 1.5, null, '3'])
      expect(() => parseWorkflowNode({ ...hierarchy.nodes[0], rank })).toThrow()
    for (const managementRole of ['admin', 'owner', null, 1])
      expect(() => parseWorkflowNode({ ...hierarchy.nodes[0], managementRole })).toThrow()
    expect(() => parseWorkflowNode({ ...hierarchy.nodes[0], canManage: true })).toThrow()
  })

  it('rejects orphaned, cyclic or duplicated department identities and malformed frames', () => {
    for (const patch of [
      { id: hierarchy.nodes[0].id },
      { id: ' review-team' },
      { id: '' },
      { parentId: 'missing' },
      { parentId: 'review-team' },
      { width: 0 },
      { height: -1 },
      { x: 100_001 },
      { width: Infinity }
    ]) {
      expect(() =>
        parseWorkflowDefinition({
          ...hierarchy,
          departments: [hierarchy.departments[0], { ...hierarchy.departments[1], ...patch }]
        })
      ).toThrow()
    }
    expect(() =>
      parseWorkflowDefinition({
        ...hierarchy,
        departments: [hierarchy.departments[0], hierarchy.departments[0]]
      })
    ).toThrow()
    expect(() =>
      parseWorkflowDefinition({
        ...hierarchy,
        departments: [
          { ...hierarchy.departments[0], parentId: 'review-team' },
          hierarchy.departments[1]
        ]
      })
    ).toThrow()
    expect(() =>
      parseWorkflowDefinition({
        ...hierarchy,
        nodes: [{ ...hierarchy.nodes[0], departmentId: 'missing' }]
      })
    ).toThrow()
  })

  it('allows incomplete department names and unassigned administrator drafts for readiness validation', () => {
    const definition = {
      ...hierarchy,
      departments: [{ ...hierarchy.departments[0], name: '' }],
      nodes: [{ ...hierarchy.nodes[1], departmentId: null }]
    }
    expect(parseWorkflowDefinition(definition)).toEqual(definition)
  })
})

describe('workflow authoring contract', () => {
  it('rejects removed user nodes and legacy publication guards', () => {
    expect(() =>
      parseWorkflowDefinition({
        ...fixture,
        nodes: [{ kind: 'user', id: 'user', name: 'User', task: 'Review', x: 0, y: 0 }]
      })
    ).toThrow()
    expect(() =>
      parseWorkflowRequest({
        operation: 'save',
        definition: fixture,
        expectedRevision: 1,
        expectedUsageRevision: 'old'
      })
    ).toThrow()
    expect(() =>
      parseWorkflowRequest({ operation: 'completeUserInput', instanceId: 'i', inputId: 'p' })
    ).toThrow()
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
        definition: fixture,
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
  it('edits independent organizations without looking up their original template', () => {
    const request = {
      operation: 'saveInstance',
      id: 'instance',
      name: 'Independent team',
      color: '#4A82E8',
      definition: hierarchy,
      bindings: [],
      expectedRevision: 4
    }
    expect(parseWorkflowRequest(request)).toEqual(request)
    expect(parseWorkflowRequest({ ...request, expectedRevision: 0 })).toEqual({
      ...request,
      expectedRevision: 0
    })
    expect(() => parseWorkflowRequest({ ...save, expectedRevision: 4 })).toThrow(
      'independent definition'
    )
    expect(() => parseWorkflowRequest({ ...request, definition: undefined })).toThrow()
    expect(() => parseWorkflowRequest({ ...request, templateId: 'template' })).toThrow()
  })
  it('round-trips independent instances and isolated editing drafts', () => {
    for (const request of [
      {
        operation: 'save',
        definition: fixture,
        expectedRevision: 2,
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
          definition: fixture,
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
      drafts: [{ definition: fixture, baseRevision: 2, revision: 1, updatedAt: 1 }],
      affectedConversationIds: ['chat']
    }
    expect(parseWorkflowResponse(response)).toEqual(response)
    for (const projectId of ['project', null]) {
      const withProject = { ...response, instances: [{ ...response.instances[0], projectId }] }
      expect(parseWorkflowResponse(withProject)).toEqual(withProject)
    }
    for (const activity of [
      null,
      { startedAt: 1_000, completedAt: null },
      { startedAt: 1_000, completedAt: 10_000 }
    ]) {
      const withActivity = { ...response, instances: [{ ...response.instances[0], activity }] }
      expect(parseWorkflowResponse(withActivity)).toEqual(withActivity)
    }
    for (const activity of [
      { startedAt: 1_000, completedAt: 999 },
      { startedAt: -1, completedAt: null },
      { startedAt: 1.5, completedAt: null },
      { startedAt: 1_000 },
      { startedAt: 1_000, completedAt: Infinity },
      { startedAt: 1_000, completedAt: null, running: true }
    ])
      expect(() =>
        parseWorkflowResponse({ ...response, instances: [{ ...response.instances[0], activity }] })
      ).toThrow()
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

describe('organization member name uniqueness', () => {
  it('uses the same Unicode name comparison fixtures as the host', () => {
    for (const { name, key } of memberNames) expect(workflowMemberNameKey(name)).toBe(key)
  })

  it('rejects duplicate names across departments on every definition write', () => {
    const definition = structuredClone(hierarchy)
    definition.nodes[0].name = 'Boss'
    definition.nodes[1].name = '　bOSS '
    for (const request of [
      { operation: 'save', definition, expectedRevision: 1 },
      { operation: 'saveDraft', definition, expectedRevision: 1, expectedDraftRevision: 0 },
      {
        operation: 'saveInstance',
        definition,
        id: 'instance',
        name: 'Team',
        color: '#123456',
        expectedRevision: 1,
        bindings: []
      }
    ])
      expect(() => parseWorkflowRequest(request)).toThrow('organization_duplicate_member_name')
    // Reading and inspecting old data is still possible so users can rename duplicates.
    expect(parseWorkflowDefinition(definition)).toEqual(definition)
    expect(parseWorkflowRequest({ operation: 'validate', definition })).toEqual({
      operation: 'validate',
      definition
    })
    definition.nodes[1].name = 'HR Manager'
    expect(
      parseWorkflowRequest({ operation: 'save', definition, expectedRevision: 1 })
    ).toHaveProperty('definition', definition)
  })

  it('leaves empty names under the existing incomplete-draft rule', () => {
    const definition = structuredClone(hierarchy)
    definition.nodes[0].name = ''
    definition.nodes[1].name = '　 '
    expect(() =>
      parseWorkflowRequest({ operation: 'save', definition, expectedRevision: 1 })
    ).not.toThrow()
  })
})

describe('organization department path names', () => {
  it('rejects ambiguous sibling names and path separators at every write boundary', () => {
    for (const [name, sibling, code] of [
      ['　RESEARCH ', true, 'organization_duplicate_department_name'],
      ['HR/Payroll', false, 'organization_department_name_separator']
    ] as const) {
      const definition = structuredClone(hierarchy)
      definition.departments[0].name = 'Research'
      definition.departments[1].name = name
      if (sibling) definition.departments[1].parentId = null
      for (const request of [
        { operation: 'save', definition, expectedRevision: 1 },
        { operation: 'saveDraft', definition, expectedRevision: 1, expectedDraftRevision: 0 },
        {
          operation: 'saveInstance',
          definition,
          id: 'instance',
          name: 'Team',
          color: '#123456',
          expectedRevision: 1,
          bindings: []
        }
      ])
        expect(() => parseWorkflowRequest(request)).toThrow(code)
      expect(parseWorkflowDefinition(definition)).toEqual(definition)
    }
  })

  it('allows a repeated department name at different hierarchy levels', () => {
    const definition = structuredClone(hierarchy)
    definition.departments[0].name = 'Research'
    definition.departments[1].name = 'research'
    expect(
      parseWorkflowRequest({ operation: 'save', definition, expectedRevision: 1 })
    ).toHaveProperty('definition', definition)
  })
})
