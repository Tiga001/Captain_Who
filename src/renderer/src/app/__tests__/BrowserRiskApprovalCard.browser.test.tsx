import type { AgentProposedAction } from '@mycopilot/protocol'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { AgentApprovalDialog } from '../../features/chat/components/AgentApprovalDialog'
import '../../styles/global.css'
import '../../features/chat/ChatConversationPage.approvals.css'

const translations: Record<string, string> = {
  'mcp.builtin.browserAutomation.name': 'Browser automation',
  'agent.approval.dialog.approve': 'Yes',
  'agent.approval.dialog.reject': 'No',
  'agent.approval.dialog.rejectPlaceholder': 'No, tell me how to adjust',
  'agent.browserRisk.approval.title': 'Allow “{capability}” to access this destination?',
  'agent.browserRisk.approval.reason': 'Call reason',
  'agent.browserRisk.approval.origin': 'Origin',
  'agent.browserRisk.approval.risks': 'Risk',
  'agent.browserRisk.approval.createdAt': 'Created',
  'agent.browserRisk.approval.expiresAt': 'Expires',
  'agent.browserRisk.approval.taskGrantHint':
    'Approval applies only to this exact destination scope in the current task.',
  'agent.browserRisk.approval.expired': 'This browser access request has expired.',
  'agent.browserRisk.risk.insecure_http': 'Unencrypted HTTP',
  'agent.browserRisk.risk.loopback': 'Loopback address',
  'agent.browserRisk.risk.non_standard_port': 'Non-standard port'
}

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => translations[key] ?? key })
}))

const HIDDEN_CANARY = 'CDP_COOKIE_AUTHORIZATION_TARGET_CANARY'

function action(
  overrides: Partial<
    Extract<AgentProposedAction, { type: 'browser_risk_approval' }>['approval']
  > = {}
): Extract<AgentProposedAction, { type: 'browser_risk_approval' }> {
  const now = Math.floor(Date.now() / 1000)
  return {
    type: 'browser_risk_approval',
    approval: {
      schemaVersion: 1,
      actionId: '94c2f39c-ddaa-49bb-a3ef-8756053d68c8',
      riskApprovalId: 'a8a6102c-8ad6-45d5-bb0d-3e4f0ad2a30f',
      runId: 'run-owned',
      callId: `tc1_${'a'.repeat(43)}`,
      capabilityId: 'browser_automation',
      capabilityActivationId: '67b4ea45-d0e2-42d0-aee4-6da42d5e45a7',
      displayName: '<img src=x onerror=alert(1)>Browser automation',
      reason: '<script>alert(1)</script>Inspect the local fixture',
      destination: {
        normalizedUrl: 'http://127.0.0.1:3000/safe-path',
        origin: 'http://127.0.0.1:3000',
        scheme: 'http',
        asciiHost: '127.0.0.1',
        effectivePort: 3000,
        addressClass: 'loopback'
      },
      trigger: 'tool_argument',
      triggerToolName: 'browser_navigate',
      riskKinds: ['insecure_http', 'loopback', 'non_standard_port'],
      manifestDigest: `sha256:${'e'.repeat(64)}`,
      policyRevision: 7,
      createdAt: now,
      expiresAt: now + 900,
      approvalStatus: 'required',
      ...overrides
    }
  }
}

function getButton(container: HTMLElement, label: string): HTMLButtonElement {
  const button = [...container.querySelectorAll('button')].find((candidate) =>
    candidate.textContent?.trim().endsWith(label)
  )
  if (!button) throw new Error(`Expected button: ${label}`)
  return button
}

afterEach(() => {
  vi.useRealTimers()
  vi.clearAllMocks()
})

