import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useState } from 'react'
import { page } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import type { ChatMessage, ChatWorkflowDeliveryTimelineItem } from '../chatTypes'
import { getFrontendCssVariables } from '../../../config/frontendConfig'
import { copyTextToClipboard } from '../../../components/clipboard'
import { parseTimelineItem } from '../../storage/persistedAgentRunTimelineValidators'
import { applyAgentEventToChatMessage } from '../../agentRun/agentEventReducer'
import '../../../styles/global.css'
import '../ChatConversationPage.css'

vi.mock('../../../config/FrontendConfigProvider', async () => {
  const { getTranslation } = await import('../../../config/languageRegistry')
  return {
    useFrontendConfig: () => ({
      language: 'zh-CN',
      showCacheHitRate: false,
      t: (key: Parameters<typeof getTranslation>[1]) => getTranslation('zh-CN', key)
    })
  }
})
vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))
vi.mock('../../../components/toast/ToastContext', () => ({
  useToast: () => ({ showToast: vi.fn() })
}))
vi.mock('../../../components/clipboard', () => ({
  copyTextToClipboard: vi.fn(async () => undefined)
}))
vi.mock('../components/ImagePreview', () => ({
  useImagePreview: () => vi.fn(),
  useImagePreviewNotice: () => vi.fn()
}))
const { ChatMessageItem } = await import('../components/ChatMessageItem')

function mail(number: number): ChatWorkflowDeliveryTimelineItem {
  return {
    id: `workflow-delivery-${number}`,
    type: 'workflow_delivery',
    inputId: `input-${number}`,
    deliveryId: `workflow-message-${number}`,
    instanceId: 'workflow-1',
    workflowName: '文案润色',
    content: `[Organization mail]\n\n公共背景：完成文案任务。\n\n实际邮件${number}`,
    createdAt: number,
    traceSequence: number * 2,
    sources: [
      {
        nodeId: 'writer',
        nodeName: '文书1号',
        conversationId: 'writer-chat',
        conversationTitle: '文书对话',
        content: `实际邮件${number}`
      }
    ]
  }
}
function conversationMessage(collapsed = false): ChatMessage {
  return {
    id: 'assistant-1',
    role: 'assistant',
    createdAt: 1,
    status: 'sent',
    content: '最终回复',
    uiState: { timelineCollapsed: collapsed },
    agentRun: {
      runId: 'run-1',
      status: 'completed',
      completedAt: 50,
      startedAt: 1,
      toolDefinitions: [],
      toolCalls: [
        {
          id: 'query',
          tool: 'organization_get_mailbox',
          args: {},
          approvalStatus: 'not_required',
          reason: ''
        }
      ],
      toolResults: [],
      approvals: [],
      fileChangeProposals: [],
      timeline: [
        { id: 'before', type: 'message', content: '接手之前的回复', traceSequence: 1 },
        mail(1),
        { id: 'between', type: 'message', content: '收到第一封，继续处理', traceSequence: 3 },
        mail(2),
        { id: 'query', type: 'tool_call', callId: 'query', traceSequence: 5 }
      ]
    }
  }
}
function Fixture({
  item,
  onUiStateChange
}: {
  item: ChatMessage
  onUiStateChange?: (messageId: string, uiState: ChatMessage['uiState']) => void
}) {
  return (
    <div style={{ width: 820, padding: 32 }} className="chat-conversation-page__messages">
      <ChatMessageItem
        message={item}
        onUiStateChange={onUiStateChange}
        showTokenUsageDetails={false}
      />
    </div>
  )
}
function InteractiveFixture({ item }: { item: ChatMessage }) {
  const [uiState, setUiState] = useState(item.uiState)
  return (
    <Fixture
      item={{ ...item, uiState }}
      onUiStateChange={(_, nextUiState) => setUiState(nextUiState)}
    />
  )
}
function processHeader(container: HTMLElement) {
  return page.elementLocator(
    container.querySelector<HTMLButtonElement>('.agent-run__elapsed-button')!
  )
}
function visibleOrder(container: HTMLElement) {
  return [
    ...container.querySelectorAll('.agent-run > .chat-agent-text, .workflow-delivery__bubble')
  ].map((node) => node.textContent)
}
let previousStyle: string | null
beforeEach(async () => {
  vi.mocked(copyTextToClipboard).mockClear()
  previousStyle = document.documentElement.getAttribute('style')
  for (const [name, value] of Object.entries(getFrontendCssVariables()))
    document.documentElement.style.setProperty(name, value)
  await page.viewport(1000, 900)
})
afterEach(() => {
  if (previousStyle === null) document.documentElement.removeAttribute('style')
  else document.documentElement.setAttribute('style', previousStyle)
})

