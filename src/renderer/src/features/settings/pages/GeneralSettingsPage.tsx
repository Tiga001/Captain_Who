import { useState } from 'react'
import type { FocusEvent } from 'react'
import type { AgentReadPermission, AgentWritePermission } from '@mycopilot/protocol'
import { Check, ChevronDown } from 'lucide-react'
import { featureFlags } from '../../../config/featureFlags'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import type { AppLanguage } from '../../../config/frontendTranslations'
import type { UiPreferencesSnapshot } from '../../storage/storageClient'
import './GeneralSettingsPage.css'

const LANGUAGE_DISPLAY_OPTIONS: Array<{ value: AppLanguage; label: string }> = [
  { value: 'zh-CN', label: '中文（中国）' },
  { value: 'en-US', label: 'English (United States)' }
]

interface GeneralSettingsPageProps {
  onUiPreferencesChange: (patch: Partial<UiPreferencesSnapshot>) => void
  uiPreferences: UiPreferencesSnapshot
}

interface PermissionSegmentProps {
  ariaLabel: string
  disabled?: boolean
  onChange: (value: string) => void
  options: Array<{ label: string; value: string }>
  value: string
}

function PermissionSegment({
  ariaLabel,
  disabled = false,
  onChange,
  options,
  value
}: PermissionSegmentProps) {
  return (
    <span className="general-permission-segment" role="radiogroup" aria-label={ariaLabel}>
      {options.map((option) => (
        <button
          type="button"
          role="radio"
          aria-checked={option.value === value}
          data-selected={option.value === value || undefined}
          disabled={disabled}
          key={option.value}
          onClick={() => onChange(option.value)}
        >
          {option.label}
        </button>
      ))}
    </span>
  )
}

interface SettingsToggleProps {
  checked: boolean
  disabled?: boolean
  label: string
  onChange?: (checked: boolean) => void
}

function SettingsToggle({ checked, disabled = false, label, onChange }: SettingsToggleProps) {
  return (
    <button
      aria-checked={checked}
      aria-label={label}
      className="settings-switch"
      data-state={checked ? 'on' : 'off'}
      disabled={disabled}
      onClick={() => onChange?.(!checked)}
      role="switch"
      type="button"
    >
      <span className="settings-switch__thumb" aria-hidden="true" />
    </button>
  )
}

