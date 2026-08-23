import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { AutomationListOutput, AutomationRun, AutomationTask } from '@mycopilot/protocol'
import {
  AutomationClientError,
  createAutomation as createAutomationRequest,
  createAutomationRequestId,
  deleteAutomation as deleteAutomationRequest,
  getAutomationErrorDetails,
  listAutomations,
  runAutomationNow as runAutomationNowRequest,
  setAutomationEnabled as setAutomationEnabledRequest,
  updateAutomation as updateAutomationRequest
} from './automationClient'
import {
  cacheAutomationTask,
  cacheAutomationTasks,
  getCachedAutomationTask,
  removeCachedAutomationTask
} from './automationCache'
import { subscribeAutomationRealtime, synchronizeAutomationSequence } from './automationRealtime'
import type {
  AutomationDraft,
  AutomationErrorDetails,
  AutomationFilter,
  AutomationLoadStatus,
  AutomationMutationInput,
  AutomationMutationKind
} from './automationTypes'

export interface UseAutomationsOptions {
  enabled?: boolean
  filter?: AutomationFilter
  query?: string
  limit?: number
}

export interface UseAutomationsResult {
  tasks: AutomationTask[]
  counts: AutomationListOutput['counts']
  attentionCount: number
  lastSequence: number
  nextCursor: string | null
  status: AutomationLoadStatus
  error: AutomationErrorDetails | null
  mutationError: AutomationErrorDetails | null
  isRefreshing: boolean
  isLoadingMore: boolean
  isCreating: boolean
  pendingById: Readonly<Record<string, AutomationMutationKind>>
  refresh(): Promise<AutomationListOutput | null>
  loadMore(): Promise<void>
  create(draft: AutomationDraft, requestId?: string): Promise<AutomationTask>
  update(task: AutomationTask, config: AutomationMutationInput): Promise<AutomationTask>
  setEnabled(task: AutomationTask, enabled: boolean): Promise<AutomationTask>
  runNow(task: AutomationTask, requestId?: string): Promise<AutomationRun>
  remove(task: AutomationTask): Promise<void>
}

interface AutomationsState {
  tasks: AutomationTask[]
  counts: AutomationListOutput['counts']
  attentionCount: number
  lastSequence: number
  nextCursor: string | null
  status: AutomationLoadStatus
  error: AutomationErrorDetails | null
  isRefreshing: boolean
  isLoadingMore: boolean
}

interface PendingMutation {
  kind: AutomationMutationKind
  fingerprint: string
  promise: Promise<unknown>
}

interface PendingCreate {
  fingerprint: string
  promise: Promise<AutomationTask>
  requestId: string
}

const EMPTY_COUNTS = { all: 0, active: 0, paused: 0 }
const INITIAL_STATE: AutomationsState = {
  tasks: [],
  counts: EMPTY_COUNTS,
  attentionCount: 0,
  lastSequence: 0,
  nextCursor: null,
  status: 'idle',
  error: null,
  isRefreshing: false,
  isLoadingMore: false
}

function taskMatches(task: AutomationTask, filter: AutomationFilter, query: string): boolean {
  if (filter !== 'all' && task.status !== filter) return false
  const normalized = query.trim().toLocaleLowerCase()
  if (!normalized) return true
  return [
    task.title,
    task.prompt,
    task.targetSnapshot.projectName,
    task.targetSnapshot.conversationTitle,
    task.targetSnapshot.modelDisplayName
  ].some((value) => value?.toLocaleLowerCase().includes(normalized))
}

function mergeTasks(
  current: readonly AutomationTask[],
  incoming: readonly AutomationTask[]
): AutomationTask[] {
  const next = new Map(current.map((task) => [task.automationId, task]))
  for (const task of incoming) {
    const existing = next.get(task.automationId)
    if (!existing || task.revision >= existing.revision) next.set(task.automationId, task)
  }
  return [...next.values()]
}

