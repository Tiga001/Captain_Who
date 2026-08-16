import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatMessage } from '../chatTypes'
import { ChatMessageItem } from '../components/ChatMessageItem'

const translations: Record<string, string> = {
  'agent.thinking': '正在思考',
  'agent.processed': '已处理 {duration}',
  'agent.llmRetry.rateLimited': '服务限流，{seconds}秒后自动重试（{attempt}/{maxAttempts}）',
  'agent.llmRetry.temporarilyUnavailable':
    '模型服务暂时不可用，{seconds}秒后自动重试（{attempt}/{maxAttempts}）',
  'agent.llmRetry.retrying': '模型服务正在自动重试（{attempt}/{maxAttempts}）',
  'agent.llmRetry.reconnecting': '正在重新连接（{attempt}/{maxAttempts}）'
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

function retryMessage(category: 'rate_limited' | 'network'): ChatMessage {
  return {
    id: 'assistant-retry',
    role: 'assistant',
    content: '',
    createdAt: Date.now() - 1_000,
    status: 'pending',
    agentRun: {
      runId: 'run-retry',
      status: 'running',
      startedAt: Date.now() - 1_000,
      toolDefinitions: [],
      toolCalls: [],
      toolResults: [],
      approvals: [],
      diffs: [],
      timeline: [],
      llmRetry: {
        category,
        providerCode: 'provider_secret_code',
        delayMs: 5_000,
        retryAt: Date.now() + 5_000,
        attempt: 2,
        maxAttempts: 3
      }
    }
  }
}

describe('LLM retry transient status', () => {
  it('shows a concise reconnect status without provider diagnostics', async () => {
    const screen = await render(
      <ChatMessageItem message={retryMessage('rate_limited')} showTokenUsageDetails={false} />
    )

    const text = screen.container.textContent ?? ''
    expect(text).toContain('正在重新连接')
    expect(text).toContain('1/2')
    expect(screen.container.textContent).not.toContain('provider_secret_code')
  })

  it('uses the same stable label for transport failures', async () => {
    const screen = await render(
      <ChatMessageItem message={retryMessage('network')} showTokenUsageDetails={false} />
    )

    expect(screen.container.textContent).toContain('正在重新连接（1/2）')
    expect(screen.container.textContent).not.toContain('provider_secret_code')
  })
})
