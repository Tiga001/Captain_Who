import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import {
  parseAgentCollaborationSettings,
  parseAgentCollaborationSettingsGetInput,
  parseAgentCollaborationSettingsUpdate,
  AGENT_COLLABORATION_GET_SETTINGS_METHOD,
  AGENT_COLLABORATION_UPDATE_SETTINGS_METHOD,
  AGENT_COLLABORATION_SETTINGS_CHANGED_METHOD,
  AGENT_COLLABORATION_APPROVALS_DECIDE_METHOD,
  AGENT_COLLABORATION_APPROVALS_LIST_METHOD,
  AGENT_COLLABORATION_GET_AGENT_METHOD,
  AGENT_COLLABORATION_GET_TREE_METHOD,
  AGENT_COLLABORATION_LIST_EVENTS_METHOD,
  AGENT_COLLABORATION_LOAD_OBSERVER_CONVERSATION_METHOD,
  AGENT_COLLABORATION_LOCATE_CONVERSATION_METHOD,
  AGENT_COLLABORATION_EVENT_NOTIFICATION_METHOD,
  AGENT_COLLABORATION_OBSERVER_EVENT_NOTIFICATION_METHOD,
  AGENT_COLLABORATION_RESYNC_NOTIFICATION_METHOD,
  AGENT_COLLABORATION_TEMPLATES_CREATE_METHOD,
  AGENT_COLLABORATION_TEMPLATES_DELETE_METHOD,
  AGENT_COLLABORATION_TEMPLATES_LIST_METHOD,
  AGENT_COLLABORATION_TEMPLATES_SET_ENABLED_METHOD,
  AGENT_COLLABORATION_TEMPLATES_SET_PROJECT_ASSIGNMENT_METHOD,
  AGENT_COLLABORATION_TEMPLATES_UPDATE_METHOD,
  parseAgentTemplateCreateRequest,
  parseAgentTemplateProjectAssignmentRequest,
  parseAgentTemplate,
  parseAgentTemplateList,
  parseAgentDetail,
  parseAgentConversationLocator,
  parseAgentObserverConversation,
  parseAgentObserverEventEnvelope,
  parseAgentTreeLookup,
  parseAgentTreeSnapshot,
  parseCollaborationEventEnvelope,
  parseCollaborationEventsPage,
  parseCollaborationApprovalList,
  parseCollaborationApprovalDecisionResult,
  parseCollaborationResyncEnvelope
} from './agentCollaboration'

const fixture = JSON.parse(
  readFileSync(
    fileURLToPath(new URL('../fixtures/agent-collaboration-contract-v1.json', import.meta.url)),
    'utf8'
  )
) as {
  methods: string[]
  notifications: { event: string; observerEvent: string; resync: string; settingsChanged: string }
  settings: unknown
  tree: unknown
  detail: unknown
  locator: unknown
  template: unknown
  templateList: unknown
  approvalList: unknown
  approvalDecision: unknown
  resync: unknown
  event: unknown
  observer: unknown
  observerEvent: unknown
}

const round5Scenario = JSON.parse(
  readFileSync(
    fileURLToPath(
      new URL('../fixtures/agent-collaboration-round5-scenario-v1.json', import.meta.url)
    ),
    'utf8'
  )
) as {
  schemaVersion: number
  scenario: string
  harness: { toolCalls: string[]; minimumParallelChildren: number }
  runningTree: unknown
  settledTree: unknown
  settledEventPage: unknown
  observerConversations: unknown[]
  approvalList: unknown
  approvalDecisions: { approve: unknown; reject: unknown }
}

