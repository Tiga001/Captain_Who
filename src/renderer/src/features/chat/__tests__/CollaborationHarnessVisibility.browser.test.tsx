import type { AgentToolCall } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import type { CollaborationTimelineActivity } from '../../agentCollaboration/CollaborationTimelineActivity'
import type { ChatMessage } from '../chatTypes'
import { ChatMessageItem } from '../components/ChatMessageItem'

const translations: Record<string, string> = {
  'agent.processed': '已处理 {duration}',
  'agent.thinking': '正在思考',
  'agent.command.waitingForCompletion': '正在等待命令完成',
  'collaboration.activity.copyAgentStatus': '{name}: {status}',
  'collaboration.activity.moreAgents': '另有 {count} 个',
  'collaboration.activity.openAgentActivity': '查看子智能体 {name}：{status}',
  'collaboration.activity.statusListSeparator': '；',
  'collaboration.activity.status.started': '已开始工作'
}

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'zh-CN',
    t: (key: string) => translations[key] ?? key
  })
}))

vi.mock('../../storage/storageClient', () => ({
  loadAttachmentImage: vi.fn(),
  loadImageFile: vi.fn(),
  revealStoredProjectFile: vi.fn()
}))

vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))

const harnessTools = [
  'spawn_agent',
  'send_message',
  'followup_task',
  'wait_agent',
  'list_agents',
  'interrupt_agent'
] as const

function call(tool: string, index: number): AgentToolCall {
  return {
    id: `call-${index}`,
    tool,
    args: { secretDiagnosticMarker: `RAW-HARNESS-${tool}` },
    approvalStatus: 'not_required',
    reason: `internal ${tool}`
  }
}

function message(toolCalls: AgentToolCall[]): ChatMessage {
  return {
    id: 'assistant-message',
    role: 'assistant',
    content: '',
    createdAt: 1,
    status: 'pending',
    agentRun: {
      runId: 'run-root',
      status: 'running',
      startedAt: 1,
      firstResponseAt: 2,
      toolDefinitions: [],
      toolCalls,
      toolResults: [],
      approvals: [],
      fileChangeProposals: [],
      timeline: toolCalls.map((toolCall) => ({
        id: `timeline-${toolCall.id}`,
        type: 'tool_call' as const,
        callId: toolCall.id
      }))
    }
  }
}

function settledMessage(toolCalls: AgentToolCall[]): ChatMessage {
  const settled = message(toolCalls)
  settled.content = '主智能体最终正文'
  settled.status = 'sent'
  if (!settled.agentRun) throw new Error('missing root run fixture')
  settled.agentRun.status = 'completed'
  settled.agentRun.completedAt = 10
  return settled
}

function activity(
  rootAnchorMessageId: string | null,
  rootTraceBoundarySequence: number | null
): CollaborationTimelineActivity {
  return {
    activityId: 'activity-reviewer-started',
    agentId: 'agent-reviewer',
    occurredAt: 5,
    rootAnchorMessageId,
    rootTraceBoundarySequence,
    runId: 'run-reviewer',
    semantic: 'started',
    sequence: 1,
    taskNameSnapshot: 'Reviewer',
    turnId: 'turn-reviewer'
  }
}

describe('collaboration Harness timeline projection', () => {
  it('keeps all six raw coordination calls out of the chat timeline', async () => {
    const calls = harnessTools.map(call)
    const screen = await render(
      <ChatMessageItem message={message(calls)} showTokenUsageDetails={false} />
    )

    expect(screen.container.querySelector('.agent-activity')).toBeNull()
    for (const tool of harnessTools) {
      expect(screen.container.textContent).not.toContain(tool)
      expect(screen.container.textContent).not.toContain(`RAW-HARNESS-${tool}`)
    }
    expect(screen.container.textContent).toContain('正在思考')
    expect(screen.container.textContent).not.toContain('正在等待命令完成')
  })

  it('continues to render an ordinary Tool beside hidden collaboration bookkeeping', async () => {
    const visible = call('custom_visible_tool', harnessTools.length)
    visible.args = { marker: 'VISIBLE-TOOL-DETAIL' }
    const screen = await render(
      <ChatMessageItem
        message={message([...harnessTools.map(call), visible])}
        showTokenUsageDetails={false}
      />
    )

    expect(screen.container.textContent).toContain('VISIBLE-TOOL-DETAIL')
    expect(screen.container.textContent).not.toContain('RAW-HARNESS-wait_agent')
  })

  it('folds trusted anchored activity for a settled root run without exposing raw Harness calls', async () => {
    const settled = settledMessage(harnessTools.map(call))
    const onUiStateChange = vi.fn()
    const props = {
      collaborationTimelineActivities: [activity('assistant-message', 1)],
      message: settled,
      mode: 'interactive' as const,
      onOpenCollaborationAgent: vi.fn(),
      onUiStateChange,
      showTokenUsageDetails: false
    }
    const screen = await render(<ChatMessageItem {...props} />)

    const disclosure = screen.container.querySelector<HTMLButtonElement>(
      '.agent-run__elapsed-button'
    )
    expect(disclosure).not.toBeNull()
    expect(disclosure).toHaveAttribute('aria-expanded', 'false')
    expect(screen.container.querySelector('[data-semantic="started"]')).toBeNull()
    expect(screen.getByText('主智能体最终正文').elements()).toHaveLength(1)

    await userEvent.click(disclosure!)
    expect(onUiStateChange).toHaveBeenCalledWith('assistant-message', {
      timelineCollapsed: false
    })

    const expanded = structuredClone(settled)
    expanded.uiState = { timelineCollapsed: false }
    await screen.rerender(<ChatMessageItem {...props} message={expanded} />)

    expect(screen.container.querySelector('[data-semantic="started"]')).not.toBeNull()
    expect(screen.getByText('主智能体最终正文').elements()).toHaveLength(1)
    for (const tool of harnessTools) {
      expect(screen.container.textContent).not.toContain(tool)
      expect(screen.container.textContent).not.toContain(`RAW-HARNESS-${tool}`)
    }
  })

  it.each([
    {
      activities: [] as CollaborationTimelineActivity[],
      label: 'no child activity',
      mode: 'interactive' as const,
      status: 'completed' as const
    },
    {
      activities: [activity('assistant-message', null)],
      label: 'an incomplete durable anchor',
      mode: 'interactive' as const,
      status: 'completed' as const
    },
    {
      activities: [activity('another-message', 1)],
      label: 'another message anchor',
      mode: 'interactive' as const,
      status: 'completed' as const
    },
    {
      activities: [activity('assistant-message', 1)],
      label: 'an unsettled root run',
      mode: 'interactive' as const,
      status: 'running' as const
    },
    {
      activities: [activity('assistant-message', 1)],
      label: 'observer mode',
      mode: 'observer' as const,
      status: 'completed' as const
    }
  ])('does not add a collaboration-only disclosure for $label', async (variant) => {
    const candidate = settledMessage(harnessTools.map(call))
    if (!candidate.agentRun) throw new Error('missing root run fixture')
    candidate.agentRun.status = variant.status
    if (variant.status === 'running') candidate.agentRun.completedAt = undefined

    const screen = await render(
      <ChatMessageItem
        collaborationTimelineActivities={variant.activities}
        message={candidate}
        mode={variant.mode}
        onOpenCollaborationAgent={vi.fn()}
        showTokenUsageDetails={false}
      />
    )

    expect(screen.container.querySelector('.agent-run__elapsed-button')).toBeNull()
    expect(screen.getByText('主智能体最终正文').elements()).toHaveLength(1)
  })
})
