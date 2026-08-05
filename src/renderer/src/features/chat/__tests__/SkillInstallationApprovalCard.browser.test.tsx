import type { AgentProposedAction } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatAgentRunView } from '../chatTypes'
import { SkillInstallationApprovalCard } from '../components/SkillInstallationApprovalCard'
import { SkillInstallationToolActivity } from '../components/toolActivities/SkillInstallationToolActivity'

const translations: Record<string, string> = {
  'agent.skillInstallation.title': '安装“{name}”？',
  'agent.skillInstallation.approve': '批准安装',
  'agent.skillInstallation.reject': '拒绝',
  'agent.skillInstallation.source': '来源',
  'agent.skillInstallation.description': '描述',
  'agent.skillInstallation.resources': '资源组成',
  'agent.skillInstallation.resourcesValue':
    '参考资料 {references} · 资产 {assets} · 脚本 {scripts}',
  'agent.skillInstallation.expired': '安装准备已过期。',
  'agent.skillInstallation.unknownSource': '未知来源',
  'agent.skillInstallation.unnamed': '未命名 Skill',
  'agent.skillInstallation.noDescription': '未提供描述。',
  'agent.skillInstallation.inspecting': '正在检查来源、下载并验证 Skill',
  'agent.skillInstallation.identified': '已识别 Skill',
  'agent.skillInstallation.installed': '已安装 Skill',
  'agent.approval.dialog.rejectPlaceholder': '拒绝原因'
}

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => translations[key] ?? key })
}))

const action: Extract<AgentProposedAction, { type: 'skill_installation' }> = {
  type: 'skill_installation',
  installation: {
    schemaVersion: 1,
    id: 'install-action',
    installRef: `skill_install_${'a'.repeat(32)}`,
    approvalStatus: 'required',
    expiresAt: Date.now() + 60_000,
    preview: {
      name: 'social',
      description: 'Create social posts.',
      sourceSummary: { kind: 'github', url: 'https://github.com/example/social' },
      resolvedRevision: '0123456789abcdef',
      fileCount: 4,
      totalBytes: 2_048,
      resourceSummary: { total: 3, references: 1, assets: 1, scripts: 1, bytes: 1_024 },
      containsScripts: true,
      warnings: [
        {
          code: 'containsScripts',
          message: 'This Skill contains scripts.',
          requiresAcknowledgement: true
        }
      ],
      compatibility: 'compatibleWithWarnings',
      operation: 'install',
      impact: 'addManagedSkill'
    }
  }
}

describe('SkillInstallationApprovalCard', () => {
  it('shows the complete trusted backend inspection stage while prepare is pending', async () => {
    const screen = await render(
      <SkillInstallationToolActivity
        call={{
          id: 'prepare-pending',
          tool: 'skills_prepare_install',
          args: { source: 'https://github.com/example/social' },
          approvalStatus: 'not_required'
        }}
        run={{
          runId: 'run-1',
          status: 'running',
          toolDefinitions: [],
          toolCalls: [],
          toolResults: [],
          approvals: [],
          diffs: [],
          timeline: []
        }}
      />
    )

    await expect.element(screen.getByText('正在检查来源、下载并验证 Skill')).toBeVisible()
  })

  it('renders the frozen chat-specific preview and submits only the typed action', async () => {
    const onApprove = vi.fn()
    const screen = await render(
      <SkillInstallationApprovalCard
        action={action}
        messageId="assistant-message"
        onApprove={onApprove}
        onReject={vi.fn()}
      />
    )

    await expect.element(screen.getByText('安装“social”？')).toBeVisible()
    await expect.element(screen.getByText('Create social posts.')).toBeVisible()
    await expect.element(screen.getByText('https://github.com/example/social')).toBeVisible()
    await expect.element(screen.getByText('参考资料 1 · 资产 1 · 脚本 1')).toBeVisible()
    expect(screen.container.textContent).not.toContain('0123456789abcdef')
    expect(screen.container.textContent).not.toContain('This Skill contains scripts.')
    await screen.getByRole('button', { name: '批准安装' }).click()

    expect(onApprove).toHaveBeenCalledWith('assistant-message', action)
  })

  it('blocks an expired approval but still lets the user reject it and unblock the run', async () => {
    const onReject = vi.fn()
    const expiredAction = {
      ...action,
      installation: { ...action.installation, expiresAt: Date.now() - 1 }
    }
    const screen = await render(
      <SkillInstallationApprovalCard
        action={expiredAction}
        messageId="assistant-message"
        onApprove={vi.fn()}
        onReject={onReject}
      />
    )

    await expect.element(screen.getByRole('button', { name: '批准安装' })).toBeDisabled()
    await screen.getByRole('button', { name: '拒绝' }).click()
    expect(onReject).toHaveBeenCalledWith('assistant-message', expiredAction, undefined)
  })

  it('uses the Skill loading icon and the same three-field summary in identified and installed Timeline states', async () => {
    const baseRun: ChatAgentRunView = {
      runId: 'run-1',
      status: 'running',
      toolDefinitions: [],
      toolCalls: [],
      toolResults: [],
      approvals: [],
      diffs: [],
      timeline: []
    }
    const inspection = await render(
      <SkillInstallationToolActivity
        call={{
          id: 'prepare-install',
          tool: 'skills_prepare_install',
          args: { source: 'https://github.com/example/social' },
          approvalStatus: 'not_required'
        }}
        result={{
          callId: 'prepare-install',
          tool: 'skills_prepare_install',
          ok: true,
          result: {
            status: 'ready',
            name: 'social',
            description: action.installation.preview.description,
            sourceSummary: action.installation.preview.sourceSummary,
            resourceSummary: action.installation.preview.resourceSummary,
            resolvedRevision: action.installation.preview.resolvedRevision,
            warnings: action.installation.preview.warnings
          }
        }}
        run={baseRun}
        settledStatus="completed"
      />
    )

    expect(inspection.container.querySelector('.lucide-wand-sparkles')).not.toBeNull()
    await inspection.getByText('已识别 Skill').click()
    expect(inspection.container.textContent).toContain('Create social posts.')
    expect(inspection.container.textContent).toContain('https://github.com/example/social')
    expect(inspection.container.textContent).toContain('参考资料 1 · 资产 1 · 脚本 1')
    expect(inspection.container.textContent).not.toContain('0123456789abcdef')
    expect(inspection.container.textContent).not.toContain('This Skill contains scripts.')

    const installed = await render(
      <SkillInstallationToolActivity
        call={{
          id: action.installation.id,
          tool: 'skills_commit_install',
          args: { installRef: action.installation.installRef },
          approvalStatus: 'approved'
        }}
        result={{
          callId: action.installation.id,
          tool: 'skills_commit_install',
          ok: true,
          result: { status: 'installed', availableFrom: 'nextRun' }
        }}
        run={{
          ...baseRun,
          status: 'completed',
          skillInstallations: [{ action: action.installation, status: 'installed' }]
        }}
        settledStatus="completed"
      />
    )

    expect(installed.container.querySelector('.lucide-wand-sparkles')).not.toBeNull()
    await installed.getByText('已安装 Skill').click()
    expect(installed.container.textContent).toContain('Create social posts.')
    expect(installed.container.textContent).toContain('https://github.com/example/social')
    expect(installed.container.textContent).toContain('参考资料 1 · 资产 1 · 脚本 1')
    expect(installed.container.textContent).not.toContain('0123456789abcdef')
    expect(installed.container.textContent).not.toContain('This Skill contains scripts.')
  })
})
