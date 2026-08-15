import type { AgentProposedAction } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { AgentApprovalDialog } from '../../features/chat/components/AgentApprovalDialog'
import '../../styles/global.css'
import '../../features/chat/ChatConversationPage.approvals.css'

const translations: Record<string, string> = {
  'agent.approval.dialog.approve': '批准',
  'agent.approval.dialog.commandPolicyHint': '确认后执行',
  'agent.approval.dialog.reject': '拒绝',
  'agent.approval.dialog.rejectPlaceholder': '说明拒绝原因',
  'agent.approval.dialog.toolTitle': '运行 {tool}'
}

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => translations[key] ?? key })
}))

vi.mock('../../features/chat/components/toolActivities/toolActivityUtils', () => ({
  formatToolDetails: () => '',
  getToolDisplayName: () => 'Skill 脚本'
}))

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
    expect(onApprove).toHaveBeenCalledWith('assistant-message', action, {
      rememberForRun: false
    })
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

    await screen.getByRole('button', { name: '批准' }).click()
    expect(onApprove).toHaveBeenCalledWith('assistant-message', action, {
      rememberForRun: false
    })
  })
})

describe('AgentApprovalDialog Office approval', () => {
  it('shows logical Office paths without exposing frozen execution details', async () => {
    const parentIdentity = { revision: 'office-path-parent-v1:test', device: 1, inode: 2 }
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
      <AgentApprovalDialog target={{ action, messageId: 'assistant-message' }} />
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
  })
})
