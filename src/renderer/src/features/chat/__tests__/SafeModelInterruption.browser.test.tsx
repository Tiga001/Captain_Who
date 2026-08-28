import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatMessage } from '../chatTypes'
import { ChatMessageItem } from '../components/ChatMessageItem'

const translations: Record<string, string> = {
  'agent.interruption.serviceConnectionFailed': '模型服务连接失败'
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
  it('shows one compact translated reason without a raw error body', async () => {
    const screen = await render(
      <ChatMessageItem message={interruptedMessage()} showTokenUsageDetails={false} />
    )

    await expect.element(screen.getByText('模型服务连接失败')).toBeVisible()
    expect(screen.container.querySelector('.agent-run__interruption')).not.toBeNull()
    expect(screen.container.querySelector('.agent-run__error')).toBeNull()
  })
})
