import type {
  AgentProposedAction,
  CollaborationApprovalProjection,
  CollaborationApprovalStatus
} from '@mycopilot/protocol'
import { page } from 'vitest/browser'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { CollaborationApprovalPanel } from '../CollaborationApprovalPanel'

const translations: Record<string, string> = {
  'collaboration.approval.title': '子智能体请求审批',
  'collaboration.approval.openAgent': '查看来源子智能体 {task}',
  'collaboration.approval.decisionFailed': '审批操作未完成：{error}',
  'collaboration.approval.loadFailed': '审批请求加载失败：{error}',
  'collaboration.approval.retry': '重试',
  'collaboration.approval.observerNotice': '只读对话中不能处理审批，请返回根对话操作。',
  'collaboration.approval.status.pending': '等待你的决定',
  'collaboration.approval.status.approved': '已批准',
  'collaboration.approval.status.executing': '正在继续',
  'collaboration.approval.status.rejected': '已拒绝',
  'collaboration.approval.status.cancelled': '已取消',
  'collaboration.approval.status.completed': '已完成',
  'collaboration.approval.status.failed': '执行失败',
  'collaboration.approval.status.expired': '已过期',
  'collaboration.approval.status.interrupted': '已中断',
  'agent.approval.dialog.approve': '批准',
  'agent.approval.dialog.commandPolicyHint': '确认后执行',
  'agent.approval.dialog.reject': '拒绝',
  'agent.approval.dialog.rejectPlaceholder': '说明拒绝原因',
  'agent.approval.dialog.toolTitle': '运行 {tool}'
}

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'zh-CN',
    t: (key: string) => translations[key] ?? key
  })
}))

const commandAction: AgentProposedAction = {
  type: 'command',
  command: {
    id: 'command-action',
    command: 'cargo test -p example',
    cwd: null,
    timeoutMs: null,
    approvalStatus: 'required',
    riskLevel: null,
    reason: '运行受控测试',
    observe: null
  }
}

function approval(
  status: CollaborationApprovalStatus = 'pending'
): CollaborationApprovalProjection {
  return {
    schemaVersion: 1,
    approvalId: 'approval-stable',
    rootAgentId: 'agent-root',
    rootConversationId: 'conversation-root',
    sourceAgentId: 'agent-child',
    sourceTaskPath: '/root/review',
    sourceConversationId: 'conversation-child',
    runId: 'run-child',
    actionId: 'command-action',
    actionType: 'command',
    toolName: 'run_command',
    action: commandAction,
    status,
    createdAt: 10,
    updatedAt: status === 'pending' ? 10 : 20
  }
}

describe('CollaborationApprovalPanel', () => {
  it('routes a root decision by stable approval identity without offering remember-for-run', async () => {
    const onDecision = vi.fn(async () => undefined)
    const onOpenAgent = vi.fn()
    const screen = await render(
      <CollaborationApprovalPanel
        approvals={[approval(), approval()]}
        mode="interactive"
        onDecision={onDecision}
        onOpenAgent={onOpenAgent}
      />
    )

    expect(screen.container.querySelectorAll('[data-approval-id="approval-stable"]')).toHaveLength(
      1
    )
    expect(screen.container.textContent).toContain('/root/review')
    expect(screen.container.textContent).toContain('cargo test -p example')
    expect(screen.container.querySelector('[data-choice="remember"]')).toBeNull()

    await screen.getByRole('button', { name: '批准' }).click()
    expect(onDecision).toHaveBeenCalledWith('approval-stable', 'approve', null)

    await screen.getByRole('button', { name: '查看来源子智能体 /root/review' }).click()
    expect(onOpenAgent).toHaveBeenCalledWith('agent-child')
    await page.screenshot({
      element: screen.getByRole('region', { name: '子智能体请求审批' }).element(),
      path: '__screenshots__/CollaborationApprovalPanel.browser.test.tsx/root-child-approval.png'
    })
  })

  it('routes root rejection guidance to the original child approval', async () => {
    const onDecision = vi.fn(async () => undefined)
    const screen = await render(
      <CollaborationApprovalPanel
        approvals={[approval()]}
        mode="interactive"
        onDecision={onDecision}
      />
    )

    await screen.getByLabelText('说明拒绝原因').fill('  请改用只读命令  ')
    await screen.getByRole('button', { name: '拒绝' }).click()
    expect(onDecision).toHaveBeenCalledWith('approval-stable', 'reject', '请改用只读命令')
  })

  it('renders pending child approvals without any write entrypoint in observer mode', async () => {
    const screen = await render(
      <CollaborationApprovalPanel approvals={[approval()]} mode="observer" />
    )

    expect(screen.container.textContent).toContain('等待你的决定')
    expect(screen.container.textContent).toContain('只读对话中不能处理审批')
    await expect
      .element(screen.getByRole('region', { name: '子智能体请求审批' }))
      .toBeInTheDocument()
    expect(screen.container.querySelector('.agent-approval-dialog')).toBeNull()
    expect(screen.container.querySelector('button')).toBeNull()
    expect(screen.container.querySelector('input')).toBeNull()
  })

  it('remounts the decision shell after a transport failure so the user can retry', async () => {
    const onDecision = vi
      .fn<() => Promise<void>>()
      .mockRejectedValueOnce(new Error('temporarily unavailable'))
      .mockResolvedValueOnce(undefined)
    const screen = await render(
      <CollaborationApprovalPanel
        approvals={[approval()]}
        mode="interactive"
        onDecision={onDecision}
      />
    )

    await screen.getByRole('button', { name: '批准' }).click()
    await expect.element(screen.getByRole('alert')).toHaveTextContent('temporarily unavailable')
    await expect.element(screen.getByRole('button', { name: '批准' })).toBeEnabled()
    await screen.getByRole('button', { name: '批准' }).click()
    expect(onDecision).toHaveBeenCalledTimes(2)
  })

  it('shows durable settled status without approval controls', async () => {
    const screen = await render(
      <CollaborationApprovalPanel
        approvals={[approval('completed')]}
        mode="interactive"
        onDecision={vi.fn()}
      />
    )

    expect(screen.container.textContent).toContain('已完成')
    expect(screen.container.querySelector('.agent-approval-dialog')).toBeNull()
  })

  it('keeps a durable approval visible while offering a retry after refresh fails', async () => {
    const onRetryLoad = vi.fn()
    const screen = await render(
      <CollaborationApprovalPanel
        approvals={[approval()]}
        loadError="temporarily unavailable"
        mode="interactive"
        onDecision={vi.fn()}
        onRetryLoad={onRetryLoad}
      />
    )

    expect(screen.container.querySelector('[data-approval-id="approval-stable"]')).not.toBeNull()
    await expect.element(screen.getByRole('alert')).toHaveTextContent('temporarily unavailable')
    await screen.getByRole('button', { name: '重试' }).click()
    expect(onRetryLoad).toHaveBeenCalledOnce()
  })
})
