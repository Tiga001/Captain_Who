import {
  renderSettingsNodes,
  settingLabel,
  settingDescription
} from '../settings/settingsDefinition'
import { useSettingsPageNavigation } from '../settings/settingsSearchNavigation'
import {
  browserSettings,
  browserAutomationSettings,
  browserGeneralSettings,
  browserDownloadSettings
} from './BrowserAutomationSettings.definition'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type {
  BrowserDownloadSettingsView,
  BrowserLinkOpenTarget,
  BrowserPreferencesView
} from '@mycopilot/protocol'

import { useToast } from '../../components/toast/ToastContext'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { ClearBrowsingDataDialog } from '../browser/ClearBrowsingDataDialog'
import { getBrowserPreferences, updateBrowserPreferences } from '../browser/browserDataClient'
import { SettingsBreadcrumbs } from '../settings/components/SettingsBreadcrumbs'
import { SettingsSelect } from '../settings/components/SettingsSelect'
import type { SettingsSelectOption } from '../settings/components/SettingsSelect'
import { BrowserDownloadHistoryPage } from './BrowserDownloadHistoryPage'
import { BrowserHistoryPage } from './BrowserHistoryPage'
import { useBuiltinMcpCapabilities } from './useBuiltinMcpCapabilities'
import {
  chooseBrowserDownloadDirectory,
  getBrowserDownloadSettings,
  resetBrowserDownloadDirectory,
  setBrowserDownloadAskWhereToSave
} from './browserDownloadClient'
import { toSafeMcpDisplayText } from './mcpSafeDisplay'

interface BrowserAutomationSettingsPageProps {
  initialView?: BrowserAutomationView
  onCloseSettings?: () => void
  onNavigateSettingsRoot: () => void
}

export type BrowserAutomationView = 'settings' | 'downloadHistory' | 'history'

