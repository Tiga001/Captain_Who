// Renderer browser regression: command-session waits reuse the transient thinking line.
import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatMessage } from '../chatTypes'
import { ChatMessageItem } from '../components/ChatMessageItem'

const translations: Record<string, string> = {
  'agent.command.waitingForCompletion': '正在等待命令完成',
  'agent.processed': '已处理 {duration}',
  'agent.thinking': '正在思考'
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

vi.mock('../../../host/hostClient', () => ({
  hostClient: {}
}))

const commandSessionCall: AgentToolCall = {
  id: 'command-session-call',
  tool: 'command_session',
  args: { sessionId: 'cmd_1234567890abcdef1234567890abcdef', action: 'wait' },
  approvalStatus: 'not_required'
}

function commandSessionMessage(result?: AgentToolResult): ChatMessage {
  return {
    id: 'assistant-message',
    role: 'assistant',
    content: result ? '命令已经完成。' : '',
    createdAt: 1,
    status: 'sent',
    agentRun: {
      runId: 'run-1',
      status: result ? 'completed' : 'running',
      startedAt: 1,
      firstResponseAt: 2,
      completedAt: result ? 3 : undefined,
      toolDefinitions: [],
      toolCalls: [commandSessionCall],
      toolResults: result ? [result] : [],
      approvals: [],
      diffs: [],
      timeline: [
        { id: 'command-session-timeline', type: 'tool_call', callId: commandSessionCall.id }
      ]
    }
  }
}

describe('command_session transient waiting status', () => {
  it('shows only the plain waiting line while retaining the hidden tool record', async () => {
    const screen = await render(
      <ChatMessageItem message={commandSessionMessage()} showTokenUsageDetails={false} />
    )

    expect(screen.container.querySelector('.agent-thinking')?.textContent).toBe('正在等待命令完成')
    expect(screen.container.querySelector('.agent-tool-activity')).toBeNull()
    expect(screen.container.textContent).not.toContain('command_session')
  })

  it('removes the waiting line after the command-session result settles the run', async () => {
    const result: AgentToolResult = {
      callId: commandSessionCall.id,
      tool: 'command_session',
      ok: true,
      result: { status: 'completed' }
    }
    const screen = await render(
      <ChatMessageItem message={commandSessionMessage(result)} showTokenUsageDetails={false} />
    )

    expect(screen.container.querySelector('.agent-thinking')).toBeNull()
    expect(screen.container.textContent).not.toContain('正在等待命令完成')
    expect(screen.container.textContent).toContain('命令已经完成。')
  })
})
