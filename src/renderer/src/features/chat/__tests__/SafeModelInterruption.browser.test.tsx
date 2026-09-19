import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatMessage } from '../chatTypes'
import { ChatMessageItem } from '../components/ChatMessageItem'

const translations: Record<string, string> = {
  'agent.interruption.serviceConnectionFailed': '模型服务连接失败',
  'agent.interruption.outputLimitReached': '输出达到上限，回复未完成',
  'agent.interruption.emptyResponse': '模型未返回有效回复',
  'agent.interruption.streamInterrupted': '模型连接中断，回复未完成',
  'agent.interruption.admissionUnconfirmed': '发送结果尚未确认。已尝试核对，未自动重发。'
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

function interruptedMessage(): ChatMessage {
  return {
    id: 'assistant-safe-interruption',
    role: 'assistant',
    content: '',
    createdAt: 1,
    status: 'sent',
    agentRun: {
      runId: 'run-safe-interruption',
      status: 'failed',
      startedAt: 1,
      completedAt: 2,
      toolDefinitions: [],
      toolCalls: [],
      toolResults: [],
      approvals: [],
      fileChangeProposals: [],
      timeline: [],
      interruption: { reason: 'service_connection_failed' }
    }
  }
}

describe('safe model interruption status', () => {
  it.each([
    ['output_limit_reached', '输出达到上限，回复未完成', ''],
    ['empty_response', '模型未返回有效回复', ''],
    ['stream_interrupted', '模型连接中断，回复未完成', '已生成的部分正文']
  ] as const)(
    'shows %s once without a recovery button, including empty replies',
    async (reason, text, content) => {
      const message = interruptedMessage()
      message.content = content
      message.agentRun!.interruption = { reason }
      message.agentRun!.finishReason = reason === 'output_limit_reached' ? 'length' : undefined
      message.agentRun!.error = 'raw provider diagnostic must not be duplicated'
      const screen = await render(
        <ChatMessageItem message={message} showTokenUsageDetails={false} />
      )
      await expect.element(screen.getByText(text)).toBeVisible()
      expect(screen.container.querySelectorAll('.agent-run__interruption')).toHaveLength(1)
      expect(screen.container.querySelector('.agent-run__notice')).toBeNull()
      expect(screen.container.querySelector('.agent-run__error')).toBeNull()
      expect(screen.container.querySelector('button')).toBeNull()
      if (content) await expect.element(screen.getByText(content)).toBeVisible()
    }
  )

  it('uses the same red interruption row for legacy token-limit finish reasons', async () => {
    const message = interruptedMessage()
    message.agentRun!.interruption = undefined
    message.agentRun!.status = 'completed'
    message.agentRun!.finishReason = 'length'
    const screen = await render(<ChatMessageItem message={message} showTokenUsageDetails={false} />)
    await expect.element(screen.getByText('输出达到上限，回复未完成')).toBeVisible()
    expect(screen.container.querySelector('.agent-run__notice')).toBeNull()
  })

  it('keeps the interruption visible when the execution timeline is collapsed', async () => {
    const message = interruptedMessage()
    message.uiState = { timelineCollapsed: true }
    message.agentRun!.interruption = { reason: 'empty_response' }
    message.agentRun!.timeline = [
      { id: 'narration', type: 'message', content: '先前执行播报' },
      {
        id: 'compaction',
        type: 'context_compaction',
        operationId: 'op-compaction',
        status: 'applied'
      }
    ]
    const screen = await render(<ChatMessageItem message={message} showTokenUsageDetails={false} />)
    await expect.element(screen.getByText('模型未返回有效回复')).toBeVisible()
    expect(screen.container.querySelector('.agent-run__interruption')).not.toBeNull()
    expect(screen.container.textContent).not.toContain('先前执行播报')
  })
  it('distinguishes unconfirmed admission from a rejected send', async () => {
    const message = interruptedMessage()
    message.agentRun!.runId = null
    message.agentRun!.interruption = { reason: 'admission_unconfirmed' }
    const screen = await render(<ChatMessageItem message={message} showTokenUsageDetails={false} />)
    await expect
      .element(screen.getByText('发送结果尚未确认。已尝试核对，未自动重发。'))
      .toBeVisible()
  })

  it('shows one compact translated reason without a raw error body', async () => {
    const screen = await render(
      <ChatMessageItem message={interruptedMessage()} showTokenUsageDetails={false} />
    )

    await expect.element(screen.getByText('模型服务连接失败')).toBeVisible()
    expect(screen.container.querySelector('.agent-run__interruption')).not.toBeNull()
    expect(screen.container.querySelector('.agent-run__error')).toBeNull()
  })
})
