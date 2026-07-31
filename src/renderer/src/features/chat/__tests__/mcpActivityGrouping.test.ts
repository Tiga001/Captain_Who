import type { AgentToolCall } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import type {
  ChatAgentRunView,
  ChatAgentTimelineItem,
  ChatMcpToolInvocationView
} from '../chatTypes'
import { groupTimelineItems } from '../components/chatMessageItemUtils'

vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))

function invocation(
  invocationId: string,
  serverId: string,
  serverDisplayName = 'Duplicate display name'
): ChatMcpToolInvocationView {
  return {
    actionId: `action-${invocationId}`,
    invocationId,
    callId: `call-${invocationId}`,
    serverId,
    serverDisplayName,
    rawToolName: `tool-${invocationId}`,
    modelToolName: `model-${invocationId}`,
    external: true,
    state: 'completed',
    dispatchCertainty: 'response_received',
    outcome: 'succeeded',
    outputTruncated: false
  }
}

function run(
  mcpInvocations: ChatMcpToolInvocationView[],
  timeline: ChatAgentTimelineItem[],
  toolCalls: AgentToolCall[] = []
): ChatAgentRunView {
  return {
    runId: 'mcp-grouping-run',
    status: 'completed',
    toolDefinitions: [],
    toolCalls,
    toolResults: [],
    approvals: [],
    diffs: [],
    mcpInvocations,
    timeline
  }
}

describe('MCP timeline grouping', () => {
  it('groups only adjacent invocations with the same typed Server ID', () => {
    const invocations = [
      invocation('one', 'server-a'),
      invocation('two', 'server-a'),
      invocation('three', 'server-b'),
      invocation('four', 'server-a'),
      invocation('five', 'server-a'),
      invocation('six', 'server-a')
    ]
    const command: AgentToolCall = {
      id: 'ordinary-command',
      tool: 'run_command',
      args: { command: 'true' },
      approvalStatus: 'not_required'
    }
    const timeline: ChatAgentTimelineItem[] = [
      { id: 'mcp-one', type: 'mcp_tool_call', invocationId: 'one' },
      { id: 'mcp-two', type: 'mcp_tool_call', invocationId: 'two' },
      { id: 'mcp-three', type: 'mcp_tool_call', invocationId: 'three' },
      { id: 'narration', type: 'message', content: 'Now continue.' },
      { id: 'mcp-four', type: 'mcp_tool_call', invocationId: 'four' },
      { id: 'blank-narration', type: 'message', content: ' \n ' },
      { id: 'mcp-five', type: 'mcp_tool_call', invocationId: 'five' },
      { id: 'command', type: 'tool_call', callId: command.id },
      { id: 'mcp-six', type: 'mcp_tool_call', invocationId: 'six' }
    ]

    const grouped = groupTimelineItems(run(invocations, timeline, [command]), timeline)
    const mcpGroups = grouped.filter((item) => item.type === 'mcp_activity_group')

    expect(mcpGroups.map((item) => [item.serverId, item.invocationIds])).toEqual([
      ['server-a', ['one', 'two']],
      ['server-b', ['three']],
      ['server-a', ['four', 'five']],
      ['server-a', ['six']]
    ])
    expect(grouped.map((item) => item.type)).toEqual([
      'mcp_activity_group',
      'mcp_activity_group',
      'message',
      'mcp_activity_group',
      'run_command_group',
      'mcp_activity_group'
    ])
  })

  it('does not group by duplicate display name and leaves missing typed lifecycle data fail-closed', () => {
    const first = invocation('one', 'server-a')
    const second = invocation('two', 'server-b')
    const timeline: ChatAgentTimelineItem[] = [
      { id: 'mcp-one', type: 'mcp_tool_call', invocationId: first.invocationId },
      { id: 'mcp-two', type: 'mcp_tool_call', invocationId: second.invocationId },
      { id: 'mcp-missing', type: 'mcp_tool_call', invocationId: 'missing' }
    ]

    const grouped = groupTimelineItems(run([first, second], timeline), timeline)

    expect(grouped).toEqual([
      {
        id: 'mcp-activity-group-one',
        type: 'mcp_activity_group',
        serverId: 'server-a',
        invocationIds: ['one']
      },
      {
        id: 'mcp-activity-group-two',
        type: 'mcp_activity_group',
        serverId: 'server-b',
        invocationIds: ['two']
      },
      { id: 'mcp-missing', type: 'mcp_tool_call', invocationId: 'missing' }
    ])
  })
})
