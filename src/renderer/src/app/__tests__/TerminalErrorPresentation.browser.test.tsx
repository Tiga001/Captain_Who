import { useState } from 'react'
import type { AgentEvent } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { applyAgentEventToChatMessage } from '../../features/agentRun/agentEventReducer'
import type { ChatMessage } from '../../features/chat/chatTypes'
import { AgentRunView } from '../../features/chat/components/AgentRunView'
import {
  parsePersistedAgentRunJson,
  stringifyPersistedAgentRun
} from '../../features/storage/persistedAgentRun'

vi.mock('../../config/FrontendConfigProvider', async () => {
  const { getTranslation } = await import('../../config/languageRegistry')
  return {
    useFrontendConfig: () => ({
      language: 'zh-CN',
      t: (key: Parameters<typeof getTranslation>[1]) => getTranslation('zh-CN', key)
    })
  }
})
vi.mock('../../host/hostClient', () => ({ hostClient: {} }))
vi.mock('../../features/chat/components/ImagePreview', () => ({
  useImagePreview: () => vi.fn(),
  useImagePreviewNotice: () => vi.fn()
}))

const ERROR = '会话轨迹已经保存，但无法刷新派生上下文状态。'

function failedMessage(finalContent = ERROR, runId = 'run-error'): ChatMessage {
  const initial: ChatMessage = {
    id: `message-${runId}`,
    role: 'assistant',
    content: '',
    createdAt: 1,
    status: 'pending',
    agentRun: {
      runId,
      status: 'running',
      startedAt: 1,
      toolDefinitions: [],
      toolCalls: [],
      toolResults: [],
      approvals: [],
      fileChangeProposals: [],
      timeline: []
    }
  }
  const events: AgentEvent[] = [
    {
      type: 'error',
      runId,
      traceSequence: 1,
      message: ERROR,
      recoverable: false,
      code: 'conversation_context_refresh_failed'
    },
    { type: 'done', runId, success: false, status: 'failed', content: finalContent }
  ]
  return events.reduce(applyAgentEventToChatMessage, initial)
}

function ErrorView({ message }: { message: ChatMessage }) {
  const [collapsed, setCollapsed] = useState(false)
  return (
    <AgentRunView
      conversationId="conversation-error"
      message={message}
      mode="interactive"
      onTimelineCollapsedChange={(_, next) => setCollapsed(next)}
      timelineCollapsedOverride={collapsed}
    />
  )
}

function occurrences(element: Element, text: string) {
  return (element.textContent ?? '').split(text).length - 1
}

describe('terminal error presentation', () => {
  it('shows an Error followed by Done once when expanded, collapsed, or replayed', async () => {
    const message = failedMessage()
    const view = await render(<ErrorView message={message} />)

    expect(occurrences(view.container, ERROR)).toBe(1)
    expect(view.container.querySelectorAll('.agent-activity--error')).toHaveLength(1)
    const header = view.getByRole('button', { name: /已处理/ })
    await header.click()
    expect(occurrences(view.container, ERROR)).toBe(1)
    await header.click()
    expect(occurrences(view.container, ERROR)).toBe(1)

    const replayed = applyAgentEventToChatMessage(message, {
      type: 'done',
      runId: message.agentRun!.runId!,
      success: false,
      status: 'failed',
      content: ERROR
    })
    await view.rerender(<ErrorView message={replayed} />)
    expect(occurrences(view.container, ERROR)).toBe(1)
    expect(replayed.agentRun?.timeline).toEqual(message.agentRun?.timeline)
  })

  it('keeps the persisted error history while displaying the restored failure once', async () => {
    const message = failedMessage()
    const stored = stringifyPersistedAgentRun(message.agentRun)
    const restored = { ...message, agentRun: parsePersistedAgentRunJson(stored) }
    const beforeRender = JSON.stringify(restored)
    const view = await render(<ErrorView message={restored} />)

    expect(restored.agentRun?.timeline).toContainEqual(
      expect.objectContaining({ type: 'error', message: ERROR })
    )
    expect(restored.agentRun?.timeline).toContainEqual(
      expect.objectContaining({ type: 'message', content: ERROR })
    )
    expect(occurrences(view.container, ERROR)).toBe(1)
    await view.getByRole('button', { name: /已处理/ }).click()
    expect(occurrences(view.container, ERROR)).toBe(1)
    expect(JSON.stringify(restored)).toBe(beforeRender)
  })

  it('retains a distinct final response alongside the failure details', async () => {
    const response = '已完成本地分析；浏览器操作尚未开始。'
    const view = await render(<ErrorView message={failedMessage(response)} />)

    expect(occurrences(view.container, ERROR)).toBe(1)
    expect(occurrences(view.container, response)).toBe(1)
    await view.getByRole('button', { name: /已处理/ }).click()
    expect(occurrences(view.container, response)).toBe(1)
    await view.getByRole('button', { name: /已处理/ }).click()
    expect(occurrences(view.container, ERROR)).toBe(1)
    expect(occurrences(view.container, response)).toBe(1)
  })

  it('does not duplicate the final error through the run-level fallback without an error entry', async () => {
    const message = failedMessage()
    message.agentRun!.timeline = [
      { id: 'narration', type: 'message', content: '准备启动浏览器。', traceSequence: 0 },
      ...message.agentRun!.timeline.filter((item) => item.type !== 'error')
    ]
    const view = await render(<ErrorView message={message} />)

    expect(occurrences(view.container, ERROR)).toBe(1)
    expect(occurrences(view.container, '准备启动浏览器。')).toBe(1)
  })

  it('retains the same error separately for two failed Runs', async () => {
    const view = await render(
      <>
        <ErrorView message={failedMessage(ERROR, 'run-first')} />
        <ErrorView message={failedMessage(ERROR, 'run-second')} />
      </>
    )

    expect(view.container.querySelectorAll('.agent-run')).toHaveLength(2)
    expect(occurrences(view.container, ERROR)).toBe(2)
  })
})
