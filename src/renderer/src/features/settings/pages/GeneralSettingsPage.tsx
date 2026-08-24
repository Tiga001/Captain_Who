import { useState } from 'react'
import type { FocusEvent } from 'react'
import type { AgentReadPermission, AgentWritePermission } from '@mycopilot/protocol'
import { Check, ChevronDown } from 'lucide-react'
import { featureFlags } from '../../../config/featureFlags'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { appLanguageOptions } from '../../../config/languageRegistry'
import { hasNotificationHostApi } from '../../notifications/notificationClient'
import {
  deriveOrdinaryNotificationMode,
  ordinaryNotificationPatchForMode,
  type OrdinaryNotificationMode
} from '../../notifications/ordinaryNotificationMode'
import { useNotificationSettings } from '../../notifications/useNotificationSettings'
import type { UiPreferencesSnapshot } from '../../storage/storageClient'
import './GeneralSettingsPage.css'

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

function NotificationSettingsSection() {
  const { t } = useFrontendConfig()
  const available = hasNotificationHostApi()
  const { settings, status, error, refresh, update, saving } = useNotificationSettings(available)
  const [isModeMenuOpen, setModeMenuOpen] = useState(false)
  const [isCustomModeSelected, setCustomModeSelected] = useState(false)

  if (!available) return null

  const updateSetting = async (patch: Parameters<typeof update>[0]) => {
    try {
      await update(patch)
      return true
    } catch {
      return false
    }
  }

  const modeOptions: Array<{
    description: Parameters<typeof t>[0]
    label: Parameters<typeof t>[0]
    value: OrdinaryNotificationMode
  }> = [
    {
      value: 'never',
      label: 'notification.ordinaryModeNever',
      description: 'notification.ordinaryModeNeverDescription'
    },
    {
      value: 'all',
      label: 'notification.ordinaryModeAll',
      description: 'notification.ordinaryModeAllDescription'
    },
    {
      value: 'necessary',
      label: 'notification.ordinaryModeNecessary',
      description: 'notification.ordinaryModeNecessaryDescription'
    },
    {
      value: 'custom',
      label: 'notification.ordinaryModeCustom',
      description: 'notification.ordinaryModeCustomDescription'
    }
  ]

  const derivedMode = settings ? deriveOrdinaryNotificationMode(settings) : 'never'
  const selectedMode = isCustomModeSelected ? 'custom' : derivedMode
  const selectedModeOption =
    modeOptions.find((option) => option.value === selectedMode) ?? modeOptions[0]

  const closeModeMenuOnBlur = (event: FocusEvent<HTMLSpanElement>) => {
    if (!event.currentTarget.contains(event.relatedTarget)) {
      setModeMenuOpen(false)
    }
  }

  const selectMode = (mode: OrdinaryNotificationMode) => {
    setModeMenuOpen(false)
    if (!settings || saving) return

    if (mode === 'custom') {
      setCustomModeSelected(true)
      if (!settings.enabled) {
        void updateSetting(ordinaryNotificationPatchForMode('never')).then((updated) => {
          if (!updated) setCustomModeSelected(false)
        })
      }
      return
    }

    if (mode === selectedMode && settings.enabled) return
    void updateSetting(ordinaryNotificationPatchForMode(mode)).then((updated) => {
      if (updated) setCustomModeSelected(false)
    })
  }

  const renderToggleRow = (
    key: keyof NonNullable<typeof settings>,
    title: Parameters<typeof t>[0],
    description: Parameters<typeof t>[0],
    disabled = false
  ) => {
    if (!settings || typeof settings[key] !== 'boolean') return null
    return (
      <div className="settings-list-row" key={key}>
        <span className="settings-list-row__text">
          <span className="settings-list-row__title">{t(title)}</span>
          <p className="settings-list-row__description">{t(description)}</p>
        </span>
        <SettingsToggle
          checked={settings[key] as boolean}
          disabled={saving || disabled}
          label={t(title)}
          onChange={(checked) =>
            void updateSetting({
              ...(settings.enabled ? { enabled: true } : ordinaryNotificationPatchForMode('never')),
              [key]: checked
            })
          }
        />
      </div>
    )
  }

  return (
    <section className="settings-list-section" aria-labelledby="notifications-section-heading">
      <h2 id="notifications-section-heading">{t('general.sectionNotifications')}</h2>
      <div
        className="settings-list general-settings-list general-notification-settings-list"
        aria-busy={status === 'loading'}
      >
        {settings ? (
          <>
            <div className="settings-list-row general-notification-mode-row">
              <span className="settings-list-row__text">
                <span className="settings-list-row__title" id="ordinary-notification-mode-heading">
                  {t('notification.ordinaryModeLabel')}
                </span>
                <p className="settings-list-row__description" aria-live="polite">
                  {t(selectedModeOption.description)}
                </p>
              </span>

              <span
                className="settings-list-row__control general-notification-mode-control"
                onBlur={closeModeMenuOnBlur}
                onKeyDown={(event) => {
                  if (event.key === 'Escape') {
                    setModeMenuOpen(false)
                    event.stopPropagation()
                  }
                }}
              >
                <button
                  aria-controls="ordinary-notification-mode-menu"
                  aria-expanded={isModeMenuOpen}
                  aria-haspopup="listbox"
                  aria-label={t('notification.ordinaryModeAria')}
                  className="general-notification-mode-button"
                  disabled={saving}
                  onClick={() => setModeMenuOpen((current) => !current)}
                  type="button"
                >
                  <span>{t(selectedModeOption.label)}</span>
                  <ChevronDown aria-hidden="true" />
                </button>

                {isModeMenuOpen && (
                  <div
                    aria-label={t('notification.ordinaryModeAria')}
                    className="general-notification-mode-menu"
                    id="ordinary-notification-mode-menu"
                    role="listbox"
                  >
                    {modeOptions.map((option) => {
                      const isSelected = option.value === selectedMode
                      return (
                        <button
                          aria-selected={isSelected}
                          className="general-notification-mode-option"
                          data-selected={isSelected || undefined}
                          key={option.value}
                          onClick={() => selectMode(option.value)}
                          onMouseDown={(event) => event.preventDefault()}
                          role="option"
                          type="button"
                        >
                          <span>{t(option.label)}</span>
                          {isSelected && <Check aria-hidden="true" />}
                        </button>
                      )
                    })}
                  </div>
                )}
              </span>
            </div>

            <div
              aria-hidden={selectedMode !== 'custom'}
              className="general-notification-drawer"
              data-open={selectedMode === 'custom' ? 'true' : 'false'}
              inert={selectedMode !== 'custom'}
            >
              <div
                aria-label={t('notification.ordinaryCustomAria')}
                className="general-notification-drawer__inner"
                role="group"
              >
                {renderToggleRow(
                  'humanCompletedEnabled',
                  'notification.settingCompleted',
                  'notification.settingCompletedDescription'
                )}
                {renderToggleRow(
                  'humanFailedEnabled',
                  'notification.settingFailed',
                  'notification.settingFailedDescription'
                )}
                {renderToggleRow(
                  'humanApprovalEnabled',
                  'notification.settingApproval',
                  'notification.settingApprovalDescription'
                )}
                {renderToggleRow(
                  'humanCancelledEnabled',
                  'notification.settingCancelled',
                  'notification.settingCancelledDescription'
                )}
              </div>
            </div>

            {renderToggleRow(
              'soundEnabled',
              'notification.settingSound',
              'notification.settingSoundDescription'
            )}
            {renderToggleRow(
              'showTaskContent',
              'notification.settingPreview',
              'notification.settingPreviewDescription'
            )}
            {error && (
              <div className="settings-list-row" role="alert">
                <span className="settings-list-row__text">
                  <span className="settings-list-row__title">
                    {t('notification.settingsUpdateFailed')}
                  </span>
                </span>
                <button
                  className="general-settings-retry"
                  onClick={() => void refresh()}
                  type="button"
                >
                  {t('notification.retry')}
                </button>
              </div>
            )}
          </>
        ) : (
          <div className="settings-list-row" role={status === 'error' ? 'alert' : 'status'}>
            <span className="settings-list-row__text">
              <span className="settings-list-row__title">
                {status === 'error'
                  ? t('notification.settingsLoadFailed')
                  : t('notification.loading')}
              </span>
            </span>
            {status === 'error' && (
              <button
                className="general-settings-retry"
                onClick={() => void refresh()}
                type="button"
              >
                {t('notification.retry')}
              </button>
            )}
          </div>
        )}
      </div>
    </section>
  )
}

export function GeneralSettingsPage({
  onUiPreferencesChange,
  uiPreferences
}: GeneralSettingsPageProps) {
  const { language, setLanguage, t } = useFrontendConfig()
  const [isLanguageMenuOpen, setLanguageMenuOpen] = useState(false)
  const selectedLanguage =
    appLanguageOptions.find((option) => option.value === language) ?? appLanguageOptions[0]
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
                  {appLanguageOptions.map((option) => {
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

          <div className="settings-list-row">
            <span className="settings-list-row__text">
              <span className="settings-list-row__title">
                {t('general.autoApproveBuiltinExecution')}
              </span>
              <p className="settings-list-row__description">
                {t('general.autoApproveBuiltinExecutionDescription')}
              </p>
            </span>
            <SettingsToggle
              checked={uiPreferences.customPermissions.builtinExecution === 'auto_approve'}
              label={t('general.autoApproveBuiltinExecution')}
              onChange={(checked) =>
                updateCustomPermissions({
                  builtinExecution: checked ? 'auto_approve' : 'require_approval'
                })
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

      <NotificationSettingsSection />
    </article>
  )
}
