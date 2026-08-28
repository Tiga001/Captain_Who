import type { AgentEvent, AgentUsage } from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import type { ChatAgentRunView, ChatMessage } from '../../features/chat/chatTypes'
import { applyAgentEventToChatMessage } from '../../features/agentRun/agentEventReducer'

const segmentUsage: AgentUsage = {
  inputTokens: 92_510,
  outputTokens: 391,
  outputThinkingTokens: 14,
  totalTokens: 92_901,
  billableRequestCount: 1
}

const cumulativeUsage: AgentUsage = {
  inputTokens: 2_942_988,
  outputTokens: 38_253,
  outputThinkingTokens: 32_402,
  totalTokens: 2_981_241,
  billableRequestCount: 42
}

function run(): ChatAgentRunView {
  return {
    runId: 'run-cumulative',
    status: 'running',
    toolDefinitions: [],
    toolCalls: [],
    toolResults: [],
    approvals: [],
    fileChangeProposals: [],
    timeline: [],
    usage: segmentUsage
  }
}

function message(): ChatMessage {
  return {
    id: 'assistant-cumulative',
    role: 'assistant',
    content: '',
    createdAt: 1,
    status: 'pending',
    agentRun: run()
  }
}

function cumulativeDone(): Extract<AgentEvent, { type: 'done' }> {
  return {
    type: 'done',
    runId: 'run-cumulative',
    success: true,
    status: 'completed',
    content: 'done',
    usage: cumulativeUsage,
    finishReason: 'stop',
    proposedActions: []
  }
}

describe('Agent usage projection', () => {
  it('replaces a runtime-segment value with the backend run total', () => {
    const result = applyAgentEventToChatMessage(message(), cumulativeDone())

    expect(result.agentRun?.usage).toEqual(cumulativeUsage)
  })

  it('keeps cumulative usage idempotent when Done is replayed', () => {
    const first = applyAgentEventToChatMessage(message(), cumulativeDone())
    const replayed = applyAgentEventToChatMessage(first, cumulativeDone())

    expect(replayed.agentRun?.usage).toEqual(cumulativeUsage)
  })
})
