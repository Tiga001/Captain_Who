import { useEffect, useRef, useState } from 'react'
import { useModelSettings } from '../../../config/ModelSettingsProvider'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { SettingsBreadcrumbs } from '../components/SettingsBreadcrumbs'
import { renderSettingsNodes } from '../settingsDefinition'
import { useSettingsPageNavigation } from '../settingsSearchNavigation'
import { ModelForm } from './configuration/ModelForm'
import { ModelManager } from './configuration/ModelManager'
import { ModelProviderSettings } from './configuration/ModelProviderSettings'
import { WebSearchSettings } from './configuration/WebSearchSettings'
import { ImageGenerationSettings } from './configuration/ImageGenerationSettings'
import type { ModelConfig, ModelFormValues } from './configuration/configurationTypes'
import { modelConfigFromForm } from './configuration/modelPersistence'
import { configurationSettings } from './configuration/configuration.definition'
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
  const editorSessionRef = useRef(0)
  const navigateToView = (nextView: ConfigurationView) => {
    editorSessionRef.current += 1
    setView(nextView)
  }
  const navigateToSettingsRoot = () => {
    editorSessionRef.current += 1
    onNavigateSettingsRoot()
  }
  useEffect(
    () => () => {
      editorSessionRef.current += 1
    },
    []
  )

  useSettingsPageNavigation('configuration', (target) => {
    if (target.id === 'configuration.models') {
      navigateToView('manager')
    } else if (target.view?.startsWith('model')) {
      // Search never chooses or creates a model on the user's behalf.
      if (view !== 'editModel' && view !== 'createModel') navigateToView('manager')
    } else {
      navigateToView('settings')
    }
  })

  const openCreateModel = () => {
    setEditingModel(undefined)
    navigateToView('createModel')
  }

  const openEditModel = (model: ModelConfig) => {
    setEditingModel(model)
    navigateToView('editModel')
  }

  const saveModel = async (values: ModelFormValues) => {
    const editorSession = editorSessionRef.current
    const savedModel = editingModel
      ? modelConfigFromForm(values, editingModel)
      : modelConfigFromForm(values)

    await upsertModel(savedModel, { validationDialog: true })
    // Saving remains valid after navigation, but its receipt must not close a newer
    // draft, including a new editing session for the same model.
    if (editorSessionRef.current !== editorSession) return
    setEditingModel(undefined)
    navigateToView('manager')
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
              onSelect: navigateToSettingsRoot
            },
            {
              id: 'configuration',
              label: t('settings.page.configuration'),
              onSelect: () => navigateToView('settings')
            },
            { id: 'models', label: t('configuration.modelSettings') }
          ]}
        />
        <ModelManager
          models={models}
          onBack={() => navigateToView('settings')}
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
              onSelect: navigateToSettingsRoot
            },
            {
              id: 'configuration',
              label: t('settings.page.configuration'),
              onSelect: () => navigateToView('settings')
            },
            {
              id: 'models',
              label: t('configuration.modelSettings'),
              onSelect: () => navigateToView('manager')
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
          onCancel={() => navigateToView('manager')}
          onSave={saveModel}
        />
      </>
    )
  }

  return (
    <div className="configuration-page">
      {renderSettingsNodes(configurationSettings, (node) => {
        switch (node.id) {
          case 'configuration.model':
            return (
              <ModelProviderSettings
                definition={node}
                apiUrl={apiUrl}
                apiTokenStatus={apiTokenStatus}
                models={models}
                onApiTokenCommit={updateApiToken}
                onApiUrlChange={setApiUrl}
                onManageModels={() => navigateToView('manager')}
                onToggleModel={toggleModel}
              />
            )
          case 'configuration.webSearch':
            return (
              <WebSearchSettings
                definition={node}
                searchMode={searchMode}
                tavilyApiKeyStatus={tavilyApiKeyStatus}
                onSearchModeChange={setSearchMode}
                onTavilyApiKeyCommit={updateTavilyApiKey}
              />
            )
          case 'configuration.image':
            return <ImageGenerationSettings definition={node} />
        }
      })}
    </div>
  )
}
