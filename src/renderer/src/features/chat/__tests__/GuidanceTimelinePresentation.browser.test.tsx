import { describe, expect, it, vi } from 'vitest'
import type { AgentEvent } from '@mycopilot/protocol'
import { render } from 'vitest-browser-react'
import { ToastProvider } from '../../../components/toast/ToastProvider'
import type { ChatConversation, ChatMessage } from '../chatTypes'
import { ChatMessageItem } from '../components/ChatMessageItem'
import { question, submitted } from '../../humanInteraction/__tests__/humanInteractionFixtures'
import { projectHumanInteractionConversation } from '../../humanInteraction/humanInteractionPresentation'
import { humanInteractionResponseDisplay } from '../../humanInteraction/humanInteractionState'
import { applyAgentEventToChatMessage } from '../../agentRun/agentEventReducer'

const translations: Record<string, string> = {
  'agent.processed': '已处理 {duration}',
  'agent.thinking': '正在思考',
  'chat.guidanceSubmittingStatus': '正在发送',
  'chat.guidanceQueued': '已排队',
  'chat.guidanceInterrupted': '未生效',
  'humanInteraction.timeline.answer': '交互 · 共 {count} 项',
  'humanInteraction.history.skipped': '已跳过'
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

describe('pending interaction Timeline chronology', () => {
  it('keeps a submitted synchronous answer before every resumed delta while its live ToolResult is absent', async () => {
    const receipt = submitted({ ...question('sync'), mode: 'sync' })
    const display = humanInteractionResponseDisplay(receipt)!
    let message: ChatMessage = {
      ...settledGuidanceMessage(false),
      id: receipt.assistantMessageId,
      content: '先确认这两个问题。',
      status: 'pending',
      agentRun: {
        ...settledGuidanceMessage(false).agentRun!,
        runId: receipt.runId,
        status: 'waiting_for_user_input',
        completedAt: undefined,
        toolCalls: [
          {
            id: receipt.toolCallId,
            tool: 'request_user_input',
            args: {},
            approvalStatus: 'not_required',
            reason: null
          }
        ],
        toolResults: [],
        timeline: [
          {
            id: 'intro',
            type: 'message',
            content: '先确认这两个问题。',
            streamId: 'run-chat-stream-1',
            traceSequence: 1
          },
          {
            id: 'question',
            type: 'tool_call',
            callId: receipt.toolCallId,
            traceSequence: 2
          }
        ]
      }
    }
    const view = () => {
      const chat: ChatConversation = {
        id: 'chat',
        title: 'Sync answer',
        modelId: null,
        projectId: null,
        createdAt: 1,
        updatedAt: 2,
        messages: [message]
      }
      return (
        <ToastProvider>
          <ChatMessageItem
            message={projectHumanInteractionConversation(chat, [receipt]).messages[0]}
            showTokenUsageDetails={false}
          />
        </ToastProvider>
      )
    }
    const screen = await render(view())
    const assertAnswer = (resumed = false) => {
      expect(screen.container.querySelectorAll('.human-interaction-answer')).toHaveLength(1)
      expect(screen.container.querySelectorAll('.human-interaction-answer__pair')).toHaveLength(2)
      expect(screen.container.querySelectorAll('[data-human-request-id]')).toHaveLength(0)
      expect(textPosition(screen.container, '先确认这两个问题。')).toBeLessThan(
        textPosition(screen.container, '已跳过')
      )
      if (resumed)
        expect(textPosition(screen.container, '已跳过')).toBeLessThan(
          textPosition(screen.container, '收到跳过选择')
        )
    }
    const apply = async (event: AgentEvent) => {
      message = applyAgentEventToChatMessage(message, event)
      await screen.rerender(view())
    }
    assertAnswer()
    await apply({ type: 'started', runId: receipt.runId, toolDefinitions: [] })
    assertAnswer()
    await apply({
      type: 'message_stream_started',
      runId: receipt.runId,
      streamId: 'run-chat-stream-2',
      attempt: 1
    })
    assertAnswer()
    for (const delta of ['收到跳过选择，', '我会根据已有信息继续。']) {
      await apply({
        type: 'message_delta',
        runId: receipt.runId,
        streamId: 'run-chat-stream-2',
        delta
      })
      expect(message.agentRun!.toolResults).toHaveLength(0)
      assertAnswer(true)
    }
    await apply({
      type: 'message_stream_committed',
      runId: receipt.runId,
      streamId: 'run-chat-stream-2',
      traceSequence: 4
    })
    assertAnswer(true)
    await apply({
      type: 'done',
      runId: receipt.runId,
      success: true,
      status: 'completed',
      content: '收到跳过选择，我会根据已有信息继续。'
    })
    assertAnswer(true)
    // A terminal storage read reconstructs the original continuation result from durable Trace.
    // It must only replace the receipt projection, without moving or duplicating the answer.
    message = structuredClone(message)
    message.agentRun!.toolResults = [
      {
        callId: receipt.toolCallId,
        tool: 'request_user_input',
        ok: true,
        result: display
      }
    ]
    await screen.rerender(view())
    assertAnswer(true)
  })

  it('interleaves the unique batch entry before subsequent narration while collapsed, streaming and reloaded', async () => {
    const request = question(),
      open = vi.fn()
    const chat: ChatConversation = {
      id: 'chat',
      title: 'Interaction',
      modelId: null,
      projectId: null,
      createdAt: 1,
      updatedAt: 1,
      messages: [
        {
          ...settledGuidanceMessage(true),
          id: request.assistantMessageId,
          content: '兴趣爱好：问题已发出。',
          agentRun: {
            ...settledGuidanceMessage(true).agentRun!,
            runId: request.runId,
            toolCalls: [
              {
                id: request.toolCallId,
                tool: 'request_user_input_async',
                args: {},
                approvalStatus: 'not_required',
                reason: null
              }
            ],
            toolResults: [
              {
                callId: request.toolCallId,
                tool: 'request_user_input_async',
                ok: true,
                result: { accepted: true }
              }
            ],
            timeline: [
              { id: 'intro', type: 'message', content: '兴趣爱好：', traceSequence: 1 },
              { id: 'question', type: 'tool_call', callId: request.toolCallId, traceSequence: 2 },
              { id: 'after', type: 'message', content: '问题已发出。', traceSequence: 3 }
            ]
          }
        }
      ]
    }
    const view = (source: ChatConversation, pending = true) => (
      <ToastProvider>
        <ChatMessageItem
          message={
            projectHumanInteractionConversation(source, [
              pending ? request : { ...request, status: 'ignored' }
            ]).messages[0]
          }
          humanInteraction={{ openRequests: pending ? [request] : [], canInteract: true, open }}
          showTokenUsageDetails={false}
        />
      </ToastProvider>
    )
    const screen = await render(view(chat))
    const assertOrder = () => {
      expect(screen.container.querySelectorAll('[data-human-request-id]')).toHaveLength(1)
      expect(textPosition(screen.container, '兴趣爱好：')).toBeLessThan(
        textPosition(screen.container, '交互 · 共 2 项')
      )
      expect(textPosition(screen.container, '交互 · 共 2 项')).toBeLessThan(
        textPosition(screen.container, '问题已发出。')
      )
      expect(screen.container.textContent!.split('兴趣爱好：')).toHaveLength(2)
    }
    assertOrder()
    await screen.getByRole('button', { name: '交互 · 共 2 项' }).click()
    expect(open).toHaveBeenCalledExactlyOnceWith(request.requestId)
    const later = structuredClone(chat)
    later.messages[0].agentRun!.status = 'running'
    later.messages[0].agentRun!.timeline.push({
      id: 'later',
      type: 'message',
      content: '继续独立工作。',
      traceSequence: 4
    })
    await screen.rerender(view(later))
    assertOrder()
    expect(textPosition(screen.container, '问题已发出。')).toBeLessThan(
      textPosition(screen.container, '继续独立工作。')
    )
    await screen.rerender(view(structuredClone(chat)))
    assertOrder()
    await screen.rerender(view(chat, false))
    expect(screen.container.querySelectorAll('[data-human-request-id]')).toHaveLength(0)
  })
})
