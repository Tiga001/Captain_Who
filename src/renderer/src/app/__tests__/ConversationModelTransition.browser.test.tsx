import { userEvent } from 'vitest/browser'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import '../../styles/global.css'
import '../../features/chat/ChatConversationPage.messages.css'

const { retrySpy } = vi.hoisted(() => ({ retrySpy: vi.fn() }))

const translations: Record<string, string> = {
  'chat.modelTransition.cancel': '取消',
  'chat.modelTransition.confirm': '压缩并切换',
  'chat.modelTransition.providerTitle': '切换 API 厂商',
  'chat.modelTransition.providerDescription': 'API 厂商不同，新模型需要先压缩历史完成适配。',
  'chat.modelTransition.protocolTitle': '切换兼容规则',
  'chat.modelTransition.protocolDescription': '兼容规则不同，新模型需要先压缩历史完成适配。',
  'chat.modelTransition.running': '正在压缩历史并切换模型…',
  'chat.modelTransition.succeeded': '切换API厂商，历史已压缩',
  'chat.modelTransition.failed': '历史压缩失败 · 重试',
  'chat.modelTransition.modelChangeTooltip': '从 {source} 切换到 {target}'
}

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => translations[key] ?? key })
}))

const { ConversationModelTransitionDivider, ModelTransitionConfirmationDialog } =
  await import('../../features/chat/components/ConversationModelTransition')

beforeEach(() => retrySpy.mockReset())

describe('provider transition presentation', () => {
  it('uses the approved API-provider confirmation copy', async () => {
    const screen = await render(
      <ModelTransitionConfirmationDialog
        preflight={{
          conversationId: 'conversation-1',
          targetModelId: 'deepseek-model',
          decision: 'requires_compaction',
          operationId: 'operation-1',
          transitionToken: 'transition-1',
          reason: 'api_provider_changed'
        }}
        onCancel={() => undefined}
        onConfirm={() => undefined}
      />
    )

    await expect.element(screen.getByRole('heading', { name: '切换 API 厂商' })).toBeVisible()
    await expect
      .element(screen.getByText('API 厂商不同，新模型需要先压缩历史完成适配。'))
      .toBeVisible()
    await expect.element(screen.getByText('取消')).toBeVisible()
    await expect.element(screen.getByRole('button', { name: '压缩并切换' })).toBeVisible()
  })

  it('uses compatibility-rule copy for a same-provider protocol change', async () => {
    const screen = await render(
      <ModelTransitionConfirmationDialog
        preflight={{
          conversationId: 'conversation-1',
          targetModelId: 'generic-anthropic',
          decision: 'requires_compaction',
          operationId: 'operation-2',
          transitionToken: 'transition-2',
          reason: 'provider_protocol_changed'
        }}
        onCancel={() => undefined}
        onConfirm={() => undefined}
      />
    )

    await expect.element(screen.getByRole('heading', { name: '切换兼容规则' })).toBeVisible()
    await expect
      .element(screen.getByText('兼容规则不同，新模型需要先压缩历史完成适配。'))
      .toBeVisible()
  })

  it('renders running state as a divider rather than a chat bubble', async () => {
    const screen = await render(
      <ConversationModelTransitionDivider
        operation={{
          schemaVersion: 1,
          status: 'running',
          conversationId: 'conversation-1',
          targetModelId: 'deepseek-model',
          operationId: 'operation-1',
          coveredThroughMessageId: 'assistant-1',
          startedAt: 10
        }}
      />
    )

    const divider = screen.getByTestId('model-transition-divider').element()
    expect(divider.classList.contains('conversation-continuation-divider')).toBe(true)
    expect(divider.closest('.chat-message')).toBeNull()
    const runningLabel = screen.getByText('正在压缩历史并切换模型…')
    await expect.element(runningLabel).toBeVisible()
    expect(runningLabel.element()).toHaveClass('agent-running-text')
    await expect
      .element(screen.getByRole('button', { name: '正在压缩历史并切换模型…' }))
      .toBeDisabled()
    expect(divider.querySelector('.lucide-fold-vertical')).not.toBeNull()
  })

  it('uses the compaction icon and shows the durable model transition on hover', async () => {
    const screen = await render(
      <ConversationModelTransitionDivider
        operation={{
          schemaVersion: 1,
          status: 'completed',
          conversationId: 'conversation-1',
          sourceModelDisplayName: 'GPT-5.2',
          targetModelDisplayName: 'DeepSeek V4',
          targetModelId: 'deepseek-model',
          operationId: 'operation-1',
          coveredThroughMessageId: 'assistant-1',
          startedAt: 10,
          completedAt: 20,
          conversationUpdatedAt: 21,
          modelId: 'deepseek-model',
          summaryId: 'summary-1'
        }}
      />
    )

    const divider = screen.getByTestId('model-transition-divider').element()
    expect(divider.querySelector('.lucide-fold-vertical')).not.toBeNull()
    const completedLabel = screen.getByText('切换API厂商，历史已压缩')
    await expect.element(completedLabel).toBeVisible()
    expect(completedLabel.element()).not.toHaveClass('agent-running-text')

    const tooltipAnchor = divider.querySelector('.conversation-model-transition__tooltip-anchor')
    expect(tooltipAnchor).not.toBeNull()
    if (tooltipAnchor) await userEvent.hover(tooltipAnchor)

    await expect
      .element(screen.getByRole('tooltip'))
      .toHaveTextContent('从 GPT-5.2 切换到 DeepSeek V4')
  })

  it('offers retry only for a failed transition', async () => {
    const screen = await render(
      <ConversationModelTransitionDivider
        operation={{
          schemaVersion: 1,
          status: 'failed',
          conversationId: 'conversation-1',
          targetModelId: 'deepseek-model',
          operationId: 'operation-1',
          coveredThroughMessageId: 'assistant-1',
          startedAt: 10,
          completedAt: 20,
          error: {
            code: 'provider_transition_generation_failed',
            message: 'The transition could not be completed.',
            recovery: 'retry'
          }
        }}
        onRetry={retrySpy}
      />
    )

    await userEvent.click(screen.getByRole('button', { name: '历史压缩失败 · 重试' }))
    expect(screen.getByText('历史压缩失败 · 重试').element()).not.toHaveClass('agent-running-text')
    expect(retrySpy).toHaveBeenCalledTimes(1)
  })
})
