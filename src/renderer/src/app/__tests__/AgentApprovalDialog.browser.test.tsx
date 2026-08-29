import type { AgentProposedAction } from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { AgentApprovalDialog } from '../../features/chat/components/AgentApprovalDialog'
import '../../styles/global.css'
import '../../features/chat/ChatConversationPage.approvals.css'

const translations: Record<string, string> = {
  'agent.approval.dialog.approve': '批准',
  'agent.approval.dialog.approveFileChangeRemember': '本次运行记住批准',
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

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => translations[key] ?? key })
}))

vi.mock('../../features/chat/components/toolActivities/toolActivityUtils', () => ({
  formatToolDetails: () => '',
  getToolDisplayName: () => 'Skill 脚本'
}))

vi.mock('../../features/agent/agentClient', () => ({
  getAgentFileChangeDiff: fileChangeRpc.getDiff
}))

beforeEach(() => {
  fileChangeRpc.getDiff.mockReset()
})

function fileChangeAction(
  inlineDiff: { patch: string; truncated: false } | null,
  operation: 'create' | 'update' | 'delete' = 'update'
) {
  return {
    type: 'file_change',
    fileChange: {
      schemaVersion: 1,
      id: 'file-change-call',
      transactionId: 'file-change-transaction',
      operation,
      updateStrategy: operation === 'update' && inlineDiff === null ? 'rewrite' : null,
      filePath: 'src/main.ts',
      inlineDiff,
      baseRevision: operation === 'create' ? null : 'content-sha256-v1:base',
      summary: '更新入口文件',
      additions: 2,
      deletions: 1,
      lineCount: 2,
      byteCount: 18,
      approvalStatus: 'required'
    }
  } satisfies AgentProposedAction
}