describe('agent collaboration protocol', () => {
  describe('confirmed transmission metadata', () => {
    const baseEvent = {
      ...(fixture.event as object),
      activities: [],
      kind: 'mailbox_enqueued',
      agentId: 'agent-child',
      conversationId: 'conversation-child',
      transmission: {
        id: 'transfer-stable-id',
        kind: 'message',
        sourceAgentId: 'agent-root',
        targetAgentId: 'agent-child'
      }
    }

    it('keeps transmission optional without changing the event schema', () => {
      const event = parseCollaborationEventEnvelope(fixture.event)
      expect(event.schemaVersion).toBe(3)
      expect(event).not.toHaveProperty('transmission')
    })

    it.each(['message', 'task'])(
      'accepts a %s transfer with its own stable identity, independent of activity',
      (kind) => {
        const transmission = { ...baseEvent.transmission, kind }
        const event = parseCollaborationEventEnvelope({ ...baseEvent, transmission })
        expect(event.transmission).toEqual(transmission)
        expect(event.transmission?.id).not.toBe(event.eventId)
        expect(event.transmission?.id).not.toBe(event.messageId)
        expect(event.activities).toEqual([])
      }
    )

    it('accepts a root input transfer whose identity belongs to the human input message', () => {
      const transmission = {
        id: 'user-input-message',
        kind: 'user_message',
        sourceAgentId: null,
        targetAgentId: 'agent-root'
      }
      const event = parseCollaborationEventEnvelope({
        ...baseEvent,
        kind: 'turn_started',
        agentId: 'agent-root',
        conversationId: 'conversation-root',
        messageId: 'assistant-message',
        transmission
      })
      expect(event.transmission).toEqual(transmission)
      expect(event.transmission?.id).not.toBe(event.messageId)
    })

    it('accepts the root completion transfer to the human user', () => {
      const transmission = {
        id: 'completed-root-run',
        kind: 'completion',
        sourceAgentId: 'agent-root',
        targetAgentId: null
      }
      expect(
        parseCollaborationEventEnvelope({
          ...baseEvent,
          kind: 'turn_updated',
          agentId: 'agent-root',
          conversationId: 'conversation-root',
          transmission
        }).transmission
      ).toEqual(transmission)
    })

    it.each([
      { id: '' },
      { id: ' trailing ' },
      { id: 'x'.repeat(513) },
      { id: 'transfer\0id' },
      { kind: 'mailbox_updated' },
      { sourceAgentId: '' },
      { targetAgentId: '' },
      { sourceAgentId: 'agent\0root' },
      { targetAgentId: 'agent\0child' },
      { sourceAgentId: null },
      { targetAgentId: null },
      { sourceAgentId: null, targetAgentId: null },
      { sourceAgentId: 'agent-child' },
      { targetAgentId: 'agent-other' },
      { body: 'must not transmit message text' }
    ])('rejects malformed or incorrectly routed message metadata: %j', (patch) => {
      expect(() =>
        parseCollaborationEventEnvelope({
          ...baseEvent,
          transmission: { ...baseEvent.transmission, ...patch }
        })
      ).toThrow(/Collaboration.*[Tt]ransmission/)
    })

    it.each(['id', 'kind', 'sourceAgentId', 'targetAgentId'])(
      'requires the transmission field %s when metadata is present',
      (field) => {
        const transmission: Record<string, unknown> = { ...baseEvent.transmission }
        delete transmission[field]
        expect(() => parseCollaborationEventEnvelope({ ...baseEvent, transmission })).toThrow(
          new RegExp(`Missing.*${field}`)
        )
      }
    )

    it.each([null, undefined, [], 'transfer'])(
      'rejects a present transmission that is not an object: %j',
      (transmission) => {
        expect(() => parseCollaborationEventEnvelope({ ...baseEvent, transmission })).toThrow(
          /Invalid CollaborationTransmission/
        )
      }
    )

    it.each([
      { kind: 'mailbox_updated' },
      { kind: 'turn_started' },
      { kind: 'turn_updated' },
      { agentId: 'agent-root' }
    ])('rejects a message transfer attached to the wrong event: %j', (patch) => {
      expect(() => parseCollaborationEventEnvelope({ ...baseEvent, ...patch })).toThrow(
        /transmission identity/
      )
    })

    it.each([
      { eventKind: 'turn_updated', kind: 'user_message', source: null, target: 'agent-root' },
      {
        eventKind: 'turn_started',
        kind: 'user_message',
        source: 'agent-child',
        target: 'agent-root'
      },
      { eventKind: 'turn_started', kind: 'user_message', source: null, target: 'agent-child' },
      { eventKind: 'turn_started', kind: 'completion', source: 'agent-root', target: null },
      { eventKind: 'turn_updated', kind: 'completion', source: 'agent-child', target: null },
      { eventKind: 'turn_updated', kind: 'completion', source: 'agent-root', target: 'agent-child' }
    ])('rejects mismatched human/root transfers: %j', ({ eventKind, kind, source, target }) => {
      expect(() =>
        parseCollaborationEventEnvelope({
          ...baseEvent,
          agentId: 'agent-root',
          conversationId: 'conversation-root',
          kind: eventKind,
          transmission: {
            id: 'transfer-id',
            kind,
            sourceAgentId: source,
            targetAgentId: target
          }
        })
      ).toThrow(/transmission identity/)
    })

    it.each(['user_message', 'completion'])('rejects a %s transfer on a child event', (kind) => {
      expect(() =>
        parseCollaborationEventEnvelope({
          ...baseEvent,
          kind: kind === 'user_message' ? 'turn_started' : 'turn_updated',
          transmission: {
            id: 'transfer-id',
            kind,
            sourceAgentId: kind === 'user_message' ? null : 'agent-root',
            targetAgentId: kind === 'user_message' ? 'agent-root' : null
          }
        })
      ).toThrow(/transmission identity/)
    })

    it.each(['user_message', 'completion'])(
      'rejects a %s root transfer attached to a child conversation',
      (kind) => {
        expect(() =>
          parseCollaborationEventEnvelope({
            ...baseEvent,
            agentId: 'agent-root',
            kind: kind === 'user_message' ? 'turn_started' : 'turn_updated',
            transmission: {
              id: 'transfer-id',
              kind,
              sourceAgentId: kind === 'user_message' ? null : 'agent-root',
              targetAgentId: kind === 'user_message' ? 'agent-root' : null
            }
          })
        ).toThrow(/transmission identity/)
      }
    )
  })

  it('binds a complete provisional stream and its safe cursor to the exact observer message', () => {
    const observer = structuredClone(fixture.observer) as {
      messages: Array<{ messageId: string; role: string; agentRunJson: string | null }>
      liveStream?: unknown
    }
    const assistant = {
      ...observer.messages[0]!,
      messageId: 'assistant-live',
      role: 'assistant',
      inputOrigin: null
    }
    assistant.agentRunJson = JSON.stringify({ runId: 'live-run' })
    observer.messages.push(assistant)
    const liveStream = {
      runId: 'live-run',
      assistantMessageId: assistant.messageId,
      cursor: { generation: 'live-generation', sequence: 7 },
      stream: {
        streamId: 'stream-1',
        attempt: 1,
        content: '完整前缀'.repeat(2_000),
        traceBoundarySequence: 3,
        committed: false
      }
    }
    observer.liveStream = liveStream
    expect(parseAgentObserverConversation(observer)?.liveStream).toEqual(liveStream)
    for (const sequence of [0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1]) {
      observer.liveStream = { ...liveStream, cursor: { ...liveStream.cursor, sequence } }
      expect(() => parseAgentObserverConversation(observer)).toThrow()
      expect(() =>
        parseAgentObserverEventEnvelope({
          ...(fixture.observerEvent as Record<string, unknown>),
          streamCursor: { generation: 'live-generation', sequence }
        })
      ).toThrow()
    }
    observer.liveStream = { ...liveStream, assistantMessageId: 'foreign-message' }
    expect(() => parseAgentObserverConversation(observer)).toThrow(/message identity/)
    observer.liveStream = {
      ...liveStream,
      stream: { ...liveStream.stream, traceBoundarySequence: -1 }
    }
    expect(() => parseAgentObserverConversation(observer)).toThrow()
    expect(
      parseAgentObserverEventEnvelope({
        ...(fixture.observerEvent as Record<string, unknown>),
        streamCursor: liveStream.cursor
      }).streamCursor
    ).toEqual(liveStream.cursor)
  })

  it('preserves the exact browser Tool call identity for projected risk approvals', () => {
    const callId = `tc1_${'a'.repeat(43)}`
    const actionId = '11111111-1111-4111-8111-111111111111'
    const action = {
      type: 'browser_risk_approval',
      approval: {
        schemaVersion: 1,
        actionId,
        riskApprovalId: '22222222-2222-4222-8222-222222222222',
        runId: 'run-browser-risk',
        callId,
        triggerToolName: 'browser_navigate',
        capabilityId: 'browser_automation',
        capabilityActivationId: '33333333-3333-4333-8333-333333333333',
        displayName: 'Browser automation',
        reason: 'Open the local fixture',
        destination: {
          normalizedUrl: 'http://127.0.0.1:3000/fixture',
          origin: 'http://127.0.0.1:3000',
          scheme: 'http',
          asciiHost: '127.0.0.1',
          effectivePort: 3000,
          addressClass: 'loopback'
        },
        trigger: 'tool_argument',
        riskKinds: ['insecure_http', 'loopback', 'non_standard_port'],
        manifestDigest: `sha256:${'b'.repeat(64)}`,
        policyRevision: 1,
        createdAt: 1_753_843_200,
        expiresAt: 1_753_844_100,
        approvalStatus: 'required'
      }
    }
    const projection = {
      schemaVersion: 1,
      approvalId: 'approval-browser-risk',
      rootAgentId: 'agent-root',
      rootConversationId: 'conversation-root',
      sourceAgentId: 'agent-child',
      sourceTaskPath: '/root/browser',
      sourceConversationId: 'conversation-child',
      runId: 'run-browser-risk',
      actionId,
      actionType: 'browser_risk_approval',
      toolName: 'browser_navigate',
      action,
      status: 'pending',
      createdAt: 1_753_843_200_000,
      updatedAt: 1_753_843_200_000
    }

    expect(
      parseCollaborationApprovalList({ schemaVersion: 1, approvals: [projection] }).approvals[0]
        ?.action
    ).toEqual(action)
    expect(() =>
      parseCollaborationApprovalList({
        schemaVersion: 1,
        approvals: [{ ...projection, toolName: 'browser_click' }]
      })
    ).toThrow()
  })

  it('strictly parses the shared Round 5 Runtime-to-AppShell scenario', () => {
    const running = parseAgentTreeSnapshot(round5Scenario.runningTree)
    const settled = parseAgentTreeSnapshot(round5Scenario.settledTree)
    const eventPage = parseCollaborationEventsPage(round5Scenario.settledEventPage)
    const observers = round5Scenario.observerConversations.map(parseAgentObserverConversation)
    const approvals = parseCollaborationApprovalList(round5Scenario.approvalList)

    expect(round5Scenario).toMatchObject({
      schemaVersion: 1,
      scenario: 'deterministic_six_tool_roundtrip',
      harness: { minimumParallelChildren: 2 }
    })
    expect(round5Scenario.harness.toolCalls).toEqual([
      'spawn_agent',
      'spawn_agent',
      'send_message',
      'followup_task',
      'list_agents',
      'wait_agent',
      'wait_agent',
      'interrupt_agent'
    ])
    expect(running.agents.map((agent) => agent.model?.modelConfigId)).toEqual([
      'model-1',
      'model-1',
      'model-2'
    ])
    expect(settled.lastSequence).toBe(eventPage.lastSequence)
    expect(eventPage.events).toHaveLength(15)
    expect(eventPage.events.map((event) => event.sequence)).toEqual(
      Array.from({ length: 15 }, (_, index) => index + 1)
    )
    expect(eventPage.events.at(-1)).toMatchObject({
      agentId: 'agent-review',
      runId: 'run-review',
      sequence: 15,
      activities: [
        {
          schemaVersion: 4,
          semantic: 'completed',
          ownerAgentId: 'agent-root',
          ownerConversationId: 'conversation-root'
        }
      ]
    })
    expect(
      eventPage.events.flatMap((event) =>
        event.activities.map((activity) => [
          event.sequence,
          event.kind,
          activity.agentId,
          activity.semantic
        ])
      )
    ).toEqual([
      [4, 'wake_created', 'agent-review', 'started'],
      [7, 'wake_created', 'agent-compatibility', 'started'],
      [9, 'mailbox_enqueued', 'agent-review', 'updated'],
      [11, 'wake_created', 'agent-review', 'started'],
      [12, 'approval_projected', 'agent-compatibility', 'waiting_approval'],
      [13, 'approval_projected', 'agent-review', 'waiting_approval'],
      [14, 'wake_updated', 'agent-compatibility', 'interrupted'],
      [15, 'wake_updated', 'agent-review', 'completed']
    ])
    expect(
      eventPage.events.flatMap((event) =>
        event.activities.map((activity) => [
          event.sequence,
          activity.anchorMessageId,
          activity.traceBoundarySequence
        ])
      )
    ).toEqual([
      [4, 'assistant-conversation-root', 1],
      [7, 'assistant-conversation-root', 1],
      [9, 'assistant-conversation-root', 1],
      [11, 'assistant-conversation-root', 1],
      [12, 'assistant-conversation-root', null],
      [13, 'assistant-conversation-root', null],
      [14, 'assistant-conversation-root', null],
      [15, 'assistant-conversation-root', null]
    ])
    expect(eventPage.events.find((event) => event.sequence === 8)).toMatchObject({
      kind: 'mailbox_enqueued',
      messageId: 'mailbox-send-review',
      activities: []
    })
    expect(eventPage.events.find((event) => event.sequence === 10)).toMatchObject({
      kind: 'mailbox_enqueued',
      messageId: 'mailbox-followup-review',
      activities: []
    })
    expect(observers.map((observer) => observer?.conversationId)).toEqual([
      'conversation-review',
      'conversation-compatibility'
    ])
    expect(approvals.approvals.map((approval) => approval.approvalId)).toEqual([
      'approval-approve',
      'approval-reject'
    ])
    expect(
      parseCollaborationApprovalDecisionResult(round5Scenario.approvalDecisions.approve)
    ).toMatchObject({ approvalId: 'approval-approve', status: 'approved' })
    expect(
      parseCollaborationApprovalDecisionResult(round5Scenario.approvalDecisions.reject)
    ).toMatchObject({ approvalId: 'approval-reject', status: 'rejected' })
  })

  it('accepts dispatcher-local placement, including a root dispatch to a grandchild', () => {
    const event = parseCollaborationEventEnvelope(fixture.event)
    const activity = event.activities[0]!
    for (const owner of [
      { ownerAgentId: 'agent-root', ownerConversationId: 'conversation-root' },
      { ownerAgentId: 'agent-child', ownerConversationId: 'conversation-child' }
    ]) {
      for (const placement of [
        { anchorMessageId: 'assistant-owner', traceBoundarySequence: 4 },
        { anchorMessageId: 'assistant-owner', traceBoundarySequence: null },
        { anchorMessageId: null, traceBoundarySequence: null }
      ]) {
        expect(
          parseCollaborationEventEnvelope({
            ...event,
            agentId: 'agent-grandchild',
            conversationId: 'conversation-grandchild',
            activities: [{ ...activity, agentId: 'agent-grandchild', ...owner, ...placement }]
          }).activities[0]
        ).toMatchObject({ ...owner, ...placement })
      }
    }
  })

  it('routes ordinary Message updates to the actual recipient, independent of tree direction', () => {
    const event = parseCollaborationEventEnvelope(fixture.event)
    for (const sender of ['agent-root', 'agent-grandchild', 'agent-sibling']) {
      const update = {
        ...event,
        kind: 'mailbox_enqueued',
        agentId: 'agent-child',
        conversationId: 'conversation-child',
        activities: [
          {
            ...event.activities[0]!,
            semantic: 'updated',
            agentId: sender,
            ownerAgentId: 'agent-child',
            ownerConversationId: 'conversation-child',
            taskMessageId: null
          }
        ]
      }
      expect(parseCollaborationEventEnvelope(update).activities[0]).toMatchObject({
        agentId: sender,
        ownerAgentId: 'agent-child',
        taskMessageId: null
      })
      expect(() =>
        parseCollaborationEventEnvelope({
          ...update,
          activities: [
            {
              ...update.activities[0],
              ownerAgentId: 'agent-root',
              ownerConversationId: 'conversation-root'
            }
          ]
        })
      ).toThrow(/owner identity/)
    }
  })

  it('preserves separate task identities and multiple owners in one execution event', () => {
    const event = parseCollaborationEventEnvelope(fixture.event)
    const activity = { ...event.activities[0]!, agentId: 'agent-grandchild', semantic: 'completed' }
    const activities = [
      { ...activity, activityId: 'completed:root-task', taskMessageId: 'root-task' },
      { ...activity, activityId: 'completed:root-followup', taskMessageId: 'root-followup' },
      {
        ...activity,
        activityId: 'completed:child-task',
        taskMessageId: 'child-task',
        ownerAgentId: 'agent-child',
        ownerConversationId: 'conversation-child'
      }
    ]
    const terminal = {
      ...event,
      kind: 'wake_updated',
      agentId: 'agent-grandchild',
      conversationId: 'conversation-grandchild',
      activities
    }
    expect(parseCollaborationEventEnvelope(terminal).activities).toEqual(activities)
    expect(() =>
      parseCollaborationEventEnvelope({ ...terminal, activities: [activities[0], activities[0]] })
    ).toThrow(/duplicate activity identity/)
    expect(() =>
      parseCollaborationEventEnvelope({
        ...terminal,
        activities: [{ ...activities[0], taskMessageId: null }]
      })
    ).toThrow(/task identity/)
  })

  it.each([
    { schemaVersion: 3 },
    { activityId: '' },
    { activityId: 'x'.repeat(2049) },
    { activityId: 'activity\0id' },
    { ownerConversationId: 'x'.repeat(257) },
    { taskMessageId: 'x'.repeat(2049) },
    { ownerAgentId: null },
    { ownerAgentId: '' },
    { ownerAgentId: ' agent-root' },
    { ownerConversationId: '' },
    { ownerAgentId: 'agent-child' },
    { ownerConversationId: 'conversation-child' },
    { ownerAgentId: 'agent-other' },
    { taskMessageId: null },
    { anchorMessageId: null, traceBoundarySequence: 2 },
    { anchorMessageId: 'assistant-owner', traceBoundarySequence: -1 },
    { parentAgentId: 'agent-root' },
    { parentConversationId: 'conversation-root' }
  ])('rejects malformed or legacy owner activity contracts: %j', (invalidActivity) => {
    const event = parseCollaborationEventEnvelope(fixture.event)
    expect(() =>
      parseCollaborationEventEnvelope({
        ...event,
        activities: [{ ...event.activities[0]!, ...invalidActivity }]
      })
    ).toThrow()
  })

  it.each([
    'activityId',
    'ownerAgentId',
    'ownerConversationId',
    'taskMessageId',
    'anchorMessageId',
    'traceBoundarySequence'
  ])('requires the activity identity/placement field %s', (field) => {
    const event = structuredClone(fixture.event) as { activities: Record<string, unknown>[] }
    delete event.activities[0]![field]
    expect(() => parseCollaborationEventEnvelope(event)).toThrow(new RegExp(`Missing.*${field}`))
  })

  it('keeps the Rust/TypeScript method and DTO fixture stable', () => {
    expect(fixture.methods).toEqual([
      AGENT_COLLABORATION_GET_SETTINGS_METHOD,
      AGENT_COLLABORATION_UPDATE_SETTINGS_METHOD,
      AGENT_COLLABORATION_GET_TREE_METHOD,
      AGENT_COLLABORATION_GET_AGENT_METHOD,
      AGENT_COLLABORATION_LOCATE_CONVERSATION_METHOD,
      AGENT_COLLABORATION_LOAD_OBSERVER_CONVERSATION_METHOD,
      AGENT_COLLABORATION_LIST_EVENTS_METHOD,
      AGENT_COLLABORATION_TEMPLATES_LIST_METHOD,
      AGENT_COLLABORATION_TEMPLATES_CREATE_METHOD,
      AGENT_COLLABORATION_TEMPLATES_UPDATE_METHOD,
      AGENT_COLLABORATION_TEMPLATES_SET_ENABLED_METHOD,
      AGENT_COLLABORATION_TEMPLATES_SET_PROJECT_ASSIGNMENT_METHOD,
      AGENT_COLLABORATION_TEMPLATES_DELETE_METHOD,
      AGENT_COLLABORATION_APPROVALS_LIST_METHOD,
      AGENT_COLLABORATION_APPROVALS_DECIDE_METHOD
    ])
    expect(fixture.notifications).toEqual({
      event: AGENT_COLLABORATION_EVENT_NOTIFICATION_METHOD,
      observerEvent: AGENT_COLLABORATION_OBSERVER_EVENT_NOTIFICATION_METHOD,
      resync: AGENT_COLLABORATION_RESYNC_NOTIFICATION_METHOD,
      settingsChanged: AGENT_COLLABORATION_SETTINGS_CHANGED_METHOD
    })
    expect(parseAgentObserverEventEnvelope(fixture.observerEvent)).toMatchObject({
      agentId: 'agent-child',
      runId: 'run-child',
      event: { type: 'message_delta' }
    })
    const tree = parseAgentTreeSnapshot(fixture.tree)
    expect(tree.lastSequence).toBe(7)
    expect(
      parseAgentTreeLookup({ schemaVersion: 1, materialized: true, tree: fixture.tree }).tree
        ?.rootConversationId
    ).toBe('conversation-root')
    expect(parseCollaborationEventEnvelope(fixture.event)).toMatchObject({
      sequence: 7,
      kind: 'wake_created',
      activities: [
        {
          semantic: 'started',
          agentId: 'agent-child',
          taskNameSnapshot: 'Research'
        }
      ],
      rootConversationId: 'conversation-root'
    })
    expect(
      parseAgentObserverConversation(fixture.observer)?.messages[0]?.inputOrigin
    ).toMatchObject({
      kind: 'agent',
      senderAgentId: 'agent-root',
      sourceAgentMessageId: 'mailbox-task'
    })
    const detail = parseAgentDetail(fixture.detail)
    const locator = parseAgentConversationLocator(fixture.locator)
    const template = parseAgentTemplate(fixture.template)
    const templateList = parseAgentTemplateList(fixture.templateList)
    const approvals = parseCollaborationApprovalList(fixture.approvalList)
    const decision = parseCollaborationApprovalDecisionResult(fixture.approvalDecision)
    expect(detail.summary).toMatchObject({
      rootAgentId: tree.rootAgentId,
      rootConversationId: tree.rootConversationId,
      conversationId: locator.conversationId
    })
    expect(locator).toMatchObject({ agentId: detail.summary.agentId, mode: 'observer' })
    expect(templateList.templates).toEqual([template])
    expect(template.projectIds).toEqual([tree.projectId])
    expect(detail.template?.templateId).toBe(template.templateId)
    expect(approvals.approvals[0]).toMatchObject({
      rootAgentId: tree.rootAgentId,
      rootConversationId: tree.rootConversationId,
      sourceAgentId: detail.summary.agentId,
      sourceConversationId: detail.summary.conversationId
    })
    expect(decision).toMatchObject({
      approvalId: approvals.approvals[0]?.approvalId,
      accepted: true,
      status: 'approved'
    })
    expect(parseCollaborationResyncEnvelope(fixture.resync)).toEqual({
      schemaVersion: 1,
      reason: 'core_started'
    })
  })

  it('rejects unknown fields and private Provider-shaped template fields', () => {
    expect(() => parseAgentTreeSnapshot({ ...(fixture.tree as object), secret: 'token' })).toThrow()
    expect(() =>
      parseAgentTemplateCreateRequest({
        templateId: 'template-1',
        machineKey: 'research',
        name: 'Research',
        description: '',
        instructions: '',
        modelConfigId: 'model-safe-id',
        enabled: true,
        apiKey: 'must-never-cross-the-boundary'
      })
    ).toThrow()
    expect(() =>
      parseAgentTemplateProjectAssignmentRequest({
        projectId: 'project-1',
        templateId: 'template-1',
        assigned: true,
        enabled: true
      })
    ).toThrow()
    expect(() =>
      parseAgentTemplate({
        ...(fixture.template as object),
        projectIds: ['project-b', 'project-a']
      })
    ).toThrow()
    expect(() =>
      parseAgentTemplate({
        ...(fixture.template as object),
        projectIds: ['project-a', 'project-a']
      })
    ).toThrow()
  })

  it('strictly parses the identity-rich observer event and rejects forged nested payloads', () => {
    const envelope = {
      schemaVersion: 1,
      rootAgentId: 'agent-root',
      rootConversationId: 'conversation-root',
      agentId: 'agent-child',
      conversationId: 'conversation-child',
      runId: 'run-child',
      assistantMessageId: 'assistant-child',
      event: { type: 'message_delta', runId: 'run-child', streamId: 'stream-1', delta: 'hello' }
    }
    expect(AGENT_COLLABORATION_OBSERVER_EVENT_NOTIFICATION_METHOD).toBe(
      'agent.collaboration.observerEvent'
    )
    expect(parseAgentObserverEventEnvelope(envelope)).toEqual(envelope)
    expect(
      parseAgentObserverEventEnvelope({
        ...envelope,
        event: {
          type: 'message_stream_reset',
          runId: 'run-child',
          streamId: 'stream-1',
          reason: 'sensitive upstream response'
        }
      }).event
    ).toEqual({
      type: 'message_stream_reset',
      runId: 'run-child',
      streamId: 'stream-1',
      reason: 'retrying_model_request'
    })
    expect(
      parseAgentObserverEventEnvelope({
        ...envelope,
        event: {
          type: 'llm_retry',
          runId: 'run-child',
          streamId: 'stream-1',
          category: 'overloaded',
          providerCode: 'server_overloaded',
          delayMs: 100,
          retryAt: 200,
          attempt: 1,
          maxAttempts: 3
        }
      }).event
    ).toMatchObject({ type: 'llm_retry', category: 'overloaded', attempt: 1 })
    expect(() =>
      parseAgentObserverEventEnvelope({
        ...envelope,
        event: {
          type: 'message_stream_reset',
          runId: 'run-child',
          streamId: 'stream-1',
          reason: 'retry',
          forged: true
        }
      })
    ).toThrow()
    expect(() =>
      parseAgentObserverEventEnvelope({
        ...envelope,
        event: {
          type: 'llm_retry',
          runId: 'run-child',
          streamId: 'stream-1',
          category: 'overloaded',
          delayMs: 100,
          retryAt: 200,
          attempt: 3,
          maxAttempts: 2
        }
      })
    ).toThrow()
    expect(
      parseAgentObserverEventEnvelope({
        ...envelope,
        event: {
          type: 'tool_result',
          runId: 'run-child',
          result: { callId: 'call-1', tool: 'read_file', ok: true, result: { lines: 1 } }
        }
      }).event
    ).toMatchObject({ type: 'tool_result', result: { ok: true } })
    expect(
      parseAgentObserverEventEnvelope({
        ...envelope,
        event: {
          type: 'tool_call',
          runId: 'run-child',
          traceSequence: 4,
          identity: { type: 'builtin', toolName: 'read_file' },
          call: {
            id: `tc1_${'a'.repeat(43)}`,
            tool: 'read_file',
            args: {},
            approvalStatus: 'approved',
            reason: null
          }
        }
      }).event
    ).toMatchObject({ type: 'tool_call', traceSequence: 4 })
    expect(() =>
      parseAgentObserverEventEnvelope({
        ...envelope,
        event: {
          type: 'tool_call',
          runId: 'run-child',
          traceSequence: 4,
          identity: { type: 'builtin', toolName: 'different_tool' },
          call: {
            id: `tc1_${'a'.repeat(43)}`,
            tool: 'read_file',
            args: {},
            approvalStatus: 'approved',
            reason: null
          }
        }
      })
    ).toThrow(/identity/)
    expect(
      parseAgentObserverEventEnvelope({
        ...envelope,
        event: {
          type: 'context_compaction_started',
          runId: 'run-child',
          operationId: 'compact-1',
          traceSequence: 5
        }
      }).event
    ).toMatchObject({ type: 'context_compaction_started', traceSequence: 5 })
    expect(
      parseAgentObserverEventEnvelope({
        ...envelope,
        event: {
          type: 'context_compaction_finished',
          runId: 'run-child',
          operationId: 'compact-1',
          outcome: 'applied',
          traceSequence: 5
        }
      }).event
    ).toMatchObject({ type: 'context_compaction_finished', traceSequence: 5 })
    expect(
      parseAgentObserverEventEnvelope({
        ...envelope,
        event: {
          type: 'error',
          runId: 'run-child',
          traceSequence: null,
          message: 'stopped',
          recoverable: false
        }
      }).event
    ).toMatchObject({ type: 'error', traceSequence: null })
    expect(
      parseAgentObserverEventEnvelope({
        ...envelope,
        event: {
          type: 'file_change_updated',
          runId: 'run-child',
          fileChange: {
            schemaVersion: 1,
            transactionId: 'draft-1',
            conversationId: 'conversation-child',
            projectId: null,
            filePath: 'src/main.ts',
            operation: 'update',
            updateStrategy: 'rewrite',
            status: 'waiting_approval',
            baseRevision: 'content-sha256-v1:base',
            additions: 1,
            deletions: 1,
            lineCount: 2,
            byteCount: 20,
            mutationCount: 1,
            nextMutationIndex: 1,
            statsFinal: true,
            summary: null,
            createdAt: 1,
            updatedAt: 2
          }
        }
      }).event
    ).toMatchObject({
      type: 'file_change_updated',
      fileChange: { updateStrategy: 'rewrite' }
    })
    expect(
      parseAgentObserverEventEnvelope({
        ...envelope,
        event: {
          type: 'done',
          runId: 'run-child',
          success: true,
          status: 'completed',
          content: 'done',
          usage: { inputTokens: 4, outputTokens: 2, totalTokens: 6 }
        }
      }).event
    ).toMatchObject({ type: 'done', usage: { totalTokens: 6 } })
    expect(() =>
      parseAgentObserverEventEnvelope({
        ...envelope,
        event: { ...envelope.event, forged: true }
      })
    ).toThrow()
    expect(() =>
      parseAgentObserverEventEnvelope({
        ...envelope,
        event: {
          type: 'file_change_updated',
          runId: 'run-child',
          fileChange: {
            schemaVersion: 1,
            transactionId: 'draft-foreign',
            conversationId: 'conversation-foreign',
            projectId: null,
            filePath: 'src/main.ts',
            operation: 'update',
            updateStrategy: 'rewrite',
            status: 'ready',
            baseRevision: 'content-sha256-v1:base',
            additions: 1,
            deletions: 0,
            lineCount: 1,
            byteCount: 1,
            mutationCount: 1,
            nextMutationIndex: 1,
            statsFinal: false,
            summary: null,
            createdAt: 1,
            updatedAt: 2
          }
        }
      })
    ).toThrow(/FileChange identity/)
    expect(() =>
      parseAgentObserverEventEnvelope({
        ...envelope,
        event: {
          type: 'tool_call',
          runId: 'run-child',
          call: { id: 'call-1', tool: 'read_file', args: {}, approvalStatus: 'approved' }
        }
      })
    ).toThrow()
    expect(() =>
      parseAgentObserverEventEnvelope({
        ...envelope,
        event: {
          type: 'state',
          runId: 'run-child',
          state: { status: 'impossible', activeRunId: 'run-child', lastError: null, updatedAt: 1 }
        }
      })
    ).toThrow()
    expect(() =>
      parseAgentObserverEventEnvelope({
        ...envelope,
        event: {
          type: 'done',
          runId: 'run-child',
          success: true,
          proposedActions: [{ type: 'forged' }]
        }
      })
    ).toThrow()
    expect(() =>
      parseAgentObserverEventEnvelope({
        ...envelope,
        event: { ...envelope.event, runId: ' run-child ' }
      })
    ).toThrow()
    expect(() =>
      parseAgentObserverEventEnvelope({
        ...envelope,
        event: { ...envelope.event, delta: 'x'.repeat(1024 * 1024 + 1) }
      })
    ).toThrow()
    expect(() =>
      parseAgentObserverEventEnvelope({
        ...envelope,
        event: { type: 'future_event', runId: 'run-child' }
      })
    ).toThrow()
    expect(() =>
      parseAgentObserverEventEnvelope({
        ...envelope,
        event: { type: 'error', message: 'missing run', recoverable: false }
      })
    ).toThrow()
  })

  it('rejects a Command observer event with a mismatched owner identity', () => {
    const event = {
      type: 'command_output' as const,
      runId: 'run-child',
      conversationId: 'conversation-other',
      assistantMessageId: 'assistant-child',
      callId: 'call-1',
      sessionId: 'cmd_1234567890abcdef1234567890abcdef',
      sequence: 1,
      stream: 'stdout' as const,
      output: 'hello'
    }
    expect(() =>
      parseAgentObserverEventEnvelope({
        schemaVersion: 1,
        rootAgentId: 'agent-root',
        rootConversationId: 'conversation-root',
        agentId: 'agent-child',
        conversationId: 'conversation-child',
        runId: 'run-child',
        assistantMessageId: 'assistant-child',
        event
      })
    ).toThrow(/Command identity/)
  })

  it('rejects forged tree identities, broken parent graphs, and cross-workspace events', () => {
    type MutableTree = {
      workspaceId: string | null
      agents: Array<{
        agentId: string
        parentAgentId: string | null
        conversationId: string
        taskPath: string
      }>
    }
    const duplicateAgent = structuredClone(fixture.tree) as MutableTree
    duplicateAgent.agents.push({
      ...duplicateAgent.agents[1]!,
      conversationId: 'conversation-other',
      taskPath: '/root/other'
    })
    expect(() => parseAgentTreeSnapshot(duplicateAgent)).toThrow(/duplicate identity/)

    const duplicateConversation = structuredClone(fixture.tree) as MutableTree
    duplicateConversation.agents.push({
      ...duplicateConversation.agents[1]!,
      agentId: 'agent-other',
      taskPath: '/root/other'
    })
    expect(() => parseAgentTreeSnapshot(duplicateConversation)).toThrow(/duplicate identity/)

    const duplicateTaskPath = structuredClone(fixture.tree) as MutableTree
    duplicateTaskPath.agents.push({
      ...duplicateTaskPath.agents[1]!,
      agentId: 'agent-other',
      conversationId: 'conversation-other'
    })
    expect(() => parseAgentTreeSnapshot(duplicateTaskPath)).toThrow(/duplicate identity/)

    const missingParent = structuredClone(fixture.tree) as MutableTree
    missingParent.agents[1]!.parentAgentId = 'agent-outside-snapshot'
    expect(() => parseAgentTreeSnapshot(missingParent)).toThrow(/parent relation/)

    const cyclicParent = structuredClone(fixture.tree) as MutableTree
    cyclicParent.agents[1]!.parentAgentId = cyclicParent.agents[1]!.agentId
    expect(() => parseAgentTreeSnapshot(cyclicParent)).toThrow(/parent relation/)

    const foreignWorkspace = structuredClone(fixture.tree) as MutableTree
    foreignWorkspace.workspaceId = 'workspace-other'
    expect(() => parseAgentTreeSnapshot(foreignWorkspace)).toThrow(/identity/)

    expect(() =>
      parseCollaborationEventEnvelope({
        ...(fixture.event as object),
        workspaceId: 'workspace-other'
      })
    ).toThrow(/identity/)

    for (const schemaVersion of [1, 2]) {
      expect(() =>
        parseCollaborationEventEnvelope({ ...(fixture.event as object), schemaVersion })
      ).toThrow(/schemaVersion/)
    }
    const missingActivity = structuredClone(fixture.event) as Record<string, unknown>
    delete missingActivity.activities
    expect(() => parseCollaborationEventEnvelope(missingActivity)).toThrow(/Missing.*activities/)
    const missingBoundary = structuredClone(fixture.event) as {
      activities: Record<string, unknown>[]
    }
    delete missingBoundary.activities[0].traceBoundarySequence
    expect(() => parseCollaborationEventEnvelope(missingBoundary)).toThrow(/traceBoundarySequence/)
    expect(() =>
      parseCollaborationEventEnvelope({
        ...(fixture.event as object),
        activities: [
          {
            ...(fixture.event as { activities: object[] }).activities[0],
            anchorMessageId: 'assistant-root'
          }
        ]
      })
    ).not.toThrow()
    expect(() =>
      parseCollaborationEventEnvelope({
        ...(fixture.event as object),
        activities: [
          {
            ...(fixture.event as { activities: object[] }).activities[0],
            traceBoundarySequence: 3
          }
        ]
      })
    ).toThrow(/trace placement requires an anchor message/)
    expect(() =>
      parseCollaborationEventEnvelope({
        ...(fixture.event as object),
        activities: [
          { ...(fixture.event as { activities: object[] }).activities[0], semantic: 'completed' }
        ]
      })
    ).toThrow(/activity/)
    expect(() =>
      parseCollaborationEventEnvelope({
        ...(fixture.event as object),
        activities: [
          { ...(fixture.event as { activities: object[] }).activities[0], agentId: 'agent-other' }
        ]
      })
    ).toThrow(/activity/)
    expect(() =>
      parseCollaborationEventEnvelope({
        ...(fixture.event as object),
        activities: [{ ...(fixture.event as { activities: object[] }).activities[0], forged: true }]
      })
    ).toThrow(/forged/)
  })

  it('accepts bounded image previews but rejects forged observer actor combinations', () => {
    const observer = structuredClone(fixture.observer) as {
      messages: Array<{ attachments: unknown[]; inputOrigin: Record<string, unknown> }>
    }
    observer.messages[0]!.attachments = [
      {
        attachmentId: 'attachment-1',
        kind: 'image',
        name: 'preview.png',
        mimeType: 'image/png',
        sizeBytes: 1024,
        previewData: 'a'.repeat(4_096),
        previewMimeType: 'image/png',
        createdAt: 1
      }
    ]
    expect(
      parseAgentObserverConversation(observer)?.messages[0]?.attachments[0]?.previewData
    ).toHaveLength(4_096)
    observer.messages[0]!.inputOrigin = {
      kind: 'human',
      senderAgentId: 'forged-agent',
      sourceAgentMessageId: null,
      snapshotSourceConversationId: null,
      snapshotSourceMessageId: null
    }
    expect(() => parseAgentObserverConversation(observer)).toThrow()
  })
})

