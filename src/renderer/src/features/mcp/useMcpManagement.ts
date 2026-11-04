import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type Dispatch,
  type MutableRefObject,
  type SetStateAction
} from 'react'
import {
  MCP_MANAGEMENT_SCHEMA_VERSION,
  MCP_MANAGEMENT_LIMITS,
  type McpChangedNotification,
  type McpServerDetailsOutput,
  type McpServerDetailsView,
  type McpServerListItem,
  type McpServerListOutput
} from '@mycopilot/protocol'
import {
  addMcpServer,
  deleteMcpServer,
  disableMcpServer,
  enableMcpServer,
  getMcpServer,
  listMcpServers,
  listMcpTools,
  onMcpChanged,
  refreshMcpCatalog,
  requestMcpLaunchAuthorization,
  restartMcpServer,
  selectMcpExecutable,
  selectMcpWorkingDirectory,
  startMcpServer,
  stopMcpServer,
  updateMcpServer
} from './mcpManagementClient'
import {
  getMcpManagementErrorDetails,
  mcpOperationNeedsAuthoritativeConfirmation,
  shouldRefreshMcpAfterError,
  type McpManagementErrorDetails
} from './mcpManagementErrors'
import {
  toMcpCreateInput,
  toMcpMutationInput,
  toMcpPrecondition,
  type McpServerDraft
} from './mcpManagementInputs'
import {
  type ServerIdentity,
  serverIdentity,
  matchesCurrentServer,
  currentServer,
  isExactDetailsResponseForIdentity,
  isMutationResponseValidForIdentity,
  isMutationResponseValidForOperation
} from './mcpManagementIdentity'
import {
  INITIAL_STATE,
  IDLE_CATALOG,
  mergeDetailsWithFreshSummary,
  applyDetailsOutput,
  setServerDetails,
  setCatalogState,
  failClosedCatalog,
  catalogMatchesIdentity,
  isCatalogPageProjectionValid,
  catalogPageToState,
  pruneServerState,
  mergeServerListOutput,
  removeServerState
} from './mcpManagementState'
import type {
  McpManagementViewState,
  McpServerPendingOperation,
  McpToolCatalogState
} from './mcpManagementTypes'

export type {
  McpManagementViewState,
  McpServerPendingOperation,
  McpToolCatalogState
} from './mcpManagementTypes'

const MAX_TRACKED_EVENT_SOURCE_EPOCHS = 16

