import type { CredentialMutation, CredentialStatus } from '@mycopilot/protocol'
import type {
  StorageModelConfigRecord,
  StorageModelSettingsRecord,
  StorageModelSettingsUpdateRecord,
  StorageProjectFolderRecord,
  StorageProjectRecord
} from '@mycopilot/protocol'
import type { ModelConfig, ModelConfigSaveDraft, SearchMode } from '../../config/modelConfig'
import type { AppProject, AppProjectFolder } from '../../config/projectConfig'

export interface ModelSettingsSnapshot {
  configurationRevision: string | null
  apiUrl: string
  apiTokenStatus: CredentialStatus
  searchMode: SearchMode
  tavilyApiKeyStatus: CredentialStatus
  models: ModelConfig[]
}

export interface ModelSettingsSaveDraft {
  validateContextCapacityModelId?: string
  apiUrl: string
  apiTokenMutation: CredentialMutation
  searchMode: SearchMode
  tavilyApiKeyMutation: CredentialMutation
  models: Array<ModelConfig | ModelConfigSaveDraft>
}

export function mapModelSettingsFromStorage(
  settings: StorageModelSettingsRecord | null
): ModelSettingsSnapshot | null {
  if (!settings) return null
  return {
    configurationRevision: settings.configurationRevision,
    apiUrl: settings.apiUrl,
    apiTokenStatus: settings.apiTokenStatus,
    searchMode: isSearchMode(settings.searchMode) ? settings.searchMode : 'auto',
    tavilyApiKeyStatus: settings.tavilyApiKeyStatus,
    models: settings.models.map(mapModelFromStorage)
  }
}

export function mapModelSettingsToStorage(
  settings: ModelSettingsSaveDraft,
  expectedRevision: string | null
): StorageModelSettingsUpdateRecord {
  return {
    expectedRevision,
    ...(settings.validateContextCapacityModelId !== undefined
      ? { validateContextCapacityModelId: settings.validateContextCapacityModelId }
      : {}),
    apiUrl: settings.apiUrl,
    apiTokenMutation: settings.apiTokenMutation,
    searchMode: settings.searchMode,
    tavilyApiKeyMutation: settings.tavilyApiKeyMutation,
    models: settings.models.map(mapModelToStorage)
  }
}

function mapModelFromStorage(model: StorageModelConfigRecord): ModelConfig {
  return {
    id: model.id,
    providerModelId: model.providerModelId,
    displayName: model.displayName,
    apiUrlOverride: model.apiUrlOverride ?? undefined,
    apiTokenOverrideStatus: model.apiTokenOverrideStatus,
    apiTokenOverrideMutation: { type: 'keep' },
    supportsImage: model.supportsImage,
    contextWindowTokens: model.contextWindowTokens ?? undefined,
    providerProfileConfig: model.providerProfileConfig,
    providerProfileUpdate: { kind: 'unchanged' },
    inputPrice: model.inputPrice,
    cachedInputPrice: model.cachedInputPrice,
    outputPrice: model.outputPrice,
    enabled: model.enabled,
    execution: model.execution
  }
}

function mapModelToStorage(
  model: ModelConfig | ModelConfigSaveDraft
): StorageModelSettingsUpdateRecord['models'][number] {
  return {
    id: model.id,
    providerModelId: model.providerModelId,
    displayName: model.displayName,
    apiUrlOverride: model.apiUrlOverride ?? null,
    apiTokenOverrideMutation: model.apiTokenOverrideMutation,
    supportsImage: model.supportsImage,
    contextWindowTokens: model.contextWindowTokens ?? null,
    providerProfileUpdate: model.providerProfileUpdate,
    inputPrice: model.inputPrice,
    cachedInputPrice: model.cachedInputPrice,
    outputPrice: model.outputPrice,
    enabled: model.enabled
  }
}

function mapProjectFolderFromStorage(folder: StorageProjectFolderRecord): AppProjectFolder {
  return {
    id: folder.id,
    path: folder.path,
    alias: folder.alias,
    role: folder.role,
    sortOrder: folder.sortOrder,
    createdAt: folder.createdAt
  }
}

export function mapProjectFromStorage(project: StorageProjectRecord): AppProject {
  return {
    id: project.id,
    name: project.name,
    folders: [...project.folders]
      .sort((left, right) => left.sortOrder - right.sortOrder)
      .map(mapProjectFolderFromStorage),
    createdAt: project.createdAt,
    pinnedAt: project.pinnedAt ?? null
  }
}

export function mapProjectToStorage(project: AppProject): StorageProjectRecord {
  return {
    id: project.id,
    name: project.name,
    folders: project.folders.map((folder) => ({
      id: folder.id,
      path: folder.path,
      alias: folder.alias,
      role: folder.role,
      sortOrder: folder.sortOrder,
      createdAt: folder.createdAt
    })),
    createdAt: project.createdAt,
    pinnedAt: project.pinnedAt ?? null
  }
}

function isSearchMode(value: string): value is SearchMode {
  return value === 'auto' || value === 'disabled' || value === 'tavily'
}
