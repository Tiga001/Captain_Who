import type { AgentProposedAction } from '@mycopilot/protocol'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { AgentApprovalDialog } from '../../features/chat/components/AgentApprovalDialog'
import '../../styles/global.css'
import '../../features/chat/ChatConversationPage.approvals.css'

const translations: Record<string, string> = {
  'agent.approval.dialog.approve': 'Yes',
  'agent.approval.dialog.reject': 'No',
  'agent.approval.dialog.rejectPlaceholder': 'No, tell me how to adjust',
  'mcp.builtin.browserAutomation.name': 'Browser automation',
  'agent.builtinCapability.approval.title': 'Allow {capability}?',
  'agent.builtinCapability.approval.reason': 'Call reason',
  'agent.builtinCapability.approval.taskGrantHint':
    'After approval, reviewed tools for this capability may run automatically for this task.',
  'agent.builtinCapability.approval.expired': 'This capability activation request has expired.'
}

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => translations[key] ?? key })
}))

function createAction(
  overrides: Partial<
    Extract<AgentProposedAction, { type: 'builtin_capability_activation' }>['approval']
  > = {}
): Extract<AgentProposedAction, { type: 'builtin_capability_activation' }> {
  const now = Math.floor(Date.now() / 1000)
  return {
    type: 'builtin_capability_activation',
    approval: {
      actionId: '94c2f39c-ddaa-49bb-a3ef-8756053d68c8',
      activationId: '67b4ea45-d0e2-42d0-aee4-6da42d5e45a7',
      runId: 'run-owned',
      callId: `tc1_${'a'.repeat(43)}`,
      capabilityId: 'browser_automation',
      displayName: '<img src=x onerror=alert(1)>Browser automation',
      reason: 'Open the requested page in the in-app browser.',
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

describe('BuiltinCapabilityActivationApprovalCard', () => {
  it('localizes the capability name and never renders the Host display name', async () => {
    const action = createAction()
    const onApprove = vi.fn()
    const screen = await render(
      <AgentApprovalDialog
        target={{ action, messageId: 'assistant-message' }}
        onApprove={onApprove}
      />
    )

    const dialog = screen.container.querySelector('.agent-approval-dialog')
    expect(dialog?.getAttribute('data-approval-kind')).toBe('standard')
    expect(screen.container.textContent).toContain('Allow Browser automation?')
    expect(screen.container.textContent).toContain('Call reason')
    expect(screen.container.textContent).toContain(action.approval.reason)
    expect(screen.container.textContent).toContain(
      'reviewed tools for this capability may run automatically for this task'
    )
    expect(screen.container.querySelector('img')).toBeNull()
    expect(screen.container.textContent).not.toContain(action.approval.displayName)
    expect(screen.container.querySelector('[data-choice="remember"]')).toBeNull()
    expect(screen.container.textContent).not.toContain(action.approval.capabilityId)
    expect(screen.container.textContent).not.toContain(action.approval.manifestDigest)
    expect(screen.container.textContent).not.toContain('Server')
    expect(screen.container.textContent).not.toContain('transport')

    const approve = getButton(screen.container, 'Yes')
    approve.click()
    approve.click()
    expect(onApprove).toHaveBeenCalledOnce()
    expect(onApprove).toHaveBeenCalledWith('assistant-message', action, 'singleAction')
    screen.unmount()
  })

  it('rejects without guidance as an explicit empty decision', async () => {
    const action = createAction()
    const onReject = vi.fn()
    const screen = await render(
      <AgentApprovalDialog
        target={{ action, messageId: 'assistant-message' }}
        onReject={onReject}
      />
    )
    await screen.getByRole('button', { name: 'No' }).click()
    expect(onReject).toHaveBeenCalledWith('assistant-message', action, undefined)
    screen.unmount()
  })

  it('submits optional rejection guidance', async () => {
    const action = createAction()
    const onReject = vi.fn()
    const screen = await render(
      <AgentApprovalDialog
        target={{ action, messageId: 'assistant-message' }}
        onReject={onReject}
      />
    )
    await screen
      .getByRole('textbox', { name: 'No, tell me how to adjust' })
      .fill('Do not open a browser for this task')
    await screen.getByRole('button', { name: 'No' }).click()
    expect(onReject).toHaveBeenCalledWith(
      'assistant-message',
      action,
      'Do not open a browser for this task'
    )
    screen.unmount()
  })

  it('keeps Escape cancellation separate from rejection', async () => {
    const action = createAction()
    const onCancel = vi.fn()
    const screen = await render(
      <AgentApprovalDialog
        target={{ action, messageId: 'assistant-message' }}
        onCancel={onCancel}
      />
    )
    const dialog = screen.container.querySelector('.agent-approval-dialog')
    if (!dialog) throw new Error('Expected capability approval dialog')
    dialog.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, key: 'Escape' }))
    expect(onCancel).toHaveBeenCalledOnce()
    expect(onCancel).toHaveBeenCalledWith('assistant-message', action)
    screen.unmount()
  })

  it('keeps a pending activation approval actionable regardless of its legacy expiry timestamp', async () => {
    const nowSeconds = Math.floor(Date.now() / 1000)
    const action = createAction({ createdAt: nowSeconds - 901, expiresAt: nowSeconds - 1 })
    const onApprove = vi.fn()
    const screen = await render(
      <AgentApprovalDialog
        target={{ action, messageId: 'assistant-message' }}
        onApprove={onApprove}
        onReject={vi.fn()}
      />
    )
    const approve = screen.getByRole('button', { name: 'Yes' })

    await expect.element(approve).toBeEnabled()
    expect(screen.container.textContent).not.toContain('activation request has expired')
    await approve.click()
    expect(onApprove).toHaveBeenCalledWith('assistant-message', action, 'singleAction')
    screen.unmount()
  })

  it('unlocks the approval after an authoritative submission failure', async () => {
    const action = createAction()
    const onApprove = vi.fn(() => Promise.resolve(false))
    const screen = await render(
      <AgentApprovalDialog
        target={{ action, messageId: 'assistant-message' }}
        onApprove={onApprove}
      />
    )

    const approve = screen.getByRole('button', { name: 'Yes' })
    await approve.click()
    await vi.waitFor(() => expect(getButton(screen.container, 'Yes').disabled).toBe(false))
    await approve.click()
    expect(onApprove).toHaveBeenCalledTimes(2)
    screen.unmount()
  })
})
