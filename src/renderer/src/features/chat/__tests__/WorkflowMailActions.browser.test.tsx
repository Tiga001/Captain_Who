import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { ConversationNavigationProvider } from '../ConversationNavigationContext'
import { WorkflowMailActionToolActivity } from '../components/toolActivities/WorkflowMailActionToolActivity'
vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ language: 'zh-CN', t: (key: string) => key })
}))
const copy = vi.fn().mockResolvedValue(undefined)
vi.mock('../components/clipboard', () => ({
  copyTextToClipboard: (content: string) => copy(content)
}))
const call: AgentToolCall = {
  id: 'action',
  tool: 'organization_accept',
  args: { messageIds: ['private-id'] },
  approvalStatus: 'not_required',
  reason: null
}
const message = {
  messageId: 'private-id',
  sourceNodeName: '研究员',
  sourceConversationId: 'sender-chat',
  targetNodeName: '审核',
  targetConversationId: 'receiver-chat',
  content: '需要核对的材料\n保留正文',
  status: 'processing',
  success: true
}
function receipt(tool: string, messages: unknown[] = [message]): AgentToolResult {
  return {
    callId: call.id,
    tool,
    ok: true,
    result: {
      action: tool.slice(9),
      organizationName: '研究项目',
      instanceId: 'private-instance',
      messages
    }
  }
}
afterEach(() => copy.mockClear())
describe('mail actions', () => {
  it.each([
    ['organization_accept', '已接手组织中的 1 封邮件'],
    ['organization_complete', '已将组织中的 1 封邮件标记为已处理'],
    ['organization_recall', '已撤回组织中的 1 封邮件']
  ])('shows %s as a compact expandable message action', async (tool, label) => {
    const open = vi.fn()
    const view = await render(
      <ConversationNavigationProvider onOpenConversation={open}>
        <WorkflowMailActionToolActivity call={{ ...call, tool }} result={receipt(tool)} />
      </ConversationNavigationProvider>
    )
    await expect.element(view.getByText(label)).toBeVisible()
    await expect.element(view.getByText(message.content)).not.toBeVisible()
    await view.getByText(label).click()
    expect(view.container.querySelector('nav')).toBeNull()
    await expect.element(view.getByText(message.content)).toBeVisible()
    const name = tool === 'organization_recall' ? '审核' : '研究员'
    await view.getByRole('button', { name: `复制邮件 · ${name}` }).click()
    expect(copy).toHaveBeenCalledExactlyOnceWith(message.content)
    await view.getByRole('button', { name: `打开对话 · ${name}` }).click()
    expect(open).toHaveBeenCalledWith(
      tool === 'organization_recall' ? 'receiver-chat' : 'sender-chat'
    )
    for (const hidden of ['private-id', 'private-instance', '参数', '技术详情'])
      expect(view.container.textContent).not.toContain(hidden)
  })
  it('does not count rejected messages as completed or erase a committed result on cancellation', async () => {
    const view = await render(
      <WorkflowMailActionToolActivity
        call={call}
        cancelled
        result={receipt(call.tool, [
          message,
          { ...message, messageId: 'failed', success: false, error: '这封邮件已被其他轮次接手' }
        ])}
      />
    )
    await expect.element(view.getByText('已接手组织中的 1 封邮件 · 1 条未完成')).toBeVisible()
    await view.getByText('已接手组织中的 1 封邮件 · 1 条未完成').click()
    expect(view.container.textContent).not.toContain('这封邮件已被其他轮次接手')
    await view.getByRole('button', { name: '下一封' }).click()
    await expect.element(view.getByText('这封邮件已被其他轮次接手')).toBeVisible()
  })
  it.each(['organization_accept', 'organization_complete', 'organization_recall'])(
    'cycles %s receipts with actions for only the current message',
    async (tool) => {
      const open = vi.fn()
      const second = {
        ...message,
        messageId: 'second',
        sourceNodeName: '文书',
        targetNodeName: '分发',
        sourceConversationId: 'writer-chat',
        targetConversationId: 'dispatch-chat',
        content: '第二封正文'
      }
      const view = await render(
        <ConversationNavigationProvider onOpenConversation={open}>
          <WorkflowMailActionToolActivity
            call={{ ...call, tool }}
            result={receipt(tool, [message, second])}
          />
        </ConversationNavigationProvider>
      )
      await view.getByText(/已.*组织中的 2 封邮件/).click()
      await expect.element(view.getByRole('button', { name: '上一封' })).toBeDisabled()
      expect(view.container.querySelectorAll('.workflow-send-message')).toHaveLength(1)
      expect(view.container.textContent).not.toContain(second.content)
      await view.getByRole('button', { name: '下一封' }).click()
      await expect.element(view.getByText(second.content)).toBeVisible()
      await expect.element(view.getByRole('button', { name: '下一封' })).toBeDisabled()
      expect(view.container.textContent).not.toContain(message.content)
      const name = tool === 'organization_recall' ? '分发' : '文书'
      expect(view.container.querySelector('.workflow-mail-carousel__heading')).toBeNull()
      const actions = view.container.querySelector('.workflow-send-message__actions')!
      const navigation = actions.querySelector('nav')!
      const jump = actions.querySelector(`button[aria-label="打开对话 · ${name}"]`)!
      expect(navigation).not.toBeNull()
      expect(navigation.compareDocumentPosition(jump) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(
        0
      )
      await view.getByRole('button', { name: `复制邮件 · ${name}` }).click()
      expect(copy).toHaveBeenCalledExactlyOnceWith(second.content)
      await view.getByRole('button', { name: `打开对话 · ${name}` }).click()
      expect(open).toHaveBeenCalledExactlyOnceWith(
        tool === 'organization_recall' ? 'dispatch-chat' : 'writer-chat'
      )
    }
  )
  it('never infers completion from an ended model turn without a receipt', async () => {
    const view = await render(
      <WorkflowMailActionToolActivity call={call} settledStatus="completed" />
    )
    await expect.element(view.getByText('接手组织中的 1 封邮件的结果待确认')).toBeVisible()
    expect(view.container.querySelector('details')).toBeNull()
  })
})
