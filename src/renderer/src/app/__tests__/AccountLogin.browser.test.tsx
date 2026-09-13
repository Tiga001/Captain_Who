import { useEffect, useState } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { AuthState } from '@mycopilot/host-api'

const mocks = vi.hoisted(() => ({
  state: {
    revision: 0,
    status: 'signedOut',
    profile: null,
    error: null,
    remembered: false
  } as AuthState,
  listener: null as ((state: AuthState) => void) | null,
  login: vi.fn(),
  sendCode: vi.fn(),
  verifyCode: vi.fn(),
  logout: vi.fn(),
  refresh: vi.fn(),
  restore: vi.fn(),
  open: vi.fn(),
  ping: vi.fn()
}))
vi.mock('../../host/hostClient', () => ({
  hostClient: {
    core: { ping: mocks.ping },
    agent: {
      getLocalTokenUsage: async () => ({
        timezone: 'Asia/Shanghai',
        startedAt: Date.now(),
        days: [],
        totalTokens: '0',
        todayTokens: '0',
        peakDailyTokens: '0',
        unreportedRequestCount: 0
      })
    },
    auth: {
      getState: async () => mocks.state,
      onStateChanged: (listener: (state: AuthState) => void) => {
        mocks.listener = listener
        return () => {
          mocks.listener = null
        }
      },
      login: mocks.login,
      sendEmailCode: mocks.sendCode,
      verifyEmailCode: mocks.verifyCode,
      logout: mocks.logout,
      refreshProfile: mocks.refresh,
      restoreSession: mocks.restore,
      openWebsite: mocks.open
    }
  }
}))
vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'zh-CN',
    resolvedColorScheme: 'light',
    t: (key: string) => key
  })
}))

import { AccountAuthProvider } from '../../features/auth/AccountAuthProvider'
import { useAccountAuth } from '../../features/auth/AccountAuthContext'
import { AppStartupProvider } from '../../features/startup/AppStartupProvider'
import { AppStartupGate } from '../../features/startup/AppStartupGate'
import { useAppStartupStage } from '../../features/startup/AppStartupContext'
import type { AppStartupStageId } from '../../features/startup/appStartupStages'
import { ProfileSettingsPage } from '../../features/settings/pages/ProfileSettingsPage'
import { AccountAvatar } from '../../features/auth/AccountAvatar'
import '../../styles/global.css'

const cloudProfile = {
  userId: 'user-1',
  displayName: 'Cloud Captain',
  email: 'captain@example.com',
  avatarDataUrl: null,
  occupation: '',
  organization: ''
}
function emit(patch: Partial<AuthState>): void {
  mocks.state = { ...mocks.state, ...patch, revision: mocks.state.revision + 1 }
  mocks.listener?.(mocks.state)
}
function ReadyStage({ id }: { id: AppStartupStageId }) {
  const stage = useAppStartupStage(id)
  useEffect(() => {
    stage.markReady()
  }, [stage])
  return null
}
function Workspace() {
  const auth = useAccountAuth()!
  const [ticks, setTicks] = useState(0)
  useEffect(() => {
    const timer = setInterval(() => setTicks((n) => n + 1), 30)
    return () => clearInterval(timer)
  }, [])
  return (
    <div data-testid="workspace">
      <output data-testid="ticks">{ticks}</output>
      <output data-testid="can-send">{String(auth.canStartTurn())}</output>
      <button onClick={() => void auth.logout()}>sign-out</button>
      <button onClick={auth.requestLogin}>open-login</button>
      <ProfileSettingsPage />
    </div>
  )
}
function Harness() {
  return (
    <AccountAuthProvider>
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
            <ReadyStage key={id} id={id} />
          ))}
        </AppStartupGate>
      </AppStartupProvider>
    </AccountAuthProvider>
  )
}
beforeEach(() => {
  vi.clearAllMocks()
  mocks.state = { revision: 0, status: 'signedOut', profile: null, error: null, remembered: false }
  mocks.listener = null
  mocks.ping.mockResolvedValue({ ok: true })
  mocks.refresh.mockResolvedValue({ ok: true })
  mocks.sendCode.mockResolvedValue({ ok: true })
  mocks.restore.mockResolvedValue({ ok: true })
  mocks.open.mockResolvedValue(undefined)
  mocks.login.mockImplementation(async () => {
    emit({ status: 'signedIn', profile: cloudProfile, error: null, remembered: true })
    return { ok: true }
  })
  mocks.verifyCode.mockImplementation(async () => {
    emit({ status: 'signedIn', profile: cloudProfile, error: null, remembered: true })
    return { ok: true }
  })
  mocks.logout.mockImplementation(async () => {
    emit({ status: 'signedOut', profile: null, error: null })
    return { ok: true }
  })
})