export function useMcpManagement() {
  const [state, setState] = useState<McpManagementViewState>(INITIAL_STATE)
  const [detailsById, setDetailsById] = useState<ReadonlyMap<string, McpServerDetailsView>>(
    () => new Map()
  )
  const [catalogsById, setCatalogsById] = useState<ReadonlyMap<string, McpToolCatalogState>>(
    () => new Map()
  )
  const [pendingOperations, setPendingOperations] = useState<
    ReadonlyMap<string, McpServerPendingOperation>
  >(() => new Map())
  const [isAdding, setIsAdding] = useState(false)

  const mountedRef = useRef(false)
  const lifecycleEpochRef = useRef(0)
  const outputRef = useRef<McpServerListOutput | null>(null)
  const detailsRef = useRef<ReadonlyMap<string, McpServerDetailsView>>(new Map())
  const catalogsRef = useRef<ReadonlyMap<string, McpToolCatalogState>>(new Map())
  const pendingRef = useRef<ReadonlyMap<string, McpServerPendingOperation>>(new Map())
  const enableFlowRef = useRef<ReadonlySet<string>>(new Set())
  const isAddingRef = useRef(false)
  const mutationEpochRef = useRef(0)
  const refreshDirtyRef = useRef(false)
  const refreshVisibleRef = useRef(false)
  const refreshLoopRef = useRef<Promise<void> | null>(null)
  const refreshCallbackRef = useRef<() => Promise<McpServerListOutput | null>>(async () => null)
  const eventSequencesByEpochRef = useRef<ReadonlyMap<string, number>>(new Map())
  const notificationScheduledRef = useRef(false)
  const catalogRequestRef = useRef<ReadonlyMap<string, number>>(new Map())
  const detailsRequestRef = useRef<ReadonlyMap<string, number>>(new Map())

  const runRefreshLoop = useCallback(async () => {
    while (mountedRef.current && refreshDirtyRef.current) {
      refreshDirtyRef.current = false
      const showRefreshIndicator = refreshVisibleRef.current
      refreshVisibleRef.current = false
      const lifecycleEpoch = lifecycleEpochRef.current
      const mutationEpoch = mutationEpochRef.current
      setState((current) => ({
        ...current,
        errorMessage: current.output ? current.errorMessage : null,
        isRefreshing: Boolean(current.output) && showRefreshIndicator,
        status: current.output ? 'ready' : 'loading'
      }))

      try {
        const fetched = await listMcpServers()
        if (
          !mountedRef.current ||
          lifecycleEpoch !== lifecycleEpochRef.current ||
          mutationEpoch !== mutationEpochRef.current
        ) {
          continue
        }
        const output = mergeServerListOutput(outputRef.current, fetched)
        outputRef.current = output
        setState({ errorMessage: null, isRefreshing: false, output, status: 'ready' })
        pruneServerState(output, detailsRef, setDetailsById, catalogsRef, setCatalogsById)
      } catch (error) {
        if (!mountedRef.current || lifecycleEpoch !== lifecycleEpochRef.current) continue
        const message = getMcpManagementErrorDetails(error).message
        setState((current) =>
          current.output
            ? { ...current, errorMessage: message, isRefreshing: false, status: 'ready' }
            : { errorMessage: message, isRefreshing: false, output: null, status: 'error' }
        )
      }
    }
  }, [])

  const refresh = useCallback(
    async (showRefreshIndicator = false): Promise<McpServerListOutput | null> => {
      if (showRefreshIndicator) refreshVisibleRef.current = true
      refreshDirtyRef.current = true
      if (!refreshLoopRef.current) {
        const loop = runRefreshLoop().finally(() => {
          if (refreshLoopRef.current === loop) {
            refreshLoopRef.current = null
            if (mountedRef.current && refreshDirtyRef.current) {
              queueMicrotask(() => {
                if (mountedRef.current && refreshDirtyRef.current) {
                  void refreshCallbackRef.current()
                }
              })
            }
          }
        })
        refreshLoopRef.current = loop
      }
      await refreshLoopRef.current
      return outputRef.current
    },
    [runRefreshLoop]
  )
  const refreshVisible = useCallback(
    (): Promise<McpServerListOutput | null> => refresh(true),
    [refresh]
  )
  useEffect(() => {
    refreshCallbackRef.current = refresh
  }, [refresh])

  const scheduleEventRefresh = useCallback(() => {
    if (notificationScheduledRef.current) return
    notificationScheduledRef.current = true
    queueMicrotask(() => {
      notificationScheduledRef.current = false
      if (mountedRef.current) void refresh()
    })
  }, [refresh])

  useEffect(() => {
    lifecycleEpochRef.current += 1
    mountedRef.current = true
    void refresh()
    const unsubscribe = onMcpChanged((event) => {
      if (!acceptMcpChangedEvent(event, eventSequencesByEpochRef)) return
      scheduleEventRefresh()
    })

    return () => {
      mountedRef.current = false
      lifecycleEpochRef.current += 1
      refreshDirtyRef.current = false
      refreshVisibleRef.current = false
      notificationScheduledRef.current = false
      unsubscribe()
    }
  }, [refresh, scheduleEventRefresh])

  const addServer = useCallback(
    async (draft: McpServerDraft): Promise<McpServerDetailsView | null> => {
      if (isAddingRef.current) return null
      isAddingRef.current = true
      setIsAdding(true)
      mutationEpochRef.current += 1
      try {
        const output = await addMcpServer(toMcpCreateInput(draft))
        if (mountedRef.current) {
          const applied = applyDetailsOutput(
            outputRef,
            setState,
            detailsRef,
            setDetailsById,
            catalogsRef,
            setCatalogsById,
            output
          )
          if (!applied) {
            await refresh()
            return null
          }
          void refresh()
          return applied
        }
        return null
      } catch (error) {
        const details = getMcpManagementErrorDetails(error)
        if (mustRefreshAfterMutationError(details)) await refresh()
        throw error
      } finally {
        isAddingRef.current = false
        if (mountedRef.current) setIsAdding(false)
      }
    },
    [refresh]
  )

  const loadDetails = useCallback(
    async (server: McpServerListItem): Promise<McpServerDetailsView | null> => {
      const identity = serverIdentity(server)
      const requestNumber = (detailsRequestRef.current.get(server.serverId) ?? 0) + 1
      detailsRequestRef.current = new Map(detailsRequestRef.current).set(
        server.serverId,
        requestNumber
      )
      try {
        const output = await getMcpServer({
          schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
          serverId: server.serverId
        })
        if (
          !mountedRef.current ||
          detailsRequestRef.current.get(server.serverId) !== requestNumber
        ) {
          return null
        }
        if (
          !matchesCurrentServer(outputRef.current, identity) ||
          !isExactDetailsResponseForIdentity(output, identity)
        ) {
          void refresh()
          return null
        }
        const current = currentServer(outputRef.current, server.serverId)
        if (!current) return null
        const merged = mergeDetailsWithFreshSummary(output.server, current)
        setServerDetails(detailsRef, setDetailsById, merged)
        return merged
      } catch (error) {
        const details = getMcpManagementErrorDetails(error)
        if (shouldRefreshMcpAfterError(details)) await refresh()
        throw error
      }
    },
    [refresh]
  )

  const updateServer = useCallback(
    async (
      server: McpServerListItem,
      draft: McpServerDraft
    ): Promise<McpServerDetailsView | null> =>
      runServerDetailsMutation({
        server,
        operation: 'update',
        mutation: () =>
          updateMcpServer({
            schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
            serverId: server.serverId,
            precondition: toMcpPrecondition(server),
            displayName: draft.displayName,
            transport: 'stdio',
            executable: draft.executable,
            arguments: [...draft.arguments],
            cwd: draft.cwd,
            approvalMode: draft.approvalMode
          }),
        pendingRef,
        setPendingOperations,
        mountedRef,
        outputRef,
        setState,
        detailsRef,
        setDetailsById,
        catalogsRef,
        setCatalogsById,
        mutationEpochRef,
        responseIdentity: serverIdentity(server),
        refresh
      }),
    [refresh]
  )

  const authorizeLaunch = useCallback(
    async (server: McpServerListItem): Promise<McpServerDetailsView | null> => {
      if (!beginPending(pendingRef, setPendingOperations, server.serverId, 'authorization')) {
        return null
      }
      mutationEpochRef.current += 1
      const identity = serverIdentity(server)
      try {
        const output = await requestMcpLaunchAuthorization(toMcpMutationInput(server))
        if (
          !output.authorized ||
          output.server.launchAuthorizationState !== 'authorized' ||
          output.server.trust !== 'userApproved'
        ) {
          return null
        }
        const detailsOutput: McpServerDetailsOutput = {
          schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
          registryRevision: output.server.registryRevision,
          server: output.server
        }
        if (!isMutationResponseValidForIdentity(detailsOutput, identity, true)) {
          if (mountedRef.current) await refresh()
          return null
        }
        if (mountedRef.current) {
          const applied = applyDetailsOutput(
            outputRef,
            setState,
            detailsRef,
            setDetailsById,
            catalogsRef,
            setCatalogsById,
            detailsOutput
          )
          if (!applied) {
            await refresh()
            return null
          }
          void refresh()
          return applied
        }
        return null
      } catch (error) {
        const details = getMcpManagementErrorDetails(error)
        if (mustRefreshAfterMutationError(details)) await refresh()
        throw error
      } finally {
        if (mountedRef.current) {
          endPending(pendingRef, setPendingOperations, server.serverId)
        }
      }
    },
    [refresh]
  )

  const enableServer = useCallback(
    (server: McpServerListItem) =>
      runServerDetailsMutation({
        server,
        operation: 'enable',
        mutation: () => enableMcpServer(toMcpMutationInput(server)),
        pendingRef,
        setPendingOperations,
        mountedRef,
        outputRef,
        setState,
        detailsRef,
        setDetailsById,
        catalogsRef,
        setCatalogsById,
        mutationEpochRef,
        responseIdentity: serverIdentity(server),
        refresh
      }),
    [refresh]
  )

  const disableServer = useCallback(
    (server: McpServerListItem) =>
      runServerDetailsMutation({
        server,
        operation: 'disable',
        mutation: () => disableMcpServer(toMcpMutationInput(server)),
        pendingRef,
        setPendingOperations,
        mountedRef,
        outputRef,
        setState,
        detailsRef,
        setDetailsById,
        catalogsRef,
        setCatalogsById,
        mutationEpochRef,
        responseIdentity: serverIdentity(server),
        refresh
      }),
    [refresh]
  )

  const startServer = useCallback(
    (server: McpServerListItem) =>
      runServerDetailsMutation({
        server,
        operation: 'start',
        mutation: () => startMcpServer(toMcpMutationInput(server)),
        pendingRef,
        setPendingOperations,
        mountedRef,
        outputRef,
        setState,
        detailsRef,
        setDetailsById,
        catalogsRef,
        setCatalogsById,
        mutationEpochRef,
        responseIdentity: serverIdentity(server),
        refresh
      }),
    [refresh]
  )

  const setServerEnabled = useCallback(
    async (server: McpServerListItem, enabled: boolean): Promise<McpServerDetailsView | null> => {
      if (enableFlowRef.current.has(server.serverId)) return null
      enableFlowRef.current = new Set(enableFlowRef.current).add(server.serverId)
      try {
        if (!enabled) return await disableServer(server)

        let current: McpServerListItem = server
        if (server.launchAuthorizationState !== 'authorized') {
          const authorized = await authorizeLaunch(server)
          if (!authorized || authorized.launchAuthorizationState !== 'authorized') return null
          current = authorized
        }

        const enabledServer = await enableServer(current)
        if (!enabledServer) return null
        return await startServer(enabledServer)
      } finally {
        const next = new Set(enableFlowRef.current)
        next.delete(server.serverId)
        enableFlowRef.current = next
      }
    },
    [authorizeLaunch, disableServer, enableServer, startServer]
  )

  const stopServer = useCallback(
    (server: McpServerListItem) =>
      runServerDetailsMutation({
        server,
        operation: 'stop',
        mutation: () => stopMcpServer(toMcpMutationInput(server)),
        pendingRef,
        setPendingOperations,
        mountedRef,
        outputRef,
        setState,
        detailsRef,
        setDetailsById,
        catalogsRef,
        setCatalogsById,
        mutationEpochRef,
        responseIdentity: serverIdentity(server),
        refresh
      }),
    [refresh]
  )

  const restartServer = useCallback(
    (server: McpServerListItem) =>
      runServerDetailsMutation({
        server,
        operation: 'restart',
        mutation: () => restartMcpServer(toMcpMutationInput(server)),
        pendingRef,
        setPendingOperations,
        mountedRef,
        outputRef,
        setState,
        detailsRef,
        setDetailsById,
        catalogsRef,
        setCatalogsById,
        mutationEpochRef,
        responseIdentity: serverIdentity(server),
        refresh
      }),
    [refresh]
  )

  const deleteServer = useCallback(
    async (server: McpServerListItem): Promise<McpServerDetailsView | null> => {
      if (!beginPending(pendingRef, setPendingOperations, server.serverId, 'delete')) return null
      mutationEpochRef.current += 1
      try {
        const output = await deleteMcpServer(toMcpMutationInput(server))
        const identity = serverIdentity(server)
        if (mountedRef.current && isMutationResponseValidForIdentity(output, identity, false)) {
          removeServerState(
            outputRef,
            setState,
            detailsRef,
            setDetailsById,
            catalogsRef,
            setCatalogsById,
            server.serverId,
            output.registryRevision
          )
        } else if (mountedRef.current) {
          await refresh()
          return null
        }
        void refresh()
        return output.server
      } catch (error) {
        const details = getMcpManagementErrorDetails(error)
        if (mustRefreshAfterMutationError(details)) await refresh()
        throw error
      } finally {
        if (mountedRef.current) endPending(pendingRef, setPendingOperations, server.serverId)
      }
    },
    [refresh]
  )

  const loadTools = useCallback(
    async (server: McpServerListItem, loadMore = false): Promise<McpToolCatalogState | null> => {
      const existing = catalogsRef.current.get(server.serverId) ?? IDLE_CATALOG
      if (existing.status === 'loading' || existing.isRefreshing) return null
      if (loadMore && !existing.nextCursor) return existing
      const identity = serverIdentity(server)
      if (loadMore && !catalogMatchesIdentity(existing, identity)) {
        failClosedCatalog(catalogsRef, setCatalogsById, identity)
        void refresh()
        return null
      }
      const requestNumber = (catalogRequestRef.current.get(server.serverId) ?? 0) + 1
      catalogRequestRef.current = new Map(catalogRequestRef.current).set(
        server.serverId,
        requestNumber
      )
      const cursor = loadMore ? existing.nextCursor : undefined
      setCatalogState(catalogsRef, setCatalogsById, server.serverId, {
        ...existing,
        errorMessage: null,
        isRefreshing: loadMore,
        status: loadMore ? existing.status : 'loading'
      })
      try {
        const page = await listMcpTools({
          schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
          serverId: server.serverId,
          ...(cursor === undefined ? {} : { cursor }),
          limit: MCP_MANAGEMENT_LIMITS.toolPageSize
        })
        if (
          !mountedRef.current ||
          catalogRequestRef.current.get(server.serverId) !== requestNumber ||
          !matchesCurrentServer(outputRef.current, identity)
        ) {
          return null
        }
        const displayed = catalogsRef.current.get(server.serverId) ?? IDLE_CATALOG
        const current = currentServer(outputRef.current, server.serverId)
        const minimumGeneration = Math.max(
          server.catalogGeneration,
          current?.catalogGeneration ?? 0,
          displayed.catalogGeneration ?? 0
        )
        const loadMoreChangedCatalog =
          loadMore &&
          (existing.catalogGeneration === undefined ||
            page.catalogGeneration !== existing.catalogGeneration ||
            page.catalogCompleteness !== existing.catalogCompleteness)
        if (!isCatalogPageProjectionValid(page, server.serverId) || loadMoreChangedCatalog) {
          failClosedCatalog(catalogsRef, setCatalogsById, identity)
          void refresh()
          return null
        }
        if (page.catalogGeneration < minimumGeneration) {
          if (displayed.catalogGeneration === undefined) {
            failClosedCatalog(catalogsRef, setCatalogsById, identity)
          } else {
            setCatalogState(catalogsRef, setCatalogsById, server.serverId, {
              ...displayed,
              isRefreshing: false
            })
          }
          void refresh()
          return null
        }
        const next = catalogPageToState(page, identity, loadMore ? existing : undefined)
        setCatalogState(catalogsRef, setCatalogsById, server.serverId, next)
        return next
      } catch (error) {
        if (
          mountedRef.current &&
          catalogRequestRef.current.get(server.serverId) === requestNumber
        ) {
          const latest = catalogsRef.current.get(server.serverId) ?? existing
          const failed: McpToolCatalogState = {
            ...latest,
            errorMessage: getMcpManagementErrorDetails(error).message,
            isRefreshing: false,
            status: latest.tools.length > 0 ? 'ready' : 'error'
          }
          setCatalogState(catalogsRef, setCatalogsById, server.serverId, failed)
        }
        throw error
      }
    },
    [refresh]
  )

  const refreshCatalog = useCallback(
    async (server: McpServerListItem): Promise<McpToolCatalogState | null> => {
      if (!beginPending(pendingRef, setPendingOperations, server.serverId, 'refreshCatalog')) {
        return null
      }
      const identity = serverIdentity(server)
      mutationEpochRef.current += 1
      const current = catalogsRef.current.get(server.serverId) ?? IDLE_CATALOG
      const requestNumber = (catalogRequestRef.current.get(server.serverId) ?? 0) + 1
      catalogRequestRef.current = new Map(catalogRequestRef.current).set(
        server.serverId,
        requestNumber
      )
      setCatalogState(catalogsRef, setCatalogsById, server.serverId, {
        ...current,
        errorMessage: null,
        isRefreshing: true
      })
      try {
        const page = await refreshMcpCatalog(toMcpMutationInput(server))
        if (
          !mountedRef.current ||
          catalogRequestRef.current.get(server.serverId) !== requestNumber ||
          !matchesCurrentServer(outputRef.current, identity)
        ) {
          return null
        }
        const displayed = catalogsRef.current.get(server.serverId) ?? IDLE_CATALOG
        const currentServerSummary = currentServer(outputRef.current, server.serverId)
        const minimumGeneration = Math.max(
          server.catalogGeneration,
          currentServerSummary?.catalogGeneration ?? 0,
          displayed.catalogGeneration ?? 0
        )
        if (!isCatalogPageProjectionValid(page, server.serverId)) {
          failClosedCatalog(catalogsRef, setCatalogsById, identity)
          void refresh()
          return null
        }
        if (page.catalogGeneration < minimumGeneration) {
          setCatalogState(catalogsRef, setCatalogsById, server.serverId, {
            ...displayed,
            isRefreshing: false
          })
          void refresh()
          return null
        }
        const next = catalogPageToState(page, identity)
        setCatalogState(catalogsRef, setCatalogsById, server.serverId, next)
        void refresh()
        return next
      } catch (error) {
        const details = getMcpManagementErrorDetails(error)
        if (
          mountedRef.current &&
          catalogRequestRef.current.get(server.serverId) === requestNumber
        ) {
          const latest = catalogsRef.current.get(server.serverId) ?? current
          setCatalogState(catalogsRef, setCatalogsById, server.serverId, {
            ...latest,
            errorMessage: details.message,
            isRefreshing: false,
            status: latest.tools.length > 0 ? 'ready' : 'error'
          })
        }
        if (mustRefreshAfterMutationError(details)) await refresh()
        throw error
      } finally {
        if (mountedRef.current) endPending(pendingRef, setPendingOperations, server.serverId)
      }
    },
    [refresh]
  )

  return {
    addServer,
    authorizeLaunch,
    catalogsById,
    deleteServer,
    detailsById,
    disableServer,
    enableServer,
    isAdding,
    loadDetails,
    loadTools,
    pendingOperations,
    refresh: refreshVisible,
    refreshCatalog,
    restartServer,
    selectExecutable: selectMcpExecutable,
    selectWorkingDirectory: selectMcpWorkingDirectory,
    setServerEnabled,
    startServer,
    state,
    stopServer,
    updateServer
  }
}