describe('AgentApprovalDialog FileChange approval', () => {
  it('uses the complete Direct inline Diff without an RPC and enables approval', async () => {
    const action = fileChangeAction({ patch: '@@ -1 +1 @@\n-old\n+new', truncated: false })
    const onApprove = vi.fn()
    const screen = await render(
      <AgentApprovalDialog
        target={{ action, messageId: 'assistant-message' }}
        onApprove={onApprove}
      />
    )

    await expect.element(screen.getByText(/-old/)).toBeVisible()
    expect(fileChangeRpc.getDiff).not.toHaveBeenCalled()
    const approve = screen.getByRole('button', { name: /^1\s*批准$/ })
    await expect.element(approve).toBeEnabled()
    await approve.click()
    expect(onApprove).toHaveBeenCalledWith('assistant-message', action, 'singleAction')
  })

  it.each(['create', 'update'] as const)(
    'offers and forwards run-scoped approval for %s',
    async (operation) => {
      const action = fileChangeAction(
        { patch: '@@ -0,0 +1 @@\n+current\n', truncated: false },
        operation
      )
      const onApprove = vi.fn()
      const screen = await render(
        <AgentApprovalDialog
          target={{ action, messageId: 'assistant-message' }}
          onApprove={onApprove}
        />
      )

      const remember = screen.container.querySelector<HTMLButtonElement>('[data-choice="remember"]')
      expect(remember).not.toBeNull()
      await remember!.click()
      expect(onApprove).toHaveBeenCalledWith(
        'assistant-message',
        action,
        'remainingApplyPatchInRun'
      )
    }
  )

  it('does not offer run-scoped approval for delete', async () => {
    const action = fileChangeAction(
      { patch: '@@ -1 +0,0 @@\n-current\n', truncated: false },
      'delete'
    )
    const onApprove = vi.fn()
    const screen = await render(
      <AgentApprovalDialog
        target={{ action, messageId: 'assistant-message' }}
        onApprove={onApprove}
      />
    )

    expect(screen.container.querySelector('[data-choice="remember"]')).toBeNull()
    await screen.getByRole('button', { name: /^1\s*批准$/ }).click()
    expect(onApprove).toHaveBeenCalledWith('assistant-message', action, 'singleAction')
  })

  it('keeps approval disabled until every staged Diff page is loaded in order', async () => {
    const action = fileChangeAction(null)
    let resolveFirst!: (page: {
      transactionId: string
      patch: string
      offset: number
      nextOffset: number | null
      truncated: boolean
    }) => void
    fileChangeRpc.getDiff
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveFirst = resolve
          })
      )
      .mockResolvedValueOnce({
        transactionId: action.fileChange.transactionId,
        patch: '+second\n',
        offset: 7,
        nextOffset: null,
        truncated: false
      })
    const onApprove = vi.fn()
    const onReject = vi.fn()
    const screen = await render(
      <AgentApprovalDialog
        target={{ action, messageId: 'assistant-message' }}
        onApprove={onApprove}
        onReject={onReject}
      />
    )

    const approve = screen.getByRole('button', { name: /^1\s*批准$/ })
    const reject = screen.getByRole('button', { name: '拒绝' })
    await expect.element(approve).toBeDisabled()
    await expect.element(reject).toBeEnabled()
    await vi.waitFor(() => expect(fileChangeRpc.getDiff).toHaveBeenCalledTimes(1))
    resolveFirst({
      transactionId: action.fileChange.transactionId,
      patch: '+first\n',
      offset: 0,
      nextOffset: 7,
      truncated: true
    })

    await expect.element(screen.getByText(/\+first/)).toBeVisible()
    expect(screen.getByText(/\+second/).query()).toBeNull()
    await expect.element(screen.getByText('1 / 2')).toBeVisible()
    await expect.element(approve).toBeEnabled()
    expect(fileChangeRpc.getDiff).toHaveBeenNthCalledWith(
      1,
      action.fileChange.transactionId,
      0,
      50_000,
      undefined
    )
    expect(fileChangeRpc.getDiff).toHaveBeenNthCalledWith(
      2,
      action.fileChange.transactionId,
      7,
      50_000,
      undefined
    )

    await screen.getByRole('button', { name: '下一页' }).click()
    await expect.element(screen.getByText(/\+second/)).toBeVisible()
    expect(screen.getByText(/\+first/).query()).toBeNull()
    await expect.element(screen.getByText('2 / 2')).toBeVisible()
  })

  it('loads a child staged Diff through its exact observer root conversation', async () => {
    const action = fileChangeAction(null)
    fileChangeRpc.getDiff.mockResolvedValue({
      transactionId: action.fileChange.transactionId,
      patch: '+reviewed by root\n',
      offset: 0,
      nextOffset: null,
      truncated: false
    })
    const screen = await render(
      <AgentApprovalDialog
        observerRootConversationId="conversation-root"
        target={{ action, messageId: 'child-approval' }}
      />
    )

    await expect.element(screen.getByText(/reviewed by root/)).toBeVisible()
    expect(fileChangeRpc.getDiff).toHaveBeenCalledWith(
      action.fileChange.transactionId,
      0,
      50_000,
      'conversation-root'
    )
    await expect.element(screen.getByRole('button', { name: /^1\s*批准$/ })).toBeEnabled()
  })

  it('fails closed on an incomplete page chain while leaving rejection available', async () => {
    const action = fileChangeAction(null)
    fileChangeRpc.getDiff.mockResolvedValue({
      transactionId: action.fileChange.transactionId,
      patch: '+partial\n',
      offset: 0,
      nextOffset: null,
      truncated: true
    })
    const onApprove = vi.fn()
    const screen = await render(
      <AgentApprovalDialog
        target={{ action, messageId: 'assistant-message' }}
        onApprove={onApprove}
      />
    )

    await expect.element(screen.getByRole('alert')).toBeVisible()
    await expect.element(screen.getByRole('button', { name: /^1\s*批准$/ })).toBeDisabled()
    await expect.element(screen.getByRole('button', { name: '拒绝' })).toBeEnabled()
  })

  it('fails closed immediately when the reviewed transaction changes', async () => {
    const direct = fileChangeAction({ patch: '+already reviewed', truncated: false })
    const staged = fileChangeAction(null)
    staged.fileChange.transactionId = 'file-change-transaction-next'
    fileChangeRpc.getDiff.mockImplementation(() => new Promise(() => undefined))
    const onApprove = vi.fn()
    const screen = await render(
      <AgentApprovalDialog
        target={{ action: direct, messageId: 'assistant-message-direct' }}
        onApprove={onApprove}
      />
    )
    await expect.element(screen.getByRole('button', { name: /^1\s*批准$/ })).toBeEnabled()

    await screen.rerender(
      <AgentApprovalDialog
        target={{ action: staged, messageId: 'assistant-message-staged' }}
        onApprove={onApprove}
      />
    )

    await expect.element(screen.getByRole('button', { name: /^1\s*批准$/ })).toBeDisabled()
    await expect.element(screen.getByRole('button', { name: '拒绝' })).toBeEnabled()
    expect(onApprove).not.toHaveBeenCalled()
  })
})