export function GeneralSettingsPage({
  onUiPreferencesChange,
  uiPreferences
}: GeneralSettingsPageProps) {
  const { language, setLanguage, t } = useFrontendConfig()
  const [isLanguageMenuOpen, setLanguageMenuOpen] = useState(false)
  const selectedLanguage =
    LANGUAGE_DISPLAY_OPTIONS.find((option) => option.value === language) ??
    LANGUAGE_DISPLAY_OPTIONS[0]
  const canAutoApproveFileEdits = uiPreferences.customPermissions.write !== 'denied'

  const closeLanguageMenuOnBlur = (event: FocusEvent<HTMLSpanElement>) => {
    if (!event.currentTarget.contains(event.relatedTarget)) {
      setLanguageMenuOpen(false)
    }
  }
  const updateCustomPermissions = (patch: Partial<UiPreferencesSnapshot['customPermissions']>) => {
    onUiPreferencesChange({
      customPermissions: {
        ...uiPreferences.customPermissions,
        ...patch
      }
    })
  }

  return (
    <article className="settings-list-page general-settings-page">
      <h1>{t('settings.page.general')}</h1>

      <section className="settings-list-section" aria-labelledby="general-section-heading">
        <h2 id="general-section-heading">{t('general.sectionGeneral')}</h2>
        <div className="settings-list general-settings-list">
          <div className="settings-list-row general-settings-language-row">
            <span className="settings-list-row__text">
              <span className="settings-list-row__title" id="language-setting-heading">
                {t('general.language')}
              </span>
            </span>

            <span
              className="settings-list-row__control general-language-control"
              onBlur={closeLanguageMenuOnBlur}
            >
              <span className="sr-only">{t('general.languageAria')}</span>
              <button
                className="general-language-button"
                type="button"
                aria-haspopup="listbox"
                aria-expanded={isLanguageMenuOpen}
                aria-label={t('general.languageAria')}
                onClick={() => setLanguageMenuOpen((current) => !current)}
              >
                <span>{selectedLanguage.label}</span>
                <ChevronDown aria-hidden="true" />
              </button>

              {isLanguageMenuOpen && (
                <div
                  className="general-language-menu"
                  role="listbox"
                  aria-label={t('general.languageAria')}
                >
                  {LANGUAGE_DISPLAY_OPTIONS.map((option) => {
                    const isSelected = option.value === language
                    return (
                      <button
                        className="general-language-option"
                        data-selected={isSelected || undefined}
                        type="button"
                        role="option"
                        aria-selected={isSelected}
                        key={option.value}
                        onMouseDown={(event) => event.preventDefault()}
                        onClick={() => {
                          setLanguage(option.value)
                          setLanguageMenuOpen(false)
                        }}
                      >
                        <span>{option.label}</span>
                        {isSelected && <Check aria-hidden="true" />}
                      </button>
                    )
                  })}
                </div>
              )}
            </span>
          </div>
        </div>
      </section>

      <section className="settings-list-section" aria-labelledby="permission-modes-heading">
        <h2 id="permission-modes-heading">{t('general.sectionPermissions')}</h2>

        <div className="settings-list general-permission-modes-list">
          <div className="settings-list-row">
            <span className="settings-list-row__text">
              <span className="settings-list-row__title">{t('chat.defaultPermission')}</span>
              <p className="settings-list-row__description">
                {t('general.defaultPermissionDescription')}
              </p>
            </span>
            <SettingsToggle checked disabled label={t('general.defaultPermissionLocked')} />
          </div>

          <div className="settings-list-row">
            <span className="settings-list-row__text">
              <span className="settings-list-row__title">{t('chat.fullPermission')}</span>
              <p className="settings-list-row__description">
                {t('general.fullPermissionDescription')}
              </p>
            </span>
            <SettingsToggle
              checked={uiPreferences.fullPermissionEnabled}
              label={t('chat.fullPermission')}
              onChange={(fullPermissionEnabled) => onUiPreferencesChange({ fullPermissionEnabled })}
            />
          </div>

          <div className="settings-list-row">
            <span className="settings-list-row__text">
              <span className="settings-list-row__title">{t('chat.customPermission')}</span>
              <p className="settings-list-row__description">
                {t('general.customPermissionDescription')}
              </p>
            </span>
            <SettingsToggle
              checked={uiPreferences.customPermissionEnabled}
              label={t('chat.customPermission')}
              onChange={(customPermissionEnabled) =>
                onUiPreferencesChange({ customPermissionEnabled })
              }
            />
          </div>
        </div>
      </section>

      <section className="settings-list-section" aria-labelledby="custom-permissions-heading">
        <h2 id="custom-permissions-heading">{t('general.customPermissions')}</h2>

        <div className="settings-list general-permissions-list">
          <div className="settings-list-row">
            <span className="settings-list-row__text">
              <span className="settings-list-row__title">{t('general.readPermission')}</span>
            </span>
            <span className="settings-list-row__control">
              <PermissionSegment
                ariaLabel={t('general.readPermission')}
                value={uiPreferences.customPermissions.read}
                options={[
                  { value: 'workspace_only', label: t('general.workspaceOnly') },
                  { value: 'all', label: t('general.allLocations') }
                ]}
                onChange={(value) =>
                  updateCustomPermissions({ read: value as AgentReadPermission })
                }
              />
            </span>
          </div>

          <div className="settings-list-row">
            <span className="settings-list-row__text">
              <span className="settings-list-row__title">{t('general.writePermission')}</span>
            </span>
            <span className="settings-list-row__control">
              <PermissionSegment
                ariaLabel={t('general.writePermission')}
                value={uiPreferences.customPermissions.write}
                options={[
                  { value: 'denied', label: t('general.writeDenied') },
                  { value: 'workspace_only', label: t('general.workspaceOnly') },
                  { value: 'all', label: t('general.allLocations') }
                ]}
                onChange={(value) =>
                  updateCustomPermissions({ write: value as AgentWritePermission })
                }
              />
            </span>
          </div>

          <div
            className="general-permission-drawer"
            data-open={canAutoApproveFileEdits ? 'true' : 'false'}
            aria-hidden={!canAutoApproveFileEdits}
          >
            <div className="general-permission-drawer__inner">
              <div className="settings-list-row">
                <span className="settings-list-row__text">
                  <span className="settings-list-row__title">
                    {t('general.autoApproveFileEdits')}
                  </span>
                  <p className="settings-list-row__description">
                    {t('general.autoApproveFileEditsDescription')}
                  </p>
                </span>
                <SettingsToggle
                  checked={uiPreferences.customPermissions.patch === 'auto_approve'}
                  disabled={!canAutoApproveFileEdits}
                  label={t('general.autoApproveFileEdits')}
                  onChange={(checked) =>
                    updateCustomPermissions({
                      patch: checked ? 'auto_approve' : 'require_approval'
                    })
                  }
                />
              </div>
            </div>
          </div>

          <div className="settings-list-row">
            <span className="settings-list-row__text">
              <span className="settings-list-row__title">{t('general.autoApproveCommands')}</span>
              <p className="settings-list-row__description">
                {t('general.autoApproveCommandsDescription')}
              </p>
            </span>
            <SettingsToggle
              checked={uiPreferences.customPermissions.command === 'auto_approve'}
              label={t('general.autoApproveCommands')}
              onChange={(checked) =>
                updateCustomPermissions({ command: checked ? 'auto_approve' : 'require_approval' })
              }
            />
          </div>
        </div>
      </section>

      {featureFlags.contextWindowIndicator && (
        <section className="settings-list-section" aria-labelledby="composer-section-heading">
          <h2 id="composer-section-heading">{t('general.sectionComposer')}</h2>
          <div className="settings-list general-settings-list">
            <div className="settings-list-row">
              <span className="settings-list-row__text">
                <span className="settings-list-row__title">
                  {t('general.showContextWindowUsage')}
                </span>
              </span>
              <SettingsToggle
                checked={uiPreferences.showContextWindowUsage}
                label={t('general.showContextWindowUsage')}
                onChange={(showContextWindowUsage) =>
                  onUiPreferencesChange({ showContextWindowUsage })
                }
              />
            </div>
          </div>
        </section>
      )}
    </article>
  )
}
