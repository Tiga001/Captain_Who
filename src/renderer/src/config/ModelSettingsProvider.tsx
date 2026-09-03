import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import type {
  CredentialMutation,
  CredentialStatus,
  ProviderProfileUiDescriptor,
  ProviderVendorDescriptor,
  ProviderVendorModelPolicyDescriptor,
  ProviderVendorModelPolicyInput
} from '@mycopilot/protocol'
import { useToast } from '../components/toast/ToastContext'
import {
  loadModelSettings,
  loadProviderProfileUiDescriptors,
  loadProviderVendorDescriptors,
  resolveProviderVendorModelPolicy as resolveProviderVendorModelPolicyFromStorage,
  saveModelSettings
} from '../features/storage/storageClient'
import type { ModelSettingsSnapshot } from '../features/storage/storageClient'
import { useAppStartupStage } from '../features/startup/AppStartupContext'
import { useFrontendConfig } from './FrontendConfigProvider'
import { classifyModelSettingsSaveError } from '../features/settings/pages/configuration/modelSettingsErrors'
import {
  INITIAL_MODEL_SAVE_DRAFTS,
  isModelConnectionAvailable,
  modelConfig,
  prepareModelsForGlobalApiUrlChange
} from './modelConfig'
import type { ModelConfig, ModelConfigSaveDraft, SearchMode } from './modelConfig'
import type { ModelSettingsSaveDraft } from '../features/storage/storageClient'

interface ModelSettingsContextValue {
  apiUrl: string
  apiTokenStatus: CredentialStatus
  enabledModels: ModelConfig[]
  models: ModelConfig[]
  providerProfileDescriptors: ProviderProfileUiDescriptor[]
  providerVendorDescriptors: ProviderVendorDescriptor[]
  resolveProviderVendorModelPolicy: (
    input: ProviderVendorModelPolicyInput
  ) => Promise<ProviderVendorModelPolicyDescriptor>
  searchMode: SearchMode
  tavilyApiKeyStatus: CredentialStatus
  deleteModel: (modelId: string) => void
  updateApiToken: (mutation: CredentialMutation) => Promise<void>
  setApiUrl: (value: string) => Promise<void>
  setSearchMode: (value: SearchMode) => void
  updateTavilyApiKey: (mutation: CredentialMutation) => Promise<void>
  toggleModel: (modelId: string) => void
  upsertModel: (model: ModelConfig | ModelConfigSaveDraft) => Promise<ModelConfig>
}

const ModelSettingsContext = createContext<ModelSettingsContextValue | null>(null)

type ModelSettingsHydrationStatus = 'loading' | 'ready' | 'failed'

// Renderer reloads can briefly overlap a core-server restart in development. Retry the
// read, but never make a failed read indistinguishable from an empty first-run database.
const MODEL_SETTINGS_LOAD_RETRY_DELAYS_MS = [0, 50, 200] as const

function waitForModelSettingsRetry(delayMs: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, delayMs))
}

function credentialStatusAfterMutation(
  current: CredentialStatus,
  mutation: CredentialMutation
): CredentialStatus {
  if (mutation.type === 'replace') return 'configured'
  if (mutation.type === 'clear') return 'missing'
  return current
}

function keepSaveDraft(settings: ModelSettingsSnapshot): ModelSettingsSaveDraft {
  return {
    apiUrl: settings.apiUrl,
    apiTokenMutation: { type: 'keep' },
    searchMode: settings.searchMode,
    tavilyApiKeyMutation: { type: 'keep' },
    models: settings.models
  }
}

