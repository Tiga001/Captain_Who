import { useEffect, useState } from 'react'
import { beforeEach, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { HostInvocationError, type AuthState, type LicenseState } from '@mycopilot/host-api'

const mocks = vi.hoisted(() => ({
  state: {} as LicenseState,
  listener: null as ((state: LicenseState) => void) | null,
  get: vi.fn(),
  refresh: vi.fn(),
  openManagement: vi.fn(),
  logout: vi.fn(),
  requestLogin: vi.fn(),
  startTurn: vi.fn(),
  mounted: vi.fn(),
  unmounted: vi.fn(),
  handled: vi.fn(),
  mainError: null as unknown
}))
vi.mock('../../host/hostClient', () => ({
  hostClient: {
    core: { ping: async () => ({ ok: true }) },
    license: {
      getState: mocks.get,
      refresh: mocks.refresh,
      openManagement: mocks.openManagement,
      onStateChanged: (listener: (state: LicenseState) => void) => {
        mocks.listener = listener
        return () => {
          mocks.listener = null
        }
      }
    }
  }
}))
vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'en-US',
    resolvedColorScheme: 'light',
    t: (key: string) => key
  })
}))

import { AccountAuthContext } from '../../features/auth/AccountAuthContext'
import { LicenseProvider } from '../../features/license/LicenseProvider'
import { LicenseContext, useLicense } from '../../features/license/LicenseContext'
import { LicenseStatus } from '../../features/license/LicenseStatus'
import { AppStartupGate } from '../../features/startup/AppStartupGate'
import { AppStartupProvider } from '../../features/startup/AppStartupProvider'
import { useAppStartupStage } from '../../features/startup/AppStartupContext'
import type { AppStartupStageId } from '../../features/startup/appStartupStages'

function emit(patch: Partial<LicenseState>) {
  mocks.state = { ...mocks.state, ...patch, revision: mocks.state.revision + 1 }
  mocks.listener?.(mocks.state)
}
function Ready({ id }: { id: AppStartupStageId }) {
  const { markReady } = useAppStartupStage(id)
  useEffect(() => {
    markReady()
  }, [markReady])
  return null
}
function Workspace() {
  const license = useLicense()!
  const [ticks, setTicks] = useState(0)
  useEffect(() => {
    mocks.mounted()
    const timer = setInterval(() => setTicks((n) => n + 1), 25)
    return () => {
      clearInterval(timer)
      mocks.unmounted()
    }
  }, [])
  return (
    <div>
      <output data-testid="ticks">{ticks}</output>
      <output data-testid="can-start">{String(license.canStartTurn())}</output>
      <output data-testid="license-state">{license.state.status}</output>
      <button
        onClick={() => {
          if (license.canStartTurn()) mocks.startTurn()
          else license.requestAccess()
        }}
      >
        start-new-turn
      </button>
      <button onClick={() => mocks.handled(license.handleDenied?.(mocks.mainError))}>
        main-admission-rejected
      </button>
    </div>
  )
}
function Harness({
  authStatus = 'signedIn',
  startup = true
}: {
  authStatus?: AuthState['status']
  startup?: boolean
}) {
  return (
    <AccountAuthContext.Provider
      value={{
        state: {
          revision: 1,
          status: authStatus,
          profile: {
            userId: 'user',
            displayName: 'Captain',
            email: 'captain@example.com',
            avatarDataUrl: null,
            occupation: '',
            organization: ''
          },
          error: null,
          remembered: true
        },
        loginRequested: false,
        requestLogin: mocks.requestLogin,
        dismissLogin: vi.fn(),
        canStartTurn: () => authStatus === 'signedIn',
        logout: mocks.logout
      }}
    >
      <LicenseProvider>
        {startup ? (
          <AppStartupProvider>
            <AppStartupGate>
              <Workspace />
              {(
                [
                  'modelSettings',
                  'projects',
                  'uiPreferences',
                  'composerDrafts',
                  'conversationMetas'
                ] as const
              ).map((id) => (
                <Ready key={id} id={id} />
              ))}
            </AppStartupGate>
          </AppStartupProvider>
        ) : (
          <Workspace />
        )}
      </LicenseProvider>
    </AccountAuthContext.Provider>
  )
}
beforeEach(() => {
  vi.clearAllMocks()
  mocks.state = {
    revision: 0,
    status: 'checking',
    reason: null,
    expiresAt: null,
    verifiedAt: null,
    cacheValidUntil: null,
    error: null
  }
  mocks.get.mockImplementation(async () => mocks.state)
  mocks.refresh.mockImplementation(async () => {
    emit({ status: 'allowed', reason: 'active', error: null })
    return mocks.state
  })
  mocks.logout.mockResolvedValue({ ok: true })
  mocks.openManagement.mockReset().mockResolvedValue(undefined)
  mocks.mainError = null
})

