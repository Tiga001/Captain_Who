import { useMemo, useState, type FormEvent } from 'react'
import { ChevronDown } from 'lucide-react'
import type { ProviderProfileUiDescriptor } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { DEFAULT_MODEL_CONTEXT_WINDOW_TOKENS } from '../../../../config/modelConfig'
import { SettingsSelect } from '../../components/SettingsSelect'
import type { SettingsSelectOption } from '../../components/SettingsSelect'
import type { ModelConfig, ModelFormValues } from './configurationTypes'
import {
  initialProviderProfileFormState,
  selectProviderProfile,
  updateDeepSeekProviderSettings,
  type ProviderProfileSelection
} from './providerProfileForm'
import {
  providerSettingsEditors,
  type DeepSeekProviderSettingsDraft
} from './providerSettingsEditors'
import { SecretInput } from './SecretInput'

interface ModelFormProps {
  model?: ModelConfig
  providerProfileDescriptors: readonly ProviderProfileUiDescriptor[]
  onCancel: () => void
  onSave: (values: ModelFormValues) => void | Promise<void>
}

function isValidPriceInput(value: string): boolean {
  const normalized = value.trim().replaceAll(',', '')
  if (normalized.length === 0) return true
  const parsed = Number(normalized)
  return Number.isFinite(parsed) && parsed >= 0
}

function isValidContextWindowInput(value: string): boolean {
  const normalized = value.trim().replaceAll(',', '')
  if (normalized.length === 0) return true
  if (!/^\d+$/.test(normalized)) return false
  const parsed = Number(normalized)
  return Number.isSafeInteger(parsed) && parsed > 0 && parsed <= 4_294_967_295
}

function isValidApiUrl(value: string): boolean {
  const normalized = value.trim()
  if (normalized.length === 0) return true

  try {
    const parsed = new URL(normalized)
    return parsed.protocol === 'http:' || parsed.protocol === 'https:'
  } catch {
    return false
  }
}

function toFormValues(model?: ModelConfig): ModelFormValues {
  return {
    id: model?.id ?? '',
    displayName: model?.displayName ?? '',
    apiUrlOverride: model?.apiUrlOverride ?? '',
    apiTokenOverride: model?.apiTokenOverride ?? '',
    contextWindowTokens: model?.contextWindowTokens?.toString() ?? '',
    inputPrice: model?.inputPrice ?? '0',
    cachedInputPrice: model?.cachedInputPrice ?? '',
    outputPrice: model?.outputPrice ?? '0',
    supportsImage: model?.supportsImage ?? false
  }
}

