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
  workflowName: '新功能开发',
  instanceId: 'workflow-private-id',
  outputs: [
    {
      flowId: 'flow-a',
      targetNodeId: 'node-a',
      targetNodeName: '开发',
      targetConversationId: 'chat-a'
    },
    { flowId: 'flow-b', targetNodeId: 'node-b', targetNodeName: '验收', targetConversationId: null }
  ]
}
const call: AgentToolCall = {
  id: 'call-send',
  tool: 'workflow_send',
  approvalStatus: 'not_required',
  reason: null,
  args: {
    outputs: [
      { flowId: 'flow-a', message: '暗号：升龙拳\n保留换行' },
      { flowId: 'flow-b', message: '请验收' }
    ],
    _workflowSend: metadata
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

describe('workflow send presentation', () => {
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
    const summary = view.getByText('正在向工作流【新功能开发】的节点【开发、验收】发送消息')
    await expect.element(summary).toBeVisible()
    await summary.click()
    await expect.element(view.getByText('暗号：升龙拳\n保留换行')).toBeVisible()
    await page.screenshot({
      element: view.container.firstElementChild as HTMLElement,
      path: '../../../../../../.cache/workflow-authoring/workflow-send-expanded.png'
    })
    await view.getByRole('button', { name: '复制消息 · 开发' }).click()
    expect(copy).toHaveBeenCalledExactlyOnceWith('暗号：升龙拳\n保留换行')
    await view.getByRole('button', { name: '打开对话 · 开发' }).click()
    expect(open).toHaveBeenCalledExactlyOnceWith('chat-a')
    expect(view.getByRole('button', { name: '打开对话 · 验收' }).elements()).toHaveLength(0)
    expect(view.container.textContent).not.toContain('workflow_send')
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
      .element(view.getByText('已向工作流【新功能开发】的节点【开发、验收】发送了消息'))
      .toBeVisible()
    await view.getByText('已向工作流【新功能开发】的节点【开发、验收】发送了消息').click()
    expect(
      view.container.querySelector('.agent-activity--workflow-send svg.lucide-network')
    ).not.toBeNull()
    expect(view.container.textContent).not.toContain('private-delivery-id')
    expect(view.container.textContent).not.toContain('参数')
    expect(view.container.textContent).not.toContain('结果')
  })

  it('handles legacy receipts and malformed partial arguments without raw identifiers or false success', async () => {
    const legacy = { ...call, args: { outputs: [{ flowId: 'legacy-uuid', message: '旧消息' }] } }
    const view = await render(<WorkflowSendToolActivity call={legacy} settledStatus="completed" />)
    await expect.element(view.getByText('向工作流节点发送消息的结果待确认')).toBeVisible()
    await view.getByText('向工作流节点发送消息的结果待确认').click()
    await expect.element(view.getByText('旧消息')).toBeVisible()
    expect(view.container.textContent).not.toContain('legacy-uuid')
    await view.rerender(<WorkflowSendToolActivity call={{ ...call, args: '{"outputs":[' }} />)
    await expect.element(view.getByText('正在向工作流节点发送消息')).toBeVisible()
  })

  it('preserves failure and cancellation wording instead of claiming delivery', async () => {
    const view = await render(
      <WorkflowSendToolActivity
        call={call}
        result={{ ...receipt, ok: false, error: '输出门要求全部出口' }}
      />
    )
    await expect
      .element(view.getByText('向工作流【新功能开发】的节点【开发、验收】发送消息失败'))
      .toBeVisible()
    await view.getByText('向工作流【新功能开发】的节点【开发、验收】发送消息失败').click()
    await expect.element(view.getByText('输出门要求全部出口')).toBeVisible()
    await view.rerender(<WorkflowSendToolActivity call={call} cancelled />)
    await expect
      .element(view.getByText('已取消向工作流【新功能开发】的节点【开发、验收】发送消息'))
      .toBeVisible()
  })
})
