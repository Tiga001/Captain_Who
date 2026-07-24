import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
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
import {
  GitReviewDiffRequestQueue,
  type GitReviewDiffPriority,
  type GitReviewDiffQueueItem
} from './gitReviewDiffRequestQueue'
import { GitReviewPayloadCacheBudget } from './gitReviewPayloadCacheBudget'

const MAX_CONCURRENT_DIFF_REQUESTS = 4
const MAX_QUEUED_DIFF_REQUESTS = 64
const MAX_CACHED_DIFF_FILES = 64
const MAX_CACHED_DIFF_CHARACTERS = 24 * 1024 * 1024
const MAX_CONCURRENT_FULL_CONTENT_REQUESTS = 4
const MAX_QUEUED_FULL_CONTENT_REQUESTS = 16
const MAX_CACHED_FULL_CONTENT_FILES = 24
const MAX_CACHED_FULL_CONTENT_CHARACTERS = 12 * 1024 * 1024

type GitReviewSummaryState =
  | { status: 'idle' | 'loading'; value?: GitReviewSummary }
  | { error: string; status: 'error'; value?: GitReviewSummary }
  | { status: 'ready'; value: GitReviewSummary }

export type GitReviewDiffState =
  | { status: 'idle' }
  | { status: 'loading' }
  | { error: string; reason?: 'snapshotExpired'; status: 'error' }
  | { status: 'ready'; value: GitReviewFileDiff }

export type GitReviewFileContentState =
  | { status: 'idle' }
  | { status: 'loading' }
  | { error: string; reason?: 'snapshotExpired'; status: 'error' }
  | { status: 'ready'; value: GitReviewFileContent }

interface FullContentQueueItem {
  fileId: string
  requestKey: string
  snapshotId: string
}

interface FullContentRequestRecord extends FullContentQueueItem {
  phase: 'queued' | 'running'
}

interface DiffRequestRecord extends GitReviewDiffQueueItem {
  phase: 'queued' | 'running'
}

