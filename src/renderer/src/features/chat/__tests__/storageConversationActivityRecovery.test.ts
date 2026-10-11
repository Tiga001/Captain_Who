import type {
  AgentToolCall,
  AgentToolResult,
  StorageChatConversationRecord
} from '@mycopilot/protocol'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { mapConversationFromStorage } from '../../storage/storageConversationMapping'
import { parsePersistedAgentRun, stringifyPersistedAgentRun } from '../../storage/persistedAgentRun'
import { applyAgentEventToChatMessage } from '../../agentRun/agentEventReducer'
import { ensureAgentRun } from '../../agentRun/agentEventReducerShared'
import type { ChatAgentRunView, ChatMessage } from '../chatTypes'

const thumbnail = 'data:image/png;base64,aGVsbG8='
const calls: AgentToolCall[] = [
  {
    id: 'search',
    tool: 'web_search',
    args: { query: 'durable facts' },
    approvalStatus: 'not_required',
    reason: null
  },
  {
    id: 'image',
    tool: 'read_image',
    args: { path: '/fake/image.png' },
    approvalStatus: 'not_required',
    reason: null
  },
  {
    id: 'pending-file',
    tool: 'read_word',
    args: { filePath: '/fake/pending.docx' },
    approvalStatus: 'not_required',
    reason: null
  },
  {
    id: 'pending-search',
    tool: 'web_search',
    args: { query: 'pending' },
    approvalStatus: 'not_required',
    reason: null
  }
]
const results: AgentToolResult[] = [
  {
    callId: 'search',
    tool: 'web_search',
    ok: true,
    result: {
      query: 'durable facts',
      provider: 'fake',
      answer: 'Persisted answer',
      responseTime: 0.2,
      results: [
        { title: 'Low', url: 'https://example.test/low', content: 'Low source', score: 0.1 },
        {
          title: 'High',
          url: 'https://example.test/high',
          content: 'High source',
          score: 0.9,
          favicon: 'https://example.test/icon.png'
        },
        {
          title: 'Duplicate',
          url: 'https://example.test/high',
          content: 'Do not repeat',
          score: 1
        },
        { title: 'Unsafe scheme', url: 'javascript:alert(1)', score: 1 }
      ]
    }
  },
  {
    callId: 'image',
    tool: 'read_image',
    ok: true,
    result: { path: '/fake/image.png', mimeType: 'image/png', thumbnailDataUrl: thumbnail }
  }
]

function storedConversation(status: ChatAgentRunView['status']): StorageChatConversationRecord {
  const terminal = ['completed', 'failed', 'cancelled'].includes(status)
  return {
    id: 'conversation',
    title: 'Fake recovery',
    modelId: 'fake-model',
    projectId: null,
    createdAt: 1,
    updatedAt: 2,
    messages: [
      {
        id: 'assistant',
        role: 'assistant',
        content: 'Durable text',
        createdAt: 1,
        status: terminal ? 'sent' : 'pending',
        agentRunJson: stringifyPersistedAgentRun({
          ...ensureAgentRun(undefined, 'run', status),
          ...(terminal ? { completedAt: 100 } : {}),
          toolCalls: calls,
          toolResults: results,
          webSearchActivities: [],
          readActivities: [],
          coreCheckpoint: {
            generation: '01a12a4f-5137-7e90-b173-44145df4714e',
            sequence: 3,
            fingerprint: 'a'.repeat(64)
          }
        })
      }
    ]
  }
}

afterEach(() => vi.restoreAllMocks())

describe('storage activity recovery without Renderer-written projections', () => {
  it.each([
    'running',
    'waiting_for_approval',
    'waiting_for_user_input',
    'completed',
    'failed',
    'cancelled'
  ] as const)(
    'reconstructs safe web/read activities for %s from durable calls and results',
    (status) => {
      vi.spyOn(Date, 'now').mockReturnValue(100)
      const loaded = mapConversationFromStorage(storedConversation(status)).messages[0]
      expect(loaded.agentRun?.status).toBe(status)
      expect(loaded.agentRun?.coreCheckpoint?.sequence).toBe(3)
      expect(loaded.agentRun?.webSearchActivities?.[0]).toMatchObject({
        callId: 'search',
        status: 'completed',
        answer: 'Persisted answer',
        provider: 'fake',
        responseTime: 0.2
      })
      expect(
        loaded.agentRun?.webSearchActivities?.[0].sources.map((source) => source.title)
      ).toEqual(['High', 'Low'])
      expect(loaded.agentRun?.webSearchActivities?.[0].sources[0].faviconUrl).toBe(
        'https://example.test/icon.png'
      )
      expect(loaded.agentRun?.readActivities?.[0]).toMatchObject({
        callId: 'image',
        status: 'completed',
        path: '/fake/image.png',
        fileName: 'image.png',
        extension: 'png',
        thumbnailDataUrl: thumbnail
      })
      expect(loaded.agentRun?.readActivities?.[0].fullDataUrl).toBeUndefined()
      const pendingStatus = ['completed', 'failed', 'cancelled'].includes(status)
        ? status
        : 'running'
      expect(loaded.agentRun?.readActivities?.[1].status).toBe(pendingStatus)
      expect(loaded.agentRun?.webSearchActivities?.[1].status).toBe(pendingStatus)
      expect(loaded.agentRun?.readActivities).toHaveLength(2)
      expect(loaded.agentRun?.webSearchActivities).toHaveLength(2)
    }
  )

  it('matches live Renderer presentation and does not need to save the reconstructed views', () => {
    vi.spyOn(Date, 'now').mockReturnValue(100)
    let live: ChatMessage = { id: 'assistant', role: 'assistant', content: '', createdAt: 1 }
    for (const [sequence, call] of calls.entries()) {
      live = applyAgentEventToChatMessage(live, {
        type: 'tool_call',
        runId: 'run',
        call,
        traceSequence: sequence,
        identity: { type: 'builtin', toolName: call.tool }
      })
    }
    for (const result of results) {
      live = applyAgentEventToChatMessage(live, { type: 'tool_result', runId: 'run', result })
    }
    const source = storedConversation('waiting_for_approval')
    const before = source.messages[0].agentRunJson
    const restored = mapConversationFromStorage(source).messages[0]
    expect(restored.agentRun?.readActivities).toEqual(live.agentRun?.readActivities)
    expect(restored.agentRun?.webSearchActivities).toEqual(live.agentRun?.webSearchActivities)
    expect(source.messages[0].agentRunJson).toBe(before)
    expect(JSON.parse(before!).webSearchActivities).toEqual([])
  })
})

describe('failed read recovery without a resolved path', () => {
  it('retains the entire checkpoint and its failed read when the safe display path is empty', () => {
    const stored = storedConversation('running')
    const message = stored.messages[0]
    const projection = JSON.parse(message.agentRunJson!)
    projection.toolCalls = []
    projection.toolResults = []
    projection.readActivities = [
      {
        callId: 'missing-path',
        tool: 'read_file',
        kind: 'file',
        status: 'failed',
        path: '',
        fileName: 'read_file',
        updatedAt: 123,
        error: 'A path is required.'
      }
    ]
    message.agentRunJson = JSON.stringify(projection)
    expect(parsePersistedAgentRun(projection)?.readActivities).toEqual(projection.readActivities)
    const restored = mapConversationFromStorage(stored).messages[0]
    expect(restored.agentRun?.runId).toBe('run')
    expect(restored.agentRun?.coreCheckpoint).toEqual(projection.coreCheckpoint)
    expect(restored.agentRun?.readActivities).toEqual(projection.readActivities)
  })
})
