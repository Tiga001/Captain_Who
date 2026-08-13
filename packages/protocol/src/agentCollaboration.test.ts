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
  AGENT_COLLABORATION_RESYNC_NOTIFICATION_METHOD,
  AGENT_COLLABORATION_TEMPLATES_CREATE_METHOD,
  AGENT_COLLABORATION_TEMPLATES_DELETE_METHOD,
  AGENT_COLLABORATION_TEMPLATES_LIST_METHOD,
  AGENT_COLLABORATION_TEMPLATES_SET_ENABLED_METHOD,
  AGENT_COLLABORATION_TEMPLATES_UPDATE_METHOD,
  parseAgentTemplateCreateRequest,
  parseAgentTemplate,
  parseAgentTemplateList,
  parseAgentDetail,
  parseAgentConversationLocator,
  parseAgentObserverConversation,
  parseAgentTreeLookup,
  parseAgentTreeSnapshot,
  parseCollaborationEventEnvelope,
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
  notifications: { event: string; resync: string }
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
}

describe('agent collaboration protocol', () => {
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
      AGENT_COLLABORATION_TEMPLATES_DELETE_METHOD,
      AGENT_COLLABORATION_APPROVALS_LIST_METHOD,
      AGENT_COLLABORATION_APPROVALS_DECIDE_METHOD
    ])
    expect(fixture.notifications).toEqual({
      event: AGENT_COLLABORATION_EVENT_NOTIFICATION_METHOD,
      resync: AGENT_COLLABORATION_RESYNC_NOTIFICATION_METHOD
    })
    const tree = parseAgentTreeSnapshot(fixture.tree)
    expect(tree.lastSequence).toBe(7)
    expect(
      parseAgentTreeLookup({ schemaVersion: 1, materialized: true, tree: fixture.tree }).tree
        ?.rootConversationId
    ).toBe('conversation-root')
    expect(parseCollaborationEventEnvelope(fixture.event)).toMatchObject({
      sequence: 7,
      kind: 'turn_started',
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
    expect(template.projectId).toBe(tree.projectId)
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
        projectId: 'project-1',
        machineKey: 'research',
        name: 'Research',
        description: '',
        instructions: '',
        modelConfigId: 'model-safe-id',
        enabled: true,
        apiKey: 'must-never-cross-the-boundary'
      })
    ).toThrow()
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