describe('organization mail inside a run', () => {
  it('inserts a live delivery before provisional response text and preserves its disclosure through completion', async () => {
    const fixture = conversationMessage()
    const active: ChatMessage = {
      ...fixture,
      status: 'pending',
      content: '正在接着处理',
      uiState: { timelineCollapsed: true },
      agentRun: {
        ...fixture.agentRun!,
        status: 'running',
        completedAt: undefined,
        timeline: [
          fixture.agentRun!.timeline[0],
          { id: 'stream', type: 'message', content: '正在接着处理', streamId: 'next-stream' }
        ]
      }
    }
    const view = await render(<Fixture item={active} />)
    const delivery = mail(1)
    const updated = applyAgentEventToChatMessage(active, {
      type: 'workflow_delivery_applied',
      conversationId: 'conversation-1',
      assistantMessageId: active.id,
      runId: active.agentRun!.runId!,
      inputId: delivery.inputId,
      deliveryId: delivery.deliveryId,
      instanceId: delivery.instanceId,
      workflowName: delivery.workflowName,
      content: delivery.content,
      createdAt: delivery.createdAt,
      sequence: delivery.traceSequence,
      sources: delivery.sources
    })
    await view.rerender(<Fixture item={updated} />)
    expect(visibleOrder(view.container)).toEqual(['接手之前的回复', '实际邮件1', '正在接着处理'])
    await view.getByRole('button', { name: '展开组织上下文' }).click()
    await view.rerender(
      <Fixture
        item={{
          ...updated,
          content: '最终回复',
          status: 'sent',
          agentRun: { ...updated.agentRun!, status: 'completed', completedAt: 60 }
        }}
      />
    )
    expect(view.container.querySelectorAll('.workflow-delivery')).toHaveLength(1)
    expect(view.container.textContent).toContain('公共背景：完成文案任务。')
    expect(view.container.textContent).not.toContain('正在接着处理')
    expect(visibleOrder(view.container).at(-1)).toBe('最终回复')
  })

  it('keeps only mail and the final answer when execution details are collapsed and restored', async () => {
    const item = conversationMessage(true)
    const view = await render(<Fixture item={item} />)
    expect(visibleOrder(view.container)).toEqual(['实际邮件1', '实际邮件2', '最终回复'])
    expect(view.container.querySelectorAll('article')).toHaveLength(1)
    expect(view.container.querySelectorAll('.chat-guidance')).toHaveLength(0)
    expect(view.container.querySelector('.agent-activity--workflow-query')).toBeNull()
    expect(view.container.textContent).not.toContain('公共背景')
    const restored = {
      ...item,
      agentRun: {
        ...item.agentRun!,
        timeline: item.agentRun!.timeline.map((entry) =>
          parseTimelineItem(JSON.parse(JSON.stringify(entry)))!
        )
      }
    }
    await view.rerender(<Fixture item={restored} />)
    expect(visibleOrder(view.container)).toEqual(['实际邮件1', '实际邮件2', '最终回复'])
    expect(view.container.querySelector('.agent-activity--workflow-query')).toBeNull()
  })

  it('restores the full sequence when the process header is expanded and hides narration again when collapsed', async () => {
    const view = await render(<InteractiveFixture item={conversationMessage(true)} />)
    const header = processHeader(view.container)
    await expect.element(header).toHaveAttribute('aria-expanded', 'false')
    expect(visibleOrder(view.container)).toEqual(['实际邮件1', '实际邮件2', '最终回复'])

    await header.click()
    await expect.element(header).toHaveAttribute('aria-expanded', 'true')
    expect(visibleOrder(view.container)).toEqual([
      '接手之前的回复',
      '实际邮件1',
      '收到第一封，继续处理',
      '实际邮件2',
      '最终回复'
    ])
    expect(view.container.querySelector('.agent-activity--workflow-query')).not.toBeNull()

    await header.click()
    await expect.element(header).toHaveAttribute('aria-expanded', 'false')
    expect(visibleOrder(view.container)).toEqual(['实际邮件1', '实际邮件2', '最终回复'])
    expect(view.container.querySelector('.agent-activity--workflow-query')).toBeNull()
    expect(view.container.querySelectorAll('.workflow-delivery')).toHaveLength(2)
    expect(view.container.querySelectorAll('.chat-guidance')).toHaveLength(0)
  })

  it.each(['completed', 'cancelled'] as const)(
    'does not promote narration into a final answer after a %s run without one',
    async (status) => {
      const fixture = conversationMessage(true)
      const view = await render(
        <InteractiveFixture
          item={{
            ...fixture,
            // Stopped records can retain the delta accumulator, but it is not a final answer.
            content: status === 'cancelled' ? '收到第一封，继续处理' : '',
            agentRun: { ...fixture.agentRun!, status }
          }}
        />
      )
      expect(visibleOrder(view.container)).toEqual(['实际邮件1', '实际邮件2'])
      await processHeader(view.container).click()
      expect(visibleOrder(view.container)).toEqual([
        '接手之前的回复',
        '实际邮件1',
        '收到第一封，继续处理',
        '实际邮件2'
      ])
      await processHeader(view.container).click()
      expect(visibleOrder(view.container)).toEqual(['实际邮件1', '实际邮件2'])
    }
  )

  it('retains a failed run public partial answer even when the hidden timeline contains the same text', async () => {
    const fixture = conversationMessage(true)
    const partial = '已完成初步分析，连接中断前保留这些结果。'
    const view = await render(
      <InteractiveFixture
        item={{
          ...fixture,
          content: partial,
          agentRun: {
            ...fixture.agentRun!,
            status: 'failed',
            interruption: { reason: 'stream_interrupted' },
            timeline: [
              mail(1),
              { id: 'partial', type: 'message', content: partial, traceSequence: 3 }
            ]
          }
        }}
      />
    )
    expect(visibleOrder(view.container)).toEqual(['实际邮件1', partial])
    await processHeader(view.container).click()
    expect(visibleOrder(view.container)).toEqual(['实际邮件1', partial])
    await processHeader(view.container).click()
    expect(visibleOrder(view.container)).toEqual(['实际邮件1', partial])
  })

  it('shows the trusted source, expands and copies each mail independently', async () => {
    const view = await render(<Fixture item={conversationMessage()} />)
    const first = view.container.querySelector<HTMLElement>('[data-workflow-input-id="input-1"]')!
    expect(first.querySelector('[data-input-origin="workflow"]')?.textContent).toContain(
      '来自组织文案润色· 文书1号'
    )
    await page.elementLocator(first).getByRole('button', { name: '复制消息', exact: true }).click()
    expect(copyTextToClipboard).toHaveBeenLastCalledWith('实际邮件1')
    await page.elementLocator(first).getByRole('button', { name: '展开组织上下文' }).click()
    expect(first.textContent).toContain('公共背景：完成文案任务。')
    const second = view.container.querySelector('[data-workflow-input-id="input-2"]')!
    expect(second.textContent).not.toContain('公共背景')
    await page.elementLocator(first).getByRole('button', { name: '已复制', exact: true }).click()
    expect(copyTextToClipboard).toHaveBeenLastCalledWith(mail(1).content)
    await page.elementLocator(first).getByRole('button', { name: '收起组织上下文' }).click()
    expect(first.textContent).not.toContain('公共背景')
    expect(first.querySelector('button[aria-label="编辑消息"]')).toBeNull()
  })
})
