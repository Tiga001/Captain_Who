import { useEffect, useMemo, useRef, useState, type FormEvent } from 'react'
import { ChevronDown } from 'lucide-react'
import type {
  ProviderProfileUiDescriptor,
  ProviderVendorDescriptor,
  ProviderVendorModelPolicyDescriptor,
  ProviderVendorModelPolicyInput
} from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../../config/translationFormat'
import { DEFAULT_MODEL_CONTEXT_WINDOW_TOKENS } from '../../../../config/modelConfig'
import { ConfirmationDialog } from '../../../../components/dialog/ConfirmationDialog'
import { SettingsSelect } from '../../components/SettingsSelect'
import type { SettingsSelectOption } from '../../components/SettingsSelect'
import type { ModelConfig, ModelFormValues } from './configurationTypes'
import {
  applyResolvedProviderPolicy,
  detectProviderProtocolDialect,
  initialProviderProfileFormState,
  initialNewProviderProfileFormState,
  selectProviderVendor,
  updateDeepSeekProviderSettings,
  updateMoonshotProviderSettings,
  type ProviderProfileSelection
} from './providerProfileForm'
import {
  DeepSeekProviderSettingsEditor,
  MoonshotProviderSettingsEditor
} from './providerSettingsEditors'
import { CredentialInput } from './CredentialInput'
import { classifyModelSettingsSaveError, type ModelSettingsSaveError } from './modelSettingsErrors'

interface ModelFormProps {
  model?: ModelConfig
  globalApiUrl: string
  providerProfileDescriptors: readonly ProviderProfileUiDescriptor[]
  providerVendorDescriptors: readonly ProviderVendorDescriptor[]
  resolveProviderVendorModelPolicy: (
    input: ProviderVendorModelPolicyInput
  ) => Promise<ProviderVendorModelPolicyDescriptor>
  onCancel: () => void
  onSave: (values: ModelFormValues) => void | Promise<void>
}

type PolicyResolutionState =
  | { status: 'idle' | 'loading' | 'failed' | 'stored_unsupported' }
  | { status: 'resolved'; descriptor: ProviderVendorModelPolicyDescriptor }

const MAX_MODEL_DISPLAY_NAME_BYTES = 512

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

function isDisplayNameWithinLimit(value: string): boolean {
  return (
    new TextEncoder().encode(value.trim().normalize('NFKC')).byteLength <=
    MAX_MODEL_DISPLAY_NAME_BYTES
  )
}

const MAX_PROVIDER_API_URL_BYTES = 4_096

function isLoopbackHostname(hostname: string): boolean {
  const normalized = hostname.toLowerCase().replace(/^\[|\]$/g, '')
  if (normalized === 'localhost' || normalized === '::1') return true
  const octets = normalized.split('.')
  return (
    octets.length === 4 &&
    octets.every((octet) => /^\d{1,3}$/.test(octet) && Number(octet) <= 255) &&
    Number(octets[0]) === 127
  )
}

function isValidApiUrl(value: string): boolean {
  const normalized = value.trim()
  if (normalized.length === 0) return true
  const containsControlCharacter = Array.from(value).some((character) => {
    const codePoint = character.codePointAt(0) ?? 0
    return codePoint <= 0x1f || (codePoint >= 0x7f && codePoint <= 0x9f)
  })
  if (
    new TextEncoder().encode(value).byteLength > MAX_PROVIDER_API_URL_BYTES ||
    containsControlCharacter ||
    normalized.includes('?') ||
    normalized.includes('#')
  ) {
    return false
  }

  try {
    const parsed = new URL(normalized)
    if (parsed.username || parsed.password) return false
    if (parsed.protocol === 'https:') return true
    return import.meta.env.DEV && parsed.protocol === 'http:' && isLoopbackHostname(parsed.hostname)
  } catch {
    return false
  }
}

function toFormValues(model?: ModelConfig): ModelFormValues {
  return {
    providerModelId: model?.providerModelId ?? '',
    displayName: model?.displayName ?? '',
    apiUrlOverride: model?.apiUrlOverride ?? '',
    apiTokenOverrideStatus: model?.apiTokenOverrideStatus ?? 'missing',
    apiTokenOverrideMutation: { type: 'keep' },
    contextWindowTokens: model?.contextWindowTokens?.toString() ?? '',
    inputPrice: model?.inputPrice ?? '0',
    cachedInputPrice: model?.cachedInputPrice ?? '',
    outputPrice: model?.outputPrice ?? '0',
    supportsImage: model?.supportsImage ?? false,
    providerProfileUpdate: model?.providerProfileUpdate ?? { kind: 'select_generic' }
  }
}

