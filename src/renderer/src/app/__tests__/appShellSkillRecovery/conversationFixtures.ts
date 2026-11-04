import type { AgentCommandSessionGetOutput, AgentCommandSessionSnapshot } from '@mycopilot/protocol'
import type { ChatConversation, ChatQueuedMessage } from '../../../features/chat/chatTypes'
import { skillSelection } from './turnFixtures'

export function storedConversation(): ChatConversation {
  return {
    id: 'conversation-a',
    projectId: 'project-a',
    modelId: 'model-1',
    title: 'Audit repository',
    messages: [
      {
        id: 'user-old',
        role: 'user',
        content: 'original',
        createdAt: 1,
        status: 'sent'
      },
      {
        id: 'assistant-old',
        role: 'assistant',
        content: 'done',
        createdAt: 2,
        status: 'sent',
        agentRun: {
          runId: 'run-old',
          status: 'completed',
          toolDefinitions: [],
          toolCalls: [],
          toolResults: [],
          approvals: [],
          fileChangeProposals: [],
          timeline: [],
          activatedSkills: [
            {
              ...skillSelection,
              name: 'Repository auditor',
              source: { id: 'project-a', kind: 'workspace' }
            }
          ]
        }
      }
    ],
    createdAt: 1,
    updatedAt: 2,
    pinnedAt: null,
    archivedAt: null,
    unreadAt: null
  }
}

export function storedConversationWithRun(
  assistantMessageId: string,
  runId: string,
  status: 'running' | 'completed' | 'failed' | 'cancelled'
): ChatConversation {
  const stored = storedConversation()
  return {
    ...stored,
    messages: [
      ...stored.messages,
      {
        id: assistantMessageId,
        role: 'assistant',
        content: status === 'completed' ? 'authoritative answer' : '',
        createdAt: 3,
        status: status === 'running' ? 'pending' : status === 'failed' ? 'error' : 'sent',
        agentRun: {
          runId,
          status,
          completedAt: status === 'running' ? undefined : 4,
          toolDefinitions: [],
          toolCalls: [],
          toolResults: [],
          approvals: [],
          fileChangeProposals: [],
          timeline: []
        }
      }
    ]
  }
}

export function storedConversationWithCommandRun(): ChatConversation {
  const stored = storedConversationWithRun('assistant-command', 'run-command', 'completed')
  const assistant = stored.messages.at(-1)
  if (!assistant?.agentRun) throw new Error('missing command run fixture')
  assistant.agentRun.toolCalls = [
    {
      id: 'command-call',
      tool: 'run_command',
      args: { command: 'python3 snake_game/main.py' },
      approvalStatus: 'approved',
      reason: null
    }
  ]
  assistant.agentRun.timeline = [
    { id: 'tool-call-command-call', type: 'tool_call', callId: 'command-call' }
  ]
  return stored
}

export function commandSessionSnapshot(
  overrides: Partial<AgentCommandSessionSnapshot> = {}
): AgentCommandSessionSnapshot {
  return {
    schemaVersion: 2,
    sessionId: 'cmd_1234567890abcdef1234567890abcdef',
    conversationId: 'conversation-a',
    assistantMessageId: 'assistant-command',
    originRunId: 'run-command',
    callId: 'command-call',
    projectId: 'project-a',
    command: 'python3 snake_game/main.py',
    cwd: '/workspace/a',
    commandDigest: `sha256:${'a'.repeat(64)}`,
    status: 'running',
    startedAt: 10,
    latestSequence: 0,
    outputTruncated: false,
    ...overrides
  }
}

export function commandSessionGetOutput(
  snapshot: AgentCommandSessionSnapshot,
  output = ''
): AgentCommandSessionGetOutput {
  return {
    session: snapshot,
    transcript: {
      requestedAfterSequence: 0,
      firstAvailableSequence: output ? 1 : undefined,
      latestSequence: output ? 1 : 0,
      truncatedBefore: false,
      outputCaptureTruncated: false,
      chunks: output ? [{ sequence: 1, stream: 'stdout', output }] : []
    }
  }
}

export function queuedMessage(id: string, content: string, createdAt: number): ChatQueuedMessage {
  return {
    id,
    clientMessageId: `client-${id}`,
    content,
    attachments: [],
    modelId: 'model-1',
    permissionMode: 'full',
    projectId: 'project-a',
    skills: [],
    status: 'pending',
    createdAt
  }
}