describe('BrowserRiskApprovalCard', () => {
  it('renders only the safe destination projection as plain text', async () => {
    const proposed = action()
    const screen = await render(
      <AgentApprovalDialog target={{ action: proposed, messageId: 'assistant-message' }} />
    )

    expect(screen.container.textContent).toContain(
      'Allow “Browser automation” to access this destination?'
    )
    expect(screen.container.textContent).not.toContain(proposed.approval.displayName)
    expect(screen.container.textContent).toContain(
      '<script>alert(1)</script>Inspect the local fixture'
    )
    expect(screen.container.textContent).toContain('http://127.0.0.1:3000/safe-path')
    expect(screen.container.textContent).toContain('Unencrypted HTTP')
    expect(screen.container.textContent).toContain('Loopback address')
    expect(screen.container.textContent).toContain('Non-standard port')
    expect(screen.container.textContent).toContain('Created')
    expect(screen.container.textContent).not.toContain('Expires')
    expect(screen.container.querySelectorAll('time')).toHaveLength(1)
    expect(screen.container.querySelector('img')).toBeNull()
    expect(screen.container.querySelector('script')).toBeNull()
    expect(screen.container.querySelector('a')).toBeNull()
    expect(screen.container.querySelector('[data-choice="remember"]')).toBeNull()

    const hidden = [
      proposed.approval.actionId,
      proposed.approval.riskApprovalId,
      proposed.approval.callId,
      proposed.approval.capabilityActivationId,
      proposed.approval.manifestDigest,
      proposed.approval.triggerToolName,
      HIDDEN_CANARY
    ]
    for (const value of hidden) {
      expect(screen.container.textContent).not.toContain(value)
      expect(screen.container.innerHTML).not.toContain(value)
    }
    await screen.unmount()
  })

  it('approves at most once and keeps rejection guidance optional', async () => {
    const proposed = action()
    const onApprove = vi.fn()
    const screen = await render(
      <AgentApprovalDialog
        target={{ action: proposed, messageId: 'assistant-message' }}
        onApprove={onApprove}
      />
    )
    const approve = getButton(screen.container, 'Yes')
    approve.click()
    approve.click()
    expect(onApprove).toHaveBeenCalledOnce()
    expect(onApprove).toHaveBeenCalledWith('assistant-message', proposed)
    await screen.unmount()

    const onReject = vi.fn()
    const rejectionScreen = await render(
      <AgentApprovalDialog
        target={{ action: proposed, messageId: 'assistant-message' }}
        onReject={onReject}
      />
    )
    await rejectionScreen
      .getByRole('textbox', { name: 'No, tell me how to adjust' })
      .fill('Use the public HTTPS endpoint instead')
    await rejectionScreen.getByRole('button', { name: 'No' }).click()
    expect(onReject).toHaveBeenCalledWith(
      'assistant-message',
      proposed,
      'Use the public HTTPS endpoint instead'
    )
    await rejectionScreen.unmount()
  })

  it('keeps cancellation separate from rejection', async () => {
    const onCancel = vi.fn()
    const proposed = action()
    const screen = await render(
      <AgentApprovalDialog
        target={{ action: proposed, messageId: 'assistant-message' }}
        onApprove={vi.fn()}
        onCancel={onCancel}
      />
    )
    await expect.element(screen.getByRole('dialog')).toBeVisible()
    const dialog = screen.container.querySelector('.agent-approval-dialog')
    if (!dialog) throw new Error('Expected browser risk approval dialog')
    dialog.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, key: 'Escape' }))
    expect(onCancel).toHaveBeenCalledWith('assistant-message', proposed)
    await screen.unmount()
  })

  it('keeps pending approval actionable despite a legacy client expiry timestamp', async () => {
    const now = Math.floor(Date.now() / 1000)
    const expired = action({ createdAt: now - 901, expiresAt: now - 1 })
    const onApprove = vi.fn()
    const expiredScreen = await render(
      <AgentApprovalDialog
        target={{ action: expired, messageId: 'assistant-message' }}
        onApprove={onApprove}
        onReject={vi.fn()}
      />
    )
    const approve = expiredScreen.getByRole('button', { name: 'Yes' })
    await expect.element(approve).toBeEnabled()
    expect(expiredScreen.container.textContent).not.toContain('browser access request has expired')
    await approve.click()
    expect(onApprove).toHaveBeenCalledWith('assistant-message', expired)
    await expiredScreen.unmount()
  })
})
