import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { CSSProperties } from 'react'
import { page } from 'vitest/browser'
import { frontendConfig, getFrontendCssVariables } from '../../../config/frontendConfig'
import { classicLightTheme } from '../../../config/themes/classic'
import '../../../styles/global.css'
import '../ChatConversationPage.css'
import { render } from 'vitest-browser-react'
import { ConversationNavigationProvider } from '../ConversationNavigationContext'
import { WorkflowSendToolActivity } from '../components/toolActivities/WorkflowSendToolActivity'
import { AgentToolActivity } from '../components/toolActivities/AgentToolActivity'
import type { ChatAgentRunView } from '../chatTypes'

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ language: 'zh-CN', t: (key: string) => key })
}))
vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))
const copy = vi.fn().mockResolvedValue(undefined)
vi.mock('../components/clipboard', () => ({ copyTextToClipboard: (text: string) => copy(text) }))

const metadata = {
  organizationName: '新功能开发',
  instanceId: 'workflow-private-id',
  messages: [
    {
      targetNodeId: 'node-a',
      targetNodeName: '开发',
      targetConversationId: 'chat-a'
    },
    { targetNodeId: 'node-b', targetNodeName: '验收', targetConversationId: null }
  ]
}
const call: AgentToolCall = {
  id: 'call-send',
  tool: 'organization_send',
  approvalStatus: 'not_required',
  reason: null,
  args: {
    messages: [
      { to: '开发', message: '暗号：升龙拳\n保留换行' },
      { to: '验收', message: '请验收' }
    ],
    _organizationSend: metadata
  }
}
const receipt: AgentToolResult = {
  callId: call.id,
  tool: call.tool,
  ok: true,
  result: { ...metadata, accepted: true, deliveryId: 'private-delivery-id' }
}
afterEach(() => {
  copy.mockClear()
})

describe('organization send presentation', () => {
  it('renders running destinations and exact per-target messages with copy and conversation actions', async () => {
    await page.viewport(1000, 700)
    const open = vi.fn()
    const view = await render(
      <div
        style={
          {
            ...getFrontendCssVariables(frontendConfig, classicLightTheme),
            width: 820,
            padding: 24,
            background: 'var(--mc-color-surface-main-panel)',
            fontFamily: 'var(--mc-font-family)'
          } as CSSProperties
        }
      >
        <ConversationNavigationProvider onOpenConversation={open}>
          <WorkflowSendToolActivity call={call} />
        </ConversationNavigationProvider>
      </div>
    )
    const summary = view.getByText('正在向组织【新功能开发】的节点【开发、验收】发送邮件')
    await expect.element(summary).toBeVisible()
    await summary.click()
    await expect.element(view.getByText('暗号：升龙拳\n保留换行')).toBeVisible()
    await page.screenshot({
      element: view.container.firstElementChild as HTMLElement,
      path: '../../../../../../.cache/workflow-authoring/workflow-send-expanded.png'
    })
    await view.getByRole('button', { name: '复制邮件 · 开发' }).click()
    expect(copy).toHaveBeenCalledExactlyOnceWith('暗号：升龙拳\n保留换行')
    await view.getByRole('button', { name: '打开对话 · 开发' }).click()
    expect(open).toHaveBeenCalledExactlyOnceWith('chat-a')
    expect(view.getByRole('button', { name: '打开对话 · 验收' }).elements()).toHaveLength(0)
    expect(view.container.textContent).not.toContain('organization_send')
    expect(view.container.textContent).not.toContain('flow-a')
  })

  it('routes the tool to the dedicated presentation and uses frozen receipt names after cancellation', async () => {
    const view = await render(
      <AgentToolActivity
        call={call}
        result={receipt}
        cancelled
        run={{} as ChatAgentRunView}
        showImageGenerationPreview={false}
      />
    )
    await expect
      .element(view.getByText('已向组织【新功能开发】的节点【开发、验收】发送了邮件'))
      .toBeVisible()
    await view.getByText('已向组织【新功能开发】的节点【开发、验收】发送了邮件').click()
    expect(
      view.container.querySelector('.agent-activity--workflow-send svg.lucide-network')
    ).not.toBeNull()
    expect(view.container.textContent).not.toContain('private-delivery-id')
    expect(view.container.textContent).not.toContain('参数')
    expect(view.container.textContent).not.toContain('结果')
  })

  it('handles missing display metadata and malformed partial arguments without raw identifiers or false success', async () => {
    const withoutMetadata = {
      ...call,
      args: { messages: [{ replyTo: 'private-message-id', message: '正在发送的邮件' }] }
    }
    const view = await render(
      <WorkflowSendToolActivity call={withoutMetadata} settledStatus="completed" />
    )
    await expect.element(view.getByText('向组织成员发送邮件的结果待确认')).toBeVisible()
    await view.getByText('向组织成员发送邮件的结果待确认').click()
    await expect.element(view.getByText('正在发送的邮件')).toBeVisible()
    expect(view.container.textContent).not.toContain('private-message-id')
    await view.rerender(<WorkflowSendToolActivity call={{ ...call, args: '{"messages":[' }} />)
    await expect.element(view.getByText('正在向组织成员发送邮件')).toBeVisible()
  })

  it('uses semantic recipients before a receipt and keeps replies aligned with the exact recipient', async () => {
    const semanticCall = {
      ...call,
      args: {
        messages: [
          { replyTo: 'private-reply-id', message: '回复来信' },
          { to: '验收', message: '直接发送' }
        ]
      }
    }
    const view = await render(<WorkflowSendToolActivity call={semanticCall} />)
    await expect.element(view.getByText('正在向组织成员【验收】发送邮件')).toBeVisible()
    await view.getByText('正在向组织成员【验收】发送邮件').click()
    await expect.element(view.getByText('原邮件发送者')).toBeVisible()
    await expect.element(view.getByText('验收', { exact: true })).toBeVisible()
    expect(view.container.textContent).not.toContain('private-reply-id')
    const open = vi.fn()
    await view.rerender(
      <ConversationNavigationProvider onOpenConversation={open}>
        <WorkflowSendToolActivity call={semanticCall} result={receipt} />
      </ConversationNavigationProvider>
    )
    await view.getByText('已向组织【新功能开发】的节点【开发、验收】发送了邮件').click()
    await view.getByRole('button', { name: '打开对话 · 开发' }).click()
    expect(open).toHaveBeenCalledExactlyOnceWith('chat-a')
    expect(view.container.querySelectorAll('.workflow-send-message')[0].textContent).toContain(
      '回复来信'
    )
    expect(view.container.querySelectorAll('.workflow-send-message')[1].textContent).toContain(
      '直接发送'
    )
  })

  it('preserves failure and cancellation wording instead of claiming delivery', async () => {
    const view = await render(
      <WorkflowSendToolActivity
        call={call}
        result={{ ...receipt, ok: false, error: '收件节点当前不可用' }}
      />
    )
    await expect
      .element(view.getByText('向组织【新功能开发】的节点【开发、验收】发送邮件失败'))
      .toBeVisible()
    await view.getByText('向组织【新功能开发】的节点【开发、验收】发送邮件失败').click()
    await expect.element(view.getByText('收件节点当前不可用')).toBeVisible()
    await view.rerender(<WorkflowSendToolActivity call={call} cancelled />)
    await expect
      .element(view.getByText('已取消向组织【新功能开发】的节点【开发、验收】发送邮件'))
      .toBeVisible()
  })
})
