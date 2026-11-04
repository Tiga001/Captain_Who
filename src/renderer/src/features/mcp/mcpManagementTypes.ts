import type {
  McpCatalogCompletenessView,
  McpServerListOutput,
  McpToolSummaryView
} from '@mycopilot/protocol'

export interface McpManagementViewState {
  errorMessage: string | null
  isRefreshing: boolean
  output: McpServerListOutput | null
  status: 'loading' | 'ready' | 'error'
}

export type McpServerPendingOperation =
  | 'authorization'
  | 'delete'
  | 'disable'
  | 'enable'
  | 'refreshCatalog'
  | 'restart'
  | 'start'
  | 'stop'
  | 'update'

export interface McpToolCatalogState {
  catalogCompleteness?: McpCatalogCompletenessView
  catalogGeneration?: number
  errorMessage: string | null
  isRefreshing: boolean
  nextCursor?: string
  serverConfigDigest?: string
  serverConfigEpoch?: string
  serverRegistryRevision?: number
  status: 'idle' | 'loading' | 'ready' | 'error'
  tools: readonly McpToolSummaryView[]
}
