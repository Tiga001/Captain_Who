import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { useEffect } from 'react'

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
  const { language, setLanguage, showCacheHitRate, setShowCacheHitRate } = useFrontendConfig()
  return (
    <div>
      <output aria-label="current-language">{language}</output>
      <button
        type="button"
        role="switch"
        aria-label="show-cache-hit-rate"
        aria-checked={showCacheHitRate}
        onClick={() => setShowCacheHitRate(!showCacheHitRate)}
      >
        cache-hit-rate
      </button>
      <button type="button" onClick={() => setLanguage('ja-JP')}>
        use-japanese
      </button>
    </div>
  )
}

function TranslationEffectProbe({ onLoad }: { onLoad: (label: string) => void }) {
  const { t } = useFrontendConfig()
  useEffect(() => {
    onLoad(t('chat.commands.new'))
  }, [onLoad, t])
  return <output aria-label="translated-label">{t('chat.commands.new')}</output>
}

describe('FrontendConfig display preferences and notification locale mirror', () => {
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

  it('defaults cache hit rates off for existing configuration without changing language or theme', async () => {
    window.localStorage.setItem(
      FRONTEND_CONFIG_STORAGE_KEY,
      JSON.stringify({ language: 'en-GB', colorSchemePreference: 'dark' })
    )
    const screen = await render(
      <FrontendConfigProvider>
        <LanguageProbe />
      </FrontendConfigProvider>
    )

    await expect.element(screen.getByRole('switch')).not.toBeChecked()
    await expect
      .element(screen.getByRole('status', { name: 'current-language' }))
      .toHaveTextContent('en-GB')
    expect(document.documentElement.dataset.colorSchemePreference).toBe('dark')
    expect(JSON.parse(window.localStorage.getItem(FRONTEND_CONFIG_STORAGE_KEY)!)).toMatchObject({
      language: 'en-GB',
      colorSchemePreference: 'dark',
      showCacheHitRate: false
    })
    await screen.unmount()
  })

  it('persists both cache display modes across remounts without changing unrelated preferences', async () => {
    const renderProvider = () =>
      render(
        <FrontendConfigProvider>
          <LanguageProbe />
        </FrontendConfigProvider>
      )
    let screen = await renderProvider()
    await expect.element(screen.getByRole('switch')).not.toBeChecked()
    const initial = JSON.parse(window.localStorage.getItem(FRONTEND_CONFIG_STORAGE_KEY)!)

    for (const showCacheHitRate of [true, false]) {
      await screen.getByRole('switch').click()
      await expect
        .element(screen.getByRole('switch'))
        .toHaveAttribute('aria-checked', String(showCacheHitRate))
      expect(JSON.parse(window.localStorage.getItem(FRONTEND_CONFIG_STORAGE_KEY)!)).toEqual({
        ...initial,
        showCacheHitRate
      })
      await screen.unmount()
      screen = await renderProvider()
      await expect
        .element(screen.getByRole('switch'))
        .toHaveAttribute('aria-checked', String(showCacheHitRate))
    }
    await screen.unmount()
  })

  it('does not reload translation-dependent consumers when only the cache display mode changes', async () => {
    window.localStorage.setItem(FRONTEND_CONFIG_STORAGE_KEY, JSON.stringify({ language: 'en-GB' }))
    const onLoad = vi.fn()
    const screen = await render(
      <FrontendConfigProvider>
        <LanguageProbe />
        <TranslationEffectProbe onLoad={onLoad} />
      </FrontendConfigProvider>
    )

    await expect.poll(() => onLoad.mock.calls.length).toBe(1)
    expect(onLoad).toHaveBeenLastCalledWith('New chat')
    await screen.getByRole('switch').click()
    await expect.element(screen.getByRole('switch')).toBeChecked()
    expect(onLoad).toHaveBeenCalledTimes(1)
    await screen.getByRole('switch').click()
    await expect.element(screen.getByRole('switch')).not.toBeChecked()
    expect(onLoad).toHaveBeenCalledTimes(1)

    await screen.getByRole('button', { name: 'use-japanese' }).click()
    await expect.poll(() => onLoad.mock.calls.length).toBe(2)
    expect(onLoad).toHaveBeenLastCalledWith('新しいチャット')
    await expect
      .element(screen.getByRole('status', { name: 'translated-label' }))
      .toHaveTextContent('新しいチャット')
    await screen.unmount()
  })

  it.each([null, 'true', 1])(
    'does not enable cache hit rates for an invalid stored value %s',
    async (showCacheHitRate) => {
      window.localStorage.setItem(FRONTEND_CONFIG_STORAGE_KEY, JSON.stringify({ showCacheHitRate }))
      const screen = await render(
        <FrontendConfigProvider>
          <LanguageProbe />
        </FrontendConfigProvider>
      )

      await expect.element(screen.getByRole('switch')).not.toBeChecked()
      await screen.unmount()
    }
  )
})
