import { useState } from 'react'
import { useModelSettings } from '../../../config/ModelSettingsProvider'
import { ModelForm } from './configuration/ModelForm'
import { ModelManager } from './configuration/ModelManager'
import { ModelProviderSettings } from './configuration/ModelProviderSettings'
import { WebSearchSettings } from './configuration/WebSearchSettings'
import { ImageGenerationSettings } from './configuration/ImageGenerationSettings'
import type { ModelConfig, ModelFormValues } from './configuration/configurationTypes'
import { modelConfigFromForm } from './configuration/modelPersistence'
import './ConfigurationSettingsPage.css'

type ConfigurationView = 'settings' | 'manager' | 'createModel' | 'editModel'

export function ConfigurationSettingsPage() {
  const {
    apiToken,
    apiUrl,
    deleteModel,
    models,
    searchMode,
    setApiToken,
    setApiUrl,
    setSearchMode,
    setTavilyApiKey,
    tavilyApiKey,
    toggleModel,
    upsertModel
  } = useModelSettings()
  const [view, setView] = useState<ConfigurationView>('settings')
  const [editingModel, setEditingModel] = useState<ModelConfig | undefined>()

  const openCreateModel = () => {
    setEditingModel(undefined)
    setView('createModel')
  }

  const openEditModel = (model: ModelConfig) => {
    setEditingModel(model)
    setView('editModel')
  }

  const saveModel = (values: ModelFormValues) => {
    const savedModel = modelConfigFromForm(values, editingModel)

    upsertModel(savedModel, editingModel?.id)
    setEditingModel(undefined)
    setView('manager')
  }

  if (view === 'manager') {
    return (
      <ModelManager
        models={models}
        onBack={() => setView('settings')}
        onCreate={openCreateModel}
        onDelete={deleteModel}
        onEdit={openEditModel}
      />
    )
  }

  if (view === 'createModel' || view === 'editModel') {
    return (
      <ModelForm
        model={view === 'editModel' ? editingModel : undefined}
        onCancel={() => setView('manager')}
        onSave={saveModel}
      />
    )
  }

  return (
    <div className="configuration-page">
      <ModelProviderSettings
        apiUrl={apiUrl}
        apiToken={apiToken}
        models={models}
        onApiTokenChange={setApiToken}
        onApiUrlChange={setApiUrl}
        onManageModels={() => setView('manager')}
        onToggleModel={toggleModel}
      />

      <WebSearchSettings
        searchMode={searchMode}
        tavilyApiKey={tavilyApiKey}
        onSearchModeChange={setSearchMode}
        onTavilyApiKeyChange={setTavilyApiKey}
      />

      <ImageGenerationSettings />
    </div>
  )
}
