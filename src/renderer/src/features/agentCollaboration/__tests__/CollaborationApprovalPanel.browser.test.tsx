import type {
  AgentProposedAction,
  CollaborationApprovalDecisionResult,
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
  'collaboration.approval.decisionNotAccepted': '审批仍在等待中，服务端未受理本次操作，请重试。',
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
  'agent.approval.dialog.fileChangePolicyHint': '完整审阅后执行',
  'agent.approval.dialog.fileChangeTitle': '修改文件',
  'agent.approval.dialog.reject': '拒绝',
  'agent.approval.dialog.rejectPlaceholder': '说明拒绝原因',
  'agent.approval.dialog.toolTitle': '运行 {tool}',
  'agent.fileChange.togglePreview': '文件修改差异分页',
  'files.pdf.nextPage': '下一页',
  'files.pdf.previousPage': '上一页',
  'files.preview.loading': '正在加载完整差异',
  'files.preview.error': '无法加载完整差异'
}

const fileChangeRpc = vi.hoisted(() => ({ getDiff: vi.fn() }))

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'zh-CN',
    t: (key: string) => translations[key] ?? key
  })
}))

vi.mock('../../agent/agentClient', () => ({
  getAgentFileChangeDiff: fileChangeRpc.getDiff
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

function fileChangeApproval(): CollaborationApprovalProjection {
  return {
    ...approval(),
    actionId: 'file-change-action',
    actionType: 'file_change',
    toolName: 'apply_patch',
    action: {
      type: 'file_change',
      fileChange: {
        schemaVersion: 1,
        id: 'file-change-action',
        transactionId: 'file-change-transaction',
        operation: 'update',
        updateStrategy: 'rewrite',
        filePath: 'src/main.ts',
        inlineDiff: null,
        baseRevision: 'content-sha256-v1:base',
        summary: '更新入口文件',
        additions: 1,
        deletions: 1,
        lineCount: 1,
        byteCount: 12,
        approvalStatus: 'required'
      }
    }
  }
}

function decisionResult(
  overrides: Partial<CollaborationApprovalDecisionResult> = {}
): CollaborationApprovalDecisionResult {
  return {
    schemaVersion: 1,
    approvalId: 'approval-stable',
    accepted: true,
    alreadySettled: false,
    status: 'approved',
    ...overrides
  }
}

describe('CollaborationApprovalPanel', () => {
  it('routes staged child Diff reads through the exact root conversation', async () => {
    const projected = fileChangeApproval()
    fileChangeRpc.getDiff.mockResolvedValue({
      transactionId: 'file-change-transaction',
      patch: '-old\n+new\n',
      offset: 0,
      nextOffset: null,
      truncated: false
    })
    const screen = await render(
      <CollaborationApprovalPanel
        approvals={[projected]}
        mode="interactive"
        onDecision={vi.fn(async () => decisionResult())}
      />
    )

    await expect.element(screen.getByText(/\+new/)).toBeVisible()
    expect(fileChangeRpc.getDiff).toHaveBeenCalledWith(
      'file-change-transaction',
      0,
      50_000,
      'conversation-root'
    )
    await expect.element(screen.getByRole('button', { name: '批准' })).toBeEnabled()
  })

  it('routes a root decision by stable approval identity without offering remember-for-run', async () => {
    const onDecision = vi.fn(async () => decisionResult())
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
    const onDecision = vi.fn(async () => decisionResult({ status: 'rejected' }))
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
      .fn<() => Promise<CollaborationApprovalDecisionResult>>()
      .mockRejectedValueOnce(new Error('temporarily unavailable'))
      .mockResolvedValueOnce(decisionResult())
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

  it('does not latch an unaccepted decision that is still pending', async () => {
    const onDecision = vi
      .fn<() => Promise<CollaborationApprovalDecisionResult>>()
      .mockResolvedValueOnce(
        decisionResult({ accepted: false, alreadySettled: true, status: 'pending' })
      )
      .mockResolvedValueOnce(decisionResult())
    const screen = await render(
      <CollaborationApprovalPanel
        approvals={[approval()]}
        mode="interactive"
        onDecision={onDecision}
      />
    )

    await screen.getByRole('button', { name: '批准' }).click()
    await expect.element(screen.getByRole('alert')).toHaveTextContent('服务端未受理')
    await expect.element(screen.getByRole('button', { name: '批准' })).toBeEnabled()

    await screen.getByRole('button', { name: '批准' }).click()
    expect(onDecision).toHaveBeenCalledTimes(2)
  })

  it('keeps one decision in flight when the approve control is clicked repeatedly', async () => {
    const pending = deferred<CollaborationApprovalDecisionResult>()
    const onDecision = vi.fn(() => pending.promise)
    const screen = await render(
      <CollaborationApprovalPanel
        approvals={[approval()]}
        mode="interactive"
        onDecision={onDecision}
      />
    )

    const approve = screen.getByRole('button', { name: '批准' })
    await approve.click()
    await approve.click({ force: true })
    expect(onDecision).toHaveBeenCalledOnce()

    pending.resolve(decisionResult())
  })

  it('shows durable settled status without approval controls', async () => {
    const screen = await render(
      <CollaborationApprovalPanel
        approvals={[approval('completed')]}
        mode="interactive"
        onDecision={vi.fn(async () => decisionResult({ status: 'completed' }))}
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
        onDecision={vi.fn(async () => decisionResult())}
        onRetryLoad={onRetryLoad}
      />
    )

    expect(screen.container.querySelector('[data-approval-id="approval-stable"]')).not.toBeNull()
    await expect.element(screen.getByRole('alert')).toHaveTextContent('temporarily unavailable')
    await screen.getByRole('button', { name: '重试' }).click()
    expect(onRetryLoad).toHaveBeenCalledOnce()
  })
})

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((finish) => {
    resolve = finish
  })
  return { promise, resolve }
}
