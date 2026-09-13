import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { AutomationRunHistory } from '../components/AutomationRunHistory'
import { makeAutomationRun } from './automationUiFixtures'
import { AccountAuthContext } from '../../auth/AccountAuthContext'
import { LicenseContext } from '../../license/LicenseContext'
import '../ScheduledPage.css'

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ language: 'en-US', t: (key: string) => key })
}))

describe('AutomationRunHistory', () => {
  it.each([
    {
      code: 'ACCOUNT_LOGIN_REQUIRED',
      signedIn: false,
      action: 'auth.login',
      text: 'license.runLoginRequired'
    },
    {
      code: 'ACCOUNT_LICENSE_REQUIRED',
      signedIn: true,
      action: 'license.manage',
      text: 'license.runLicenseRequired'
    },
    {
      code: 'ACCOUNT_LICENSE_UNAVAILABLE',
      signedIn: true,
      action: 'license.retry',
      text: 'license.runVerificationRequired'
    }
  ])(
    'shows an explicit recovery action without opening anything for $code history events',
    async ({ code, signedIn, action, text }) => {
      const login = vi.fn()
      const access = vi.fn()
      const refresh = vi.fn()
      const screen = await render(
        <AccountAuthContext.Provider
          value={{
            state: {
              revision: 1,
              status: signedIn ? 'signedIn' : 'signedOut',
              profile: null,
              error: null,
              remembered: true
            },
            loginRequested: false,
            canStartTurn: () => signedIn,
            requestLogin: login,
            dismissLogin: vi.fn(),
            logout: vi.fn()
          }}
        >
          <LicenseContext.Provider
            value={{
              state: {
                revision: 1,
                status: 'denied',
                reason: 'expired',
                expiresAt: null,
                verifiedAt: null,
                cacheValidUntil: null,
                error: null
              },
              canStartTurn: () => false,
              requestAccess: access,
              refresh
            }}
          >
            <AutomationRunHistory
              onAcknowledge={vi.fn()}
              onLoadMore={vi.fn()}
              onOpenConversation={vi.fn()}
              onRetry={vi.fn()}
              runs={[
                makeAutomationRun({
                  status: 'failed',
                  conversationId: null,
                  errorCode: code,
                  errorMessage: 'Internal details'
                })
              ]}
            />
          </LicenseContext.Provider>
        </AccountAuthContext.Provider>
      )
      await expect.element(screen.getByText('license.runNotExecuted')).toBeVisible()
      await expect.element(screen.getByText(text)).toBeVisible()
      expect(login).not.toHaveBeenCalled()
      expect(access).not.toHaveBeenCalled()
      expect(refresh).not.toHaveBeenCalled()
      await screen.getByRole('button', { name: action }).click()
      expect(
        signedIn ? (code === 'ACCOUNT_LICENSE_UNAVAILABLE' ? refresh : access) : login
      ).toHaveBeenCalledOnce()
    }
  )
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