export function ModelForm({ model, providerProfileDescriptors, onCancel, onSave }: ModelFormProps) {
  const { t } = useFrontendConfig()
  const initialValues = useMemo(() => toFormValues(model), [model])
  const initialProviderProfile = useMemo(
    () => initialProviderProfileFormState(model?.providerProfileConfig, providerProfileDescriptors),
    [model, providerProfileDescriptors]
  )
  const [values, setValues] = useState<ModelFormValues>(initialValues)
  const [providerProfile, setProviderProfile] = useState(initialProviderProfile)
  const [isProviderSettingsOpen, setProviderSettingsOpen] = useState(false)
  const [isSaving, setSaving] = useState(false)
  const [saveError, setSaveError] = useState(false)
  const [isAdvancedOpen, setIsAdvancedOpen] = useState(
    Boolean(
      initialValues.apiUrlOverride.trim() ||
      initialValues.apiTokenOverride.trim() ||
      initialProviderProfile.selection !== 'generic'
    )
  )
  const isEditing = Boolean(model)
  const isContextWindowValid = isValidContextWindowInput(values.contextWindowTokens)
  const isInputPriceValid = isValidPriceInput(values.inputPrice)
  const isCachedInputPriceValid = isValidPriceInput(values.cachedInputPrice)
  const isOutputPriceValid = isValidPriceInput(values.outputPrice)
  const hasOverrideUrl = values.apiUrlOverride.trim().length > 0
  const hasOverrideToken = values.apiTokenOverride.trim().length > 0
  const isConnectionPairComplete = hasOverrideUrl === hasOverrideToken
  const isOverrideUrlValid = isValidApiUrl(values.apiUrlOverride)
  const canSave =
    values.id.trim().length > 0 &&
    isContextWindowValid &&
    isInputPriceValid &&
    isCachedInputPriceValid &&
    isOutputPriceValid &&
    isConnectionPairComplete &&
    isOverrideUrlValid

  const deepSeekDescriptor = providerProfileDescriptors.find(
    (descriptor) =>
      descriptor.profileId === 'deepseek_v4_chat' &&
      descriptor.settingsKind === 'deepseek_v4_chat' &&
      descriptor.selectable
  )
  const providerProfileOptions: SettingsSelectOption<ProviderProfileSelection>[] = [
    ...(providerProfile.unsupportedProfile
      ? [
          {
            disabled: true,
            label: `${t('configuration.providerProfile.unsupported')} (${providerProfile.unsupportedProfile.id}@${providerProfile.unsupportedProfile.version})`,
            value: 'unsupported' as const
          }
        ]
      : []),
    {
      label: t('configuration.providerProfile.generic'),
      value: 'generic'
    },
    ...(deepSeekDescriptor
      ? [
          {
            label: deepSeekDescriptor.displayName,
            value: 'deepseek_v4_chat' as const
          }
        ]
      : [])
  ]
  const ProviderSettingsEditor = deepSeekDescriptor
    ? providerSettingsEditors[deepSeekDescriptor.settingsKind]
    : undefined

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!canSave || isSaving) return
    setSaving(true)
    setSaveError(false)
    try {
      await onSave({
        ...values,
        id: values.id.trim(),
        displayName: values.displayName.trim(),
        apiUrlOverride: values.apiUrlOverride.trim(),
        apiTokenOverride: values.apiTokenOverride.trim(),
        contextWindowTokens:
          values.contextWindowTokens.trim().replaceAll(',', '') ||
          DEFAULT_MODEL_CONTEXT_WINDOW_TOKENS.toString(),
        inputPrice: values.inputPrice.trim() || '0',
        // Empty is intentional: Host freezes the effective cached price from inputPrice when a
        // run starts, while the editable model keeps inheritance visible to the user.
        cachedInputPrice: values.cachedInputPrice.trim(),
        outputPrice: values.outputPrice.trim() || '0',
        providerProfileUpdate: providerProfile.update
      })
    } catch {
      // The settings provider restores the last Host-authoritative snapshot and presents the
      // sanitized failure. Restore this local draft too, without reflecting raw Provider details.
      setValues(initialValues)
      setProviderProfile(initialProviderProfile)
      setProviderSettingsOpen(false)
      setSaveError(true)
    } finally {
      setSaving(false)
    }
  }

  return (
    <form
      className="model-form-page"
      aria-labelledby="model-form-heading"
      onSubmit={(event) => void submit(event)}
    >
      <h1 id="model-form-heading">
        {isEditing ? t('configuration.editModel') : t('configuration.newModel')}
      </h1>

      <div className="model-form-page__fields settings-list" data-advanced-open={isAdvancedOpen}>
        <label className="configuration-field settings-list-row">
          <span className="settings-list-row__text">
            <span className="settings-list-row__title">{t('configuration.modelId')}</span>
          </span>
          <span className="settings-list-row__control">
            <input
              className="settings-list-control"
              value={values.id}
              placeholder={t('configuration.modelIdPlaceholder')}
              onChange={(event) => setValues((current) => ({ ...current, id: event.target.value }))}
            />
          </span>
        </label>

        <label className="configuration-field settings-list-row">
          <span className="settings-list-row__text">
            <span className="settings-list-row__title">
              {t('configuration.contextWindowTokens')}
            </span>
          </span>
          <span className="settings-list-row__control model-form-price-control">
            <input
              className="settings-list-control"
              inputMode="numeric"
              aria-invalid={!isContextWindowValid}
              value={values.contextWindowTokens}
              placeholder={t('configuration.contextWindowTokensPlaceholder')}
              onChange={(event) =>
                setValues((current) => ({
                  ...current,
                  contextWindowTokens: event.target.value
                }))
              }
            />
            {!isContextWindowValid && (
              <small className="model-form-field-error">
                {t('configuration.invalidContextWindowTokens')}
              </small>
            )}
          </span>
        </label>

        <label className="configuration-field settings-list-row">
          <span className="settings-list-row__text">
            <span className="settings-list-row__title">{t('configuration.displayName')}</span>
          </span>
          <span className="settings-list-row__control">
            <input
              className="settings-list-control"
              value={values.displayName}
              placeholder={t('configuration.displayNamePlaceholder')}
              onChange={(event) =>
                setValues((current) => ({ ...current, displayName: event.target.value }))
              }
            />
          </span>
        </label>

        <div className="configuration-field settings-list-row">
          <span className="settings-list-row__text">
            <span className="settings-list-row__title">{t('configuration.inputPrice')}</span>
          </span>
          <span className="settings-list-row__control model-form-input-price-grid">
            <label className="model-form-input-price-field">
              <span>{t('configuration.inputPriceCacheMiss')}</span>
              <input
                className="settings-list-control"
                inputMode="decimal"
                aria-invalid={!isInputPriceValid}
                value={values.inputPrice}
                onChange={(event) =>
                  setValues((current) => ({ ...current, inputPrice: event.target.value }))
                }
              />
              {!isInputPriceValid && (
                <small className="model-form-field-error">{t('configuration.invalidPrice')}</small>
              )}
            </label>
            <label className="model-form-input-price-field">
              <span>{t('configuration.inputPriceCacheHit')}</span>
              <input
                className="settings-list-control"
                inputMode="decimal"
                aria-invalid={!isCachedInputPriceValid}
                value={values.cachedInputPrice}
                placeholder={t('configuration.inputPriceCacheHitPlaceholder')}
                onChange={(event) =>
                  setValues((current) => ({
                    ...current,
                    cachedInputPrice: event.target.value
                  }))
                }
              />
              {!isCachedInputPriceValid && (
                <small className="model-form-field-error">{t('configuration.invalidPrice')}</small>
              )}
            </label>
          </span>
        </div>

        <label className="configuration-field settings-list-row">
          <span className="settings-list-row__text">
            <span className="settings-list-row__title">{t('configuration.outputPrice')}</span>
          </span>
          <span className="settings-list-row__control model-form-price-control">
            <input
              className="settings-list-control"
              inputMode="decimal"
              aria-invalid={!isOutputPriceValid}
              value={values.outputPrice}
              onChange={(event) =>
                setValues((current) => ({ ...current, outputPrice: event.target.value }))
              }
            />
            {!isOutputPriceValid && (
              <small className="model-form-field-error">{t('configuration.invalidPrice')}</small>
            )}
          </span>
        </label>

        <div className="configuration-field settings-list-row">
          <span className="settings-list-row__text">
            <span className="settings-list-row__title">
              {t('configuration.supportsImageInput')}
            </span>
          </span>
          <button
            className="settings-switch"
            type="button"
            role="switch"
            aria-checked={values.supportsImage}
            data-state={values.supportsImage ? 'on' : 'off'}
            onClick={() =>
              setValues((current) => ({ ...current, supportsImage: !current.supportsImage }))
            }
          >
            <span className="settings-switch__thumb" aria-hidden="true" />
            <span className="sr-only">{t('configuration.supportsImageInput')}</span>
          </button>
        </div>

        <div className="model-form-more-row">
          <button
            className="model-form-more-button"
            type="button"
            aria-expanded={isAdvancedOpen}
            aria-controls="model-form-advanced-settings"
            data-invalid={!isConnectionPairComplete || !isOverrideUrlValid || undefined}
            onClick={() => setIsAdvancedOpen((isOpen) => !isOpen)}
          >
            {t('configuration.more')}
            <ChevronDown aria-hidden="true" />
          </button>
        </div>

        <div
          className="model-form-advanced"
          id="model-form-advanced-settings"
          data-open={isAdvancedOpen}
          aria-hidden={!isAdvancedOpen}
        >
          <div className="model-form-advanced__inner">
            <label className="configuration-field settings-list-row">
              <span className="settings-list-row__text">
                <span className="settings-list-row__title">{t('configuration.modelApiUrl')}</span>
              </span>
              <span className="settings-list-row__control model-form-price-control">
                <input
                  className="settings-list-control"
                  type="url"
                  aria-invalid={!isOverrideUrlValid}
                  value={values.apiUrlOverride}
                  placeholder={t('configuration.modelApiUrlPlaceholder')}
                  tabIndex={isAdvancedOpen ? 0 : -1}
                  onChange={(event) => {
                    setValues((current) => ({
                      ...current,
                      apiUrlOverride: event.target.value
                    }))
                    setProviderProfile((current) =>
                      current.selection === 'generic'
                        ? selectProviderProfile(current, 'generic')
                        : current
                    )
                  }}
                />
                {!isOverrideUrlValid && (
                  <small className="model-form-field-error">
                    {t('configuration.invalidApiUrl')}
                  </small>
                )}
              </span>
            </label>

            <div className="configuration-field settings-list-row">
              <span className="settings-list-row__text">
                <span className="settings-list-row__title">{t('configuration.modelApiToken')}</span>
              </span>
              <span className="settings-list-row__control model-form-price-control">
                <SecretInput
                  ariaLabel={t('configuration.modelApiToken')}
                  value={values.apiTokenOverride}
                  placeholder={t('configuration.modelApiTokenPlaceholder')}
                  tabIndex={isAdvancedOpen ? 0 : -1}
                  onChange={(value) =>
                    setValues((current) => ({
                      ...current,
                      apiTokenOverride: value
                    }))
                  }
                />
                {!isConnectionPairComplete && (
                  <small className="model-form-field-error">
                    {t('configuration.modelConnectionPairRequired')}
                  </small>
                )}
              </span>
            </div>

            <div className="configuration-field settings-list-row">
              <span className="settings-list-row__text">
                <span className="settings-list-row__title">
                  {t('configuration.providerProfile.vendor')}
                </span>
              </span>
              <span className="settings-list-row__control model-provider-profile-control">
                <SettingsSelect
                  ariaLabel={t('configuration.providerProfile.vendor')}
                  className="model-provider-profile-select"
                  options={providerProfileOptions}
                  tabIndex={isAdvancedOpen ? 0 : -1}
                  value={providerProfile.selection}
                  onChange={(selection) => {
                    if (selection === 'unsupported') return
                    setProviderProfile((current) => selectProviderProfile(current, selection))
                  }}
                />
              </span>
            </div>

            {providerProfile.selection === 'deepseek_v4_chat' && deepSeekDescriptor && (
              <div className="configuration-field settings-list-row">
                <span className="settings-list-row__text">
                  <span className="settings-list-row__title">
                    {t('configuration.providerSettings.title')}
                  </span>
                </span>
                <button
                  className="secondary-settings-button model-provider-settings-button"
                  type="button"
                  tabIndex={isAdvancedOpen ? 0 : -1}
                  onClick={() => setProviderSettingsOpen(true)}
                >
                  {t('configuration.providerSettings.open')}
                </button>
              </div>
            )}
          </div>
        </div>
      </div>

      {saveError && (
        <p className="model-form-field-error" role="alert">
          {t('configuration.saveFailedSafe')}
        </p>
      )}

      <div className="model-form-page__actions">
        <button
          className="secondary-settings-button"
          type="button"
          disabled={isSaving}
          onClick={onCancel}
        >
          {t('configuration.cancel')}
        </button>
        <button className="primary-settings-button" type="submit" disabled={!canSave || isSaving}>
          {isSaving ? t('configuration.saving') : t('configuration.save')}
        </button>
      </div>

      {isProviderSettingsOpen && ProviderSettingsEditor && (
        <ProviderSettingsEditor
          initialSettings={{ reasoning: providerProfile.reasoning }}
          onCancel={() => setProviderSettingsOpen(false)}
          onConfirm={(settings: DeepSeekProviderSettingsDraft) => {
            setProviderProfile((current) =>
              updateDeepSeekProviderSettings(current, settings.reasoning)
            )
            setProviderSettingsOpen(false)
          }}
        />
      )}
    </form>
  )
}
