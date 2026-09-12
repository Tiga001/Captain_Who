import { useEffect, useState } from 'react'
import type { GitRepositoryInspection } from '@mycopilot/protocol'
import { inspectGitRepository } from './gitReviewClient'

export function useGitRepositorySources(projectId: string, revision: string, enabled: boolean) {
  const key = JSON.stringify([projectId, revision])
  const [result, setResult] = useState<{
    key: string
    inspection?: GitRepositoryInspection
    error?: string
  } | null>(null)
  useEffect(() => {
    if (!enabled || !projectId) return
    let cancelled = false
    void inspectGitRepository(projectId)
      .then((inspection) => {
        if (cancelled) return
        if (inspection.projectId !== projectId) throw new Error('Git returned a different project.')
        setResult({ key, inspection })
      })
      .catch((error: unknown) => {
        if (!cancelled)
          setResult({ key, error: error instanceof Error ? error.message : String(error) })
      })
    return () => {
      cancelled = true
    }
  }, [enabled, key, projectId])
  return result?.key === key ? result : null
}