describe('startup account login and reusable overlay', () => {
  it('keeps signed-out profile settings read-only without a login entry or explanatory copy', async () => {
    const screen = await render(
      <AccountAuthProvider>
        <ProfileSettingsPage />
      </AccountAuthProvider>
    )
    await expect.element(screen.getByRole('heading', { name: 'auth.signedOut' })).toBeVisible()
    expect(
      screen.container.querySelector('[aria-labelledby="profile-account-heading"] button')
    ).toBeNull()
    expect(screen.container.querySelector('.profile-settings-avatar-actions')).toBeNull()
    expect(screen.container.textContent).not.toContain('auth.loginToSend')
    expect(screen.container.textContent).not.toContain('auth.localData')
    expect(screen.container.querySelectorAll('.settings-list-row')).toHaveLength(2)
  })

  it('retains cloud profile editing and refresh without adding a settings login entry', async () => {
    mocks.state = { ...mocks.state, status: 'signedIn', profile: cloudProfile, remembered: true }
    const screen = await render(
      <AccountAuthProvider>
        <ProfileSettingsPage />
      </AccountAuthProvider>
    )
    await expect
      .element(screen.getByRole('heading', { name: cloudProfile.displayName }))
      .toBeVisible()
    await expect.element(screen.getByText(cloudProfile.email).first()).toBeVisible()
    expect(screen.container.textContent).not.toContain('auth.loginToSend')
    expect(screen.container.textContent).not.toContain('auth.localData')
    expect(
      screen.container.querySelectorAll('[aria-labelledby="profile-account-heading"] button')
    ).toHaveLength(2)
    await screen.getByRole('button', { name: 'auth.editProfile' }).click()
    expect(mocks.open).toHaveBeenCalledWith('profile')
    mocks.refresh.mockClear()
    await screen.getByRole('button', { name: 'auth.refresh' }).click()
    expect(mocks.refresh).toHaveBeenCalledTimes(1)
  })

  it('requires both core readiness and login, then logout leaves the mounted workspace running', async () => {
    let coreReady!: (value: unknown) => void
    mocks.ping.mockReturnValue(
      new Promise((resolve) => {
        coreReady = resolve
      })
    )
    const screen = await render(<Harness />)
    const interactive = () =>
      screen.container.querySelector('.app-startup-root')?.getAttribute('data-interactive')
    expect(screen.container.textContent).not.toContain('auth.subtitle')
    expect(screen.container.textContent).not.toContain('auth.localData')
    await expect.element(screen.getByRole('button', { name: 'auth.passwordMode' })).toBeVisible()
    await expect.element(screen.getByRole('button', { name: 'auth.codeMode' })).toBeVisible()
    await expect.element(screen.getByRole('button', { name: 'auth.register' })).toBeVisible()
    await expect.element(screen.getByRole('button', { name: 'auth.reset' })).toBeVisible()
    await screen
      .getByRole('textbox', { name: 'auth.email', exact: true })
      .fill('CAPTAIN@example.com')
    await screen.getByLabelText('auth.password', { exact: true }).fill('local-test-password')
    await screen.getByRole('button', { name: 'auth.login', exact: true }).click()
    expect(mocks.login).toHaveBeenCalledWith({
      email: 'captain@example.com',
      password: 'local-test-password'
    })
    expect(interactive()).toBe('false')
    coreReady({ ok: true })
    await expect.poll(interactive).toBe('true')
    await expect.element(screen.getByText('captain@example.com').first()).toBeVisible()
    const before = Number(screen.container.querySelector('[data-testid="ticks"]')?.textContent)
    await screen.getByRole('button', { name: 'sign-out', exact: true }).click()
    expect(interactive()).toBe('true')
    await expect.element(screen.getByTestId('can-send')).toHaveTextContent('false')
    await expect
      .poll(() => Number(screen.container.querySelector('[data-testid="ticks"]')?.textContent))
      .toBeGreaterThan(before)
    expect(mocks.ping).toHaveBeenCalledTimes(1)
    await screen.getByRole('button', { name: 'open-login' }).click()
    await expect.element(screen.getByRole('dialog')).toBeVisible()
    await screen.getByRole('button', { name: 'auth.cancel' }).click()
    await expect.poll(interactive).toBe('true')
    expect(mocks.ping).toHaveBeenCalledTimes(1)
  })
  it('sends and verifies email login codes without exposing the SDK challenge to the page', async () => {
    const screen = await render(<Harness />)
    await screen.getByRole('button', { name: 'auth.codeMode' }).click()
    expect(screen.container.textContent).not.toContain('auth.subtitle')
    expect(screen.container.textContent).not.toContain('auth.localData')
    await expect.element(screen.getByRole('button', { name: 'auth.passwordMode' })).toBeVisible()
    await expect.element(screen.getByRole('button', { name: 'auth.register' })).toBeVisible()
    await expect.element(screen.getByRole('button', { name: 'auth.reset' })).toBeVisible()
    await screen
      .getByRole('textbox', { name: 'auth.email', exact: true })
      .fill('captain@example.com')
    await screen.getByRole('button', { name: 'auth.sendCode' }).click()
    expect(mocks.sendCode).toHaveBeenCalledWith('captain@example.com')
    await screen.getByRole('textbox', { name: 'auth.code', exact: true }).fill('123456')
    await screen.getByRole('button', { name: 'auth.login', exact: true }).click()
    expect(mocks.verifyCode).toHaveBeenCalledWith({ email: 'captain@example.com', code: '123456' })
    await expect
      .poll(() =>
        screen.container.querySelector('.app-startup-root')?.getAttribute('data-interactive')
      )
      .toBe('true')
    expect(mocks.login).not.toHaveBeenCalled()
  })
  it('retains the overlay on validation errors and offers a retry', async () => {
    mocks.state = { ...mocks.state, status: 'error', error: 'network' }
    const screen = await render(<Harness />)
    await expect.element(screen.getByRole('alert')).toHaveTextContent('auth.error.network')
    await screen.getByRole('button', { name: 'auth.retry' }).click()
    expect(mocks.restore).toHaveBeenCalledTimes(1)
    expect(
      screen.container.querySelector('.app-startup-root')?.getAttribute('data-interactive')
    ).toBe('false')
  })
  it('uses the built-in boat for null or broken avatars', async () => {
    const screen = await render(<AccountAvatar />)
    const image = screen.container.querySelector('img')!
    const boat = image.src
    expect(boat).toContain('brand-mark-light')
    await screen.rerender(<AccountAvatar src="data:image/png;base64,bm90LWFuLWltYWdl" />)
    await expect.poll(() => image.src).toBe(boat)
  })
})
