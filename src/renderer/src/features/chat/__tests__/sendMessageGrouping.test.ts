import type { AgentToolCall } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import type { ChatAgentRunView, ChatAgentTimelineItem } from '../chatTypes'
import { groupTimelineItems } from '../components/chatMessageItemUtils'

vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))

describe('send message timeline grouping', () => {
  it('merges only visibly adjacent sends and leaves narration and other tools in order', () => {
    const toolCalls: AgentToolCall[] = [
      'send_message',
      'send_message',
      'send_message',
      'run_command',
      'send_message',
      'wait_agent',
      'send_message'
    ].map((tool, index) => ({
      id: `call-${index}`,
      tool,
      args: {},
      approvalStatus: 'not_required',
      reason: null
    }))
    const callItem = (index: number): ChatAgentTimelineItem => ({
      id: `timeline-${index}`,
      type: 'tool_call',
      callId: toolCalls[index].id
    })
    const timeline: ChatAgentTimelineItem[] = [
      callItem(0),
      { id: 'blank', type: 'message', content: ' \n ' },
      callItem(1),
      { id: 'narration', type: 'message', content: '下一步' },
      callItem(2),
      callItem(3),
      callItem(4),
      callItem(5),
      callItem(6)
    ]
    const run: ChatAgentRunView = {
      runId: 'send-group-run',
      status: 'running',
      toolDefinitions: [],
      toolCalls,
      toolResults: [],
      approvals: [],
      fileChangeProposals: [],
      timeline
    }
    const grouped = groupTimelineItems(run, timeline)
    expect(grouped.map((item) => item.type)).toEqual([
      'send_message_group',
      'message',
      'send_message_group',
      'run_command_group',
      'send_message_group'
    ])
    expect(
      grouped.filter((item) => item.type === 'send_message_group').map((item) => item.callIds)
    ).toEqual([['call-0', 'call-1'], ['call-2'], ['call-4', 'call-6']])
  })
})