describe('AgentApprovalDialog Skill script approval', () => {
  it('shows the complete frozen execution snapshot and never offers remember-for-run', async () => {
    const action: AgentProposedAction = {
      type: 'skill_script',
      script: {
        id: 'script-action',
        scriptUri: 'skill://package/example/revision/scripts/build.py',
        skillId: 'installed:user:example',
        skillRevision: 'sha256:revision',
        resourcePath: 'scripts/build.py',
        resourceDigest: 'sha256:script',
        source: {
          sourceId: 'installed:user',
          sourceKind: 'installed',
          trust: 'untrusted'
        },
        interpreter: 'python3',
        args: ['--output', 'budget.xlsx'],
        requirements: {
          pythonDistributions: ['openpyxl'],
          commands: ['libreoffice']
        },
        preflight: {
          status: 'ready',
          interpreter: 'python3',
          interpreterVersion: 'Python 3 (static identity)',
          dependencies: [
            {
              kind: 'python_distribution',
              name: 'openpyxl',
              status: 'available',
              version: '3.1.5'
            }
          ],
          runtimeFingerprint: 'sha256:runtime'
        },
        timeoutMs: 30_000,
        approvalStatus: 'required',
        reason: '生成预算工作簿'
      }
    }
    const onApprove = vi.fn()
    const screen = await render(
      <AgentApprovalDialog
        target={{ action, messageId: 'assistant-message' }}
        onApprove={onApprove}
      />
    )

    const snapshot = screen.container.querySelector('.agent-approval-dialog__command')
    expect(snapshot?.getAttribute('data-multiline')).toBe('true')
    expect(snapshot?.textContent).toContain(action.script.scriptUri)
    expect(snapshot?.textContent).toContain(action.script.resourcePath)
    expect(snapshot?.textContent).toContain('budget.xlsx')
    expect(snapshot?.textContent).toContain('openpyxl')
    expect(snapshot?.textContent).toContain('libreoffice')
    expect(snapshot?.textContent).toContain('sha256:script')
    expect(snapshot?.textContent).toContain('sha256:runtime')
    expect(snapshot?.textContent).toContain('30000')
    expect(screen.container.querySelector('[data-choice="remember"]')).toBeNull()

    await screen.getByRole('button', { name: '批准' }).click()
    expect(onApprove).toHaveBeenCalledWith('assistant-message', action, 'singleAction')
  })
})

describe('AgentApprovalDialog command approval', () => {
  it('shows a multiline heredoc as one scrollable preformatted command', async () => {
    const command = [
      "python3 <<'PY'",
      'for page in range(1, 80):',
      "    print(f'page={page}')",
      'PY'
    ].join('\n')
    const action: AgentProposedAction = {
      type: 'command',
      command: {
        id: 'command-action',
        command,
        cwd: null,
        timeoutMs: null,
        approvalStatus: 'required',
        riskLevel: null,
        reason: '检查 PDF 页面',
        observe: null
      }
    }
    const onApprove = vi.fn()
    const screen = await render(
      <AgentApprovalDialog
        target={{ action, messageId: 'assistant-message' }}
        onApprove={onApprove}
      />
    )

    const code = screen.container.querySelector<HTMLElement>('.agent-approval-dialog__command')
    expect(screen.container.querySelectorAll('.agent-approval-dialog__command')).toHaveLength(1)
    expect(code?.getAttribute('data-multiline')).toBe('true')
    expect(code?.textContent).toBe(command)
    expect(window.getComputedStyle(code!).maxHeight).toBe('180px')
    expect(window.getComputedStyle(code!).overflow).toBe('auto')
    expect(window.getComputedStyle(code!).whiteSpace).toBe('pre-wrap')
    expect(screen.container.querySelector('[data-choice="remember"]')).toBeNull()

    await screen.getByRole('button', { name: '批准' }).click()
    expect(onApprove).toHaveBeenCalledWith('assistant-message', action, 'singleAction')
  })

  it('unlocks a standard approval after an unaccepted or failed submission', async () => {
    const action: AgentProposedAction = {
      type: 'command',
      command: {
        id: 'retry-command-action',
        command: 'pnpm test',
        cwd: null,
        timeoutMs: null,
        approvalStatus: 'required',
        riskLevel: null,
        reason: '运行测试',
        observe: null
      }
    }
    const onApprove = vi
      .fn<() => Promise<boolean>>()
      .mockResolvedValueOnce(false)
      .mockRejectedValueOnce(new Error('temporarily unavailable'))
      .mockResolvedValueOnce(true)
    const screen = await render(
      <AgentApprovalDialog
        target={{ action, messageId: 'assistant-message' }}
        onApprove={onApprove}
      />
    )

    const approve = screen.getByRole('button', { name: '批准' })
    await approve.click()
    await expect.element(approve).toBeEnabled()

    await approve.click()
    await expect.element(approve).toBeEnabled()

    await approve.click()
    expect(onApprove).toHaveBeenCalledTimes(3)
  })
})

