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
  'agent.skillInstallation.waitingApproval': '等待批准安装 Skill',
  'agent.skillInstallation.prepareExpired': 'Skill 安装准备已过期',
  'agent.skillInstallation.installed': '已安装 Skill',
  'agent.skillInstallation.cancelled': '已取消安装 Skill',
  'agent.skillInstallation.installFailed': 'Skill 安装失败',
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
          approvalStatus: 'not_required',
          reason: null
        }}
        run={{
          runId: 'run-1',
          status: 'running',
          toolDefinitions: [],
          toolCalls: [],
          toolResults: [],
          approvals: [],
          fileChangeProposals: [],
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

  it('keeps approval actionable despite a legacy client expiry timestamp', async () => {
    const onApprove = vi.fn()
    const onReject = vi.fn()
    const expiredAction = {
      ...action,
      installation: { ...action.installation, expiresAt: Date.now() - 1 }
    }
    const screen = await render(
      <SkillInstallationApprovalCard
        action={expiredAction}
        messageId="assistant-message"
        onApprove={onApprove}
        onReject={onReject}
      />
    )

    const approve = screen.getByRole('button', { name: '批准安装' })
    await expect.element(approve).toBeEnabled()
    expect(screen.container.textContent).not.toContain('安装准备已过期。')
    await approve.click()
    expect(onApprove).toHaveBeenCalledWith('assistant-message', expiredAction)
  })

  it('keeps rejection available for a legacy expired approval', async () => {
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

    await screen.getByRole('button', { name: '拒绝' }).click()
    expect(onReject).toHaveBeenCalledWith('assistant-message', expiredAction, undefined)
  })

  it('does not turn a waiting timeline item into expired from the local clock', async () => {
    const expiredAction = {
      ...action.installation,
      expiresAt: Date.now() - 1
    }
    const screen = await render(
      <SkillInstallationToolActivity
        call={{
          id: expiredAction.id,
          tool: 'skills_commit_install',
          args: { installRef: expiredAction.installRef },
          approvalStatus: 'required',
          reason: null
        }}
        run={{
          runId: 'run-1',
          status: 'waiting_for_approval',
          toolDefinitions: [],
          toolCalls: [],
          toolResults: [],
          approvals: [],
          fileChangeProposals: [],
          timeline: [],
          skillInstallations: [{ action: expiredAction, status: 'waiting_for_approval' }]
        }}
      />
    )

    await expect.element(screen.getByText('等待批准安装 Skill')).toBeVisible()
    expect(screen.container.textContent).not.toContain('Skill 安装准备已过期')
  })

  it('uses the Skill loading icon and the same three-field summary in identified and installed Timeline states', async () => {
    const baseRun: ChatAgentRunView = {
      runId: 'run-1',
      status: 'running',
      toolDefinitions: [],
      toolCalls: [],
      toolResults: [],
      approvals: [],
      fileChangeProposals: [],
      timeline: []
    }
    const inspection = await render(
      <SkillInstallationToolActivity
        call={{
          id: 'prepare-install',
          tool: 'skills_prepare_install',
          args: { source: 'https://github.com/example/social' },
          approvalStatus: 'not_required',
          reason: null
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
          approvalStatus: 'approved',
          reason: null
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

  it('settles a direct installation failure without keeping the running shimmer', async () => {
    const screen = await render(
      <SkillInstallationToolActivity
        call={{
          id: 'failed-install',
          tool: 'skills_commit_install',
          args: { installRef: `skill_install_${'b'.repeat(32)}` },
          approvalStatus: 'not_required',
          reason: null
        }}
        result={{
          callId: 'failed-install',
          tool: 'skills_commit_install',
          ok: false,
          result: null,
          error: 'The Skill installation reference does not belong to this run.'
        }}
        run={{
          runId: 'run-1',
          status: 'completed',
          toolDefinitions: [],
          toolCalls: [],
          toolResults: [],
          approvals: [],
          fileChangeProposals: [],
          timeline: []
        }}
      />
    )

    const label = screen.getByText('Skill 安装失败')
    await expect.element(label).toBeVisible()
    expect(label.element()).not.toHaveClass('agent-running-text')
    expect(screen.container.querySelector('.lucide-circle-x')).not.toBeNull()
    await label.click()
    expect(screen.container.textContent).toContain(
      'The Skill installation reference does not belong to this run.'
    )
  })
})
