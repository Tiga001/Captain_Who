import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { ToastProvider } from '../../../components/toast/ToastProvider'
import type { ChatMessage } from '../chatTypes'
import { ChatMessageItem } from '../components/ChatMessageItem'

const translations: Record<string, string> = {
  'agent.processed': '已处理 {duration}',
  'agent.thinking': '正在思考',
  'chat.guidanceSubmittingStatus': '正在发送',
  'chat.guidanceQueued': '已排队',
  'chat.guidanceInterrupted': '未生效'
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

function settledGuidanceMessage(collapsed: boolean): ChatMessage {
  return {
    id: 'assistant-guidance',
    role: 'assistant',
    content: '最终回答',
    createdAt: 1,
    status: 'sent',
    uiState: { timelineCollapsed: collapsed },
    agentRun: {
      runId: 'run-guidance',
      status: 'completed',
      startedAt: 1,
      completedAt: 10,
      toolDefinitions: [],
      toolCalls: [],
      toolResults: [],
      approvals: [],
      fileChangeProposals: [],
      timeline: [
        {
          id: 'narration-before',
          type: 'message',
          content: '生效前的过程文本',
          traceSequence: 0
        },
        {
          id: 'guidance-applied',
          type: 'user_guidance',
          guidanceId: 'guidance-1',
          clientMessageId: 'client-1',
          content: '第一条追加消息',
          attachments: [],
          status: 'applied',
          createdAt: 2,
          sequence: 1,
          traceSequence: 1
        },
        {
          id: 'narration-after',
          type: 'message',
          content: '生效后的过程文本',
          traceSequence: 2
        },
        {
          id: 'guidance-queued',
          type: 'user_guidance',
          guidanceId: 'guidance-2',
          clientMessageId: 'client-2',
          content: '第二条仍在排队',
          attachments: [],
          status: 'queued',
          createdAt: 3
        },
        {
          id: 'final-answer',
          type: 'message',
          content: '最终回答',
          traceSequence: 3
        }
      ]
    }
  }
}

function textPosition(container: HTMLElement, text: string) {
  const content = container.textContent ?? ''
  const position = content.indexOf(text)
  expect(position).toBeGreaterThanOrEqual(0)
  return position
}

function renderMessage(message: ChatMessage) {
  return render(
    <ToastProvider>
      <ChatMessageItem message={message} showTokenUsageDetails={false} />
    </ToastProvider>
  )
}

describe('mid-turn guidance Timeline presentation', () => {
  it('keeps applied guidance interleaved at its durable position while expanded', async () => {
    const screen = await renderMessage(settledGuidanceMessage(false))

    const before = textPosition(screen.container, '生效前的过程文本')
    const guidance = textPosition(screen.container, '第一条追加消息')
    const after = textPosition(screen.container, '生效后的过程文本')
    const queued = textPosition(screen.container, '第二条仍在排队')
    const finalAnswer = textPosition(screen.container, '最终回答')

    expect(before).toBeLessThan(guidance)
    expect(guidance).toBeLessThan(after)
    expect(after).toBeLessThan(queued)
    expect(queued).toBeLessThan(finalAnswer)
  })

  it('shows every guidance message above the final answer while execution details are collapsed', async () => {
    const screen = await renderMessage(settledGuidanceMessage(true))

    expect(screen.container.textContent).not.toContain('生效前的过程文本')
    expect(screen.container.textContent).not.toContain('生效后的过程文本')
    expect(screen.container.querySelectorAll('.chat-guidance')).toHaveLength(2)
    expect(screen.container.textContent).toContain('已排队')

    const firstGuidance = textPosition(screen.container, '第一条追加消息')
    const secondGuidance = textPosition(screen.container, '第二条仍在排队')
    const finalAnswer = textPosition(screen.container, '最终回答')

    expect(firstGuidance).toBeLessThan(secondGuidance)
    expect(secondGuidance).toBeLessThan(finalAnswer)
  })
})
