export interface ModelConfig {
  id: string
  displayName: string
  shortName?: string
  providerPath?: string
  supportsImage: boolean
  inputPrice: string
  outputPrice: string
  enabled: boolean
}

export type SearchMode = 'auto' | 'disabled' | 'tavily'

export interface ModelFormValues {
  id: string
  displayName: string
  inputPrice: string
  outputPrice: string
  supportsImage: boolean
}

export const modelConfig = {
  api: {
    defaultUrl: '',
    defaultToken: ''
  },
  webSearch: {
    defaultMode: 'auto' satisfies SearchMode,
    defaultTavilyApiKey: ''
  },
  defaults: {
    selectedModelId: 'gpt-5.5'
  },
  models: [
    {
      id: 'claude-opus-4-7',
      displayName: 'claude-opus-4-7',
      shortName: 'opus-4.7',
      supportsImage: true,
      inputPrice: '0.028',
      outputPrice: '0.14',
      enabled: true
    },
    {
      id: 'claude-sonnet-4.6',
      displayName: 'claude-sonnet-4.6',
      shortName: 'sonnet-4.6',
      supportsImage: true,
      inputPrice: '0.0168',
      outputPrice: '0.084',
      enabled: true
    },
    {
      id: 'gpt-5.5',
      displayName: 'gpt-5.5',
      shortName: 'gpt-5.5',
      supportsImage: true,
      inputPrice: '0.021',
      outputPrice: '0.126',
      enabled: true
    },
    {
      id: 'deepseek-v4-pro',
      displayName: 'deepseek-v4-pro',
      shortName: 'v4-pro',
      providerPath: 'deepseek/deepseek-v4-pro',
      supportsImage: false,
      inputPrice: '0.012',
      outputPrice: '0.024',
      enabled: true
    },
    {
      id: 'deepseek-v4-flash',
      displayName: 'deepseek-v4-flash',
      shortName: 'v4-flash',
      providerPath: 'deepseek/deepseek-v4-flash',
      supportsImage: false,
      inputPrice: '0.00105',
      outputPrice: '0.0021',
      enabled: true
    },
    {
      id: 'minimax-m2.5',
      displayName: 'minimax-m2.5',
      shortName: 'minimax-m2.5',
      providerPath: 'minimax/minimax-m2.5',
      supportsImage: false,
      inputPrice: '0.001407',
      outputPrice: '0.005628',
      enabled: true
    }
  ] satisfies ModelConfig[]
} as const

export const INITIAL_MODELS: ModelConfig[] = modelConfig.models.map((model) => ({ ...model }))
