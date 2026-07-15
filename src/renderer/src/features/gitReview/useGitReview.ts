import { useCallback, useEffect, useRef, useState } from 'react'
import type {
  GitReviewFileDiff,
  GitReviewFileMutationAction,
  GitReviewScope,
  GitReviewSummary
} from '@mycopilot/protocol'
import { getGitReviewFileDiff, getGitReviewSummary, mutateGitReviewFile } from './gitReviewClient'

type GitReviewSummaryState =
  | { status: 'idle' | 'loading'; value?: GitReviewSummary }
  | { error: string; status: 'error'; value?: GitReviewSummary }
  | { status: 'ready'; value: GitReviewSummary }

export type GitReviewDiffState =
  | { status: 'idle' }
  | { status: 'loading' }
  | { error: string; status: 'error' }
  | { status: 'ready'; value: GitReviewFileDiff }

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

export function useGitReview(projectId: string, isActive: boolean) {
  const [scope, setScope] = useState<GitReviewScope>('unstaged')
  const [summaryState, setSummaryState] = useState<GitReviewSummaryState>({ status: 'idle' })
  const [diffStates, setDiffStates] = useState<Record<string, GitReviewDiffState>>({})
  const [mutationError, setMutationError] = useState<string | null>(null)
  const [pendingFileId, setPendingFileId] = useState<string | null>(null)
  const summaryRequestRef = useRef(0)
  const summaryQueryRef = useRef('')
  const diffStatesRef = useRef(diffStates)
  const diffRequestsRef = useRef(new Map<string, string>())
  const mutationRequestRef = useRef<string | null>(null)

  useEffect(() => {
    diffStatesRef.current = diffStates
  }, [diffStates])

  useEffect(() => {
    mutationRequestRef.current = null
    setMutationError(null)
    setPendingFileId(null)
  }, [projectId, scope])

  const refresh = useCallback(async () => {
    if (!projectId) return
    const requestId = summaryRequestRef.current + 1
    const queryKey = `${projectId}:${scope}`
    const preserveCurrentSummary = summaryQueryRef.current === queryKey
    summaryRequestRef.current = requestId
    summaryQueryRef.current = queryKey
    setSummaryState((current) => ({
      status: 'loading',
      value: preserveCurrentSummary ? current.value : undefined
    }))
    setDiffStates({})
    diffStatesRef.current = {}
    diffRequestsRef.current.clear()

    try {
      const summary = await getGitReviewSummary({ projectId, scope })
      if (summaryRequestRef.current !== requestId) return
      setSummaryState({ status: 'ready', value: summary })
    } catch (error) {
      if (summaryRequestRef.current !== requestId) return
      setSummaryState((current) => ({
        error: errorMessage(error),
        status: 'error',
        value: current.value
      }))
    }
  }, [projectId, scope])

  useEffect(() => {
    if (!isActive) return
    void refresh()
  }, [isActive, refresh])

  useEffect(() => {
    if (!isActive) return undefined
    const handleFocus = (): void => {
      void refresh()
    }
    window.addEventListener('focus', handleFocus)
    return () => window.removeEventListener('focus', handleFocus)
  }, [isActive, refresh])

  const loadFileDiff = useCallback(
    async (fileId: string) => {
      const summary = summaryState.value
      if (!summary) return
      const existing = diffStatesRef.current[fileId]
      if (existing?.status === 'loading' || existing?.status === 'ready') return

      const requestKey = `${summary.snapshotId}:${crypto.randomUUID()}`
      diffRequestsRef.current.set(fileId, requestKey)
      const loadingState = { status: 'loading' } as const
      diffStatesRef.current = { ...diffStatesRef.current, [fileId]: loadingState }
      setDiffStates(diffStatesRef.current)
      try {
        const diff = await getGitReviewFileDiff({ fileId, snapshotId: summary.snapshotId })
        if (diffRequestsRef.current.get(fileId) !== requestKey) return
        if (diff.status === 'snapshotExpired') {
          diffRequestsRef.current.delete(fileId)
          void refresh()
          return
        }
        diffStatesRef.current = {
          ...diffStatesRef.current,
          [fileId]: { status: 'ready', value: diff }
        }
        setDiffStates(diffStatesRef.current)
      } catch (error) {
        if (diffRequestsRef.current.get(fileId) !== requestKey) return
        diffRequestsRef.current.delete(fileId)
        diffStatesRef.current = {
          ...diffStatesRef.current,
          [fileId]: { error: errorMessage(error), status: 'error' }
        }
        setDiffStates(diffStatesRef.current)
      }
    },
    [refresh, summaryState.value]
  )

  const mutateFile = useCallback(
    async (fileId: string, action: GitReviewFileMutationAction) => {
      const summary = summaryState.value
      if (!summary || summary.scope !== scope || mutationRequestRef.current) return

      const requestKey = `${summary.snapshotId}:${fileId}:${crypto.randomUUID()}`
      mutationRequestRef.current = requestKey
      setMutationError(null)
      setPendingFileId(fileId)
      try {
        await mutateGitReviewFile({
          action,
          fileId,
          snapshotId: summary.snapshotId
        })
        if (mutationRequestRef.current !== requestKey) return
        await refresh()
      } catch (error) {
        if (mutationRequestRef.current !== requestKey) return
        setMutationError(errorMessage(error))
        throw error
      } finally {
        if (mutationRequestRef.current === requestKey) {
          mutationRequestRef.current = null
          setPendingFileId(null)
        }
      }
    },
    [refresh, scope, summaryState.value]
  )

  return {
    dismissMutationError: () => setMutationError(null),
    diffStates,
    loadFileDiff,
    mutateFile,
    mutationError,
    pendingFileId,
    refresh,
    scope,
    setScope,
    summaryState
  }
}