it.each(['checking', 'denied', 'unavailable'] as const)(
  'lets a signed-in account enter the workspace while license status is %s',
  async (status) => {
    mocks.state = { ...mocks.state, status }
    const screen = await render(<Harness />)
    const workspace = () => screen.container.querySelector('.app-startup-workspace')
    await expect.poll(() => workspace()?.getAttribute('aria-hidden')).toBe('false')
    expect(workspace()?.getAttribute('inert')).toBeNull()
    await expect.element(screen.getByRole('dialog')).not.toBeInTheDocument()
    await expect.element(screen.getByTestId('can-start')).toHaveTextContent('false')
    expect(mocks.openManagement).not.toHaveBeenCalled()
    expect(mocks.refresh).not.toHaveBeenCalled()
    expect(mocks.startTurn).not.toHaveBeenCalled()
  }
)

it('preserves the mounted live workspace and running effects after a license expires', async () => {
  mocks.state = { ...mocks.state, status: 'allowed', reason: 'active' }
  const screen = await render(<Harness />)
  const workspace = () => screen.container.querySelector('.app-startup-workspace')
  await expect.poll(() => workspace()?.getAttribute('aria-hidden')).toBe('false')
  await expect.element(screen.getByTestId('can-start')).toHaveTextContent('true')
  const initialWorkspace = workspace()
  const ticks = Number(screen.getByTestId('ticks').element().textContent)
  emit({ status: 'denied', reason: 'expired' })
  await expect.element(screen.getByTestId('can-start')).toHaveTextContent('false')
  expect(workspace()).toBe(initialWorkspace)
  expect(workspace()?.getAttribute('aria-hidden')).toBe('false')
  expect(workspace()?.getAttribute('inert')).toBeNull()
  await expect.element(screen.getByRole('dialog')).not.toBeInTheDocument()
  await expect
    .poll(() => Number(screen.getByTestId('ticks').element().textContent))
    .toBeGreaterThan(ticks)
  expect(mocks.mounted).toHaveBeenCalledOnce()
  expect(mocks.unmounted).not.toHaveBeenCalled()
  expect(mocks.refresh).not.toHaveBeenCalled()
  expect(mocks.openManagement).not.toHaveBeenCalled()
  await screen.getByRole('button', { name: 'start-new-turn' }).click()
  expect(mocks.openManagement).toHaveBeenCalledOnce()
  expect(mocks.startTurn).not.toHaveBeenCalled()
  await expect.element(screen.getByRole('dialog')).not.toBeInTheDocument()
})

it.each([
  { status: 'denied', error: null },
  { status: 'unavailable', error: 'notProvisioned' }
] as const)('opens license management only after an explicit action for %j', async (patch) => {
  mocks.state = { ...mocks.state, ...patch }
  const screen = await render(<Harness startup={false} />)
  await expect.element(screen.getByTestId('license-state')).toHaveTextContent(patch.status)
  expect(mocks.openManagement).not.toHaveBeenCalled()
  await screen.getByRole('button', { name: 'start-new-turn' }).click()
  expect(mocks.openManagement).toHaveBeenCalledOnce()
  expect(mocks.refresh).not.toHaveBeenCalled()
  expect(mocks.startTurn).not.toHaveBeenCalled()
  await expect.element(screen.getByRole('dialog')).not.toBeInTheDocument()
})

