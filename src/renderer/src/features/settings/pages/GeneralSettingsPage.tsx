import {
  renderSettingsNodes,
  settingLabel,
  settingDescription,
  type SettingsNode
} from '../settingsDefinition'
import { useSettingsPageNavigation } from '../settingsSearchNavigation'
import {
  generalSettingsNodes,
  READ_PERMISSION_OPTIONS,
  WRITE_PERMISSION_OPTIONS,
  NOTIFICATION_MODE_OPTIONS
} from './GeneralSettingsPage.definition'
import { useState } from 'react'
import type { FocusEvent } from 'react'
import type { AgentReadPermission, AgentWritePermission } from '@mycopilot/protocol'
import { Check, ChevronDown } from 'lucide-react'
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

function NotificationSettingsSection({ definition }: { definition: SettingsNode }) {
  const { t } = useFrontendConfig()
  const available = hasNotificationHostApi()
  const { settings, status, error, refresh, update, saving } = useNotificationSettings(available)
  const [isModeMenuOpen, setModeMenuOpen] = useState(false)
  const [isCustomModeSelected, setCustomModeSelected] = useState(false)

  useSettingsPageNavigation('general', (target) => {
    if (target.view === 'notificationCustom') setCustomModeSelected(true)
  })

  if (!available) return null

  const updateSetting = async (patch: Parameters<typeof update>[0]) => {
    try {
      await update(patch)
      return true
    } catch {
      return false
    }
  }

  const modeOptions = NOTIFICATION_MODE_OPTIONS

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

  const renderToggleRow = (node: SettingsNode) => {
    const key = node.id.slice('notifications.'.length) as keyof NonNullable<typeof settings>
    if (!settings || typeof settings[key] !== 'boolean') return null
    return (
      <div className="settings-list-row" key={key} data-setting-id={node.id}>
        <span className="settings-list-row__text">
          <span className="settings-list-row__title">{settingLabel(node, t)}</span>
          <p className="settings-list-row__description">{settingDescription(node, t)}</p>
        </span>
        <SettingsToggle
          checked={settings[key] as boolean}
          disabled={saving}
          label={settingLabel(node, t)}
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
    <section
      className="settings-list-section"
      aria-labelledby="notifications-section-heading"
      data-setting-id={definition.id}
    >
      <h2 id="notifications-section-heading">{settingLabel(definition, t)}</h2>
      <div
        className="settings-list general-settings-list general-notification-settings-list"
        aria-busy={status === 'loading'}
      >
        {settings ? (
          <>
            {renderSettingsNodes(definition.children ?? [], (node) => {
              switch (node.id) {
                case 'notifications.mode':
                  return (
                    <div className="settings-list-row general-notification-mode-row">
                      <span className="settings-list-row__text">
                        <span
                          className="settings-list-row__title"
                          id="ordinary-notification-mode-heading"
                        >
                          {settingLabel(node, t)}
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
                  )
                case 'notifications.custom':
                  return (
                    <div
                      aria-hidden={selectedMode !== 'custom'}
                      className="general-notification-drawer"
                      data-open={selectedMode === 'custom' ? 'true' : 'false'}
                      inert={selectedMode !== 'custom'}
                    >
                      <div
                        aria-label={settingLabel(node, t)}
                        className="general-notification-drawer__inner"
                        role="group"
                      >
                        {renderSettingsNodes(node.children ?? [], renderToggleRow)}
                      </div>
                    </div>
                  )
                default:
                  return renderToggleRow(node)
              }
            })}
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

      {renderSettingsNodes(generalSettingsNodes, (section) => {
        switch (section.id) {
          case 'general.preferences':
            return (
              <section className="settings-list-section" aria-labelledby="general-section-heading">
                <h2 id="general-section-heading">{settingLabel(section, t)}</h2>
                <div className="settings-list general-settings-list">
                  {renderSettingsNodes(section.children, (node) => {
                    switch (node.id) {
                      case 'general.language':
                        return (
                          <div className="settings-list-row general-settings-language-row">
                            <span className="settings-list-row__text">
                              <span
                                className="settings-list-row__title"
                                id="language-setting-heading"
                              >
                                {settingLabel(node, t)}
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
                        )
                      default:
                        return null
                    }
                  })}
                </div>
              </section>
            )
          case 'general.permissions':
            return (
              <section className="settings-list-section" aria-labelledby="permission-modes-heading">
                <h2 id="permission-modes-heading">{settingLabel(section, t)}</h2>

                <div className="settings-list general-permission-modes-list">
                  {renderSettingsNodes(section.children, (node) => {
                    switch (node.id) {
                      case 'general.defaultPermission':
                        return (
                          <div className="settings-list-row">
                            <span className="settings-list-row__text">
                              <span className="settings-list-row__title">
                                {settingLabel(node, t)}
                              </span>
                              <p className="settings-list-row__description">
                                {settingDescription(node, t)}
                              </p>
                            </span>
                            <SettingsToggle
                              checked
                              disabled
                              label={t('general.defaultPermissionLocked')}
                            />
                          </div>
                        )
                      case 'general.fullPermission':
                        return (
                          <div className="settings-list-row">
                            <span className="settings-list-row__text">
                              <span className="settings-list-row__title">
                                {settingLabel(node, t)}
                              </span>
                              <p className="settings-list-row__description">
                                {settingDescription(node, t)}
                              </p>
                            </span>
                            <SettingsToggle
                              checked={uiPreferences.fullPermissionEnabled}
                              label={settingLabel(node, t)}
                              onChange={(fullPermissionEnabled) =>
                                onUiPreferencesChange({ fullPermissionEnabled })
                              }
                            />
                          </div>
                        )
                      case 'general.customPermission':
                        return (
                          <div className="settings-list-row">
                            <span className="settings-list-row__text">
                              <span className="settings-list-row__title">
                                {settingLabel(node, t)}
                              </span>
                              <p className="settings-list-row__description">
                                {settingDescription(node, t)}
                              </p>
                            </span>
                            <SettingsToggle
                              checked={uiPreferences.customPermissionEnabled}
                              label={settingLabel(node, t)}
                              onChange={(customPermissionEnabled) =>
                                onUiPreferencesChange({ customPermissionEnabled })
                              }
                            />
                          </div>
                        )
                      default:
                        return null
                    }
                  })}
                </div>
              </section>
            )
          case 'general.customPermissions':
            return (
              <section
                className="settings-list-section"
                aria-labelledby="custom-permissions-heading"
              >
                <h2 id="custom-permissions-heading">{settingLabel(section, t)}</h2>

                <div className="settings-list general-permissions-list">
                  {renderSettingsNodes(section.children, (node) => {
                    switch (node.id) {
                      case 'general.readPermission':
                        return (
                          <div className="settings-list-row">
                            <span className="settings-list-row__text">
                              <span className="settings-list-row__title">
                                {settingLabel(node, t)}
                              </span>
                            </span>
                            <span className="settings-list-row__control">
                              <PermissionSegment
                                ariaLabel={settingLabel(node, t)}
                                value={uiPreferences.customPermissions.read}
                                options={READ_PERMISSION_OPTIONS.map((option) => ({
                                  value: option.value,
                                  label: t(option.label)
                                }))}
                                onChange={(value) =>
                                  updateCustomPermissions({ read: value as AgentReadPermission })
                                }
                              />
                            </span>
                          </div>
                        )
                      case 'general.writePermission':
                        return (
                          <div className="settings-list-row">
                            <span className="settings-list-row__text">
                              <span className="settings-list-row__title">
                                {settingLabel(node, t)}
                              </span>
                            </span>
                            <span className="settings-list-row__control">
                              <PermissionSegment
                                ariaLabel={settingLabel(node, t)}
                                value={uiPreferences.customPermissions.write}
                                options={WRITE_PERMISSION_OPTIONS.map((option) => ({
                                  value: option.value,
                                  label: t(option.label)
                                }))}
                                onChange={(value) =>
                                  updateCustomPermissions({ write: value as AgentWritePermission })
                                }
                              />
                            </span>
                          </div>
                        )
                      case 'general.autoApproveFileEdits':
                        return (
                          <div
                            className="general-permission-drawer"
                            data-open={canAutoApproveFileEdits ? 'true' : 'false'}
                            aria-hidden={!canAutoApproveFileEdits}
                          >
                            <div className="general-permission-drawer__inner">
                              <div className="settings-list-row">
                                <span className="settings-list-row__text">
                                  <span className="settings-list-row__title">
                                    {settingLabel(node, t)}
                                  </span>
                                  <p className="settings-list-row__description">
                                    {settingDescription(node, t)}
                                  </p>
                                </span>
                                <SettingsToggle
                                  checked={uiPreferences.customPermissions.patch === 'auto_approve'}
                                  disabled={!canAutoApproveFileEdits}
                                  label={settingLabel(node, t)}
                                  onChange={(checked) =>
                                    updateCustomPermissions({
                                      patch: checked ? 'auto_approve' : 'require_approval'
                                    })
                                  }
                                />
                              </div>
                            </div>
                          </div>
                        )
                      case 'general.autoApproveCommands':
                        return (
                          <div className="settings-list-row">
                            <span className="settings-list-row__text">
                              <span className="settings-list-row__title">
                                {settingLabel(node, t)}
                              </span>
                              <p className="settings-list-row__description">
                                {settingDescription(node, t)}
                              </p>
                            </span>
                            <SettingsToggle
                              checked={uiPreferences.customPermissions.command === 'auto_approve'}
                              label={settingLabel(node, t)}
                              onChange={(checked) =>
                                updateCustomPermissions({
                                  command: checked ? 'auto_approve' : 'require_approval'
                                })
                              }
                            />
                          </div>
                        )
                      case 'general.autoApproveBuiltinExecution':
                        return (
                          <div className="settings-list-row">
                            <span className="settings-list-row__text">
                              <span className="settings-list-row__title">
                                {settingLabel(node, t)}
                              </span>
                              <p className="settings-list-row__description">
                                {settingDescription(node, t)}
                              </p>
                            </span>
                            <SettingsToggle
                              checked={
                                uiPreferences.customPermissions.builtinExecution === 'auto_approve'
                              }
                              label={settingLabel(node, t)}
                              onChange={(checked) =>
                                updateCustomPermissions({
                                  builtinExecution: checked ? 'auto_approve' : 'require_approval'
                                })
                              }
                            />
                          </div>
                        )
                      default:
                        return null
                    }
                  })}
                </div>
              </section>
            )
          case 'general.composer':
            return (
              <section className="settings-list-section" aria-labelledby="composer-section-heading">
                <h2 id="composer-section-heading">{settingLabel(section, t)}</h2>
                <div className="settings-list general-settings-list">
                  {renderSettingsNodes(section.children, (node) => {
                    switch (node.id) {
                      case 'general.showContextWindowUsage':
                        return (
                          <div className="settings-list-row">
                            <span className="settings-list-row__text">
                              <span className="settings-list-row__title">
                                {settingLabel(node, t)}
                              </span>
                            </span>
                            <SettingsToggle
                              checked={uiPreferences.showContextWindowUsage}
                              label={settingLabel(node, t)}
                              onChange={(showContextWindowUsage) =>
                                onUiPreferencesChange({ showContextWindowUsage })
                              }
                            />
                          </div>
                        )
                      default:
                        return null
                    }
                  })}
                </div>
              </section>
            )
          case 'notifications':
            return <NotificationSettingsSection definition={section} />
          default:
            return null
        }
      })}
    </article>
  )
}
