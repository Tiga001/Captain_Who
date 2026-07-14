import { createContext, useContext, useEffect, useMemo, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import { useToast } from '../components/toast/ToastContext'
import { loadModelSettings, saveModelSettings } from '../features/storage/storageClient'
import { useFrontendConfig } from './FrontendConfigProvider'
import { INITIAL_MODELS, isModelConnectionAvailable, modelConfig } from './modelConfig'
import type { ModelConfig, SearchMode } from './modelConfig'

interface ModelSettingsContextValue {
  apiUrl: string
  apiToken: string
  enabledModels: ModelConfig[]
  models: ModelConfig[]
  searchMode: SearchMode
  tavilyApiKey: string
  deleteModel: (modelId: string) => void
  setApiToken: (value: string) => void
  setApiUrl: (value: string) => void
  setSearchMode: (value: SearchMode) => void
  setTavilyApiKey: (value: string) => void
  toggleModel: (modelId: string) => void
  upsertModel: (model: ModelConfig, previousModelId?: string) => void
}

const ModelSettingsContext = createContext<ModelSettingsContextValue | null>(null)

export function ModelSettingsProvider({ children }: { children: ReactNode }) {
  const { t } = useFrontendConfig()
  const { showToast } = useToast()
  const [apiUrl, setApiUrl] = useState<string>(modelConfig.api.defaultUrl)
  const [apiToken, setApiToken] = useState<string>(modelConfig.api.defaultToken)
  const [searchMode, setSearchMode] = useState<SearchMode>(modelConfig.webSearch.defaultMode)
  const [tavilyApiKey, setTavilyApiKey] = useState<string>(
    modelConfig.webSearch.defaultTavilyApiKey
  )
  const [models, setModels] = useState<ModelConfig[]>(INITIAL_MODELS)
  const [isHydrated, setIsHydrated] = useState(false)
  const saveQueueRef = useRef<Promise<void>>(Promise.resolve())
  const latestSaveRevisionRef = useRef(0)

  useEffect(() => {
    let isCancelled = false

    void loadModelSettings()
      .then((settings) => {
        if (isCancelled) return

        if (settings) {
          setApiUrl(settings.apiUrl)
          setApiToken(settings.apiToken)
          setSearchMode(settings.searchMode)
          setTavilyApiKey(settings.tavilyApiKey)
          setModels(settings.models)
        }
      })
      .catch((error) => {
        console.error('Failed to load model settings from SQLite', error)
      })
      .finally(() => {
        if (!isCancelled) {
          setIsHydrated(true)
        }
      })

    return () => {
      isCancelled = true
    }
  }, [])

  useEffect(() => {
    if (!isHydrated) return

    const revision = latestSaveRevisionRef.current + 1
    latestSaveRevisionRef.current = revision
    const settings = {
      apiToken,
      apiUrl,
      models,
      searchMode,
      tavilyApiKey
    }
    const save = saveQueueRef.current.catch(() => undefined).then(() => saveModelSettings(settings))
    saveQueueRef.current = save
    void save.catch((error) => {
      console.error('Failed to save model settings to SQLite', error)
      if (latestSaveRevisionRef.current === revision) {
        const detail = error instanceof Error ? error.message : String(error)
        showToast(`${t('configuration.saveFailed')}: ${detail}`, { durationMs: 5000 })
      }
    })
  }, [apiToken, apiUrl, isHydrated, models, searchMode, showToast, t, tavilyApiKey])

  const value = useMemo<ModelSettingsContextValue>(() => {
    const enabledModels = models.filter(
      (model) => model.enabled && isModelConnectionAvailable(model, apiUrl, apiToken)
    )

    return {
      apiUrl,
      apiToken,
      enabledModels,
      models,
      searchMode,
      tavilyApiKey,
      deleteModel: (modelId) => {
        setModels((currentModels) => currentModels.filter((model) => model.id !== modelId))
      },
      setApiToken,
      setApiUrl,
      setSearchMode,
      setTavilyApiKey,
      toggleModel: (modelId) => {
        setModels((currentModels) =>
          currentModels.map((model) =>
            model.id === modelId ? { ...model, enabled: !model.enabled } : model
          )
        )
      },
      upsertModel: (savedModel, previousModelId) => {
        setModels((currentModels) => {
          const targetId = previousModelId ?? savedModel.id
          const existingIndex = currentModels.findIndex((model) => model.id === targetId)

          if (existingIndex === -1) {
            return [...currentModels.filter((model) => model.id !== savedModel.id), savedModel]
          }

          const before = currentModels
            .slice(0, existingIndex)
            .filter((model) => model.id !== savedModel.id)
          const after = currentModels
            .slice(existingIndex + 1)
            .filter((model) => model.id !== savedModel.id)

          return [...before, savedModel, ...after]
        })
      }
    }
  }, [apiToken, apiUrl, models, searchMode, tavilyApiKey])

  return <ModelSettingsContext.Provider value={value}>{children}</ModelSettingsContext.Provider>
}

export function useModelSettings() {
  const context = useContext(ModelSettingsContext)

  if (!context) {
    throw new Error('useModelSettings must be used within ModelSettingsProvider')
  }

  return context
}