it.each(['checking', 'unavailable'] as const)(
  'offers verification for %s and does not automatically submit after a successful retry',
  async (status) => {
    mocks.state = { ...mocks.state, status, error: status === 'unavailable' ? 'network' : null }
    const screen = await render(<Harness startup={false} />)
    await expect.element(screen.getByTestId('license-state')).toHaveTextContent(status)
    await screen.getByRole('button', { name: 'start-new-turn' }).click()
    await expect.element(screen.getByRole('dialog')).toHaveTextContent('license.verificationNeeded')
    expect(mocks.openManagement).not.toHaveBeenCalled()
    expect(mocks.startTurn).not.toHaveBeenCalled()
    await screen.getByRole('button', { name: 'license.retry' }).click()
    await expect.element(screen.getByRole('dialog')).not.toBeInTheDocument()
    await expect.element(screen.getByTestId('can-start')).toHaveTextContent('true')
    expect(mocks.refresh).toHaveBeenCalledOnce()
    expect(mocks.startTurn).not.toHaveBeenCalled()
    expect(mocks.openManagement).not.toHaveBeenCalled()
    await screen.getByRole('button', { name: 'start-new-turn' }).click()
    expect(mocks.startTurn).toHaveBeenCalledOnce()
  }
)

it('opens management when an explicit verification retry confirms no license', async () => {
  mocks.state = { ...mocks.state, status: 'unavailable', error: 'network' }
  mocks.refresh.mockImplementationOnce(async () => {
    emit({ status: 'denied', reason: 'expired', error: null })
    return mocks.state
  })
  const screen = await render(<Harness startup={false} />)
  await screen.getByRole('button', { name: 'start-new-turn' }).click()
  await screen.getByRole('button', { name: 'license.retry' }).click()
  await expect.element(screen.getByRole('dialog')).not.toBeInTheDocument()
  expect(mocks.openManagement).toHaveBeenCalledOnce()
  expect(mocks.startTurn).not.toHaveBeenCalled()
})

it('does not let a late state snapshot replace a newer denial event', async () => {
  let resolve: (value: LicenseState) => void = () => undefined
  mocks.get.mockImplementation(
    () =>
      new Promise<LicenseState>((done) => {
        resolve = done
      })
  )
  const screen = await render(<Harness />)
  emit({ status: 'denied', reason: 'revoked' })
  resolve({ ...mocks.state, revision: 0, status: 'allowed', reason: 'active' })
  await expect.element(screen.getByTestId('license-state')).toHaveTextContent('denied')
  await expect.element(screen.getByTestId('can-start')).toHaveTextContent('false')
  await expect
    .poll(() =>
      screen.container.querySelector('.app-startup-workspace')?.getAttribute('aria-hidden')
    )
    .toBe('false')
  expect(mocks.openManagement).not.toHaveBeenCalled()
})

it('uses the Main decision rather than comparing server timestamps to the renderer clock', async () => {
  mocks.state = {
    ...mocks.state,
    status: 'allowed',
    reason: 'active',
    expiresAt: '2000-01-01T00:00:00Z',
    cacheValidUntil: '2000-01-01T00:00:00Z'
  }
  const screen = await render(<Harness />)
  await expect.element(screen.getByTestId('can-start')).toHaveTextContent('true')
  emit({ status: 'denied', reason: 'expired' })
  await expect.element(screen.getByTestId('can-start')).toHaveTextContent('false')
})

it.each([
  ['ACCOUNT_LOGIN_REQUIRED', 'login'],
  ['ACCOUNT_LICENSE_REQUIRED', 'management'],
  ['ACCOUNT_LICENSE_UNAVAILABLE', 'verification']
] as const)(
  'handles the Main admission error %s even if Renderer last saw allowed',
  async (code, target) => {
    mocks.state = { ...mocks.state, status: 'allowed', reason: 'active' }
    mocks.mainError = new HostInvocationError({
      message: 'Admission rejected',
      code: -32000,
      data: { code }
    })
    const screen = await render(<Harness startup={false} />)
    await expect.element(screen.getByTestId('can-start')).toHaveTextContent('true')
    await screen.getByRole('button', { name: 'main-admission-rejected' }).click()
    expect(mocks.handled).toHaveBeenCalledWith(true)
    expect(mocks.requestLogin).toHaveBeenCalledTimes(target === 'login' ? 1 : 0)
    expect(mocks.openManagement).toHaveBeenCalledTimes(target === 'management' ? 1 : 0)
    if (target === 'verification') {
      await expect
        .element(screen.getByRole('dialog'))
        .toHaveTextContent('license.verificationNeeded')
    } else {
      await expect.element(screen.getByRole('dialog')).not.toBeInTheDocument()
    }
    expect(mocks.startTurn).not.toHaveBeenCalled()
  }
)

