// Renderer startup layer: exposes attempt-scoped stage reporting without requiring Electron.

import { createContext, useContext, useMemo } from 'react'
import type { AppStartupStageId, AppStartupStages, AppStartupStageStatus } from './appStartupStages'

export interface AppStartupState {
  attempt: number
  startedAt: number
  stages: AppStartupStages
}

export interface AppStartupReporterContextValue {
  attempt: number
  reportStage(attempt: number, stageId: AppStartupStageId, status: AppStartupStageStatus): void
}

export interface AppStartupStatusContextValue extends AppStartupState {
  hasFailed: boolean
  ready: boolean
  retry(): void
}

export const AppStartupReporterContext = createContext<AppStartupReporterContextValue | null>(null)
export const AppStartupStatusContext = createContext<AppStartupStatusContextValue | null>(null)

const NOOP_REPORT_STAGE: AppStartupReporterContextValue['reportStage'] = () => undefined

export function useAppStartupStage(stageId: AppStartupStageId) {
  const context = useContext(AppStartupReporterContext)
  const attempt = context?.attempt ?? 0
  const reportStage = context?.reportStage ?? NOOP_REPORT_STAGE

  return useMemo(
    () => ({
      attempt,
      // Startup state records only lifecycle status. Product copy is selected by the gate from the
      // translation catalog, so raw backend or exception text never becomes renderer state.
      markFailed: (error?: unknown) => {
        void error
        reportStage(attempt, stageId, 'failed')
      },
      markPending: () => reportStage(attempt, stageId, 'pending'),
      markReady: () => reportStage(attempt, stageId, 'ready')
    }),
    [attempt, reportStage, stageId]
  )
}

export function useAppStartupStatus(): AppStartupStatusContextValue | null {
  return useContext(AppStartupStatusContext)
}
