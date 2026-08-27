import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import {
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
  notifications: { event: string; observerEvent: string; resync: string }
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
      activity: { semantic: 'completed' }
    })
    expect(
      eventPage.events.flatMap((event) =>
        event.activity
          ? [[event.sequence, event.kind, event.activity.agentId, event.activity.semantic]]
          : []
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
        event.activity
          ? [
              [
                event.sequence,
                event.activity.rootAnchorMessageId,
                event.activity.rootTraceBoundarySequence
              ]
            ]
          : []
      )
    ).toEqual([
      [4, 'assistant-conversation-root', 1],
      [7, 'assistant-conversation-root', 1],
      [9, 'assistant-conversation-root', 1],
      [11, 'assistant-conversation-root', 1],
      [12, null, null],
      [13, null, null],
      [14, null, null],
      [15, null, null]
    ])
    expect(eventPage.events.find((event) => event.sequence === 8)).toMatchObject({
      kind: 'mailbox_enqueued',
      messageId: 'mailbox-send-review',
      activity: null
    })
    expect(eventPage.events.find((event) => event.sequence === 10)).toMatchObject({
      kind: 'mailbox_enqueued',
      messageId: 'mailbox-followup-review',
      activity: null
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

  it('keeps the Rust/TypeScript method and DTO fixture stable', () => {
    expect(fixture.methods).toEqual([
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
      resync: AGENT_COLLABORATION_RESYNC_NOTIFICATION_METHOD
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
      activity: {
        semantic: 'started',
        agentId: 'agent-child',
        taskNameSnapshot: 'Research'
      },
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
          type: 'file_draft_updated',
          runId: 'run-child',
          draft: {
            draftId: 'draft-1',
            conversationId: 'conversation-child',
            filePath: 'src/main.ts',
            mode: 'rewrite',
            status: 'waiting_approval',
            additions: 1,
            deletions: 1,
            lineCount: 2,
            byteCount: 20,
            chunkCount: 1,
            nextChunkIndex: 1,
            statsFinal: true,
            createdAt: 1,
            updatedAt: 2
          }
        }
      }).event
    ).toMatchObject({ type: 'file_draft_updated', draft: { mode: 'rewrite' } })
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
          type: 'file_draft_updated',
          runId: 'run-child',
          draft: {
            draftId: 'draft-foreign',
            conversationId: 'conversation-foreign',
            filePath: 'src/main.ts',
            mode: 'rewrite',
            status: 'ready',
            additions: 1,
            deletions: 0,
            lineCount: 1,
            byteCount: 1,
            chunkCount: 1,
            nextChunkIndex: 1,
            statsFinal: true,
            createdAt: 1,
            updatedAt: 2
          }
        }
      })
    ).toThrow(/file draft identity/)
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

    expect(() =>
      parseCollaborationEventEnvelope({ ...(fixture.event as object), schemaVersion: 1 })
    ).toThrow(/schemaVersion/)
    const missingActivity = structuredClone(fixture.event) as Record<string, unknown>
    delete missingActivity.activity
    expect(() => parseCollaborationEventEnvelope(missingActivity)).toThrow(/Missing.*activity/)
    const missingBoundary = structuredClone(fixture.event) as {
      activity: Record<string, unknown>
    }
    delete missingBoundary.activity.rootTraceBoundarySequence
    expect(() => parseCollaborationEventEnvelope(missingBoundary)).toThrow(
      /rootTraceBoundarySequence/
    )
    expect(() =>
      parseCollaborationEventEnvelope({
        ...(fixture.event as object),
        activity: {
          ...(fixture.event as { activity: object }).activity,
          rootAnchorMessageId: 'assistant-root'
        }
      })
    ).toThrow(/placement fields/)
    expect(() =>
      parseCollaborationEventEnvelope({
        ...(fixture.event as object),
        activity: {
          ...(fixture.event as { activity: object }).activity,
          rootTraceBoundarySequence: 3
        }
      })
    ).toThrow(/placement fields/)
    expect(() =>
      parseCollaborationEventEnvelope({
        ...(fixture.event as object),
        activity: { ...(fixture.event as { activity: object }).activity, semantic: 'completed' }
      })
    ).toThrow(/activity/)
    expect(() =>
      parseCollaborationEventEnvelope({
        ...(fixture.event as object),
        activity: { ...(fixture.event as { activity: object }).activity, agentId: 'agent-other' }
      })
    ).toThrow(/activity/)
    expect(() =>
      parseCollaborationEventEnvelope({
        ...(fixture.event as object),
        activity: { ...(fixture.event as { activity: object }).activity, forged: true }
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