it('recognizes a wrapped Main error but ignores unrelated error messages', async () => {
  const screen = await render(<Harness startup={false} />)
  mocks.mainError = new Error('An unrelated operation mentioned ACCOUNT_LICENSE_REQUIRED')
  await screen.getByRole('button', { name: 'main-admission-rejected' }).click()
  expect(mocks.handled).toHaveBeenLastCalledWith(false)
  expect(mocks.openManagement).not.toHaveBeenCalled()
  await expect.element(screen.getByRole('dialog')).not.toBeInTheDocument()
  mocks.mainError = { cause: { data: { code: 'ACCOUNT_LICENSE_REQUIRED' } } }
  await screen.getByRole('button', { name: 'main-admission-rejected' }).click()
  expect(mocks.handled).toHaveBeenLastCalledWith(true)
  expect(mocks.openManagement).toHaveBeenCalledOnce()
})

it('recognizes an exact stable Main error message preserved by Electron IPC', async () => {
  mocks.mainError = new Error('ACCOUNT_LICENSE_REQUIRED')
  const screen = await render(<Harness startup={false} />)
  await screen.getByRole('button', { name: 'main-admission-rejected' }).click()
  expect(mocks.handled).toHaveBeenCalledWith(true)
  expect(mocks.openManagement).toHaveBeenCalledOnce()
  expect(mocks.startTurn).not.toHaveBeenCalled()
})

it('requests login rather than license management while signed out, including a stale Main denial', async () => {
  mocks.state = { ...mocks.state, status: 'allowed', reason: 'active' }
  mocks.mainError = { code: 'ACCOUNT_LICENSE_REQUIRED' }
  const screen = await render(<Harness authStatus="signedOut" startup={false} />)
  await expect.element(screen.getByTestId('can-start')).toHaveTextContent('false')
  await screen.getByRole('button', { name: 'start-new-turn' }).click()
  expect(mocks.requestLogin).toHaveBeenCalledOnce()
  await screen.getByRole('button', { name: 'main-admission-rejected' }).click()
  expect(mocks.requestLogin).toHaveBeenCalledTimes(2)
  expect(mocks.openManagement).not.toHaveBeenCalled()
  expect(mocks.startTurn).not.toHaveBeenCalled()
  await expect.element(screen.getByRole('dialog')).not.toBeInTheDocument()
})

it.each(['allowed', 'checking', 'unavailable', 'denied', 'signedOut'] as const)(
  'shows no fixed expiry only for an allowed license with a null expiration (status=%s)',
  async (status) => {
    const screen = await render(
      <LicenseContext.Provider
        value={{
          state: { ...mocks.state, status, expiresAt: null },
          canStartTurn: () => status === 'allowed',
          requestAccess: vi.fn(),
          refresh: mocks.refresh
        }}
      >
        <LicenseStatus />
      </LicenseContext.Provider>
    )
    if (status === 'allowed') {
      await expect.element(screen.getByText('license.noExpiry')).toBeVisible()
      await expect.element(screen.getByText('license.validity')).toBeVisible()
    } else {
      expect(screen.container.textContent).not.toContain('license.noExpiry')
    }
  }
)

it('shows the actual expiry for time-limited licenses instead of no fixed expiry', async () => {
  const expiresAt = '2027-09-13T08:00:00Z'
  const screen = await render(
    <LicenseContext.Provider
      value={{
        state: { ...mocks.state, status: 'allowed', expiresAt },
        canStartTurn: () => true,
        requestAccess: vi.fn(),
        refresh: mocks.refresh
      }}
    >
      <LicenseStatus />
    </LicenseContext.Provider>
  )
  expect(screen.container.querySelector('time')?.getAttribute('datetime')).toBe(expiresAt)
  expect(screen.container.textContent).not.toContain('license.noExpiry')
})
