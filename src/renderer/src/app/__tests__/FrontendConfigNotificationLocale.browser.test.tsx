import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'

const host = vi.hoisted(() => ({
  setLocale: vi.fn<(language: string) => Promise<void>>(),
  setNativeThemeSource: vi.fn<() => Promise<void>>()
}))

vi.mock('../../host/hostClient', () => ({
  hostClient: {
    app: { setNativeThemeSource: host.setNativeThemeSource },
    notifications: { setLocale: host.setLocale }
  }
}))

import { FrontendConfigProvider, useFrontendConfig } from '../../config/FrontendConfigProvider'
import { FRONTEND_CONFIG_STORAGE_KEY } from '../../config/frontendConfig'

function LanguageProbe() {
  const { language, setLanguage } = useFrontendConfig()
  return (
    <div>
      <output aria-label="current-language">{language}</output>
      <button type="button" onClick={() => setLanguage('ja-JP')}>
        use-japanese
      </button>
    </div>
  )
}

describe('FrontendConfig notification locale mirror', () => {
  beforeEach(() => {
    window.localStorage.clear()
    host.setLocale.mockReset().mockResolvedValue(undefined)
    host.setNativeThemeSource.mockReset().mockResolvedValue(undefined)
  })

  it('syncs the persisted application language on mount and every language change', async () => {
    window.localStorage.setItem(FRONTEND_CONFIG_STORAGE_KEY, JSON.stringify({ language: 'en-GB' }))
    const screen = await render(
      <FrontendConfigProvider>
        <LanguageProbe />
      </FrontendConfigProvider>
    )

    await expect.poll(() => host.setLocale.mock.calls.length).toBe(1)
    expect(host.setLocale).toHaveBeenLastCalledWith('en-GB')
    await screen.getByRole('button', { name: 'use-japanese' }).click()
    await expect
      .element(screen.getByRole('status', { name: 'current-language' }))
      .toHaveTextContent('ja-JP')
    await expect.poll(() => host.setLocale.mock.calls.length).toBe(2)
    expect(host.setLocale).toHaveBeenLastCalledWith('ja-JP')
    await screen.unmount()
  })

  it('keeps the Renderer language authoritative when the cold-start mirror cannot persist', async () => {
    host.setLocale.mockRejectedValue(new Error('disk unavailable'))
    const screen = await render(
      <FrontendConfigProvider>
        <LanguageProbe />
      </FrontendConfigProvider>
    )

    await screen.getByRole('button', { name: 'use-japanese' }).click()
    await expect
      .element(screen.getByRole('status', { name: 'current-language' }))
      .toHaveTextContent('ja-JP')
    await expect.poll(() => host.setLocale.mock.calls.length).toBe(2)
    await screen.unmount()
  })
})
