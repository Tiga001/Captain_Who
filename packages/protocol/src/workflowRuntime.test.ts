import { describe, expect, it } from 'vitest'
import {
  parseWorkflowRuntimeSnapshot,
  parseWorkflowNodeMessages,
  parseWorkflowMessageSource,
  type WorkflowRuntimeSnapshot,
  type WorkflowSourceMessage
} from './workflowRuntime'
import { parseWorkflowRequest, parseWorkflowResponse } from './workflows'
import { assertNoHumanInteractionMessageProof } from './storageHumanInteraction'

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
  it('validates queue metadata and node-scoped paginated message bodies', () => {
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
    const page = {
      instanceId: 'workflow',
      nodeId: 'reviewer',
      messages: [
        {
          sequence: 8,
          message: source('a'),
          inputId: 'input',
          status: 'pending',
          runStatus: null,
          error: null
        }
      ],
      nextBeforeSequence: 8
    }
    expect(parseWorkflowNodeMessages(page)).toEqual(page)
    expect(
      parseWorkflowResponse({ records: [], issues: [], nodeMessages: page }).nodeMessages
    ).toEqual(page)
    expect(() => parseWorkflowNodeMessages({ ...page, nodeId: 'other' })).toThrow()
    expect(() => parseWorkflowNodeMessages({ ...page, nextBeforeSequence: 9 })).toThrow()
    expect(
      parseWorkflowRequest({
        operation: 'nodeMessages',
        instanceId: 'workflow',
        nodeId: 'reviewer',
        beforeSequence: 8
      })
    ).toEqual({
      operation: 'nodeMessages',
      instanceId: 'workflow',
      nodeId: 'reviewer',
      beforeSequence: 8
    })
  })
  it('round-trips one independently delivered mail', () => {
    expect(parseWorkflowRuntimeSnapshot(snapshot)).toEqual(snapshot)
    expect(parseWorkflowResponse({ records: [], issues: [], runtime: snapshot }).runtime).toEqual(
      snapshot
    )
    for (const request of [
      { operation: 'runtimeSnapshot', instanceId: 'workflow', afterSequence: 4 },
      { operation: 'completeUserInput', instanceId: 'workflow', inputId: 'input' }
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
