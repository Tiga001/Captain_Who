import { describe, expect, it } from 'vitest'
import {
  parseWorkflowRuntimeSnapshot,
  parseWorkflowMessageSource,
  type WorkflowRuntimeSnapshot,
  type WorkflowSourceMessage
} from './workflowRuntime'
import { parseWorkflowRequest, parseWorkflowResponse } from './workflows'
import { assertNoHumanInteractionMessageProof } from './storageHumanInteraction'
import rustSummaryFixture from './fixtures/workflowRuntimeSummary.json'

const source = (node: string): WorkflowSourceMessage => ({
  id: `message-${node}`,
  instanceId: 'workflow',
  workflowName: 'Review',
  sourceNodeId: node,
  sourceNodeName: node,
  sourceConversationId: `chat-${node}`,
  sourceConversationTitle: node,
  targetNodeId: 'reviewer',
  targetNodeName: 'Reviewer',
  targetConversationId: 'chat-review',
  targetConversationTitle: 'Review',
  replyToMessageId: null,
  content: `${node} result`,
  createdAt: 10
})
const snapshot: WorkflowRuntimeSnapshot = {
  instanceId: 'workflow',
  sequence: 4,
  inputs: [
    {
      id: 'input',
      instanceId: 'workflow',
      nodeId: 'reviewer',
      conversationId: 'chat-review',
      executionVersion: 'v1',
      content: 'One delivered message',
      messages: [source('a')],
      mailStatus: 'pending',
      status: 'pending',
      runId: null,
      deliveryId: null,
      createdAt: 10,
      error: null
    }
  ],
  events: [
    {
      sequence: 4,
      instanceId: 'workflow',
      inputId: 'input',
      messageId: 'message-a',
      sourceNodeId: 'a',
      targetNodeId: 'reviewer',
      kind: 'delivered',
      createdAt: 10
    }
  ]
}
describe('workflow runtime boundary', () => {
  it('accepts the exact summary serialized by the Rust storage projection fixture', () => {
    expect(parseWorkflowRuntimeSnapshot(rustSummaryFixture)).toEqual(rustSummaryFixture)
    expect(
      parseWorkflowResponse({ records: [], issues: [], runtime: rustSummaryFixture }).runtime
    ).toEqual(rustSummaryFixture)
  })
  it('accepts complete runtime summaries without silently limiting historical conversations', () => {
    const compact = {
      ...snapshot,
      inputs: [],
      events: [],
      summary: {
        pendingByNode: [{ nodeId: 'reviewer', count: 7 }],
        conversationChanges: Array.from({ length: 5000 }, (_, index) => ({
          conversationId: `old-chat-${index}`,
          sequence: 3
        })),
        structureRevision: 2
      }
    }
    expect(parseWorkflowRuntimeSnapshot(compact)).toEqual(compact)
    for (const summary of [
      { ...compact.summary, structureRevision: 5 },
      { ...compact.summary, extra: true },
      { ...compact.summary, pendingByNode: [{ nodeId: 'reviewer', count: -1 }] },
      {
        ...compact.summary,
        pendingByNode: [
          { nodeId: 'reviewer', count: 1 },
          { nodeId: 'reviewer', count: 2 }
        ]
      },
      { ...compact.summary, conversationChanges: [{ conversationId: 'chat', sequence: 5 }] },
      {
        ...compact.summary,
        conversationChanges: [
          { conversationId: 'chat', sequence: 1 },
          { conversationId: 'chat', sequence: 2 }
        ]
      }
    ])
      expect(() => parseWorkflowRuntimeSnapshot({ ...compact, summary })).toThrow()
    expect(() => parseWorkflowRuntimeSnapshot({ ...compact, inputs: snapshot.inputs })).toThrow()
    expect(
      parseWorkflowRequest({
        operation: 'runtimeSnapshot',
        instanceId: 'workflow',
        summaryOnly: true
      })
    ).toEqual({
      operation: 'runtimeSnapshot',
      instanceId: 'workflow',
      summaryOnly: true
    })
    expect(() =>
      parseWorkflowRequest({
        operation: 'runtimeSnapshot',
        instanceId: 'workflow',
        summaryOnly: 'true'
      })
    ).toThrow()
  })
  it('accepts only bounded, field-specific fresh organization preference notifications', () => {
    const update = {
      nodeId: 'reviewer',
      conversationId: 'chat-review',
      organizationRevision: 2,
      modelId: 'model-next',
      permissionMode: 'default'
    }
    const notification = { ...snapshot, preferenceUpdates: [update] }
    expect(parseWorkflowRuntimeSnapshot(notification)).toEqual(notification)
    expect(parseWorkflowRuntimeSnapshot(snapshot)).not.toHaveProperty('preferenceUpdates')
    for (const bad of [
      { ...update, organizationRevision: 0 },
      { ...update, organizationRevision: 1.5 },
      { ...update, permissionMode: 'unrestricted' },
      { ...update, modelId: null },
      { ...update, nodeId: '' },
      { ...update, conversationId: '' },
      { ...update, modelId: undefined, permissionMode: undefined },
      { ...update, avatar: 'not-editable' }
    ])
      expect(() =>
        parseWorkflowRuntimeSnapshot({ ...snapshot, preferenceUpdates: [bad] })
      ).toThrow()
    expect(() =>
      parseWorkflowRuntimeSnapshot({ ...snapshot, preferenceUpdates: [update, update] })
    ).toThrow()
  })
  it('preserves independent preference revisions without accepting repeated fields or mixed bindings', () => {
    const model = {
      nodeId: 'reviewer',
      conversationId: 'chat-review',
      organizationRevision: 3,
      modelId: 'model-b'
    }
    const permission = {
      nodeId: 'reviewer',
      conversationId: 'chat-review',
      organizationRevision: 4,
      permissionMode: 'full'
    }
    const notification = { ...snapshot, preferenceUpdates: [model, permission] }
    expect(parseWorkflowRuntimeSnapshot(notification)).toEqual(notification)
    for (const updates of [
      [model, { ...model, organizationRevision: 4 }],
      [permission, { ...permission, organizationRevision: 5 }],
      [model, { ...permission, conversationId: 'other-chat' }],
      [model, { ...permission, modelId: 'overlap' }]
    ])
      expect(() =>
        parseWorkflowRuntimeSnapshot({ ...snapshot, preferenceUpdates: updates })
      ).toThrow()
  })
  it('accepts two fields for all 128 members while keeping the member and item bounds', () => {
    const updates = Array.from({ length: 128 }, (_, index) => [
      {
        nodeId: `member-${index}`,
        conversationId: `chat-${index}`,
        organizationRevision: 2,
        modelId: 'model-a'
      },
      {
        nodeId: `member-${index}`,
        conversationId: `chat-${index}`,
        organizationRevision: 4,
        permissionMode: 'full'
      }
    ]).flat()
    expect(
      parseWorkflowRuntimeSnapshot({ ...snapshot, preferenceUpdates: updates }).preferenceUpdates
    ).toEqual(updates)
    expect(() =>
      parseWorkflowRuntimeSnapshot({
        ...snapshot,
        preferenceUpdates: [...updates, { ...updates[0], nodeId: 'extra' }]
      })
    ).toThrow()
    expect(() =>
      parseWorkflowRuntimeSnapshot({
        ...snapshot,
        preferenceUpdates: [
          ...updates.filter((_, index) => index % 2 === 0),
          { ...updates[0], nodeId: 'extra' }
        ]
      })
    ).toThrow()
  })
  it('validates queue metadata', () => {
    const metadata = {
      ...snapshot,
      pausedConversationIds: ['chat-review'],
      inputRuns: [{ inputId: 'input', status: 'cancelled' }]
    }
    expect(parseWorkflowRuntimeSnapshot(metadata)).toEqual(metadata)
    expect(() =>
      parseWorkflowRuntimeSnapshot({
        ...metadata,
        pendingMessages: [{ ...source('a'), instanceId: 'other' }]
      })
    ).toThrow()
  })
  it('validates every run reference and duplicate input identity in a large projection', () => {
    const inputs = Array.from({ length: 1024 }, (_, index) => ({
      ...snapshot.inputs[0],
      id: `input-${index}`
    }))
    const inputRuns = inputs.map((input) => ({ inputId: input.id, status: 'in_progress' }))
    const large = { ...snapshot, inputs, inputRuns, events: [] }
    expect(parseWorkflowRuntimeSnapshot(large).inputRuns).toHaveLength(1024)
    expect(() =>
      parseWorkflowRuntimeSnapshot({
        ...large,
        inputRuns: [...inputRuns.slice(0, -1), { inputId: 'missing', status: 'completed' }]
      })
    ).toThrow()
    expect(() =>
      parseWorkflowRuntimeSnapshot({ ...large, inputs: [...inputs, inputs[0]] })
    ).toThrow()
  })
  it('round-trips one independently delivered mail', () => {
    expect(parseWorkflowRuntimeSnapshot(snapshot)).toEqual(snapshot)
    expect(parseWorkflowResponse({ records: [], issues: [], runtime: snapshot }).runtime).toEqual(
      snapshot
    )
    for (const request of [
      { operation: 'runtimeSnapshot', instanceId: 'workflow', afterSequence: 4 }
    ])
      expect(parseWorkflowRequest(request)).toEqual(request)
  })
  it('rejects mixed identities, forged statuses, duplicate cursors and incomplete source facts', () => {
    for (const bad of [
      { ...snapshot, instanceId: 'other' },
      { ...snapshot, sequence: 3 },
      { ...snapshot, events: [...snapshot.events, ...snapshot.events] },
      { ...snapshot, inputs: [...snapshot.inputs, ...snapshot.inputs] },
      { ...snapshot, inputs: [{ ...snapshot.inputs[0], status: 'running' }] },
      { ...snapshot, inputs: [{ ...snapshot.inputs[0], nodeId: 'other' }] },
      { ...snapshot, inputs: [{ ...snapshot.inputs[0], messages: [] }] },
      { ...snapshot, inputs: [{ ...snapshot.inputs[0], messages: [source('a'), source('b')] }] },
      { ...snapshot, inputs: [{ ...snapshot.inputs[0], mailStatus: 'applied' }] }
    ])
      expect(() => parseWorkflowRuntimeSnapshot(bad)).toThrow()
  })
  it('accepts a multi-source display label but never writable workflow provenance', () => {
    const proof = {
      inputId: 'input',
      instanceId: 'workflow',
      workflowName: 'Review',
      sources: snapshot.inputs[0].messages.map((message) => ({
        nodeId: message.sourceNodeId,
        nodeName: message.sourceNodeName,
        conversationId: message.sourceConversationId,
        conversationTitle: message.sourceConversationTitle
      }))
    }
    expect(parseWorkflowMessageSource(proof).sources).toHaveLength(1)
    const withBodies = {
      ...proof,
      sources: proof.sources.map((source, index) => ({
        ...source,
        content: snapshot.inputs[0].messages[index].content
      }))
    }
    expect(parseWorkflowMessageSource(withBodies)).toEqual(withBodies)
    for (const content of [null, 42, { text: 'forged' }])
      expect(() =>
        parseWorkflowMessageSource({
          ...proof,
          sources: [{ ...proof.sources[0], content }]
        })
      ).toThrow()
    expect(() => parseWorkflowMessageSource({ ...proof, sources: [] })).toThrow()
    for (const key of ['workflowInput', 'workflowSource'])
      for (const value of [proof, withBodies, null])
        expect(() => assertNoHumanInteractionMessageProof({ [key]: value })).toThrow('read-only')
  })
})
