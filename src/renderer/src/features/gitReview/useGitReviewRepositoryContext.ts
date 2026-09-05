import { useCallback, useEffect, useRef, useState } from 'react'
import type { GitReviewRepositoryContext } from '@mycopilot/protocol'
import { getGitReviewRepositoryContext } from './gitReviewClient'

export type GitReviewRepositoryContextState =
  | { status: 'idle' }
  | { status: 'loading' }
  | { error: string; status: 'error' }
  | { status: 'ready'; value: GitReviewRepositoryContext }

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

export function useGitReviewRepositoryContext(projectId: string) {
  const [state, setState] = useState<GitReviewRepositoryContextState>({ status: 'idle' })
  const stateRef = useRef(state)
  const requestRef = useRef(0)
  const projectRef = useRef(projectId)

  useEffect(() => {
    projectRef.current = projectId
    requestRef.current += 1
    const next = { status: 'idle' } as const
    stateRef.current = next
    setState(next)
  }, [projectId])

  useEffect(
    () => () => {
      requestRef.current += 1
    },
    []
  )

  useEffect(() => {
    stateRef.current = state
  }, [state])

  const load = useCallback(
    async (force = false): Promise<void> => {
      if (!projectId) return
      if (
        !force &&
        (stateRef.current.status === 'loading' || stateRef.current.status === 'ready')
      ) {
        return
      }
      const requestId = requestRef.current + 1
      requestRef.current = requestId
      const loading = { status: 'loading' } as const
      stateRef.current = loading
      setState(loading)
      try {
        const value = await getGitReviewRepositoryContext({ projectId })
        if (requestRef.current !== requestId || projectRef.current !== projectId) return
        const ready = { status: 'ready', value } as const
        stateRef.current = ready
        setState(ready)
      } catch (error) {
        if (requestRef.current !== requestId || projectRef.current !== projectId) return
        const failed = { error: errorMessage(error), status: 'error' } as const
        stateRef.current = failed
        setState(failed)
      }
    },
    [projectId]
  )

  return { load, state }
}
