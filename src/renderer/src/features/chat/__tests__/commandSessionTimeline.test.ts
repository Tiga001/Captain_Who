// Renderer timeline regression: command_session remains durable agent data but has no UI row.
import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import type { ChatAgentRunView } from '../chatTypes'
import {
  groupTimelineItems,
  isTimelineItemRenderable,
  isWaitingForCommandCompletion
} from '../components/chatMessageItemUtils'

vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))

function createRun(result?: AgentToolResult): ChatAgentRunView {
  const call: AgentToolCall = {
    id: 'command-session-call',
    tool: 'command_session',
    args: { sessionId: 'cmd_1234567890abcdef1234567890abcdef', action: 'wait' },
    approvalStatus: 'not_required',
    reason: null
  }

  return {
    runId: 'run-1',
    status: result ? 'completed' : 'running',
    toolDefinitions: [],
    toolCalls: [call],
    toolResults: result ? [result] : [],
    approvals: [],
    fileChangeProposals: [],
    timeline: [{ id: 'command-session-timeline', type: 'tool_call', callId: call.id }]
  }
}

describe('command_session timeline visibility', () => {
  it('hides pending and completed command_session calls without deleting their records', () => {
    const pendingRun = createRun()
    expect(isTimelineItemRenderable(pendingRun, pendingRun.timeline[0])).toBe(false)
    expect(groupTimelineItems(pendingRun, pendingRun.timeline)).toEqual([])
    expect(isWaitingForCommandCompletion(pendingRun)).toBe(true)
    expect(pendingRun.toolCalls).toHaveLength(1)

    const result: AgentToolResult = {
      callId: 'command-session-call',
      tool: 'command_session',
      ok: true,
      result: { status: 'running' }
    }
    const completedRun = createRun(result)
    expect(isTimelineItemRenderable(completedRun, completedRun.timeline[0])).toBe(false)
    expect(groupTimelineItems(completedRun, completedRun.timeline)).toEqual([])
    expect(isWaitingForCommandCompletion(completedRun)).toBe(false)
    expect(completedRun.toolResults).toEqual([result])
  })

  it('does not restore the transient waiting label after the run has settled', () => {
    const settledRun = createRun()
    settledRun.status = 'cancelled'
    settledRun.completedAt = Date.now()

    expect(isWaitingForCommandCompletion(settledRun)).toBe(false)
  })
})
