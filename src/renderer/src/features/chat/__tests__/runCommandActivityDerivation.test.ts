import type { AgentToolCall } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { getRunCommandGroupItems, getSearchGroupItems } from '../components/chatMessageItemUtils'
import type { ChatAgentRunView } from '../chatTypes'

vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))

function toolCall(id: string, tool: AgentToolCall['tool']): AgentToolCall {
  return {
    approvalStatus: 'not_required',
    args: {},
    id,
    tool
  }
}

function run(toolCalls: AgentToolCall[]): ChatAgentRunView {
  return {
    runId: 'run-1',
    status: 'running',
    toolDefinitions: [],
    toolCalls,
    toolResults: [],
    approvals: [],
    diffs: [],
    timeline: toolCalls.map((call) => ({
      id: `tool-${call.id}`,
      type: 'tool_call',
      callId: call.id
    })),
    commandOutputPreviews: {
      command: {
        callId: 'command',
        chunks: [{ sequence: 1, stream: 'stdout', output: 'live output\n' }]
      }
    }
  }
}

describe('run command activity derivation', () => {
  it('routes transient command output to the matching run_command group only', () => {
    const currentRun = run([toolCall('command', 'run_command'), toolCall('search', 'search_code')])

    expect(getRunCommandGroupItems(currentRun, ['command'])[0]?.liveOutput).toEqual(
      currentRun.commandOutputPreviews?.command
    )
    expect(getSearchGroupItems(currentRun, ['search'])[0]).not.toHaveProperty('liveOutput')
  })
})
