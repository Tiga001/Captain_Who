import type { AgentProposedAction } from '@mycopilot/protocol'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { AgentApprovalDialog } from '../../features/chat/components/AgentApprovalDialog'
import '../../styles/global.css'
import '../../features/chat/ChatConversationPage.approvals.css'

const locale = vi.hoisted(() => ({ language: 'en-US' as 'en-US' | 'zh-CN' }))

const translations = vi.hoisted(
  () =>
    ({
      'en-US': {
        'mcp.builtin.browserAutomation.name': 'Browser automation',
        'agent.approval.dialog.approve': 'Yes',
        'agent.approval.dialog.reject': 'No',
        'agent.approval.dialog.rejectPlaceholder': 'No, tell me how to adjust',
        'agent.builtinMcpApproval.title': 'Allow “{capability}” to “{tool}”?',
        'agent.builtinMcpApproval.reason': 'Call reason',
        'agent.builtinMcpApproval.operation': 'Operation',
        'agent.builtinMcpApproval.resource': 'Resource scope',
        'agent.builtinMcpApproval.resource.generic': 'Restricted data in the current page',
        'agent.builtinMcpApproval.resource.managedBrowserProfile':
          'The entire MyCopilot managed browser profile',
        'agent.builtinMcpApproval.resource.managedSurface': 'The current managed browser tab',
        'agent.builtinMcpApproval.origin': 'Page origin',
        'agent.builtinMcpApproval.files': 'Files',
        'agent.builtinMcpApproval.risks': 'Sensitive access',
        'agent.builtinMcpApproval.createdAt': 'Created',
        'agent.builtinMcpApproval.expiresAt': 'Expires',
        'agent.builtinMcpApproval.exactScopeHint':
          'Approval applies only to this exact operation in the current task.',
        'agent.builtinMcpApproval.expired': 'This sensitive browser operation request has expired.',
        'agent.builtinMcpApproval.tool.unknown': 'perform a sensitive browser operation',
        'agent.builtinMcpApproval.tool.browser_evaluate': 'run a script in the page',
        'agent.builtinMcpApproval.risk.page_script_execution': 'Run a script in the current page'
      },
      'zh-CN': {
        'mcp.builtin.browserAutomation.name': '浏览器自动化',
        'agent.approval.dialog.approve': '是',
        'agent.approval.dialog.reject': '否',
        'agent.approval.dialog.rejectPlaceholder': '否，请告诉我如何调整',
        'agent.builtinMcpApproval.title': '允许“{capability}”执行“{tool}”吗？',
        'agent.builtinMcpApproval.reason': '调用理由',
        'agent.builtinMcpApproval.operation': '操作类别',
        'agent.builtinMcpApproval.resource': '资源范围',
        'agent.builtinMcpApproval.resource.generic': '当前网页中的受限资源',
        'agent.builtinMcpApproval.resource.managedBrowserProfile': '整个 MyCopilot 受管浏览器配置',
        'agent.builtinMcpApproval.resource.managedSurface': '当前 MyCopilot 受管浏览器标签页',
        'agent.builtinMcpApproval.origin': '网页来源',
        'agent.builtinMcpApproval.files': '文件',
        'agent.builtinMcpApproval.risks': '敏感权限',
        'agent.builtinMcpApproval.createdAt': '创建时间',
        'agent.builtinMcpApproval.expiresAt': '过期时间',
        'agent.builtinMcpApproval.exactScopeHint':
          '批准仅适用于当前任务中的这次精确操作，不会永久允许。',
        'agent.builtinMcpApproval.expired': '这次敏感浏览器操作申请已过期。',
        'agent.builtinMcpApproval.tool.unknown': '执行敏感浏览器操作',
        'agent.builtinMcpApproval.tool.browser_evaluate': '在网页中执行脚本',
        'agent.builtinMcpApproval.risk.page_script_execution': '在当前网页执行脚本'
      }
    }) as const
)

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: locale.language,
    t: (key: string) => translations[locale.language][key as never] ?? key
  })
}))

type ApprovalAction = Extract<AgentProposedAction, { type: 'builtin_mcp_tool_approval' }>

