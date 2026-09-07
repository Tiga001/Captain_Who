import type { AgentProposedAction } from '@mycopilot/protocol'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { page } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { getFrontendCssVariables } from '../../../config/frontendConfig'
import { getFrontendTheme } from '../../../config/frontendTheme'
import { AgentApprovalDialog } from '../components/AgentApprovalDialog'
import {
  ConversationApprovalQueue,
  type ConversationApprovalQueueItem
} from '../components/ConversationApprovalQueue'
import '../../../styles/global.css'
import '../ChatConversationPage.approvals.css'

vi.mock('../../../config/FrontendConfigProvider', () => {
  const translations: Record<string, string> = {
    'agent.approval.navigation.label': '切换审批',
    'agent.approval.navigation.previous': '上一条审批',
    'agent.approval.navigation.next': '下一条审批',
    'collaboration.approval.openAgent': '查看来源子智能体 {task}',
    'agent.approval.dialog.approve': '批准',
    'agent.approval.dialog.commandPolicyHint': '确认后执行',
    'agent.approval.dialog.reject': '拒绝',
    'agent.approval.dialog.rejectPlaceholder': '说明拒绝原因'
  }
  return { useFrontendConfig: () => ({ t: (key: string) => translations[key] ?? key }) }
})

function item(
  id: string,
  sourceAgentId?: string,
  onApprove?: () => Promise<boolean | void>
): ConversationApprovalQueueItem {
  const action: AgentProposedAction = {
    type: 'command',
    command: {
      id,
      command: `printf '${id}'`,
      cwd: null,
      timeoutMs: null,
      approvalStatus: 'required',
      riskLevel: null,
      reason: `审批 ${id}`,
      observe: null
    }
  }
  return {
    id,
    sourceAgentId,
    label: sourceAgentId ? `智能体 ${sourceAgentId}` : '主智能体',
    content: (
      <AgentApprovalDialog target={{ action, messageId: `message-${id}` }} onApprove={onApprove} />
    )
  }
}

let previousRootStyle: string | null
beforeEach(async () => {
  previousRootStyle = document.documentElement.getAttribute('style')
  const theme = getFrontendTheme('classic-light')
  for (const [key, value] of Object.entries(getFrontendCssVariables(undefined, theme.tokens))) {
    document.documentElement.style.setProperty(key, value)
  }
  await page.viewport(1000, 720)
})
afterEach(async () => {
  if (previousRootStyle === null) document.documentElement.removeAttribute('style')
  else document.documentElement.setAttribute('style', previousRootStyle)
  await page.viewport(1280, 720)
})

