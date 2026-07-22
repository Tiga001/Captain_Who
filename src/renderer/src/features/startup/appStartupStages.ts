// Renderer startup layer: declares the blocking stages required before the workspace is interactive.

export const APP_STARTUP_STAGE_DEFINITIONS = [
  { id: 'core', blocking: true },
  { id: 'modelSettings', blocking: true },
  { id: 'projects', blocking: true },
  { id: 'uiPreferences', blocking: true },
  { id: 'composerDrafts', blocking: true },
  { id: 'conversationMetas', blocking: true }
] as const

export type AppStartupStageId = (typeof APP_STARTUP_STAGE_DEFINITIONS)[number]['id']
export type AppStartupStageStatus = 'pending' | 'ready' | 'failed'

export interface AppStartupStageState {
  error?: string
  status: AppStartupStageStatus
}

export type AppStartupStages = Record<AppStartupStageId, AppStartupStageState>

export function createPendingStartupStages(): AppStartupStages {
  return Object.fromEntries(
    APP_STARTUP_STAGE_DEFINITIONS.map(({ id }) => [id, { status: 'pending' }])
  ) as AppStartupStages
}

export function isBlockingStartupReady(stages: AppStartupStages): boolean {
  return APP_STARTUP_STAGE_DEFINITIONS.every(
    ({ blocking, id }) => !blocking || stages[id].status === 'ready'
  )
}

export function hasBlockingStartupFailure(stages: AppStartupStages): boolean {
  return APP_STARTUP_STAGE_DEFINITIONS.some(
    ({ blocking, id }) => blocking && stages[id].status === 'failed'
  )
}
