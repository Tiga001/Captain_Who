import type { AgentToolCall } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatMessage } from '../chatTypes'
import { ChatMessageItem } from '../components/ChatMessageItem'

const translations: Record<string, string> = {
  'agent.processed': '已处理 {duration}',
  'agent.thinking': '正在思考',
  'agent.command.waitingForCompletion': '正在等待命令完成'
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
      diffs: [],
      timeline: toolCalls.map((toolCall) => ({
        id: `timeline-${toolCall.id}`,
        type: 'tool_call' as const,
        callId: toolCall.id
      }))
    }
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
})
