import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { AutomationRunHistory } from '../components/AutomationRunHistory'
import { makeAutomationRun } from './automationUiFixtures'
import '../ScheduledPage.css'

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ language: 'en-US', t: (key: string) => key })
}))

describe('AutomationRunHistory', () => {
  it('shows durable attention, opens the exact message, and acknowledges it', async () => {
    const onAcknowledge = vi.fn()
    const onOpenConversation = vi.fn()
    const screen = await render(
      <AutomationRunHistory
        onAcknowledge={onAcknowledge}
        onLoadMore={vi.fn()}
        onOpenConversation={onOpenConversation}
        onRetry={vi.fn()}
        runs={[
          makeAutomationRun({
            status: 'waiting_for_approval',
            completedAt: null,
            attention: {
              schemaVersion: 1,
              attentionId: 'attention-1',
              automationId: 'automation-1',
              runId: 'run-1',
              kind: 'waiting_for_approval',
              message: 'Approval required',
              createdAt: 1_800_000_000_015,
              readAt: null
            }
          })
        ]}
      />
    )

    await expect.element(screen.getByText('automation.statusWaitingApproval')).toBeVisible()
    await screen.getByRole('button', { name: /automation.openChat/ }).click()
    expect(onAcknowledge).toHaveBeenCalledWith('attention-1')
    expect(onOpenConversation).toHaveBeenCalledWith('conversation-1', 'message-assistant-1')
  })

  it('localizes stable failure codes instead of rendering a backend-language message', async () => {
    const screen = await render(
      <AutomationRunHistory
        onAcknowledge={vi.fn()}
        onLoadMore={vi.fn()}
        onOpenConversation={vi.fn()}
        onRetry={vi.fn()}
        runs={[
          makeAutomationRun({
            status: 'failed',
            errorCode: 'permission_disabled',
            errorMessage: 'Permission has been disabled on the server'
          })
        ]}
      />
    )

    await expect.element(screen.getByText('automation.runErrorPermission')).toBeVisible()
    await expect
      .element(screen.getByText('Permission has been disabled on the server'))
      .not.toBeInTheDocument()
  })

  it('offers a retry for an authoritative history load failure', async () => {
    const onRetry = vi.fn()
    const screen = await render(
      <AutomationRunHistory
        error={new Error('offline')}
        onAcknowledge={vi.fn()}
        onLoadMore={vi.fn()}
        onOpenConversation={vi.fn()}
        onRetry={onRetry}
        runs={[]}
      />
    )

    await expect.element(screen.getByRole('alert')).toBeVisible()
    await screen.getByRole('button', { name: 'automation.retry' }).click()
    expect(onRetry).toHaveBeenCalledOnce()
  })
})
