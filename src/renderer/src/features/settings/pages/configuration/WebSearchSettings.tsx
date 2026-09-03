import { useState } from 'react'
import type { CredentialMutation, CredentialStatus } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import type { SearchMode } from './configurationTypes'
import { CredentialInput } from './CredentialInput'

interface WebSearchSettingsProps {
  searchMode: SearchMode
  tavilyApiKeyStatus: CredentialStatus
  onSearchModeChange: (value: SearchMode) => void
  onTavilyApiKeyCommit: (mutation: CredentialMutation) => Promise<void>
}

export function WebSearchSettings({
  searchMode,
  tavilyApiKeyStatus,
  onSearchModeChange,
  onTavilyApiKeyCommit
}: WebSearchSettingsProps) {
  const { t } = useFrontendConfig()
  const [isApiKeyRequiredDialogOpen, setApiKeyRequiredDialogOpen] = useState(false)
  const [tavilyApiKeyMutation, setTavilyApiKeyMutation] = useState<CredentialMutation>({
    type: 'keep'
  })
  const hasTavilyApiKey =
    tavilyApiKeyMutation.type === 'replace'
      ? tavilyApiKeyMutation.value.length > 0
      : tavilyApiKeyMutation.type === 'clear'
        ? false
        : tavilyApiKeyStatus === 'configured'
  const isSearchAllowed = searchMode !== 'disabled' && hasTavilyApiKey

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

          <div className="configuration-field settings-list-row">
            <span className="settings-list-row__text">
              <span className="settings-list-row__title">{t('configuration.tavilyApiKey')}</span>
            </span>
            <span className="settings-list-row__control">
              <CredentialInput
                ariaLabel={t('configuration.tavilyApiKey')}
                mutation={tavilyApiKeyMutation}
                onCommit={async (mutation) => {
                  await onTavilyApiKeyCommit(mutation)
                  setTavilyApiKeyMutation({ type: 'keep' })
                }}
                onMutationChange={setTavilyApiKeyMutation}
                placeholder={t('configuration.credential.placeholder')}
                status={tavilyApiKeyStatus}
              />
            </span>
          </div>
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
