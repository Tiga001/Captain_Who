import { defineSettingsNodes } from '../../settingsDefinition'

export const deepSeekThinkingModeLabels = {
  provider_default: 'configuration.deepSeekSettings.thinkingProviderDefault',
  enabled: 'configuration.deepSeekSettings.thinkingEnabled',
  disabled: 'configuration.deepSeekSettings.thinkingDisabled'
} as const
export const deepSeekEffortLabels = {
  provider_default: 'configuration.deepSeekSettings.effortProviderDefault',
  low: 'configuration.deepSeekSettings.effortLow',
  high: 'configuration.deepSeekSettings.effortHigh',
  max: 'configuration.deepSeekSettings.effortMax'
} as const
export const moonshotThinkingModeLabels = {
  provider_default: 'configuration.moonshotSettings.thinkingProviderDefault',
  enabled: 'configuration.moonshotSettings.thinkingEnabled',
  disabled: 'configuration.moonshotSettings.thinkingDisabled',
  enabled_keep_all: 'configuration.moonshotSettings.thinkingEnabledKeepAll'
} as const
export const moonshotEffortLabels = {
  provider_default: 'configuration.moonshotSettings.effortProviderDefault',
  low: 'configuration.moonshotSettings.effortLow',
  high: 'configuration.moonshotSettings.effortHigh',
  max: 'configuration.moonshotSettings.effortMax'
} as const
export const providerVendorLabels = {
  generic: 'configuration.providerProfile.generic',
  deepseek: 'configuration.providerProfile.deepSeek',
  moonshot: 'configuration.providerProfile.moonshot'
} as const

export const modelDefaultSettings = defineSettingsNodes([
  { id: 'configuration.apiUrl', title: { text: 'API URL' } },
  {
    id: 'configuration.apiToken',
    title: { text: 'API Token' },
    terms: [{ text: '密钥 API Key' }]
  }
])

export const modelInputPriceSettings = defineSettingsNodes([
  { id: 'configuration.model.inputPrice', title: 'configuration.inputPriceCacheMiss' },
  { id: 'configuration.model.cachedInputPrice', title: 'configuration.inputPriceCacheHit' }
])

export const deepSeekSettings = defineSettingsNodes([
  {
    id: 'configuration.model.deepseek.thinkingMode',
    title: 'configuration.deepSeekSettings.thinkingMode',
    terms: Object.values(deepSeekThinkingModeLabels)
  },
  {
    id: 'configuration.model.deepseek.reasoningEffort',
    title: 'configuration.deepSeekSettings.reasoningEffort',
    terms: Object.values(deepSeekEffortLabels)
  }
])

export const moonshotSettings = defineSettingsNodes([
  {
    id: 'configuration.model.moonshot.reasoningEffort',
    title: 'configuration.moonshotSettings.reasoningEffort',
    terms: Object.values(moonshotEffortLabels)
  },
  {
    id: 'configuration.model.moonshot.thinkingMode',
    title: 'configuration.moonshotSettings.thinkingMode',
    terms: Object.values(moonshotThinkingModeLabels)
  }
])

export const deepSeekProviderSettings = {
  id: 'configuration.model.deepseek',
  title: 'configuration.deepSeekSettings.title',
  description: 'configuration.deepSeekSettings.defaultThinkingDescription',
  view: 'model-deepseek',
  children: deepSeekSettings
} as const

export const moonshotProviderSettings = {
  id: 'configuration.model.moonshot',
  title: 'configuration.moonshotSettings.title',
  view: 'model-moonshot',
  children: moonshotSettings
} as const

export const providerSettings = defineSettingsNodes([
  deepSeekProviderSettings,
  moonshotProviderSettings
])

export const modelBasicSettings = defineSettingsNodes([
  { id: 'configuration.model.providerModelId', title: 'configuration.providerModelId' },
  { id: 'configuration.model.contextWindowTokens', title: 'configuration.contextWindowTokens' },
  { id: 'configuration.model.displayName', title: 'configuration.displayName' },
  {
    id: 'configuration.model.inputPrices',
    title: 'configuration.inputPrice',
    children: modelInputPriceSettings
  },
  { id: 'configuration.model.outputPrice', title: 'configuration.outputPrice' },
  { id: 'configuration.model.supportsImage', title: 'configuration.supportsImageInput' }
])

export const modelAdvancedSettings = defineSettingsNodes([
  { id: 'configuration.model.apiUrl', title: 'configuration.modelApiUrl' },
  {
    id: 'configuration.model.apiToken',
    title: 'configuration.modelApiToken',
    terms: [{ text: '密钥 API Key' }]
  },
  {
    id: 'configuration.model.vendor',
    title: 'configuration.providerProfile.vendor',
    terms: Object.values(providerVendorLabels)
  },
  {
    id: 'configuration.model.providerSettings',
    title: 'configuration.providerSettings.title',
    searchable: true,
    children: providerSettings
  }
])

export const modelFormSettings = defineSettingsNodes([
  ...modelBasicSettings,
  {
    id: 'configuration.model.advanced',
    title: 'configuration.more',
    view: 'model-advanced',
    children: modelAdvancedSettings
  }
])

export const modelProviderSettings = defineSettingsNodes([
  {
    id: 'configuration.defaultApi',
    title: 'configuration.modelSettings',
    children: modelDefaultSettings
  },
  {
    id: 'configuration.models',
    title: 'configuration.availableModels',
    searchable: true,
    terms: ['configuration.manageModels', 'configuration.newModel', 'configuration.editModel'],
    view: 'model',
    prerequisiteId: 'configuration.models',
    children: modelFormSettings
  }
])

export const webSearchSettings = defineSettingsNodes([
  {
    id: 'configuration.webSearch.enabled',
    title: 'configuration.webSearch',
    terms: ['configuration.webSearchAllowed', 'configuration.webSearchDisabled']
  },
  {
    id: 'configuration.webSearch.apiKey',
    title: 'configuration.tavilyApiKey',
    terms: [{ text: '密钥' }]
  }
])

export const imageGenerationSettings = defineSettingsNodes([
  { id: 'configuration.image.endpointUrl', title: 'configuration.imageGeneration.endpointUrl' },
  {
    id: 'configuration.image.apiKey',
    title: 'configuration.imageGeneration.apiKey',
    terms: [{ text: '密钥' }]
  },
  { id: 'configuration.image.modelId', title: 'configuration.imageGeneration.modelId' },
  { id: 'configuration.image.textToImage', title: 'configuration.imageGeneration.textToImage' },
  { id: 'configuration.image.imageToImage', title: 'configuration.imageGeneration.imageToImage' },
  { id: 'configuration.image.watermark', title: 'configuration.imageGeneration.watermark' }
])

export const modelConfigurationSection = {
  id: 'configuration.model',
  title: 'configuration.model',
  children: modelProviderSettings
} as const

export const webSearchConfigurationSection = {
  id: 'configuration.webSearch',
  title: 'configuration.webSearch',
  children: webSearchSettings
} as const

export const imageConfigurationSection = {
  id: 'configuration.image',
  title: 'configuration.imageGeneration.title',
  description: 'configuration.imageGeneration.description',
  children: imageGenerationSettings
} as const

export const configurationSettings = defineSettingsNodes([
  modelConfigurationSection,
  webSearchConfigurationSection,
  imageConfigurationSection
])