function mustRefreshAfterMutationError(details: McpManagementErrorDetails): boolean {
  return shouldRefreshMcpAfterError(details) || mcpOperationNeedsAuthoritativeConfirmation(details)
}

function acceptMcpChangedEvent(
  event: McpChangedNotification,
  sequencesByEpochRef: MutableRefObject<ReadonlyMap<string, number>>
): boolean {
  const previousSequence = sequencesByEpochRef.current.get(event.sourceEpoch)
  if (previousSequence !== undefined && event.sequence <= previousSequence) return false

  const sequences = new Map(sequencesByEpochRef.current)
  sequences.delete(event.sourceEpoch)
  sequences.set(event.sourceEpoch, event.sequence)
  while (sequences.size > MAX_TRACKED_EVENT_SOURCE_EPOCHS) {
    const oldestSourceEpoch = sequences.keys().next().value
    if (oldestSourceEpoch === undefined) break
    sequences.delete(oldestSourceEpoch)
  }
  sequencesByEpochRef.current = sequences
  return true
}

interface RunServerDetailsMutationInput {
  catalogsRef: MutableRefObject<ReadonlyMap<string, McpToolCatalogState>>
  detailsRef: MutableRefObject<ReadonlyMap<string, McpServerDetailsView>>
  mountedRef: MutableRefObject<boolean>
  mutation: () => Promise<McpServerDetailsOutput>
  mutationEpochRef: MutableRefObject<number>
  operation: McpServerPendingOperation
  outputRef: MutableRefObject<McpServerListOutput | null>
  pendingRef: MutableRefObject<ReadonlyMap<string, McpServerPendingOperation>>
  refresh: () => Promise<McpServerListOutput | null>
  responseIdentity: ServerIdentity
  server: McpServerListItem
  setDetailsById: Dispatch<SetStateAction<ReadonlyMap<string, McpServerDetailsView>>>
  setCatalogsById: Dispatch<SetStateAction<ReadonlyMap<string, McpToolCatalogState>>>
  setPendingOperations: Dispatch<SetStateAction<ReadonlyMap<string, McpServerPendingOperation>>>
  setState: Dispatch<SetStateAction<McpManagementViewState>>
}