function action(overrides: Partial<ApprovalAction['approval']> = {}): ApprovalAction {
  const now = Math.floor(Date.now() / 1000)
  return {
    type: 'builtin_mcp_tool_approval',
    approval: {
      schemaVersion: 1,
      identity: {
        actionId: '11111111-1111-4111-8111-111111111111',
        approvalId: '22222222-2222-4222-8222-222222222222',
        runId: 'run-sensitive',
        callId: `tc1_${'a'.repeat(43)}`,
        capabilityId: 'browser_automation',
        capabilityActivationId: '33333333-3333-4333-8333-333333333333',
        managedMcpId: 'CDP_ENDPOINT_TARGET_TOKEN_CANARY',
        packageName: 'COOKIE_VALUE_CANARY',
        packageVersion: 'AUTHORIZATION_HEADER_CANARY',
        upstreamCatalogDigest: `sha256:${'1'.repeat(64)}`,
        manifestDigest: `sha256:${'2'.repeat(64)}`,
        policyDigest: `sha256:${'3'.repeat(64)}`,
        policyRevision: 8,
        toolId: 'browser_evaluate',
        rawName: 'browser_evaluate',
        modelName: 'browser_evaluate',
        upstreamSchemaDigest: `sha256:${'4'.repeat(64)}`,
        hostOverlayDigest: `sha256:${'5'.repeat(64)}`,
        hostInputSchemaDigest: `sha256:${'6'.repeat(64)}`,
        argumentsDigest: `sha256:${'7'.repeat(64)}`,
        resourceScopeDigest: `sha256:${'8'.repeat(64)}`,
        origin: 'https://fixture.example'
      },
      capabilityDisplayName: 'LOCAL_STORAGE_VALUE_CANARY',
      toolDisplayName: 'REQUEST_BODY_CANARY',
      callReason: '<img src=x onerror=alert(1)>Update the empty fixture editor.',
      operationCategory: 'SESSION_STORAGE_VALUE_CANARY',
      resourceSummary: {
        scope: 'managed_surface',
        displayName: 'SCRIPT_SOURCE_CANARY browser-file:opaque-handle /Users/private/secret.txt',
        fileBasenames: ['fixture.txt'],
        origin: 'https://fixture.example'
      },
      riskKinds: ['page_script_execution'],
      createdAt: now,
      expiresAt: now + 900,
      approvalStatus: 'required',
      ...overrides
    }
  }
}

function button(container: HTMLElement, label: string): HTMLButtonElement {
  const match = [...container.querySelectorAll('button')].find((candidate) =>
    candidate.textContent?.trim().endsWith(label)
  )
  if (!match) throw new Error(`Expected button: ${label}`)
  return match
}

afterEach(() => {
  locale.language = 'en-US'
  vi.useRealTimers()
  vi.clearAllMocks()
})