export function ModelForm({
  model,
  globalApiUrl,
  providerProfileDescriptors,
  providerVendorDescriptors,
  resolveProviderVendorModelPolicy,
  onCancel,
  onSave
}: ModelFormProps) {
  const { t } = useFrontendConfig()
  const initialValues = useMemo(() => toFormValues(model), [model])
  const initialProviderProfile = useMemo(
    () =>
      model?.providerProfileConfig
        ? initialProviderProfileFormState(model.providerProfileConfig, providerProfileDescriptors)
        : initialNewProviderProfileFormState(),
    [model, providerProfileDescriptors]
  )
  const [values, setValues] = useState<ModelFormValues>(initialValues)
  const [providerProfile, setProviderProfile] = useState(initialProviderProfile)
  const [policyResolution, setPolicyResolution] = useState<PolicyResolutionState>({
    status: initialProviderProfile.selection === 'unsupported' ? 'stored_unsupported' : 'loading'
  })
  const policyRequestRef = useRef(0)
  const displayNameInputRef = useRef<HTMLInputElement>(null)
  const [isProviderSettingsOpen, setProviderSettingsOpen] = useState(false)
  const [isSaving, setSaving] = useState(false)
  const [saveError, setSaveError] = useState<ModelSettingsSaveError | null>(null)
  const [isAdvancedOpen, setIsAdvancedOpen] = useState(
    Boolean(
      initialValues.apiUrlOverride.trim() ||
      initialValues.apiTokenOverrideStatus !== 'missing' ||
      initialProviderProfile.selection !== 'generic'
    )
  )
  const isEditing = Boolean(model)
  const isContextWindowValid = isValidContextWindowInput(values.contextWindowTokens)
  const isDisplayNameValid = isDisplayNameWithinLimit(values.displayName)
  const isInputPriceValid = isValidPriceInput(values.inputPrice)
  const isCachedInputPriceValid = isValidPriceInput(values.cachedInputPrice)
  const isOutputPriceValid = isValidPriceInput(values.outputPrice)
  const hasOverrideUrl = values.apiUrlOverride.trim().length > 0
  const effectiveOverrideTokenStatus =
    values.apiTokenOverrideMutation.type === 'replace'
      ? values.apiTokenOverrideMutation.value.length > 0
        ? 'configured'
        : 'missing'
      : values.apiTokenOverrideMutation.type === 'clear'
        ? 'missing'
        : values.apiTokenOverrideStatus
  const hasOverrideToken = effectiveOverrideTokenStatus === 'configured'
  const isConnectionPairComplete = hasOverrideUrl === hasOverrideToken
  const isOverrideUrlValid = isValidApiUrl(values.apiUrlOverride)
  const effectiveApiUrl = values.apiUrlOverride.trim() || globalApiUrl
  const originalEffectiveApiUrl = model?.apiUrlOverride?.trim() || globalApiUrl
  const wireIdentityUnchanged =
    Boolean(model) &&
    values.providerModelId.trim() === model?.providerModelId &&
    effectiveApiUrl.trim() === originalEffectiveApiUrl.trim()
  const resolvedPolicy = policyResolution.status === 'resolved' ? policyResolution.descriptor : null
  const supportedPolicy =
    resolvedPolicy?.status === 'supported' && resolvedPolicy.vendorId === providerProfile.selection
      ? resolvedPolicy
      : null
  const canPreserveUnchangedProfile =
    providerProfile.update.kind === 'unchanged' &&
    !providerProfile.explicitSelection &&
    wireIdentityUnchanged
  const isProviderStateValid =
    policyResolution.status === 'stored_unsupported'
      ? canPreserveUnchangedProfile
      : policyResolution.status === 'resolved'
        ? resolvedPolicy?.status === 'supported'
          ? supportedPolicy !== null
          : canPreserveUnchangedProfile
        : false
  const canSave =
    values.displayName.trim().length > 0 &&
    isDisplayNameValid &&
    values.providerModelId.trim().length > 0 &&
    isContextWindowValid &&
    isInputPriceValid &&
    isCachedInputPriceValid &&
    isOutputPriceValid &&
    isOverrideUrlValid &&
    isProviderStateValid

  useEffect(() => {
    const selection = providerProfile.selection
    const modelId = values.providerModelId.trim()
    if (selection === 'unsupported') {
      policyRequestRef.current += 1
      setPolicyResolution({ status: 'stored_unsupported' })
      return
    }
    if (!modelId) {
      policyRequestRef.current += 1
      setPolicyResolution({ status: 'idle' })
      return
    }
    const requestId = policyRequestRef.current + 1
    policyRequestRef.current = requestId
    setPolicyResolution({ status: 'loading' })
    void resolveProviderVendorModelPolicy({
      vendorId: selection,
      modelId,
      dialect: detectProviderProtocolDialect(effectiveApiUrl)
    })
      .then((descriptor) => {
        if (policyRequestRef.current !== requestId) return
        setPolicyResolution({ status: 'resolved', descriptor })
        if (descriptor.status !== 'supported') return
        setProviderProfile((current) => applyResolvedProviderPolicy(current, descriptor))
        if (descriptor.imageInput !== 'user_configurable') {
          const supportsImage = descriptor.imageInput === 'supported'
          setValues((current) =>
            current.supportsImage === supportsImage ? current : { ...current, supportsImage }
          )
        }
      })
      .catch(() => {
        if (policyRequestRef.current === requestId) setPolicyResolution({ status: 'failed' })
      })
  }, [
    effectiveApiUrl,
    providerProfile.selection,
    resolveProviderVendorModelPolicy,
    values.providerModelId
  ])

  const selectableVendorOptions: SettingsSelectOption<ProviderProfileSelection>[] = []
  for (const descriptor of providerVendorDescriptors) {
    if (!descriptor.selectable) continue
    if (descriptor.vendorId === 'generic') {
      selectableVendorOptions.push({
        label: t('configuration.providerProfile.generic'),
        value: 'generic'
      })
    } else if (descriptor.vendorId === 'deepseek') {
      selectableVendorOptions.push({
        label: t('configuration.providerProfile.deepSeek'),
        value: 'deepseek'
      })
    } else if (descriptor.vendorId === 'moonshot') {
      selectableVendorOptions.push({
        label: t('configuration.providerProfile.moonshot'),
        value: 'moonshot'
      })
    }
  }
  if (
    providerProfile.selection !== 'unsupported' &&
    !selectableVendorOptions.some((option) => option.value === providerProfile.selection)
  ) {
    const label =
      providerProfile.selection === 'generic'
        ? t('configuration.providerProfile.generic')
        : providerProfile.selection === 'deepseek'
          ? t('configuration.providerProfile.deepSeek')
          : t('configuration.providerProfile.moonshot')
    selectableVendorOptions.unshift({ disabled: true, label, value: providerProfile.selection })
  }
  const providerProfileOptions: SettingsSelectOption<ProviderProfileSelection>[] = [
    ...(providerProfile.selection === 'unsupported'
      ? [
          {
            disabled: true,
            label: t('configuration.providerProfile.unsupported'),
            value: 'unsupported' as const
          }
        ]
      : []),
    ...selectableVendorOptions
  ]
  const deepSeekSettingsDescriptor =
    supportedPolicy &&
    (supportedPolicy.settings.kind === 'deepseek_v4_chat' ||
      supportedPolicy.settings.kind === 'deepseek_v4_vision')
      ? supportedPolicy.settings
      : null
  const moonshotSettingsDescriptor =
    supportedPolicy &&
    (supportedPolicy.settings.kind === 'moonshot_k3_chat' ||
      supportedPolicy.settings.kind === 'moonshot_k2_7_code_chat' ||
      supportedPolicy.settings.kind === 'moonshot_k2_6_chat')
      ? supportedPolicy.settings
      : null

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!canSave || isSaving) return
    setSaving(true)
    setSaveError(null)
    try {
      await onSave({
        ...values,
        providerModelId: values.providerModelId.trim(),
        displayName: values.displayName.trim(),
        apiUrlOverride: values.apiUrlOverride.trim(),
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
    } catch (error) {
      // Keep every local field and provider-owned setting intact. The settings provider already
      // restores its Host-authoritative snapshot; this editable draft is the user's recovery path.
      setSaveError(classifyModelSettingsSaveError(error))
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
            <span className="settings-list-row__title">{t('configuration.providerModelId')}</span>
          </span>
          <span className="settings-list-row__control">
            <input
              className="settings-list-control"
              value={values.providerModelId}
              placeholder={t('configuration.providerModelIdPlaceholder')}
              onChange={(event) =>
                setValues((current) => ({ ...current, providerModelId: event.target.value }))
              }
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
          <span className="settings-list-row__control model-form-price-control">
            <input
              ref={displayNameInputRef}
              className="settings-list-control"
              aria-invalid={!isDisplayNameValid}
              required
              value={values.displayName}
              placeholder={t('configuration.displayNamePlaceholder')}
              onChange={(event) =>
                setValues((current) => ({ ...current, displayName: event.target.value }))
              }
            />
            {!isDisplayNameValid && (
              <small className="model-form-field-error">
                {t('configuration.invalidDisplayName')}
              </small>
            )}
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
            disabled={
              supportedPolicy?.imageInput !== 'user_configurable' && Boolean(supportedPolicy)
            }
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
                  onChange={(event) =>
                    setValues((current) => ({
                      ...current,
                      apiUrlOverride: event.target.value
                    }))
                  }
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
                <CredentialInput
                  ariaLabel={t('configuration.modelApiToken')}
                  status={values.apiTokenOverrideStatus}
                  mutation={values.apiTokenOverrideMutation}
                  placeholder={t('configuration.modelApiTokenPlaceholder')}
                  tabIndex={isAdvancedOpen ? 0 : -1}
                  onMutationChange={(mutation) =>
                    setValues((current) => ({
                      ...current,
                      apiTokenOverrideMutation: mutation
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
                    setProviderSettingsOpen(false)
                    setProviderProfile((current) => {
                      const selected = selectProviderVendor(current, selection)
                      return supportedPolicy?.vendorId === selection
                        ? applyResolvedProviderPolicy(selected, supportedPolicy)
                        : selected
                    })
                  }}
                />
              </span>
            </div>

            {(providerProfile.selection === 'deepseek' ||
              providerProfile.selection === 'moonshot') && (
              <div className="configuration-field settings-list-row">
                <span className="settings-list-row__text">
                  <span className="settings-list-row__title">
                    {t('configuration.providerSettings.title')}
                  </span>
                </span>
                <button
                  className="secondary-settings-button model-provider-settings-button"
                  type="button"
                  disabled={!deepSeekSettingsDescriptor && !moonshotSettingsDescriptor}
                  tabIndex={isAdvancedOpen ? 0 : -1}
                  onClick={() => setProviderSettingsOpen(true)}
                >
                  {t('configuration.providerSettings.open')}
                </button>
              </div>
            )}

            {policyResolution.status === 'loading' && (
              <p className="model-provider-policy-message" role="status">
                {t('configuration.providerProfile.resolving')}
              </p>
            )}
            {policyResolution.status === 'failed' && (
              <p className="model-provider-policy-message model-form-field-error" role="alert">
                {t('configuration.providerProfile.resolveFailed')}
              </p>
            )}
            {resolvedPolicy?.status === 'unsupported' && (
              <div className="model-provider-policy-message" role="alert">
                <p>
                  {resolvedPolicy.reason === 'unsupported_model'
                    ? t('configuration.providerProfile.unsupportedModel')
                    : resolvedPolicy.reason === 'unsupported_dialect'
                      ? t('configuration.providerProfile.unsupportedDialect')
                      : t('configuration.providerProfile.unsupportedVendor')}
                </p>
                <p>{t('configuration.providerProfile.unsupportedGuidance')}</p>
              </div>
            )}
            {providerProfile.familyChanged && (
              <p className="model-provider-policy-message" role="status">
                {t('configuration.providerProfile.familyChanged')}
              </p>
            )}
          </div>
        </div>
      </div>

      {saveError?.code === 'unknown' && (
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

      {saveError?.code === 'duplicate_display_name' && (
        <ConfirmationDialog
          cancelLabel={t('configuration.acknowledge')}
          confirmLabel={t('configuration.acknowledge')}
          confirmVariant="primary"
          description={formatTranslation(t, 'configuration.duplicateDisplayName', {
            displayName: saveError.displayName
          })}
          dialogRole="alertdialog"
          onCancel={() => setSaveError(null)}
          onConfirm={() => setSaveError(null)}
          restoreFocusRef={displayNameInputRef}
          showCancelButton={false}
          title={t('configuration.duplicateDisplayNameTitle')}
        />
      )}

      {isProviderSettingsOpen &&
        providerProfile.selection === 'deepseek' &&
        providerProfile.settings &&
        deepSeekSettingsDescriptor && (
          <DeepSeekProviderSettingsEditor
            descriptor={deepSeekSettingsDescriptor}
            initialSettings={providerProfile.settings}
            onCancel={() => setProviderSettingsOpen(false)}
            onConfirm={(settings) => {
              setProviderProfile((current) => updateDeepSeekProviderSettings(current, settings))
              setProviderSettingsOpen(false)
            }}
          />
        )}
      {isProviderSettingsOpen &&
        providerProfile.selection === 'moonshot' &&
        providerProfile.settings &&
        moonshotSettingsDescriptor && (
          <MoonshotProviderSettingsEditor
            descriptor={moonshotSettingsDescriptor}
            initialSettings={providerProfile.settings}
            onCancel={() => setProviderSettingsOpen(false)}
            onConfirm={(settings) => {
              setProviderProfile((current) => updateMoonshotProviderSettings(current, settings))
              setProviderSettingsOpen(false)
            }}
          />
        )}
    </form>
  )
}
