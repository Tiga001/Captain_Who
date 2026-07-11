import { useState } from 'react'
import type { ClipboardEvent } from 'react'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import type { SearchMode } from './configurationTypes'

interface WebSearchSettingsProps {
  searchMode: SearchMode
  tavilyApiKey: string
  onSearchModeChange: (value: SearchMode) => void
  onTavilyApiKeyChange: (value: string) => void
}

export function WebSearchSettings({
  searchMode,
  tavilyApiKey,
  onSearchModeChange,
  onTavilyApiKeyChange
}: WebSearchSettingsProps) {
  const { t } = useFrontendConfig()
  const [isApiKeyRequiredDialogOpen, setApiKeyRequiredDialogOpen] = useState(false)
  const hasTavilyApiKey = tavilyApiKey.trim().length > 0
  const isSearchAllowed = searchMode !== 'disabled' && hasTavilyApiKey
  const preventClipboard = (event: ClipboardEvent<HTMLInputElement>) => {
    event.preventDefault()
  }

  const toggleWebSearch = () => {
    if (isSearchAllowed) {
      onSearchModeChange('disabled')
      return
    }

    if (!hasTavilyApiKey) {
      setApiKeyRequiredDialogOpen(true)
      return
    }

    onSearchModeChange('auto')
  }

  const updateTavilyApiKey = (value: string) => {
    onTavilyApiKeyChange(value)
    if (searchMode !== 'disabled' && value.trim().length === 0) {
      onSearchModeChange('disabled')
    }
  }

  return (
    <section
      className="configuration-section configuration-section--search settings-list-page"
      aria-labelledby="web-search-heading"
    >
      <h1 id="web-search-heading">{t('configuration.webSearch')}</h1>

      <div className="settings-list-section">
        <div className="settings-list">
          <div className="settings-list-row">
            <div className="settings-list-row__text">
              <h2 className="settings-list-row__title">{t('configuration.webSearch')}</h2>
            </div>

            <button
              className="settings-switch"
              type="button"
              role="switch"
              data-state={isSearchAllowed ? 'on' : 'off'}
              aria-checked={isSearchAllowed}
              onClick={toggleWebSearch}
            >
              <span className="sr-only">
                {isSearchAllowed
                  ? t('configuration.webSearchAllowed')
                  : t('configuration.webSearchDisabled')}
              </span>
              <span className="settings-switch__thumb" aria-hidden="true" />
            </button>
          </div>

          <label className="configuration-field settings-list-row">
            <span className="settings-list-row__text">
              <span className="settings-list-row__title">{t('configuration.tavilyApiKey')}</span>
            </span>
            <span className="settings-list-row__control">
              <input
                className="settings-list-control"
                type="password"
                value={tavilyApiKey}
                onChange={(event) => updateTavilyApiKey(event.target.value)}
                onCopy={preventClipboard}
                onCut={preventClipboard}
              />
            </span>
          </label>
        </div>
      </div>

      {isApiKeyRequiredDialogOpen && (
        <div
          className="web-search-key-dialog"
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="web-search-key-dialog-title"
        >
          <div className="web-search-key-dialog__card">
            <h2 id="web-search-key-dialog-title">{t('configuration.tavilyApiKeyRequiredTitle')}</h2>
            <p>{t('configuration.tavilyApiKeyRequiredDescription')}</p>
            <div className="web-search-key-dialog__actions">
              <button
                className="primary-settings-button"
                type="button"
                onClick={() => setApiKeyRequiredDialogOpen(false)}
              >
                {t('configuration.acknowledge')}
              </button>
            </div>
          </div>
        </div>
      )}
    </section>
  )
}
