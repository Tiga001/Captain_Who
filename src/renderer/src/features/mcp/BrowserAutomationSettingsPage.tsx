import { useCallback, useEffect, useRef, useState } from 'react'
import type { BrowserDownloadSettingsView } from '@mycopilot/protocol'

import { useToast } from '../../components/toast/ToastContext'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { SettingsBreadcrumbs } from '../settings/components/SettingsBreadcrumbs'
import { BrowserDownloadHistoryPage } from './BrowserDownloadHistoryPage'
import {
  chooseBrowserDownloadDirectory,
  getBrowserDownloadSettings,
  resetBrowserDownloadDirectory,
  setBrowserDownloadAskWhereToSave
} from './browserDownloadClient'
import { toSafeMcpDisplayText } from './mcpSafeDisplay'

interface BrowserAutomationSettingsPageProps {
  onBack: () => void
  onNavigateSettingsRoot: () => void
}

type BrowserAutomationView = 'settings' | 'history'

export function BrowserAutomationSettingsPage({
  onBack,
  onNavigateSettingsRoot
}: BrowserAutomationSettingsPageProps) {
  const { t } = useFrontendConfig()
  const { showToast } = useToast()
  const [view, setView] = useState<BrowserAutomationView>('settings')
  const [settings, setSettings] = useState<BrowserDownloadSettingsView | null>(null)
  const [mutating, setMutating] = useState(false)
  const initialLoadStarted = useRef(false)

  const showFailure = useCallback(
    (error: unknown) => {
      showToast(
        error instanceof Error
          ? toSafeMcpDisplayText(error.message, 256)
          : t('mcp.browserDownloads.error'),
        { durationMs: 3200 }
      )
    },
    [showToast, t]
  )

  useEffect(() => {
    if (initialLoadStarted.current) return
    initialLoadStarted.current = true
    void getBrowserDownloadSettings().then(setSettings).catch(showFailure)
  }, [showFailure])

  const mutateSettings = async (
    operation: () => Promise<BrowserDownloadSettingsView | null>
  ): Promise<void> => {
    if (mutating) return
    setMutating(true)
    try {
      const next = await operation()
      if (next) setSettings(next)
    } catch (error) {
      showFailure(error)
    } finally {
      setMutating(false)
    }
  }

  if (view === 'history') {
    return (
      <BrowserDownloadHistoryPage
        onBack={() => setView('settings')}
        onNavigateMcp={onBack}
        onNavigateSettingsRoot={onNavigateSettingsRoot}
      />
    )
  }

  return (
    <article className="settings-list-page mcp-settings-page browser-download-settings">
      <SettingsBreadcrumbs
        ariaLabel={t('settings.breadcrumb.label')}
        items={[
          {
            id: 'settings',
            label: t('settings.breadcrumb.root'),
            onSelect: onNavigateSettingsRoot
          },
          { id: 'mcp', label: t('settings.nav.mcp'), onSelect: onBack },
          { id: 'browser-automation', label: t('mcp.builtin.browserAutomation.name') }
        ]}
      />

      <header className="browser-download-settings__header">
        <h1>{t('mcp.builtin.browserAutomation.name')}</h1>
        <p className="settings-list-page__description">{t('mcp.browserDownloads.description')}</p>
      </header>

      <section className="mcp-settings-section" aria-labelledby="browser-download-section">
        <h2 id="browser-download-section">{t('mcp.browserDownloads.section')}</h2>
        <div className="browser-download-preferences">
          <div className="browser-download-preference-row">
            <div className="browser-download-preference-row__copy">
              <strong>{t('mcp.browserDownloads.location')}</strong>
              <p
                className={
                  settings?.locationMode === 'custom'
                    ? 'browser-download-preference-row__location browser-download-preference-row__location--custom'
                    : 'browser-download-preference-row__location'
                }
                title={settings?.locationMode === 'custom' ? settings.displayPath : undefined}
              >
                {settings ? (
                  settings.locationMode === 'custom' ? (
                    <bdi dir="ltr">{settings.displayPath}</bdi>
                  ) : (
                    t('mcp.browserDownloads.systemLocation')
                  )
                ) : (
                  t('mcp.browserDownloads.loading')
                )}
              </p>
            </div>
            <div className="browser-download-preference-row__actions">
              {settings?.locationMode === 'custom' && (
                <button
                  className="mcp-secondary-button"
                  disabled={mutating}
                  onClick={() => void mutateSettings(resetBrowserDownloadDirectory)}
                  type="button"
                >
                  {t('mcp.browserDownloads.useSystemLocation')}
                </button>
              )}
              <button
                className="mcp-secondary-button"
                disabled={mutating}
                onClick={() => void mutateSettings(chooseBrowserDownloadDirectory)}
                type="button"
              >
                {t('mcp.browserDownloads.changeLocation')}
              </button>
            </div>
          </div>

          <div className="browser-download-preference-row browser-download-preference-row--compact">
            <div className="browser-download-preference-row__copy">
              <strong>{t('mcp.browserDownloads.askWhereToSave')}</strong>
            </div>
            <button
              aria-checked={settings?.askWhereToSave ?? false}
              aria-label={t('mcp.browserDownloads.askWhereToSave')}
              className="settings-switch"
              data-state={settings?.askWhereToSave ? 'on' : 'off'}
              disabled={mutating || !settings}
              onClick={() =>
                void mutateSettings(() =>
                  setBrowserDownloadAskWhereToSave(!settings?.askWhereToSave)
                )
              }
              role="switch"
              type="button"
            >
              <span aria-hidden="true" className="settings-switch__thumb" />
            </button>
          </div>

          <div className="browser-download-preference-row browser-download-preference-row--compact">
            <div className="browser-download-preference-row__copy">
              <strong>{t('mcp.browserDownloads.history')}</strong>
            </div>
            <button
              className="mcp-secondary-button"
              onClick={() => setView('history')}
              type="button"
            >
              {t('mcp.browserDownloads.manage')}
            </button>
          </div>
        </div>
      </section>
    </article>
  )
}
