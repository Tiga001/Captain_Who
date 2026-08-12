import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import type { ProviderProfileUiDescriptor } from '@mycopilot/protocol'
import { useToast } from '../components/toast/ToastContext'
import {
  loadModelSettings,
  loadProviderProfileUiDescriptors,
  saveModelSettings
} from '../features/storage/storageClient'
import type { ModelSettingsSnapshot } from '../features/storage/storageClient'
import { useAppStartupStage } from '../features/startup/AppStartupContext'
import { useFrontendConfig } from './FrontendConfigProvider'
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
  apiToken: string
  enabledModels: ModelConfig[]
  models: ModelConfig[]
  providerProfileDescriptors: ProviderProfileUiDescriptor[]
  searchMode: SearchMode
  tavilyApiKey: string
  deleteModel: (modelId: string) => void
  setApiToken: (value: string) => void
  setApiUrl: (value: string) => void
  setSearchMode: (value: SearchMode) => void
  setTavilyApiKey: (value: string) => void
  toggleModel: (modelId: string) => void
  upsertModel: (
    model: ModelConfig | ModelConfigSaveDraft,
    previousModelId?: string
  ) => Promise<ModelConfig>
}

const ModelSettingsContext = createContext<ModelSettingsContextValue | null>(null)

type ModelSettingsHydrationStatus = 'loading' | 'ready' | 'failed'

// Renderer reloads can briefly overlap a core-server restart in development. Retry the
// read, but never make a failed read indistinguishable from an empty first-run database.
const MODEL_SETTINGS_LOAD_RETRY_DELAYS_MS = [0, 50, 200] as const

function waitForModelSettingsRetry(delayMs: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, delayMs))
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
    apiUrl: modelConfig.api.defaultUrl,
    apiToken: modelConfig.api.defaultToken,
    searchMode: modelConfig.webSearch.defaultMode,
    tavilyApiKey: modelConfig.webSearch.defaultTavilyApiKey,
    models: []
  }))
  const [providerProfileDescriptors, setProviderProfileDescriptors] = useState<
    ProviderProfileUiDescriptor[]
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
        .then(() => saveModelSettings(saveDraft))
        .then((authoritativeSettings) => {
          lastSuccessfulSettingsRef.current = authoritativeSettings
          if (latestSaveRevisionRef.current === revision) {
            applySettings(authoritativeSettings)
          }
          return authoritativeSettings
        })
        .catch(() => {
          if (latestSaveRevisionRef.current === revision) {
            const lastSuccessfulSettings = lastSuccessfulSettingsRef.current
            if (lastSuccessfulSettings) {
              applySettings(lastSuccessfulSettings)
            }
            const { showToast: presentToast, t: translate } = loadFailurePresentationRef.current
            presentToast(translate('configuration.saveFailed'), { durationMs: 5000 })
          }
          throw new Error('Model settings could not be saved')
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
          const [storedSettings, descriptors] = await Promise.all([
            loadModelSettings(),
            loadProviderProfileUiDescriptors().catch(() => [])
          ])
          if (isCancelled) return

          setProviderProfileDescriptors(descriptors)
          if (storedSettings) {
            lastSuccessfulSettingsRef.current = storedSettings
            applySettings(storedSettings)
          } else {
            const bootstrapDraft: ModelSettingsSaveDraft = {
              apiUrl: modelConfig.api.defaultUrl,
              apiToken: modelConfig.api.defaultToken,
              searchMode: modelConfig.webSearch.defaultMode,
              tavilyApiKey: modelConfig.webSearch.defaultTavilyApiKey,
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
      const detail = lastError instanceof Error ? lastError.message : String(lastError)
      const { showToast: presentToast, t: translate } = loadFailurePresentationRef.current
      presentToast(`${translate('configuration.loadFailed')}: ${detail}`, { durationMs: 5000 })
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
      void persistSettings(nextSettings, nextSettings).catch(() => undefined)
    },
    [hydrationStatus, persistSettings]
  )

  const upsertModel = useCallback(
    async (
      savedModel: ModelConfig | ModelConfigSaveDraft,
      previousModelId?: string
    ): Promise<ModelConfig> => {
      if (hydrationStatus !== 'ready') {
        throw new Error('Model settings are not ready')
      }
      const targetId = previousModelId ?? savedModel.id
      const modelForSave =
        previousModelId && previousModelId !== savedModel.id
          ? { ...savedModel, previousModelId }
          : savedModel
      const currentSettings = settingsRef.current
      const existingIndex = currentSettings.models.findIndex((model) => model.id === targetId)
      let nextModels: Array<ModelConfig | ModelConfigSaveDraft>

      if (existingIndex === -1) {
        nextModels = [...currentSettings.models, modelForSave]
      } else {
        const before = currentSettings.models.slice(0, existingIndex)
        const after = currentSettings.models.slice(existingIndex + 1)
        nextModels = [...before, modelForSave, ...after]
      }

      const authoritativeSettings = await persistSettings({
        ...currentSettings,
        models: nextModels
      })
      const authoritativeModel = authoritativeSettings.models.find(
        (model) => model.id === savedModel.id
      )
      if (!authoritativeModel) {
        throw new Error('Host did not return the saved model')
      }
      return authoritativeModel
    },
    [hydrationStatus, persistSettings]
  )

  const value = useMemo<ModelSettingsContextValue>(() => {
    const { apiToken, apiUrl, models, searchMode, tavilyApiKey } = settings
    const enabledModels = settings.models.filter(
      (model) => model.enabled && isModelConnectionAvailable(model, apiUrl, apiToken)
    )

    return {
      apiUrl,
      apiToken,
      enabledModels,
      models,
      providerProfileDescriptors,
      searchMode,
      tavilyApiKey,
      deleteModel: (modelId) => {
        updateSettings((current) => ({
          ...current,
          models: current.models.filter((model) => model.id !== modelId)
        }))
      },
      setApiToken: (value) => updateSettings((current) => ({ ...current, apiToken: value })),
      setApiUrl: (value) =>
        updateSettings((current) => ({
          ...current,
          apiUrl: value,
          models: prepareModelsForGlobalApiUrlChange(current.models, providerProfileDescriptors)
        })),
      setSearchMode: (value) => updateSettings((current) => ({ ...current, searchMode: value })),
      setTavilyApiKey: (value) =>
        updateSettings((current) => ({ ...current, tavilyApiKey: value })),
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
  }, [providerProfileDescriptors, settings, updateSettings, upsertModel])

  return <ModelSettingsContext.Provider value={value}>{children}</ModelSettingsContext.Provider>
}

export function useModelSettings() {
  const context = useContext(ModelSettingsContext)

  if (!context) {
    throw new Error('useModelSettings must be used within ModelSettingsProvider')
  }

  return context
}