export function ModelSettingsProvider({ children }: { children: ReactNode }) {
  const { t } = useFrontendConfig()
  const { showToast } = useToast()
  const {
    attempt: startupAttempt,
    markFailed: markStartupFailed,
    markPending: markStartupPending,
    markReady: markStartupReady
  } = useAppStartupStage('modelSettings')
  const [settings, setSettings] = useState<ModelSettingsSnapshot>(() => ({
    configurationRevision: null,
    apiUrl: modelConfig.api.defaultUrl,
    apiTokenStatus: 'missing',
    searchMode: modelConfig.webSearch.defaultMode,
    tavilyApiKeyStatus: 'missing',
    models: []
  }))
  const [providerProfileDescriptors, setProviderProfileDescriptors] = useState<
    ProviderProfileUiDescriptor[]
  >([])
  const [providerVendorDescriptors, setProviderVendorDescriptors] = useState<
    ProviderVendorDescriptor[]
  >([])
  const [hydrationStatus, setHydrationStatus] = useState<ModelSettingsHydrationStatus>('loading')
  const saveQueueRef = useRef<Promise<void>>(Promise.resolve())
  const latestSaveRevisionRef = useRef(0)
  const settingsRef = useRef(settings)
  const lastSuccessfulSettingsRef = useRef<ModelSettingsSnapshot | null>(null)
  const loadFailurePresentationRef = useRef({ showToast, t })

  useEffect(() => {
    loadFailurePresentationRef.current = { showToast, t }
  }, [showToast, t])

  const applySettings = useCallback((nextSettings: ModelSettingsSnapshot) => {
    settingsRef.current = nextSettings
    setSettings(nextSettings)
  }, [])

  const persistSettings = useCallback(
    (
      saveDraft: ModelSettingsSaveDraft,
      optimisticSettings?: ModelSettingsSnapshot
    ): Promise<ModelSettingsSnapshot> => {
      const revision = latestSaveRevisionRef.current + 1
      latestSaveRevisionRef.current = revision
      if (optimisticSettings) applySettings(optimisticSettings)

      const save = saveQueueRef.current
        .catch(() => undefined)
        .then(() =>
          saveModelSettings(
            saveDraft,
            lastSuccessfulSettingsRef.current?.configurationRevision ?? null
          )
        )
        .then((authoritativeSettings) => {
          lastSuccessfulSettingsRef.current = authoritativeSettings
          if (latestSaveRevisionRef.current === revision) {
            applySettings(authoritativeSettings)
          }
          return authoritativeSettings
        })
        .catch(async (error: unknown) => {
          if (latestSaveRevisionRef.current === revision) {
            let recoverySettings = lastSuccessfulSettingsRef.current
            try {
              const latestSettings = await loadModelSettings()
              if (latestSettings) {
                recoverySettings = latestSettings
                lastSuccessfulSettingsRef.current = latestSettings
              }
            } catch {
              // Preserve the last known-good snapshot when the recovery read also fails.
            }
            if (recoverySettings) {
              applySettings(recoverySettings)
            }
            const { showToast: presentToast, t: translate } = loadFailurePresentationRef.current
            const classified = classifyModelSettingsSaveError(error)
            // ModelForm owns the actionable duplicate-name recovery dialog. Emitting a toast
            // here would show the same rejection twice and steal attention from the retained
            // draft. Other failures keep the existing global notification behavior.
            if (classified.code !== 'duplicate_display_name') {
              presentToast(translate('configuration.saveFailed'), { durationMs: 5000 })
            }
          }
          throw error
        })

      saveQueueRef.current = save.then(
        () => undefined,
        () => undefined
      )
      return save
    },
    [applySettings]
  )

  useEffect(() => {
    let isCancelled = false
    setHydrationStatus('loading')
    markStartupPending()

    const hydrate = async () => {
      let lastError: unknown

      for (const retryDelayMs of MODEL_SETTINGS_LOAD_RETRY_DELAYS_MS) {
        if (isCancelled) return
        if (retryDelayMs > 0) {
          await waitForModelSettingsRetry(retryDelayMs)
          if (isCancelled) return
        }

        try {
          const [storedSettings, descriptors, vendorDescriptors] = await Promise.all([
            loadModelSettings(),
            loadProviderProfileUiDescriptors().catch(() => []),
            // The directory only powers model-management choices. A transient/mixed-version
            // descriptor failure must not make chat and the rest of the workspace unavailable.
            loadProviderVendorDescriptors().catch(() => [])
          ])
          if (isCancelled) return

          setProviderProfileDescriptors(descriptors)
          setProviderVendorDescriptors(vendorDescriptors)
          if (storedSettings) {
            lastSuccessfulSettingsRef.current = storedSettings
            applySettings(storedSettings)
          } else {
            const bootstrapDraft: ModelSettingsSaveDraft = {
              apiUrl: modelConfig.api.defaultUrl,
              apiTokenMutation: { type: 'keep' },
              searchMode: modelConfig.webSearch.defaultMode,
              tavilyApiKeyMutation: { type: 'keep' },
              models: INITIAL_MODEL_SAVE_DRAFTS
            }
            await persistSettings(bootstrapDraft)
            if (isCancelled) return
          }
          setHydrationStatus('ready')
          markStartupReady()
          return
        } catch (error) {
          lastError = error
        }
      }

      if (isCancelled) return

      setHydrationStatus('failed')
      markStartupFailed(lastError)
      console.error('Failed to load model settings from SQLite', lastError)
      const { showToast: presentToast, t: translate } = loadFailurePresentationRef.current
      presentToast(translate('configuration.loadFailed'), { durationMs: 5000 })
    }

    void hydrate()

    return () => {
      isCancelled = true
    }
  }, [
    applySettings,
    markStartupFailed,
    markStartupPending,
    markStartupReady,
    persistSettings,
    startupAttempt
  ])

  const updateSettings = useCallback(
    (update: (current: ModelSettingsSnapshot) => ModelSettingsSnapshot): void => {
      if (hydrationStatus !== 'ready') return
      const nextSettings = update(settingsRef.current)
      void persistSettings(keepSaveDraft(nextSettings), nextSettings).catch(() => undefined)
    },
    [hydrationStatus, persistSettings]
  )

  const upsertModel = useCallback(
    async (savedModel: ModelConfig | ModelConfigSaveDraft): Promise<ModelConfig> => {
      if (hydrationStatus !== 'ready') {
        throw new Error('Model settings are not ready')
      }
      const currentSettings = settingsRef.current
      const existingIds = new Set(currentSettings.models.map((model) => model.id))
      const existingIndex =
        savedModel.id === null
          ? -1
          : currentSettings.models.findIndex((model) => model.id === savedModel.id)
      let nextModels: Array<ModelConfig | ModelConfigSaveDraft>

      if (existingIndex === -1) {
        nextModels = [...currentSettings.models, savedModel]
      } else {
        const before = currentSettings.models.slice(0, existingIndex)
        const after = currentSettings.models.slice(existingIndex + 1)
        nextModels = [...before, savedModel, ...after]
      }

      const authoritativeSettings = await persistSettings({
        ...keepSaveDraft(currentSettings),
        models: nextModels
      })
      const authoritativeModel =
        savedModel.id === null
          ? (() => {
              const createdModels = authoritativeSettings.models.filter(
                (model) => !existingIds.has(model.id)
              )
              return createdModels.length === 1 ? createdModels[0] : undefined
            })()
          : authoritativeSettings.models.find((model) => model.id === savedModel.id)
      if (!authoritativeModel) {
        throw new Error('Host did not return the saved model')
      }
      return authoritativeModel
    },
    [hydrationStatus, persistSettings]
  )

  const value = useMemo<ModelSettingsContextValue>(() => {
    const { apiTokenStatus, apiUrl, models, searchMode, tavilyApiKeyStatus } = settings
    const enabledModels = settings.models.filter(
      (model) => model.enabled && isModelConnectionAvailable(model, apiUrl, apiTokenStatus)
    )

    return {
      apiUrl,
      apiTokenStatus,
      enabledModels,
      models,
      providerProfileDescriptors,
      providerVendorDescriptors,
      resolveProviderVendorModelPolicy: resolveProviderVendorModelPolicyFromStorage,
      searchMode,
      tavilyApiKeyStatus,
      deleteModel: (modelId) => {
        updateSettings((current) => ({
          ...current,
          models: current.models.filter((model) => model.id !== modelId)
        }))
      },
      updateApiToken: async (mutation) => {
        const current = settingsRef.current
        const optimistic = {
          ...current,
          apiTokenStatus: credentialStatusAfterMutation(current.apiTokenStatus, mutation)
        }
        await persistSettings(
          { ...keepSaveDraft(current), apiTokenMutation: mutation },
          mutation.type === 'clear' ? undefined : optimistic
        )
      },
      setApiUrl: async (value) => {
        const current = settingsRef.current
        const nextSettings = {
          ...current,
          apiUrl: value,
          models: prepareModelsForGlobalApiUrlChange(current.models, providerProfileDescriptors)
        }
        await persistSettings(keepSaveDraft(nextSettings), nextSettings)
      },
      setSearchMode: (value) => updateSettings((current) => ({ ...current, searchMode: value })),
      updateTavilyApiKey: async (mutation) => {
        const current = settingsRef.current
        const nextStatus = credentialStatusAfterMutation(current.tavilyApiKeyStatus, mutation)
        const nextSearchMode =
          mutation.type === 'clear' ? ('disabled' as const) : current.searchMode
        const optimistic = {
          ...current,
          tavilyApiKeyStatus: nextStatus,
          searchMode: nextSearchMode
        }
        await persistSettings(
          {
            ...keepSaveDraft(current),
            searchMode: nextSearchMode,
            tavilyApiKeyMutation: mutation
          },
          mutation.type === 'clear' ? undefined : optimistic
        )
      },
      toggleModel: (modelId) => {
        updateSettings((current) => ({
          ...current,
          models: current.models.map((model) =>
            model.id === modelId ? { ...model, enabled: !model.enabled } : model
          )
        }))
      },
      upsertModel
    }
  }, [
    persistSettings,
    providerProfileDescriptors,
    providerVendorDescriptors,
    settings,
    updateSettings,
    upsertModel
  ])

  return <ModelSettingsContext.Provider value={value}>{children}</ModelSettingsContext.Provider>
}

export function useModelSettings() {
  const context = useContext(ModelSettingsContext)

  if (!context) {
    throw new Error('useModelSettings must be used within ModelSettingsProvider')
  }

  return context
}