describe('ConversationApprovalQueue', () => {
  it('keeps the existing single-root card without a source row or avatar', async () => {
    const screen = await render(<ConversationApprovalQueue items={[item('root')]} />)

    await expect.element(screen.getByRole('dialog', { name: '审批 root' })).toBeVisible()
    expect(screen.container.querySelector('.conversation-approval-queue__header')).toBeNull()
    expect(screen.container.querySelector('.agent-avatar')).toBeNull()
    expect(screen.getByRole('navigation').elements()).toHaveLength(0)
  })

  it('retains the source row and avatar for a single child and opens that exact Agent', async () => {
    const onOpenAgent = vi.fn()
    const screen = await render(
      <ConversationApprovalQueue items={[item('child', 'reviewer')]} onOpenAgent={onOpenAgent} />
    )

    await expect.element(screen.getByText('智能体 reviewer')).toBeVisible()
    expect(screen.container.querySelector('.agent-avatar')).not.toBeNull()
    await expect.element(screen.getByText('1 / 1')).toBeVisible()
    await expect.element(screen.getByRole('button', { name: '上一条审批' })).toBeDisabled()
    await expect.element(screen.getByRole('button', { name: '下一条审批' })).toBeDisabled()
    await screen.getByRole('button', { name: '查看来源子智能体 智能体 reviewer' }).click()
    expect(onOpenAgent).toHaveBeenCalledWith('reviewer')
  })

  it('shows one mixed approval at a time and disables navigation at each end', async () => {
    const screen = await render(
      <ConversationApprovalQueue items={[item('root'), item('child', 'reviewer')]} />
    )

    await expect.element(screen.getByText('1 / 2')).toBeVisible()
    await expect.element(screen.getByText('主智能体')).toBeVisible()
    expect(screen.container.querySelector('.agent-avatar')).toBeNull()
    expect(screen.getByRole('dialog').elements()).toHaveLength(1)
    await expect.element(screen.getByRole('button', { name: '上一条审批' })).toBeDisabled()
    await screen.getByRole('button', { name: '下一条审批' }).click()
    await expect.element(screen.getByText('2 / 2')).toBeVisible()
    await expect.element(screen.getByRole('dialog', { name: '审批 child' })).toBeVisible()
    await expect.element(screen.getByRole('button', { name: '下一条审批' })).toBeDisabled()
    await expect.element(screen.getByRole('button', { name: '上一条审批' })).toBeEnabled()
    expect(screen.getByRole('dialog').elements()).toHaveLength(1)

    const hiddenRoot = screen.container.querySelector<HTMLElement>(
      '[data-approval-queue-id="root"]'
    )!
    expect(hiddenRoot.hidden).toBe(true)
    expect(hiddenRoot.inert).toBe(true)
    expect(hiddenRoot.getBoundingClientRect().height).toBe(0)
  })

  it('decreases the pending count independently, retaining the last child source until it settles', async () => {
    const root = item('root')
    const child = item('child', 'reviewer')
    const sibling = item('sibling', 'writer')
    const screen = await render(<ConversationApprovalQueue items={[root, child, sibling]} />)

    await expect.element(screen.getByText('1 / 3')).toBeVisible()
    await screen.rerender(<ConversationApprovalQueue items={[child, sibling]} />)
    await expect.element(screen.getByText('1 / 2')).toBeVisible()
    await expect.element(screen.getByRole('dialog', { name: '审批 child' })).toBeVisible()
    await screen.rerender(<ConversationApprovalQueue items={[sibling]} />)
    await expect.element(screen.getByText('智能体 writer')).toBeVisible()
    await expect.element(screen.getByRole('dialog', { name: '审批 sibling' })).toBeVisible()
    await expect.element(screen.getByText('1 / 1')).toBeVisible()
    expect(screen.container.querySelectorAll('.agent-avatar')).toHaveLength(1)
    await screen.rerender(<ConversationApprovalQueue items={[]} />)
    expect(screen.container.textContent).toBe('')
    expect(screen.container.querySelector('.conversation-approval-queue')).toBeNull()
  })

  it('retains the selected identity on new arrivals and falls back to the previous item when the last disappears', async () => {
    const root = item('root')
    const child = item('child', 'reviewer')
    const newest = item('newest', 'writer')
    const screen = await render(<ConversationApprovalQueue items={[root, child]} />)
    await screen.getByRole('button', { name: '下一条审批' }).click()
    await screen.rerender(<ConversationApprovalQueue items={[newest, root, child]} />)

    await expect.element(screen.getByRole('dialog', { name: '审批 child' })).toBeVisible()
    await expect.element(screen.getByText('3 / 3')).toBeVisible()
    await screen.rerender(<ConversationApprovalQueue items={[newest, root]} />)
    await expect.element(screen.getByRole('dialog', { name: '审批 root' })).toBeVisible()
    await expect.element(screen.getByText('2 / 2')).toBeVisible()
    await screen.rerender(<ConversationApprovalQueue items={[root]} />)
    expect(screen.container.querySelector('.conversation-approval-queue__header')).toBeNull()
  })

  it('preserves each rejection draft and an in-flight decision while switching approvals', async () => {
    let resolveApproval!: (accepted: boolean) => void
    const approve = vi.fn(() => new Promise<boolean>((resolve) => (resolveApproval = resolve)))
    const screen = await render(
      <ConversationApprovalQueue
        items={[item('root', undefined, approve), item('child', 'reviewer')]}
      />
    )

    await screen.getByRole('textbox').fill('主智能体拒绝草稿')
    await screen.getByRole('button', { name: '下一条审批' }).click()
    await expect.element(screen.getByRole('textbox')).toHaveValue('')
    await screen.getByRole('textbox').fill('子智能体拒绝草稿')
    await screen.getByRole('button', { name: '上一条审批' }).click()
    await expect.element(screen.getByRole('textbox')).toHaveValue('主智能体拒绝草稿')
    await screen.getByRole('button', { name: /^1\s*批准$/ }).click()
    expect(approve).toHaveBeenCalledTimes(1)
    await expect.element(screen.getByRole('button', { name: /^1\s*批准$/ })).toBeDisabled()
    await screen.getByRole('button', { name: '下一条审批' }).click()
    await expect.element(screen.getByRole('textbox')).toHaveValue('子智能体拒绝草稿')
    await expect.element(screen.getByRole('button', { name: /^1\s*批准$/ })).toBeEnabled()
    await screen.getByRole('button', { name: '上一条审批' }).click()
    await expect.element(screen.getByRole('button', { name: /^1\s*批准$/ })).toBeDisabled()
    await expect.element(screen.getByRole('textbox')).toBeDisabled()
    await expect.element(screen.getByText('1 / 2')).toBeVisible()
    resolveApproval(false)
    await expect.element(screen.getByRole('button', { name: /^1\s*批准$/ })).toBeEnabled()
    await expect.element(screen.getByRole('textbox')).toHaveValue('主智能体拒绝草稿')
    await expect.element(screen.getByText('1 / 2')).toBeVisible()
  })

  it('keeps the source row above the existing approval card at narrow width', async () => {
    const screen = await render(
      <div style={{ width: 360 }}>
        <ConversationApprovalQueue
          items={[item('child', '一个名字很长但不应挤掉审批切换按钮的智能体'), item('root')]}
        />
      </div>
    )
    const header = screen.container.querySelector<HTMLElement>(
      '.conversation-approval-queue__header'
    )!
    const dialog = screen.getByRole('dialog').element().getBoundingClientRect()
    const queue = screen.container.querySelector<HTMLElement>('.conversation-approval-queue')!
    expect(header.getBoundingClientRect().bottom).toBeLessThanOrEqual(dialog.top)
    expect(queue.scrollWidth).toBeLessThanOrEqual(queue.clientWidth)
    await expect.element(screen.getByRole('button', { name: '下一条审批' })).toBeVisible()
    await page.screenshot({
      element: queue,
      path: '__screenshots__/ConversationApprovalQueue.browser.test.tsx/multiple-approvals.png'
    })
  })
})
