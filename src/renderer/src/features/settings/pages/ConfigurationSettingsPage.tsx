import { useState } from 'react'
import { useModelSettings } from '../../../config/ModelSettingsProvider'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { SettingsBreadcrumbs } from '../components/SettingsBreadcrumbs'
import { ModelForm } from './configuration/ModelForm'
import { ModelManager } from './configuration/ModelManager'
import { ModelProviderSettings } from './configuration/ModelProviderSettings'
import { WebSearchSettings } from './configuration/WebSearchSettings'
import { ImageGenerationSettings } from './configuration/ImageGenerationSettings'
import type { ModelConfig, ModelFormValues } from './configuration/configurationTypes'
import { modelConfigFromForm } from './configuration/modelPersistence'
import './ConfigurationSettingsPage.css'

type ConfigurationView = 'settings' | 'manager' | 'createModel' | 'editModel'

interface ConfigurationSettingsPageProps {
  onNavigateSettingsRoot: () => void
}

export function ConfigurationSettingsPage({
  onNavigateSettingsRoot
}: ConfigurationSettingsPageProps) {
  const { t } = useFrontendConfig()
  const {
    apiTokenStatus,
    apiUrl,
    deleteModel,
    models,
    providerProfileDescriptors,
    providerVendorDescriptors,
    resolveProviderVendorModelPolicy,
    searchMode,
    setApiUrl,
    setSearchMode,
    tavilyApiKeyStatus,
    toggleModel,
    updateApiToken,
    updateTavilyApiKey,
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

  const saveModel = async (values: ModelFormValues) => {
    const savedModel = editingModel
      ? modelConfigFromForm(values, editingModel)
      : modelConfigFromForm(values)

    await upsertModel(savedModel)
    setEditingModel(undefined)
    setView('manager')
  }

  if (view === 'manager') {
    return (
      <>
        <SettingsBreadcrumbs
          ariaLabel={t('settings.breadcrumb.label')}
          items={[
            {
              id: 'settings',
              label: t('settings.breadcrumb.root'),
              onSelect: onNavigateSettingsRoot
            },
            {
              id: 'configuration',
              label: t('settings.page.configuration'),
              onSelect: () => setView('settings')
            },
            { id: 'models', label: t('configuration.modelSettings') }
          ]}
        />
        <ModelManager
          models={models}
          onBack={() => setView('settings')}
          onCreate={openCreateModel}
          onDelete={deleteModel}
          onEdit={openEditModel}
        />
      </>
    )
  }

  if (view === 'createModel' || view === 'editModel') {
    return (
      <>
        <SettingsBreadcrumbs
          ariaLabel={t('settings.breadcrumb.label')}
          items={[
            {
              id: 'settings',
              label: t('settings.breadcrumb.root'),
              onSelect: onNavigateSettingsRoot
            },
            {
              id: 'configuration',
              label: t('settings.page.configuration'),
              onSelect: () => setView('settings')
            },
            {
              id: 'models',
              label: t('configuration.modelSettings'),
              onSelect: () => setView('manager')
            },
            {
              id: view,
              label:
                view === 'editModel' ? t('configuration.editModel') : t('configuration.newModel')
            }
          ]}
        />
        <ModelForm
          model={view === 'editModel' ? editingModel : undefined}
          globalApiUrl={apiUrl}
          providerProfileDescriptors={providerProfileDescriptors}
          providerVendorDescriptors={providerVendorDescriptors}
          resolveProviderVendorModelPolicy={resolveProviderVendorModelPolicy}
          onCancel={() => setView('manager')}
          onSave={saveModel}
        />
      </>
    )
  }

  return (
    <div className="configuration-page">
      <ModelProviderSettings
        apiUrl={apiUrl}
        apiTokenStatus={apiTokenStatus}
        models={models}
        onApiTokenCommit={updateApiToken}
        onApiUrlChange={setApiUrl}
        onManageModels={() => setView('manager')}
        onToggleModel={toggleModel}
      />

      <WebSearchSettings
        searchMode={searchMode}
        tavilyApiKeyStatus={tavilyApiKeyStatus}
        onSearchModeChange={setSearchMode}
        onTavilyApiKeyCommit={updateTavilyApiKey}
      />

      <ImageGenerationSettings />
    </div>
  )
}