async function runServerDetailsMutation({
  catalogsRef,
  detailsRef,
  mountedRef,
  mutation,
  mutationEpochRef,
  operation,
  outputRef,
  pendingRef,
  refresh,
  responseIdentity,
  server,
  setDetailsById,
  setCatalogsById,
  setPendingOperations,
  setState
}: RunServerDetailsMutationInput): Promise<McpServerDetailsView | null> {
  if (!beginPending(pendingRef, setPendingOperations, server.serverId, operation)) return null
  mutationEpochRef.current += 1
  try {
    const output = await mutation()
    if (!isMutationResponseValidForOperation(output, responseIdentity, operation)) {
      if (mountedRef.current) await refresh()
      return null
    }
    if (mountedRef.current) {
      const applied = applyDetailsOutput(
        outputRef,
        setState,
        detailsRef,
        setDetailsById,
        catalogsRef,
        setCatalogsById,
        output,
        true
      )
      if (!applied) {
        await refresh()
        return null
      }
      void refresh()
      return applied
    }
    return null
  } catch (error) {
    const details = getMcpManagementErrorDetails(error)
    if (mustRefreshAfterMutationError(details)) await refresh()
    throw error
  } finally {
    if (mountedRef.current) endPending(pendingRef, setPendingOperations, server.serverId)
  }
}

function beginPending(
  pendingRef: MutableRefObject<ReadonlyMap<string, McpServerPendingOperation>>,
  setPending: Dispatch<SetStateAction<ReadonlyMap<string, McpServerPendingOperation>>>,
  serverId: string,
  operation: McpServerPendingOperation
): boolean {
  if (pendingRef.current.has(serverId)) return false
  const next = new Map(pendingRef.current).set(serverId, operation)
  pendingRef.current = next
  setPending(next)
  return true
}

function endPending(
  pendingRef: MutableRefObject<ReadonlyMap<string, McpServerPendingOperation>>,
  setPending: Dispatch<SetStateAction<ReadonlyMap<string, McpServerPendingOperation>>>,
  serverId: string
): void {
  const next = new Map(pendingRef.current)
  next.delete(serverId)
  pendingRef.current = next
  setPending(next)
}