describe('BuiltinMcpToolApprovalCard', () => {
  it('uses natural English and Chinese labels from typed identities', async () => {
    const proposed = action()
    const english = await render(
      <AgentApprovalDialog target={{ action: proposed, messageId: 'assistant-sensitive' }} />
    )
    expect(english.container.textContent).toContain(
      'Allow “Browser automation” to “run a script in the page”?'
    )
    expect(english.container.textContent).toContain('The current managed browser tab')
    expect(english.container.textContent).toContain('Run a script in the current page')
    await english.unmount()

    locale.language = 'zh-CN'
    const chinese = await render(
      <AgentApprovalDialog target={{ action: proposed, messageId: 'assistant-sensitive' }} />
    )
    expect(chinese.container.textContent).toContain('允许“浏览器自动化”执行“在网页中执行脚本”吗？')
    expect(chinese.container.textContent).toContain('当前 MyCopilot 受管浏览器标签页')
    expect(chinese.container.textContent).toContain('在当前网页执行脚本')
    await chinese.unmount()
  })

  it('renders allowlisted fields as plain text and keeps secrets and authority out of the DOM', async () => {
    const proposed = action()
    const screen = await render(
      <AgentApprovalDialog target={{ action: proposed, messageId: 'assistant-sensitive' }} />
    )

    expect(screen.container.textContent).toContain(
      '<img src=x onerror=alert(1)>Update the empty fixture editor.'
    )
    expect(screen.container.textContent).toContain('https://fixture.example')
    expect(screen.container.textContent).toContain('fixture.txt')
    expect(screen.container.querySelector('img')).toBeNull()
    expect(screen.container.querySelector('script')).toBeNull()
    expect(screen.container.querySelector('a')).toBeNull()
    expect(screen.container.querySelector('[data-choice="remember"]')).toBeNull()
    expect(screen.container.textContent).not.toContain('Expires')
    expect(screen.container.querySelectorAll('time')).toHaveLength(1)

    const hidden = [
      'CDP_ENDPOINT_TARGET_TOKEN_CANARY',
      'COOKIE_VALUE_CANARY',
      'AUTHORIZATION_HEADER_CANARY',
      'LOCAL_STORAGE_VALUE_CANARY',
      'REQUEST_BODY_CANARY',
      'SESSION_STORAGE_VALUE_CANARY',
      'SCRIPT_SOURCE_CANARY',
      'browser-file:opaque-handle',
      '/Users/private/secret.txt',
      proposed.approval.identity.actionId,
      proposed.approval.identity.approvalId,
      proposed.approval.identity.callId,
      proposed.approval.identity.argumentsDigest,
      proposed.approval.identity.resourceScopeDigest
    ]
    for (const canary of hidden) {
      expect(screen.container.textContent).not.toContain(canary)
      expect(screen.container.innerHTML).not.toContain(canary)
    }
    await screen.unmount()
  })

  it('approves at most once and supports rejection with or without guidance', async () => {
    const proposed = action()
    const onApprove = vi.fn()
    const onRejectAfterApprove = vi.fn()
    const onCancelAfterApprove = vi.fn()
    const screen = await render(
      <AgentApprovalDialog
        target={{ action: proposed, messageId: 'assistant-sensitive' }}
        onApprove={onApprove}
        onCancel={onCancelAfterApprove}
        onReject={onRejectAfterApprove}
      />
    )
    button(screen.container, 'Yes').click()
    button(screen.container, 'Yes').click()
    button(screen.container, 'No').click()
    screen.container
      .querySelector('.agent-approval-dialog')
      ?.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, key: 'Escape' }))
    expect(onApprove).toHaveBeenCalledOnce()
    expect(onApprove).toHaveBeenCalledWith('assistant-sensitive', proposed)
    expect(onRejectAfterApprove).not.toHaveBeenCalled()
    expect(onCancelAfterApprove).not.toHaveBeenCalled()
    await screen.unmount()

    const onReject = vi.fn()
    const rejection = await render(
      <AgentApprovalDialog
        target={{ action: proposed, messageId: 'assistant-sensitive' }}
        onReject={onReject}
      />
    )
    await rejection
      .getByRole('textbox', { name: 'No, tell me how to adjust' })
      .fill('Do not inspect page storage')
    await rejection.getByRole('button', { name: 'No' }).click()
    expect(onReject).toHaveBeenCalledWith(
      'assistant-sensitive',
      proposed,
      'Do not inspect page storage'
    )
    await rejection.unmount()

    const onRejectWithoutReason = vi.fn()
    const emptyRejection = await render(
      <AgentApprovalDialog
        target={{ action: proposed, messageId: 'assistant-sensitive' }}
        onReject={onRejectWithoutReason}
      />
    )
    await emptyRejection.getByRole('button', { name: 'No' }).click()
    expect(onRejectWithoutReason).toHaveBeenCalledWith('assistant-sensitive', proposed, undefined)
    await emptyRejection.unmount()
  })

  it('keeps cancellation separate and ignores the legacy client expiry timestamp', async () => {
    const proposed = action()
    const onCancel = vi.fn()
    const screen = await render(
      <AgentApprovalDialog
        target={{ action: proposed, messageId: 'assistant-sensitive' }}
        onCancel={onCancel}
      />
    )
    const dialog = screen.container.querySelector('.agent-approval-dialog')
    if (!dialog) throw new Error('Expected built-in MCP approval dialog')
    dialog.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, key: 'Escape' }))
    expect(onCancel).toHaveBeenCalledOnce()
    expect(onCancel).toHaveBeenCalledWith('assistant-sensitive', proposed)
    await screen.unmount()

    const now = Math.floor(Date.now() / 1000)
    const expired = action({ createdAt: now - 901, expiresAt: now - 1 })
    const onApprove = vi.fn()
    const expiredScreen = await render(
      <AgentApprovalDialog
        target={{ action: expired, messageId: 'assistant-sensitive' }}
        onApprove={onApprove}
        onReject={vi.fn()}
      />
    )
    const approve = expiredScreen.getByRole('button', { name: 'Yes' })
    await expect.element(approve).toBeEnabled()
    expect(expiredScreen.container.textContent).not.toContain(
      'sensitive browser operation request has expired'
    )
    await approve.click()
    expect(onApprove).toHaveBeenCalledWith('assistant-sensitive', expired)
    await expiredScreen.unmount()
  })

  it('uses a generic localized resource for an unknown safe scope', async () => {
    const proposed = action({
      resourceSummary: {
        scope: 'future_sensitive_scope',
        displayName: 'UNTRUSTED_SERVER_LABEL_CANARY',
        fileBasenames: [],
        origin: 'https://fixture.example'
      }
    })
    const screen = await render(
      <AgentApprovalDialog target={{ action: proposed, messageId: 'assistant-sensitive' }} />
    )
    expect(screen.container.textContent).toContain('Restricted data in the current page')
    expect(screen.container.textContent).not.toContain('future_sensitive_scope')
    expect(screen.container.textContent).not.toContain('UNTRUSTED_SERVER_LABEL_CANARY')
    await screen.unmount()
  })

  it('makes managed-profile-wide cookie and storage access explicit', async () => {
    const proposed = action({
      resourceSummary: {
        scope: 'managed_browser_profile',
        displayName: 'MUST_NOT_RENDER_RAW_SCOPE_LABEL',
        fileBasenames: [],
        origin: 'https://fixture.example'
      }
    })
    const screen = await render(
      <AgentApprovalDialog target={{ action: proposed, messageId: 'assistant-sensitive' }} />
    )
    expect(screen.container.textContent).toContain('The entire MyCopilot managed browser profile')
    expect(screen.container.textContent).not.toContain('MUST_NOT_RENDER_RAW_SCOPE_LABEL')
    await screen.unmount()
  })
})