export function BrowserAutomationSettingsPage({
  initialView = 'settings',
  onCloseSettings,
  onNavigateSettingsRoot
}: BrowserAutomationSettingsPageProps) {
  const { t } = useFrontendConfig()
  const { showToast } = useToast()
  const [view, setView] = useState<BrowserAutomationView>(initialView)
  const [downloadSettings, setDownloadSettings] = useState<BrowserDownloadSettingsView | null>(null)
  const [browserPreferences, setBrowserPreferences] = useState<BrowserPreferencesView | null>(null)
  const [downloadMutating, setDownloadMutating] = useState(false)
  const [browserMutating, setBrowserMutating] = useState(false)
  const [clearDialogOpen, setClearDialogOpen] = useState(false)
  const initialLoadStarted = useRef(false)
  const {
    pendingCapabilities: pendingBuiltinCapabilities,
    refresh: refreshBuiltinCapabilities,
    setAllowed: setBuiltinAllowed,
    state: builtinState
  } = useBuiltinMcpCapabilities()
  const browserAutomationCapability = builtinState.output?.capabilities.find(
    (capability) => capability.capabilityId === 'browser_automation'
  )
  const browserAutomationPending = pendingBuiltinCapabilities.has('browser_automation')
  const browserAutomationName = settingLabel(browserAutomationSettings[0], t)
  const browserAutomationError = builtinState.errorMessage
    ? toSafeMcpDisplayText(builtinState.errorMessage, 256)
    : builtinState.status === 'error'
      ? t('mcp.builtin.loadFailed')
      : null

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

  useEffect(() => setView(initialView), [initialView])

  useSettingsPageNavigation('browser', (target) => {
    if (target.view === 'clearData') {
      setView('settings')
      setClearDialogOpen(true)
      return
    }
    if (
      target.view === 'history' ||
      target.view === 'downloadHistory' ||
      target.view === 'settings'
    ) {
      setView(target.view)
    }
  })

  useEffect(() => {
    if (initialLoadStarted.current) return
    initialLoadStarted.current = true
    void Promise.all([getBrowserDownloadSettings(), getBrowserPreferences()])
      .then(([downloads, preferences]) => {
        setDownloadSettings(downloads)
        setBrowserPreferences(preferences)
      })
      .catch(showFailure)
  }, [showFailure])

  const linkTargetOptions = useMemo<Array<SettingsSelectOption<BrowserLinkOpenTarget>>>(
    () => [
      { value: 'system', label: t('mcp.browserData.systemBrowser') },
      { value: 'builtin', label: t('mcp.browserData.builtinBrowser') }
    ],
    [t]
  )

  const mutateDownloadSettings = async (
    operation: () => Promise<BrowserDownloadSettingsView | null>
  ): Promise<void> => {
    if (downloadMutating) return
    setDownloadMutating(true)
    try {
      const next = await operation()
      if (next) setDownloadSettings(next)
    } catch (error) {
      showFailure(error)
    } finally {
      setDownloadMutating(false)
    }
  }

  const setLinkTarget = async (target: BrowserLinkOpenTarget): Promise<void> => {
    if (browserMutating || target === browserPreferences?.linkOpenTarget) return
    setBrowserMutating(true)
    try {
      setBrowserPreferences(await updateBrowserPreferences(target))
    } catch (error) {
      showFailure(error)
    } finally {
      setBrowserMutating(false)
    }
  }

  const setBrowserAutomationAllowed = async (): Promise<void> => {
    if (!browserAutomationCapability || browserAutomationPending) return
    try {
      await setBuiltinAllowed(browserAutomationCapability, !browserAutomationCapability.userAllowed)
    } catch (error) {
      showFailure(error)
    }
  }

  if (view === 'downloadHistory') {
    return (
      <BrowserDownloadHistoryPage
        onBack={() => setView('settings')}
        onNavigateSettingsRoot={onNavigateSettingsRoot}
      />
    )
  }

  if (view === 'history') {
    return (
      <BrowserHistoryPage
        onBack={() => setView('settings')}
        onCloseSettings={onCloseSettings}
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
          { id: 'browser', label: t('settings.nav.browser') }
        ]}
      />

      <header className="browser-download-settings__header">
        <h1>{t('settings.nav.browser')}</h1>
        <p className="settings-list-page__description">{t('mcp.browserDownloads.description')}</p>
      </header>

      {renderSettingsNodes(browserSettings, (node) => {
        switch (node.id) {
          case 'browser-automation':
            return (
              <section aria-label={browserAutomationName} className="browser-automation-preference">
                <div className="browser-download-preferences">
                  <div
                    aria-busy={browserAutomationPending || undefined}
                    className="browser-download-preference-row browser-download-preference-row--compact"
                  >
                    <div className="browser-download-preference-row__copy">
                      <strong>{browserAutomationName}</strong>
                      <p data-error={browserAutomationError ? 'true' : undefined}>
                        {browserAutomationError ??
                          (builtinState.status === 'loading'
                            ? t('mcp.builtin.loading')
                            : settingDescription(node, t))}
                      </p>
                    </div>
                    <div className="browser-automation-preference__actions">
                      {browserAutomationError && (
                        <button
                          className="mcp-secondary-button"
                          onClick={() => void refreshBuiltinCapabilities(true)}
                          type="button"
                        >
                          {t('mcp.actions.retry')}
                        </button>
                      )}
                      <button
                        aria-checked={browserAutomationCapability?.userAllowed ?? false}
                        aria-label={t('mcp.builtin.toggleNamed').replaceAll(
                          '{name}',
                          browserAutomationName
                        )}
                        className="settings-switch"
                        data-state={browserAutomationCapability?.userAllowed ? 'on' : 'off'}
                        disabled={!browserAutomationCapability || browserAutomationPending}
                        onClick={() => void setBrowserAutomationAllowed()}
                        role="switch"
                        type="button"
                      >
                        <span aria-hidden="true" className="settings-switch__thumb" />
                      </button>
                    </div>
                  </div>
                </div>
              </section>
            )
          case 'browser-general':
            return (
              <section className="mcp-settings-section" aria-labelledby="browser-general-section">
                <h2 id="browser-general-section">{settingLabel(node, t)}</h2>
                <div className="browser-download-preferences">
                  {renderSettingsNodes(browserGeneralSettings, (item) => {
                    switch (item.id) {
                      case 'browser-link-target':
                        return (
                          <div className="browser-download-preference-row">
                            <div className="browser-download-preference-row__copy">
                              <strong>{settingLabel(item, t)}</strong>
                              <p>{settingDescription(item, t)}</p>
                            </div>
                            <SettingsSelect
                              ariaLabel={settingLabel(item, t)}
                              className="browser-link-target-select"
                              disabled={browserMutating || !browserPreferences}
                              onChange={(value) => void setLinkTarget(value)}
                              options={linkTargetOptions}
                              value={browserPreferences?.linkOpenTarget ?? 'system'}
                            />
                          </div>
                        )
                      case 'browser-data':
                        return (
                          <div className="browser-download-preference-row">
                            <div className="browser-download-preference-row__copy">
                              <strong>{settingLabel(item, t)}</strong>
                              <p>{settingDescription(item, t)}</p>
                            </div>
                            <button
                              className="mcp-secondary-button"
                              onClick={() => setClearDialogOpen(true)}
                              type="button"
                            >
                              {t('browser.clearBrowsingData')}
                            </button>
                          </div>
                        )
                      case 'browser-history':
                        return (
                          <div className="browser-download-preference-row">
                            <div className="browser-download-preference-row__copy">
                              <strong>{settingLabel(item, t)}</strong>
                              <p>{settingDescription(item, t)}</p>
                            </div>
                            <button
                              className="mcp-secondary-button"
                              onClick={() => setView('history')}
                              type="button"
                            >
                              {t('mcp.browserDownloads.manage')}
                            </button>
                          </div>
                        )
                    }
                  })}
                </div>
              </section>
            )
          case 'browser-downloads':
            return (
              <section className="mcp-settings-section" aria-labelledby="browser-download-section">
                <h2 id="browser-download-section">{settingLabel(node, t)}</h2>
                <div className="browser-download-preferences">
                  {renderSettingsNodes(browserDownloadSettings, (item) => {
                    switch (item.id) {
                      case 'browser-download-location':
                        return (
                          <div className="browser-download-preference-row">
                            <div className="browser-download-preference-row__copy">
                              <strong>{settingLabel(item, t)}</strong>
                              <p
                                className={
                                  downloadSettings?.locationMode === 'custom'
                                    ? 'browser-download-preference-row__location browser-download-preference-row__location--custom'
                                    : 'browser-download-preference-row__location'
                                }
                                title={
                                  downloadSettings?.locationMode === 'custom'
                                    ? downloadSettings.displayPath
                                    : undefined
                                }
                              >
                                {downloadSettings ? (
                                  downloadSettings.locationMode === 'custom' ? (
                                    <bdi dir="ltr">{downloadSettings.displayPath}</bdi>
                                  ) : (
                                    t('mcp.browserDownloads.systemLocation')
                                  )
                                ) : (
                                  t('mcp.browserDownloads.loading')
                                )}
                              </p>
                            </div>
                            <div className="browser-download-preference-row__actions">
                              {downloadSettings?.locationMode === 'custom' && (
                                <button
                                  className="mcp-secondary-button"
                                  disabled={downloadMutating}
                                  onClick={() =>
                                    void mutateDownloadSettings(resetBrowserDownloadDirectory)
                                  }
                                  type="button"
                                >
                                  {t('mcp.browserDownloads.useSystemLocation')}
                                </button>
                              )}
                              <button
                                className="mcp-secondary-button"
                                disabled={downloadMutating}
                                onClick={() =>
                                  void mutateDownloadSettings(chooseBrowserDownloadDirectory)
                                }
                                type="button"
                              >
                                {t('mcp.browserDownloads.changeLocation')}
                              </button>
                            </div>
                          </div>
                        )
                      case 'browser-download-ask-where':
                        return (
                          <div className="browser-download-preference-row browser-download-preference-row--compact">
                            <div className="browser-download-preference-row__copy">
                              <strong>{settingLabel(item, t)}</strong>
                            </div>
                            <button
                              aria-checked={downloadSettings?.askWhereToSave ?? false}
                              aria-label={settingLabel(item, t)}
                              className="settings-switch"
                              data-state={downloadSettings?.askWhereToSave ? 'on' : 'off'}
                              disabled={downloadMutating || !downloadSettings}
                              onClick={() =>
                                void mutateDownloadSettings(() =>
                                  setBrowserDownloadAskWhereToSave(
                                    !downloadSettings?.askWhereToSave
                                  )
                                )
                              }
                              role="switch"
                              type="button"
                            >
                              <span aria-hidden="true" className="settings-switch__thumb" />
                            </button>
                          </div>
                        )
                      case 'browser-download-history':
                        return (
                          <div className="browser-download-preference-row browser-download-preference-row--compact">
                            <div className="browser-download-preference-row__copy">
                              <strong>{settingLabel(item, t)}</strong>
                            </div>
                            <button
                              className="mcp-secondary-button"
                              onClick={() => setView('downloadHistory')}
                              type="button"
                            >
                              {t('mcp.browserDownloads.manage')}
                            </button>
                          </div>
                        )
                    }
                  })}
                </div>
              </section>
            )
        }
      })}

      {clearDialogOpen && <ClearBrowsingDataDialog onClose={() => setClearDialogOpen(false)} />}
    </article>
  )
}
