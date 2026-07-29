import type { AgentEvent, AgentToolDefinition } from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import type { ChatAgentRunView, ChatMessage } from '../../features/chat/chatTypes'
import { applyAgentEventToChatMessage } from '../../features/agentRun/agentEventReducer'

const stableTool: AgentToolDefinition = {
  name: 'read_file',
  description: 'Read a file.',
  inputSchema: { type: 'object' },
  safety: 'read_only',
  requiresWorkspace: true,
  requiresApproval: false,
  approvalMode: 'never'
}

const dynamicTool: AgentToolDefinition = {
  name: 'office_document',
  description: 'Create and edit Word documents.',
  inputSchema: {
    type: 'object',
    required: ['operation', 'reason']
  },
  safety: 'requires_approval',
  requiresWorkspace: false,
  requiresApproval: true,
  approvalMode: 'dynamic'
}

function toolSetChanged(
  toolDefinitions: AgentToolDefinition[] = [stableTool, dynamicTool]
): Extract<AgentEvent, { type: 'tool_set_changed' }> {
  return {
    type: 'tool_set_changed',
    runId: 'run-tool-set',
    stableRevision: 'tool-set-sha256-v1:stable',
    dynamicRevision: 'tool-set-sha256-v1:documents',
    effectiveRevision: 'tool-set-sha256-v1:effective',
    toolDefinitions
  }
}

function run(overrides: Partial<ChatAgentRunView> = {}): ChatAgentRunView {
  return {
    runId: 'run-tool-set',
    status: 'running',
    startedAt: 100,
    completedAt: undefined,
    toolDefinitions: [stableTool],
    todo: {
      revision: 1,
      items: [
        {
          id: 'todo-1',
          title: 'Create the document',
          status: 'in_progress',
          createdAt: 90,
          updatedAt: 95
        }
      ],
      updatedAt: 95
    },
    toolCalls: [
      {
        id: 'activate-documents',
        tool: 'skills_activate',
        args: { id: 'bundled:application:documents' },
        approvalStatus: 'not_required'
      }
    ],
    toolResults: [],
    approvals: [],
    diffs: [],
    fileDrafts: [],
    fileWritePreviews: [],
    messageStreamCheckpoints: {},
    readActivities: [],
    timeline: [{ id: 'call:activate-documents', type: 'tool_call', callId: 'activate-documents' }],
    activatedSkills: [
      {
        id: 'bundled:application:documents',
        name: 'documents',
        revision: 'skill-package-sha256-v2:documents',
        source: { kind: 'bundled', id: 'bundled:application' }
      }
    ],
    skillActivationRevision: 'activation-sha256-v1:documents',
    ...overrides
  }
}

function message(agentRun: ChatAgentRunView): ChatMessage {
  return {
    id: 'assistant-tool-set',
    role: 'assistant',
    content: 'Creating the requested document.',
    createdAt: 1,
    status: 'pending',
    attachments: [
      {
        id: 'attachment-1',
        kind: 'file',
        name: 'brief.txt',
        sizeBytes: 12
      }
    ],
    uiState: { timelineCollapsed: true },
    agentRun
  }
}

describe('Agent tool-set projection', () => {
  it('replaces effective definitions without resetting the active run or message state', () => {
    const current = message(run())
    const result = applyAgentEventToChatMessage(current, toolSetChanged())

    expect(result).toEqual({
      ...current,
      agentRun: {
        ...current.agentRun,
        toolDefinitions: [stableTool, dynamicTool],
        toolSetRevision: {
          stable: 'tool-set-sha256-v1:stable',
          dynamic: 'tool-set-sha256-v1:documents',
          effective: 'tool-set-sha256-v1:effective'
        }
      }
    })
    expect(current.agentRun?.toolDefinitions).toEqual([stableTool])
  })

  it('preserves a restored completed run when the event is replayed', () => {
    const current = {
      ...message(
        run({
          status: 'completed',
          completedAt: 500,
          finishReason: 'stop'
        })
      ),
      status: 'sent' as const
    }
    const first = applyAgentEventToChatMessage(current, toolSetChanged())
    const replayed = applyAgentEventToChatMessage(first, toolSetChanged())

    expect(replayed).toEqual(first)
    expect(replayed.status).toBe('sent')
    expect(replayed.agentRun?.status).toBe('completed')
    expect(replayed.agentRun?.completedAt).toBe(500)
    expect(replayed.agentRun?.finishReason).toBe('stop')
    expect(replayed.agentRun?.timeline).toEqual(current.agentRun?.timeline)
  })

  it('supports removing every dynamic definition without disturbing the run', () => {
    const current = message(run({ toolDefinitions: [stableTool, dynamicTool] }))
    const result = applyAgentEventToChatMessage(current, toolSetChanged([stableTool]))

    expect(result.agentRun?.toolDefinitions).toEqual([stableTool])
    expect(result.agentRun?.activatedSkills).toEqual(current.agentRun?.activatedSkills)
    expect(result.agentRun?.skillActivationRevision).toBe(current.agentRun?.skillActivationRevision)
    expect(result.agentRun?.toolCalls).toEqual(current.agentRun?.toolCalls)
  })
})