interface RequestableSnapshot {
  conversationId: string | null
  projectId: string
  scope: GitReviewScope
  summary: GitReviewSummary
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

export function useGitReview(
  projectId: string,
  isActive: boolean,
  conversationId?: string | null,
  initialScope: GitReviewScope = 'unstaged'
) {
  const [scope, setScope] = useState<GitReviewScope>(initialScope)
  const lastTurnConversationId = scope === 'lastTurn' ? (conversationId ?? null) : null
  const [summaryState, setSummaryState] = useState<GitReviewSummaryState>({ status: 'idle' })
  const [diffStates, setDiffStates] = useState<Record<string, GitReviewDiffState>>({})
  const [fileContentStates, setFileContentStates] = useState<
    Record<string, GitReviewFileContentState>
  >({})
  const [mutationError, setMutationError] = useState<string | null>(null)
  const [pendingFileId, setPendingFileId] = useState<string | null>(null)
  const summaryRequestRef = useRef(0)
  const summaryQueryRef = useRef('')
  const requestableSnapshotRef = useRef<RequestableSnapshot | null>(null)
  const refreshFallbackRef = useRef<RequestableSnapshot | null>(null)
  const diffStatesRef = useRef(diffStates)
  const diffRequestsRef = useRef(new Map<string, DiffRequestRecord>())
  const diffQueueRef = useRef(new GitReviewDiffRequestQueue(MAX_QUEUED_DIFF_REQUESTS))
  const diffCacheBudgetRef = useRef(
    new GitReviewPayloadCacheBudget({
      maxCharacters: MAX_CACHED_DIFF_CHARACTERS,
      maxFiles: MAX_CACHED_DIFF_FILES
    })
  )
  const activeDiffRequestsRef = useRef(0)
  const hotDiffFileIdsRef = useRef<ReadonlySet<string>>(new Set())
  const drainDiffQueueRef = useRef<() => void>(() => undefined)
  const fileContentStatesRef = useRef(fileContentStates)
  const fullContentRequestsRef = useRef(new Map<string, FullContentRequestRecord>())
  const fullContentQueueRef = useRef<FullContentQueueItem[]>([])
  const activeFullContentRequestsRef = useRef(0)
  const fullContentCacheBudgetRef = useRef(
    new GitReviewPayloadCacheBudget({
      maxCharacters: MAX_CACHED_FULL_CONTENT_CHARACTERS,
      maxFiles: MAX_CACHED_FULL_CONTENT_FILES
    })
  )
  const hotFullContentFileIdsRef = useRef<ReadonlySet<string>>(new Set())
  const drainFullContentQueueRef = useRef<() => void>(() => undefined)
  const mutationRequestRef = useRef<string | null>(null)
  const isActiveRef = useRef(isActive)

  // Commit activity/context before the panel's passive demand reconciliation without mutating
  // transport ownership from a React render that may later be interrupted.
  useLayoutEffect(() => {
    isActiveRef.current = isActive
    if (
      requestableSnapshotRef.current &&
      (requestableSnapshotRef.current.projectId !== projectId ||
        requestableSnapshotRef.current.scope !== scope ||
        requestableSnapshotRef.current.conversationId !== lastTurnConversationId)
    ) {
      requestableSnapshotRef.current = null
      refreshFallbackRef.current = null
    }
  }, [isActive, lastTurnConversationId, projectId, scope])

  useEffect(
    () => () => {
      isActiveRef.current = false
      requestableSnapshotRef.current = null
      refreshFallbackRef.current = null
      summaryRequestRef.current += 1
      diffRequestsRef.current.clear()
      diffQueueRef.current.clear()
      diffCacheBudgetRef.current.clear()
      hotDiffFileIdsRef.current = new Set()
      fullContentRequestsRef.current.clear()
      fullContentQueueRef.current = []
      fullContentCacheBudgetRef.current.clear()
      hotFullContentFileIdsRef.current = new Set()
      mutationRequestRef.current = null
    },
    []
  )

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
  }, [lastTurnConversationId, projectId, scope])

  const resetReviewData = useCallback(() => {
    setDiffStates({})
    diffStatesRef.current = {}
    diffRequestsRef.current.clear()
    diffQueueRef.current.clear()
    diffCacheBudgetRef.current.clear()
    hotDiffFileIdsRef.current = new Set()
    setFileContentStates({})
    fileContentStatesRef.current = {}
    fullContentRequestsRef.current.clear()
    fullContentQueueRef.current = []
    fullContentCacheBudgetRef.current.clear()
    hotFullContentFileIdsRef.current = new Set()
  }, [])

  const discardOrphanedDiffLoadingState = useCallback((fileId: string) => {
    if (diffStatesRef.current[fileId]?.status !== 'loading') return
    const nextStates = { ...diffStatesRef.current }
    delete nextStates[fileId]
    diffStatesRef.current = nextStates
    setDiffStates(nextStates)
  }, [])

  const discardOrphanedContentLoadingState = useCallback((fileId: string) => {
    if (fileContentStatesRef.current[fileId]?.status !== 'loading') return
    const nextStates = { ...fileContentStatesRef.current }
    delete nextStates[fileId]
    fileContentStatesRef.current = nextStates
    setFileContentStates(nextStates)
  }, [])

  const refresh = useCallback(async () => {
    if (!projectId) return
    const requestId = summaryRequestRef.current + 1
    const queryKey = `${projectId}:${scope}:${lastTurnConversationId ?? ''}`
    const sameQuery = summaryQueryRef.current === queryKey
    const fallbackSnapshot = sameQuery
      ? (requestableSnapshotRef.current ?? refreshFallbackRef.current)
      : null
    const preserveCurrentReview = fallbackSnapshot !== null
    summaryRequestRef.current = requestId
    summaryQueryRef.current = queryKey
    requestableSnapshotRef.current = null
    refreshFallbackRef.current = fallbackSnapshot
    setSummaryState((current) => ({
      status: 'loading',
      value: preserveCurrentReview ? current.value : undefined
    }))
    if (!preserveCurrentReview) resetReviewData()

    try {
      const summary = await getGitReviewSummary({
        ...(lastTurnConversationId ? { conversationId: lastTurnConversationId } : {}),
        projectId,
        scope
      })
      if (summaryRequestRef.current !== requestId) return
      if (preserveCurrentReview) resetReviewData()
      refreshFallbackRef.current = null
      requestableSnapshotRef.current = {
        conversationId: lastTurnConversationId,
        projectId,
        scope,
        summary
      }
      setSummaryState({ status: 'ready', value: summary })
    } catch (error) {
      if (summaryRequestRef.current !== requestId) return
      requestableSnapshotRef.current = refreshFallbackRef.current
      refreshFallbackRef.current = null
      setSummaryState((current) => ({
        error: errorMessage(error),
        status: 'error',
        value: current.value
      }))
      if (isActiveRef.current && requestableSnapshotRef.current) {
        drainDiffQueueRef.current()
        drainFullContentQueueRef.current()
      }
    }
  }, [lastTurnConversationId, projectId, resetReviewData, scope])

  const drainFullContentQueue = useCallback(() => {
    while (
      isActiveRef.current &&
      requestableSnapshotRef.current !== null &&
      activeFullContentRequestsRef.current < MAX_CONCURRENT_FULL_CONTENT_REQUESTS &&
      fullContentQueueRef.current.length > 0
    ) {
      const item = fullContentQueueRef.current.shift()
      if (!item) continue
      const record = fullContentRequestsRef.current.get(item.fileId)
      if (!record || record.requestKey !== item.requestKey || record.phase !== 'queued') continue

      record.phase = 'running'
      activeFullContentRequestsRef.current += 1
      void getGitReviewFileContent({ fileId: item.fileId, snapshotId: item.snapshotId })
        .then((content) => {
          const current = fullContentRequestsRef.current.get(item.fileId)
          if (!current || current.requestKey !== item.requestKey) return
          if (content.fileId !== item.fileId || content.snapshotId !== item.snapshotId) {
            throw new Error('Git returned full content for a different review snapshot.')
          }
          fullContentRequestsRef.current.delete(item.fileId)
          if (!isActiveRef.current) {
            discardOrphanedContentLoadingState(item.fileId)
            return
          }
          if (content.status === 'snapshotExpired') {
            const nextStates = {
              ...fileContentStatesRef.current,
              [item.fileId]: {
                error: 'snapshotExpired',
                reason: 'snapshotExpired',
                status: 'error'
              } as const
            }
            fileContentStatesRef.current = nextStates
            setFileContentStates(nextStates)
            void refresh()
            return
          }

          const nextStates = {
            ...fileContentStatesRef.current,
            [item.fileId]: { status: 'ready', value: content } as const
          }
          const evictedFileIds = fullContentCacheBudgetRef.current.record(
            item.fileId,
            (content.beforeText?.length ?? 0) + (content.afterText?.length ?? 0),
            hotFullContentFileIdsRef.current
          )
          for (const evictedFileId of evictedFileIds) delete nextStates[evictedFileId]
          fileContentStatesRef.current = nextStates
          setFileContentStates(nextStates)
        })
        .catch((error) => {
          const current = fullContentRequestsRef.current.get(item.fileId)
          if (!current || current.requestKey !== item.requestKey) return
          fullContentRequestsRef.current.delete(item.fileId)
          if (!isActiveRef.current) {
            discardOrphanedContentLoadingState(item.fileId)
            return
          }
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
  }, [discardOrphanedContentLoadingState, refresh])

  useEffect(() => {
    drainFullContentQueueRef.current = drainFullContentQueue
  }, [drainFullContentQueue])

  const loadFileContent = useCallback(
    (fileId: string) => {
      if (!isActiveRef.current) return
      const requestable = requestableSnapshotRef.current
      if (
        !requestable ||
        requestable.projectId !== projectId ||
        requestable.scope !== scope ||
        requestable.conversationId !== lastTurnConversationId
      ) {
        return
      }
      const summary = requestable.summary
      const existing = fileContentStatesRef.current[fileId]
      if (existing?.status === 'loading') return
      if (existing?.status === 'ready') {
        fullContentCacheBudgetRef.current.touch(fileId)
        return
      }
      if (existing?.status === 'error' || fullContentRequestsRef.current.has(fileId)) return
      if (fullContentQueueRef.current.length >= MAX_QUEUED_FULL_CONTENT_REQUESTS) return

      const requestKey = `${summary.snapshotId}:${fileId}:${crypto.randomUUID()}`
      const item: FullContentQueueItem = {
        fileId,
        requestKey,
        snapshotId: summary.snapshotId
      }
      fullContentRequestsRef.current.set(fileId, { ...item, phase: 'queued' })
      const loadingState = { status: 'loading' } as const
      fileContentStatesRef.current = {
        ...fileContentStatesRef.current,
        [fileId]: loadingState
      }
      setFileContentStates(fileContentStatesRef.current)
      fullContentQueueRef.current.push(item)
      drainFullContentQueueRef.current()
    },
    [lastTurnConversationId, projectId, scope]
  )

  const cancelQueuedFileContentsExcept = useCallback((keepFileIds: ReadonlySet<string>) => {
    let nextStates: Record<string, GitReviewFileContentState> | null = null
    for (const [fileId, record] of fullContentRequestsRef.current) {
      if (record.phase !== 'queued' || keepFileIds.has(fileId)) continue
      fullContentRequestsRef.current.delete(fileId)
      if (fileContentStatesRef.current[fileId]?.status === 'loading') {
        nextStates ??= { ...fileContentStatesRef.current }
        delete nextStates[fileId]
      }
    }
    fullContentQueueRef.current = fullContentQueueRef.current.filter((item) => {
      const record = fullContentRequestsRef.current.get(item.fileId)
      return record?.requestKey === item.requestKey && record.phase === 'queued'
    })
    if (nextStates) {
      fileContentStatesRef.current = nextStates
      setFileContentStates(nextStates)
    }
  }, [])

  const setHotFullContentFileIds = useCallback((fileIds: ReadonlySet<string>) => {
    const hotFileIds = new Set(fileIds)
    hotFullContentFileIdsRef.current = hotFileIds
    const evictedFileIds = fullContentCacheBudgetRef.current.trim(hotFileIds)
    if (evictedFileIds.length === 0) return

    const nextStates = { ...fileContentStatesRef.current }
    for (const fileId of evictedFileIds) delete nextStates[fileId]
    fileContentStatesRef.current = nextStates
    setFileContentStates(nextStates)
  }, [])

  useEffect(() => {
    if (!isActive) return undefined
    const handleFocus = (): void => {
      void refresh()
    }
    window.addEventListener('focus', handleFocus)
    return () => window.removeEventListener('focus', handleFocus)
  }, [isActive, refresh])

  const drainDiffQueue = useCallback(() => {
    while (
      isActiveRef.current &&
      requestableSnapshotRef.current !== null &&
      activeDiffRequestsRef.current < MAX_CONCURRENT_DIFF_REQUESTS &&
      diffQueueRef.current.size > 0
    ) {
      const item = diffQueueRef.current.shift()
      if (!item) break
      const record = diffRequestsRef.current.get(item.fileId)
      if (!record || record.requestKey !== item.requestKey || record.phase !== 'queued') continue

      record.phase = 'running'
      activeDiffRequestsRef.current += 1
      void getGitReviewFileDiff({ fileId: item.fileId, snapshotId: item.snapshotId })
        .then((diff) => {
          const current = diffRequestsRef.current.get(item.fileId)
          if (!current || current.requestKey !== item.requestKey) return
          if (diff.fileId !== item.fileId || diff.snapshotId !== item.snapshotId) {
            throw new Error('Git returned a diff for a different review snapshot.')
          }
          diffRequestsRef.current.delete(item.fileId)
          if (!isActiveRef.current) {
            discardOrphanedDiffLoadingState(item.fileId)
            return
          }
          if (diff.status === 'snapshotExpired') {
            const nextStates = {
              ...diffStatesRef.current,
              [item.fileId]: {
                error: 'snapshotExpired',
                reason: 'snapshotExpired',
                status: 'error'
              } as const
            }
            diffStatesRef.current = nextStates
            setDiffStates(nextStates)
            void refresh()
            return
          }
          const evictedFileIds = diffCacheBudgetRef.current.record(
            item.fileId,
            diff.patch?.length ?? 0,
            hotDiffFileIdsRef.current
          )
          const nextStates: Record<string, GitReviewDiffState> = {
            ...diffStatesRef.current,
            [item.fileId]: { status: 'ready', value: diff }
          }
          for (const evictedFileId of evictedFileIds) delete nextStates[evictedFileId]
          diffStatesRef.current = nextStates
          setDiffStates(nextStates)

          if (evictedFileIds.length > 0) {
            const nextContentStates = { ...fileContentStatesRef.current }
            for (const evictedFileId of evictedFileIds) {
              delete nextContentStates[evictedFileId]
              fullContentCacheBudgetRef.current.remove(evictedFileId)
            }
            fileContentStatesRef.current = nextContentStates
            setFileContentStates(nextContentStates)
          }
        })
        .catch((error) => {
          const current = diffRequestsRef.current.get(item.fileId)
          if (!current || current.requestKey !== item.requestKey) return
          diffRequestsRef.current.delete(item.fileId)
          if (!isActiveRef.current) {
            discardOrphanedDiffLoadingState(item.fileId)
            return
          }
          diffStatesRef.current = {
            ...diffStatesRef.current,
            [item.fileId]: { error: errorMessage(error), status: 'error' }
          }
          setDiffStates(diffStatesRef.current)
        })
        .finally(() => {
          activeDiffRequestsRef.current = Math.max(0, activeDiffRequestsRef.current - 1)
          drainDiffQueueRef.current()
        })
    }
  }, [discardOrphanedDiffLoadingState, refresh])

  useEffect(() => {
    drainDiffQueueRef.current = drainDiffQueue
  }, [drainDiffQueue])

  const requestFileDiff = useCallback(
    (fileId: string, priority: GitReviewDiffPriority, retryError: boolean) => {
      if (!isActiveRef.current) return
      const requestable = requestableSnapshotRef.current
      if (
        !requestable ||
        requestable.projectId !== projectId ||
        requestable.scope !== scope ||
        requestable.conversationId !== lastTurnConversationId
      ) {
        return
      }
      const summary = requestable.summary
      const existingState = diffStatesRef.current[fileId]
      if (existingState?.status === 'ready') {
        diffCacheBudgetRef.current.touch(fileId)
        return
      }
      if (existingState?.status === 'error' && !retryError) return

      const existingRequest = diffRequestsRef.current.get(fileId)
      if (existingRequest) {
        if (priority === 'high' && existingRequest.phase === 'queued') {
          existingRequest.priority = 'high'
          diffQueueRef.current.promote(fileId, existingRequest.requestKey)
        }
        return
      }

      const requestKey = `${summary.snapshotId}:${fileId}:${crypto.randomUUID()}`
      const item: GitReviewDiffQueueItem = {
        fileId,
        priority,
        requestKey,
        snapshotId: summary.snapshotId
      }
      const enqueueResult = diffQueueRef.current.enqueue(item)
      if (!enqueueResult.accepted) return

      const nextStates = { ...diffStatesRef.current }
      const evicted = enqueueResult.evicted
      if (evicted) {
        const evictedRecord = diffRequestsRef.current.get(evicted.fileId)
        if (evictedRecord?.requestKey === evicted.requestKey && evictedRecord.phase === 'queued') {
          diffRequestsRef.current.delete(evicted.fileId)
          delete nextStates[evicted.fileId]
        }
      }
      diffRequestsRef.current.set(fileId, { ...item, phase: 'queued' })
      nextStates[fileId] = { status: 'loading' }
      diffStatesRef.current = nextStates
      setDiffStates(nextStates)
      drainDiffQueueRef.current()
    },
    [lastTurnConversationId, projectId, scope]
  )

  const loadFileDiff = useCallback(
    (fileId: string, priority: GitReviewDiffPriority = 'normal') => {
      requestFileDiff(fileId, priority, false)
    },
    [requestFileDiff]
  )

  const retryFileDiff = useCallback(
    (fileId: string) => {
      requestFileDiff(fileId, 'high', true)
    },
    [requestFileDiff]
  )

  const cancelQueuedFileDiffsExcept = useCallback((keepFileIds: ReadonlySet<string>) => {
    let nextStates: Record<string, GitReviewDiffState> | null = null
    for (const [fileId, record] of diffRequestsRef.current) {
      if (record.phase !== 'queued' || keepFileIds.has(fileId)) continue
      diffQueueRef.current.remove(fileId, record.requestKey)
      diffRequestsRef.current.delete(fileId)
      if (diffStatesRef.current[fileId]?.status === 'loading') {
        nextStates ??= { ...diffStatesRef.current }
        delete nextStates[fileId]
      }
    }
    if (nextStates) {
      diffStatesRef.current = nextStates
      setDiffStates(nextStates)
    }
  }, [])

  const setHotDiffFileIds = useCallback((fileIds: ReadonlySet<string>) => {
    const hotFileIds = new Set(fileIds)
    hotDiffFileIdsRef.current = hotFileIds
    const evictedFileIds = diffCacheBudgetRef.current.trim(hotFileIds)
    if (evictedFileIds.length === 0) return

    const nextDiffStates = { ...diffStatesRef.current }
    const nextContentStates = { ...fileContentStatesRef.current }
    for (const fileId of evictedFileIds) {
      delete nextDiffStates[fileId]
      delete nextContentStates[fileId]
      fullContentCacheBudgetRef.current.remove(fileId)
    }
    diffStatesRef.current = nextDiffStates
    fileContentStatesRef.current = nextContentStates
    setDiffStates(nextDiffStates)
    setFileContentStates(nextContentStates)
  }, [])

  const mutateFile = useCallback(
    async (fileId: string, action: GitReviewFileMutationAction) => {
      const requestable = requestableSnapshotRef.current
      if (
        !requestable ||
        requestable.projectId !== projectId ||
        requestable.scope !== scope ||
        requestable.conversationId !== lastTurnConversationId ||
        scope === 'lastTurn' ||
        mutationRequestRef.current
      ) {
        return
      }
      const summary = requestable.summary

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
    [lastTurnConversationId, projectId, refresh, scope]
  )

  useEffect(() => {
    if (!isActive) {
      cancelQueuedFileDiffsExcept(EMPTY_FILE_IDS)
      cancelQueuedFileContentsExcept(EMPTY_FILE_IDS)
      setHotDiffFileIds(EMPTY_FILE_IDS)
      setHotFullContentFileIds(EMPTY_FILE_IDS)
      return
    }
    void refresh()
  }, [
    cancelQueuedFileContentsExcept,
    cancelQueuedFileDiffsExcept,
    isActive,
    refresh,
    setHotDiffFileIds,
    setHotFullContentFileIds
  ])

  return {
    cancelQueuedFileContentsExcept,
    cancelQueuedFileDiffsExcept,
    dismissMutationError: () => setMutationError(null),
    diffStates,
    fileContentStates,
    loadFileContent,
    loadFileDiff,
    mutateFile,
    mutationError,
    pendingFileId,
    refresh,
    retryFileDiff,
    setHotDiffFileIds,
    setHotFullContentFileIds,
    scope,
    setScope,
    summaryState
  }
}

const EMPTY_FILE_IDS: ReadonlySet<string> = new Set()
