// Renderer startup layer: coordinates Core readiness and extensible application bootstrap stages.

import { useCallback, useEffect, useMemo, useState } from 'react'
import type { ReactNode } from 'react'
import { hostClient } from '../../host/hostClient'
import { AppStartupReporterContext, AppStartupStatusContext } from './AppStartupContext'
import type {
  AppStartupReporterContextValue,
  AppStartupState,
  AppStartupStatusContextValue
} from './AppStartupContext'
import {
  createPendingStartupStages,
  hasBlockingStartupFailure,
  isBlockingStartupReady
} from './appStartupStages'

export function AppStartupProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<AppStartupState>(() => ({
    attempt: 0,
    startedAt: Date.now(),
    stages: createPendingStartupStages()
  }))

  const reportStage = useCallback<AppStartupReporterContextValue['reportStage']>(
    (attempt, stageId, status) => {
      setState((current) => {
        // A retry owns a new attempt. Responses from abandoned Core or storage requests must not
        // unlock the workspace or overwrite the failure state of the current startup attempt.
        if (current.attempt !== attempt) return current
        const previous = current.stages[stageId]
        if (previous.status === status) return current
        return {
          ...current,
          stages: {
            ...current.stages,
            [stageId]: { status }
          }
        }
      })
    },
    []
  )

  const retry = useCallback(() => {
    setState((current) => ({
      attempt: current.attempt + 1,
      startedAt: Date.now(),
      stages: createPendingStartupStages()
    }))
  }, [])

  useEffect(() => {
    const attempt = state.attempt
    let cancelled = false
    reportStage(attempt, 'core', 'pending')
    void hostClient.core
      .ping()
      .then(() => {
        if (!cancelled) reportStage(attempt, 'core', 'ready')
      })
      .catch(() => {
        if (!cancelled) {
          reportStage(attempt, 'core', 'failed')
        }
      })

    return () => {
      cancelled = true
    }
  }, [reportStage, state.attempt])

  const reporterValue = useMemo<AppStartupReporterContextValue>(
    () => ({ attempt: state.attempt, reportStage }),
    [reportStage, state.attempt]
  )
  const statusValue = useMemo<AppStartupStatusContextValue>(
    () => ({
      ...state,
      hasFailed: hasBlockingStartupFailure(state.stages),
      ready: isBlockingStartupReady(state.stages),
      retry
    }),
    [retry, state]
  )

  return (
    <AppStartupReporterContext.Provider value={reporterValue}>
      <AppStartupStatusContext.Provider value={statusValue}>
        {children}
      </AppStartupStatusContext.Provider>
    </AppStartupReporterContext.Provider>
  )
}