describe('collaboration settings contract', () => {
  it('rejects unknown fields, invalid flags, and unsafe revisions on both boundaries', () => {
    expect(parseAgentCollaborationSettings({ enabled: true, revision: 1, updatedAt: 0 })).toEqual({
      enabled: true,
      revision: 1,
      updatedAt: 0
    })
    expect(parseAgentCollaborationSettings(fixture.settings)).toEqual({
      enabled: true,
      revision: 1,
      updatedAt: 0
    })
    expect(parseAgentCollaborationSettingsGetInput({})).toEqual({})
    expect(() => parseAgentCollaborationSettingsGetInput({ enabled: true })).toThrow()
    for (const revision of [0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1]) {
      expect(() =>
        parseAgentCollaborationSettings({ enabled: true, revision, updatedAt: 0 })
      ).toThrow()
      expect(() =>
        parseAgentCollaborationSettingsUpdate({ enabled: false, expectedRevision: revision })
      ).toThrow()
    }
    expect(() =>
      parseAgentCollaborationSettingsUpdate({ enabled: 1, expectedRevision: 1 })
    ).toThrow()
    expect(() =>
      parseAgentCollaborationSettingsUpdate({
        enabled: false,
        expectedRevision: 1,
        runId: 'forged'
      })
    ).toThrow()
    expect(() =>
      parseAgentCollaborationSettings({ enabled: true, revision: 1, updatedAt: -1 })
    ).toThrow()
  })
})