function cachedTasksForResponse(
  responseTasks: readonly AutomationTask[],
  filter: AutomationFilter,
  query: string
): AutomationTask[] {
  cacheAutomationTasks(responseTasks)
  return responseTasks.flatMap((responseTask) => {
    const authoritative = getCachedAutomationTask(responseTask.automationId)
    return authoritative && taskMatches(authoritative, filter, query) ? [authoritative] : []
  })
}

export function useAutomations(options: UseAutomationsOptions = {}): UseAutomationsResult {
  const enabled = options.enabled ?? true
  const filter = options.filter ?? 'all'
  const query = options.query ?? ''
  const limit = options.limit ?? 50
  const [state, setState] = useState<AutomationsState>(INITIAL_STATE)
  const [pendingById, setPendingById] = useState<Readonly<Record<string, AutomationMutationKind>>>(
    {}
  )
  const [isCreating, setIsCreating] = useState(false)
  const [mutationError, setMutationError] = useState<AutomationErrorDetails | null>(null)
  const mountedRef = useRef(false)
  const requestRef = useRef(0)
  const stateRef = useRef(state)
  const refreshRef = useRef<() => Promise<AutomationListOutput | null>>(async () => null)
  const createInFlightRef = useRef<PendingCreate | null>(null)
  const mutationInFlightRef = useRef(new Map<string, PendingMutation>())

  useEffect(() => {
    stateRef.current = state
  }, [state])

  const refresh = useCallback(async (): Promise<AutomationListOutput | null> => {
    if (!enabled) return null
    const request = ++requestRef.current
    setState((current) => ({
      ...current,
      error: null,
      isRefreshing: current.status === 'ready',
      status: current.status === 'ready' ? 'ready' : 'loading'
    }))
    try {
      const output = await listAutomations({
        ...(filter === 'all' ? {} : { status: filter }),
        ...(query.trim() ? { query } : {}),
        limit
      })
      if (!mountedRef.current || request !== requestRef.current) return null
      if (output.lastSequence < stateRef.current.lastSequence) {
        setState((current) => ({
          ...current,
          isRefreshing: false,
          isLoadingMore: false,
          status: 'ready'
        }))
        return null
      }
      const tasks = cachedTasksForResponse(output.tasks, filter, query)
      synchronizeAutomationSequence(output.lastSequence)
      setState({
        tasks,
        counts: output.counts,
        attentionCount: output.attentionCount,
        lastSequence: output.lastSequence,
        nextCursor: output.nextCursor,
        status: 'ready',
        error: null,
        isRefreshing: false,
        isLoadingMore: false
      })
      return output
    } catch (error) {
      if (!mountedRef.current || request !== requestRef.current) return null
      const details = getAutomationErrorDetails(error)
      setState((current) => ({
        ...current,
        error: details,
        isRefreshing: false,
        isLoadingMore: false,
        status: current.tasks.length > 0 ? 'ready' : 'error'
      }))
      return null
    }
  }, [enabled, filter, limit, query])

  useEffect(() => {
    refreshRef.current = refresh
  }, [refresh])

  useEffect(() => {
    mountedRef.current = true
    if (!enabled) {
      setState(INITIAL_STATE)
      return () => {
        mountedRef.current = false
        requestRef.current += 1
      }
    }
    const debounce = setTimeout(() => void refresh(), query.trim() ? 180 : 0)
    const unsubscribe = subscribeAutomationRealtime((signal) => {
      if (signal.type === 'resync' || signal.sequenceGap) {
        void refreshRef.current()
        return
      }
      // Events are hints only. Re-read the authoritative list so run/attention projections stay exact.
      void refreshRef.current()
    })
    return () => {
      mountedRef.current = false
      requestRef.current += 1
      clearTimeout(debounce)
      unsubscribe()
    }
  }, [enabled, filter, limit, query, refresh])

  const loadMore = useCallback(async (): Promise<void> => {
    const cursor = stateRef.current.nextCursor
    if (!enabled || !cursor || stateRef.current.isLoadingMore) return
    const request = ++requestRef.current
    setState((current) => ({ ...current, isLoadingMore: true }))
    try {
      const output = await listAutomations({
        ...(filter === 'all' ? {} : { status: filter }),
        ...(query.trim() ? { query } : {}),
        cursor,
        limit
      })
      if (
        !mountedRef.current ||
        request !== requestRef.current ||
        stateRef.current.nextCursor !== cursor
      ) {
        return
      }
      if (output.lastSequence < stateRef.current.lastSequence) {
        setState((current) => ({ ...current, isLoadingMore: false }))
        return
      }
      const tasks = cachedTasksForResponse(output.tasks, filter, query)
      synchronizeAutomationSequence(output.lastSequence)
      setState((current) => ({
        ...current,
        tasks: mergeTasks(current.tasks, tasks).filter((task) => taskMatches(task, filter, query)),
        counts: output.counts,
        attentionCount: output.attentionCount,
        lastSequence: Math.max(current.lastSequence, output.lastSequence),
        nextCursor: output.nextCursor,
        isLoadingMore: false,
        error: null
      }))
    } catch (error) {
      if (!mountedRef.current || request !== requestRef.current) return
      setState((current) => ({
        ...current,
        error: getAutomationErrorDetails(error),
        isLoadingMore: false
      }))
    }
  }, [enabled, filter, limit, query])

  const applyTask = useCallback(
    (task: AutomationTask): void => {
      const cachedBefore = getCachedAutomationTask(task.automationId)
      cacheAutomationTask(task)
      const authoritative = getCachedAutomationTask(task.automationId)
      setState((current) => {
        const previous = current.tasks.find(
          (candidate) => candidate.automationId === task.automationId
        )
        let tasks = current.tasks.filter(
          (candidate) => candidate.automationId !== task.automationId
        )
        if (authoritative && taskMatches(authoritative, filter, query)) {
          tasks = [authoritative, ...tasks]
        }
        const counts = { ...current.counts }
        const countBaseline = cachedBefore ?? previous
        if (!authoritative && countBaseline) {
          counts.all = Math.max(0, counts.all - 1)
          counts[countBaseline.status] = Math.max(0, counts[countBaseline.status] - 1)
        } else if (authoritative && !countBaseline) {
          counts.all += 1
          counts[authoritative.status] += 1
        } else if (
          authoritative &&
          countBaseline &&
          countBaseline.status !== authoritative.status
        ) {
          counts[countBaseline.status] = Math.max(0, counts[countBaseline.status] - 1)
          counts[authoritative.status] += 1
        }
        return { ...current, tasks, counts }
      })
    },
    [filter, query]
  )

  const setPending = useCallback(
    (automationId: string, operation: AutomationMutationKind | null) => {
      setPendingById((current) => {
        const next = { ...current }
        if (operation) next[automationId] = operation
        else delete next[automationId]
        return next
      })
    },
    []
  )

  const create = useCallback(
    (draft: AutomationDraft, requestId?: string): Promise<AutomationTask> => {
      const fingerprint = JSON.stringify(draft)
      const existing = createInFlightRef.current
      if (
        existing &&
        existing.fingerprint === fingerprint &&
        (requestId === undefined || requestId === existing.requestId)
      ) {
        return existing.promise
      }
      if (existing) {
        return Promise.reject(
          new AutomationClientError({
            code: 'transport',
            message: 'Another scheduled task is still being created.',
            automationId: null,
            currentRevision: null,
            field: null,
            retryable: true
          })
        )
      }
      const stableRequestId = requestId ?? createAutomationRequestId()
      setIsCreating(true)
      setMutationError(null)
      const operation = createAutomationRequest(draft, stableRequestId)
        .then((task) => {
          if (mountedRef.current) applyTask(task)
          void refreshRef.current()
          return task
        })
        .catch((error: unknown) => {
          if (mountedRef.current) setMutationError(getAutomationErrorDetails(error))
          throw error
        })
        .finally(() => {
          createInFlightRef.current = null
          if (mountedRef.current) setIsCreating(false)
        })
      createInFlightRef.current = {
        fingerprint,
        promise: operation,
        requestId: stableRequestId
      }
      return operation
    },
    [applyTask]
  )

  const runTaskMutation = useCallback(
    <T>(
      task: AutomationTask,
      kind: AutomationMutationKind,
      fingerprint: string,
      operation: () => Promise<T>,
      onSuccess: (value: T) => void
    ): Promise<T> => {
      const existing = mutationInFlightRef.current.get(task.automationId)
      if (existing?.kind === kind && existing.fingerprint === fingerprint) {
        return existing.promise as Promise<T>
      }
      if (existing) {
        return Promise.reject(
          new AutomationClientError({
            code: 'transport',
            message: 'Another operation for this scheduled task is still in progress.',
            automationId: task.automationId,
            currentRevision: task.revision,
            field: null,
            retryable: true
          })
        )
      }
      setPending(task.automationId, kind)
      setMutationError(null)
      const promise = operation()
        .then((value) => {
          if (mountedRef.current) onSuccess(value)
          return value
        })
        .catch((error: unknown) => {
          const details = getAutomationErrorDetails(error)
          if (mountedRef.current) setMutationError(details)
          if (details.code === 'revision_conflict') void refreshRef.current()
          throw error
        })
        .finally(() => {
          mutationInFlightRef.current.delete(task.automationId)
          if (mountedRef.current) setPending(task.automationId, null)
        })
      mutationInFlightRef.current.set(task.automationId, { kind, fingerprint, promise })
      return promise
    },
    [setPending]
  )

  const update = useCallback(
    (task: AutomationTask, config: AutomationMutationInput): Promise<AutomationTask> =>
      runTaskMutation(
        task,
        'update',
        `update:${task.revision}:${JSON.stringify(config)}`,
        () =>
          updateAutomationRequest(
            { automationId: task.automationId, revision: task.revision },
            config
          ),
        (updated) => {
          applyTask(updated)
          void refreshRef.current()
        }
      ),
    [applyTask, runTaskMutation]
  )

  const setEnabled = useCallback(
    (task: AutomationTask, nextEnabled: boolean): Promise<AutomationTask> =>
      runTaskMutation(
        task,
        'set_enabled',
        `set_enabled:${task.revision}:${nextEnabled ? '1' : '0'}`,
        () =>
          setAutomationEnabledRequest(
            { automationId: task.automationId, revision: task.revision },
            nextEnabled
          ),
        (updated) => {
          applyTask(updated)
          void refreshRef.current()
        }
      ),
    [applyTask, runTaskMutation]
  )

  const runNow = useCallback(
    (task: AutomationTask, requestId?: string): Promise<AutomationRun> =>
      runTaskMutation(
        task,
        'run_now',
        'run_now',
        () => runAutomationNowRequest(task.automationId, requestId),
        (run) => {
          const cached = getCachedAutomationTask(task.automationId) ?? task
          const latestRun = cached.latestRun
          if (!latestRun || run.updatedAt >= latestRun.updatedAt) {
            applyTask({ ...cached, latestRun: run })
          }
          void refreshRef.current()
        }
      ),
    [applyTask, runTaskMutation]
  )

  const remove = useCallback(
    (task: AutomationTask): Promise<void> =>
      runTaskMutation(
        task,
        'delete',
        `delete:${task.revision}`,
        () => deleteAutomationRequest({ automationId: task.automationId, revision: task.revision }),
        () => {
          removeCachedAutomationTask(task.automationId, task.revision + 1)
          setState((current) => ({
            ...current,
            tasks: current.tasks.filter(
              (candidate) => candidate.automationId !== task.automationId
            ),
            counts: {
              ...current.counts,
              all: Math.max(0, current.counts.all - 1),
              [task.status]: Math.max(0, current.counts[task.status] - 1)
            }
          }))
          void refreshRef.current()
        }
      ).then(() => undefined),
    [runTaskMutation]
  )

  return useMemo(
    () => ({
      ...state,
      mutationError,
      pendingById,
      isCreating,
      refresh,
      loadMore,
      create,
      update,
      setEnabled,
      runNow,
      remove
    }),
    [
      create,
      isCreating,
      loadMore,
      mutationError,
      pendingById,
      refresh,
      remove,
      runNow,
      setEnabled,
      state,
      update
    ]
  )
}