describe('AgentApprovalDialog Office approval', () => {
  it('shows logical Office paths without exposing frozen execution details', async () => {
    const parentIdentity = { revision: 'office-path-parent-v1:test', device: 1, inode: 2 }
    const onApprove = vi.fn()
    const action: AgentProposedAction = {
      type: 'office_operation',
      officeOperation: {
        schemaVersion: 6,
        id: 'office-action',
        approvalStatus: 'required',
        reason: '导出预算工作簿',
        semanticArgs: {
          operation: 'view',
          filePath: 'budget.xlsx',
          mode: 'html',
          outputPath: '@downloads/budget-preview.html',
          reason: '导出预算工作簿'
        },
        prepared: {
          schemaVersion: 6,
          providerId: 'officecli',
          engineRevision: 'office-engine-sha256-v1:test',
          workspaceRevision: 'office-workspace-sha256-v1:test',
          access: 'fileWrite',
          request: {
            documentKind: 'spreadsheet',
            operation: 'view',
            documentPath: 'budget.xlsx',
            parameters: { type: 'view', mode: 'html' },
            outputPath: '@downloads/budget-preview.html',
            destinationPath: null,
            inputs: [],
            timeoutMs: null
          },
          argv: ['view', 'budget.xlsx', 'html', '-o', '@downloads/budget-preview.html'],
          resolvedRenderPlan: null,
          paths: [
            {
              slot: { type: 'document' },
              logicalPath: 'budget.xlsx',
              purpose: 'readSource',
              scope: 'workspace',
              normalizedPath: '/workspace/budget.xlsx',
              state: 'present',
              objectIdentity: { revision: 'office-path-v1:source', device: 1, inode: 3 },
              parentIdentity,
              contentRevision: 'office-file-sha256-v1:source',
              size: 128,
              writeDisposition: null
            },
            {
              slot: { type: 'output' },
              logicalPath: '@downloads/budget-preview.html',
              purpose: 'writeTarget',
              scope: 'external',
              normalizedPath: '/Users/test/Downloads/budget-preview.html',
              state: 'missing',
              objectIdentity: null,
              parentIdentity: {
                revision: 'office-path-parent-v1:downloads',
                device: null,
                inode: null
              },
              contentRevision: null,
              size: null,
              writeDisposition: 'createNew'
            }
          ],
          inputBindings: []
        }
      }
    }
    const screen = await render(
      <AgentApprovalDialog
        target={{ action, messageId: 'assistant-message' }}
        onApprove={onApprove}
      />
    )

    const snapshot = screen.container.querySelector('.agent-approval-dialog__command')
    expect(snapshot?.getAttribute('data-multiline')).toBe('true')
    expect(snapshot?.textContent).toContain('budget.xlsx')
    expect(snapshot?.textContent).toContain('@downloads/budget-preview.html')
    expect(snapshot?.textContent).not.toContain('officecli')
    expect(snapshot?.textContent).not.toContain('/Users/test/Downloads/budget-preview.html')
    expect(snapshot?.textContent).not.toContain('readSource')
    expect(snapshot?.textContent).not.toContain('createNew')
    expect(screen.container.querySelector('[data-choice="remember"]')).toBeNull()

    await screen.getByRole('button', { name: '批准' }).click()
    expect(onApprove).toHaveBeenCalledWith('assistant-message', action, 'singleAction')
  })
})
