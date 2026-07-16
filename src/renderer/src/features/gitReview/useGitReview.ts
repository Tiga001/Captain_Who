import { useCallback, useEffect, useRef, useState } from 'react'
import type {
  GitReviewFileContent,
  GitReviewFileDiff,
  GitReviewFileMutationAction,
  GitReviewScope,
  GitReviewSummary
} from '@mycopilot/protocol'
import {
  getGitReviewFileContent,
  getGitReviewFileDiff,
  getGitReviewSummary,
  mutateGitReviewFile
} from './gitReviewClient'

const MAX_CONCURRENT_FULL_CONTENT_REQUESTS = 4
const MAX_CACHED_FULL_CONTENT_FILES = 24

type GitReviewSummaryState =
  | { status: 'idle' | 'loading'; value?: GitReviewSummary }
  | { error: string; status: 'error'; value?: GitReviewSummary }
  | { status: 'ready'; value: GitReviewSummary }

export type GitReviewDiffState =
  | { status: 'idle' }
  | { status: 'loading' }
  | { error: string; status: 'error' }
  | { status: 'ready'; value: GitReviewFileDiff }

export type GitReviewFileContentState =
  | { status: 'idle' }
  | { status: 'loading' }
  | { error: string; status: 'error' }
  | { status: 'ready'; value: GitReviewFileContent }

interface FullContentQueueItem {
  fileId: string
  requestKey: string
  snapshotId: string
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

export function useGitReview(projectId: string, isActive: boolean) {
  const [scope, setScope] = useState<GitReviewScope>('unstaged')
  const [summaryState, setSummaryState] = useState<GitReviewSummaryState>({ status: 'idle' })
  const [diffStates, setDiffStates] = useState<Record<string, GitReviewDiffState>>({})
  const [fileContentStates, setFileContentStates] = useState<
    Record<string, GitReviewFileContentState>
  >({})
  const [mutationError, setMutationError] = useState<string | null>(null)
  const [pendingFileId, setPendingFileId] = useState<string | null>(null)
  const summaryRequestRef = useRef(0)
  const summaryQueryRef = useRef('')
  const diffStatesRef = useRef(diffStates)
  const diffRequestsRef = useRef(new Map<string, string>())
  const fileContentStatesRef = useRef(fileContentStates)
  const fullContentRequestsRef = useRef(new Map<string, string>())
  const fullContentQueueRef = useRef<FullContentQueueItem[]>([])
  const activeFullContentRequestsRef = useRef(0)
  const fullContentLruRef = useRef(new Map<string, true>())
  const drainFullContentQueueRef = useRef<() => void>(() => undefined)
  const mutationRequestRef = useRef<string | null>(null)

  useEffect(() => {
    diffStatesRef.current = diffStates
  }, [diffStates])

  useEffect(() => {
    fileContentStatesRef.current = fileContentStates
  }, [fileContentStates])

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
    setFileContentStates({})
    fileContentStatesRef.current = {}
    fullContentRequestsRef.current.clear()
    fullContentQueueRef.current = []
    fullContentLruRef.current.clear()

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

  const drainFullContentQueue = useCallback(() => {
    while (
      activeFullContentRequestsRef.current < MAX_CONCURRENT_FULL_CONTENT_REQUESTS &&
      fullContentQueueRef.current.length > 0
    ) {
      const item = fullContentQueueRef.current.shift()
      if (!item || fullContentRequestsRef.current.get(item.fileId) !== item.requestKey) continue

      activeFullContentRequestsRef.current += 1
      void getGitReviewFileContent({ fileId: item.fileId, snapshotId: item.snapshotId })
        .then((content) => {
          if (fullContentRequestsRef.current.get(item.fileId) !== item.requestKey) return
          if (content.fileId !== item.fileId || content.snapshotId !== item.snapshotId) {
            throw new Error('Git returned full content for a different review snapshot.')
          }
          fullContentRequestsRef.current.delete(item.fileId)
          if (content.status === 'snapshotExpired') {
            void refresh()
            return
          }

          const nextStates = {
            ...fileContentStatesRef.current,
            [item.fileId]: { status: 'ready', value: content } as const
          }
          fullContentLruRef.current.delete(item.fileId)
          fullContentLruRef.current.set(item.fileId, true)
          while (fullContentLruRef.current.size > MAX_CACHED_FULL_CONTENT_FILES) {
            const oldestFileId = fullContentLruRef.current.keys().next().value
            if (typeof oldestFileId !== 'string') break
            fullContentLruRef.current.delete(oldestFileId)
            delete nextStates[oldestFileId]
          }
          fileContentStatesRef.current = nextStates
          setFileContentStates(nextStates)
        })
        .catch((error) => {
          if (fullContentRequestsRef.current.get(item.fileId) !== item.requestKey) return
          fullContentRequestsRef.current.delete(item.fileId)
          const nextStates = {
            ...fileContentStatesRef.current,
            [item.fileId]: { error: errorMessage(error), status: 'error' } as const
          }
          fileContentStatesRef.current = nextStates
          setFileContentStates(nextStates)
        })
        .finally(() => {
          activeFullContentRequestsRef.current = Math.max(
            0,
            activeFullContentRequestsRef.current - 1
          )
          drainFullContentQueueRef.current()
        })
    }
  }, [refresh])

  useEffect(() => {
    drainFullContentQueueRef.current = drainFullContentQueue
  }, [drainFullContentQueue])

  const loadFileContent = useCallback(
    (fileId: string) => {
      const summary = summaryState.value
      if (!summary) return
      const existing = fileContentStatesRef.current[fileId]
      if (existing?.status === 'loading' || existing?.status === 'ready') return

      const requestKey = `${summary.snapshotId}:${fileId}:${crypto.randomUUID()}`
      fullContentRequestsRef.current.set(fileId, requestKey)
      const loadingState = { status: 'loading' } as const
      fileContentStatesRef.current = {
        ...fileContentStatesRef.current,
        [fileId]: loadingState
      }
      setFileContentStates(fileContentStatesRef.current)
      fullContentQueueRef.current.push({ fileId, requestKey, snapshotId: summary.snapshotId })
      drainFullContentQueueRef.current()
    },
    [summaryState.value]
  )

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
    fileContentStates,
    loadFileContent,
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
