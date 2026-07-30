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
  type McpCatalogCompletenessView,
  type McpCatalogToolsPageOutput,
  type McpChangedNotification,
  type McpServerDetailsOutput,
  type McpServerDetailsView,
  type McpServerListItem,
  type McpServerListOutput,
  type McpToolSummaryView
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

export interface McpManagementViewState {
  errorMessage: string | null
  isRefreshing: boolean
  output: McpServerListOutput | null
  status: 'loading' | 'ready' | 'error'
}

export type McpServerPendingOperation =
  | 'authorization'
  | 'delete'
  | 'disable'
  | 'enable'
  | 'refreshCatalog'
  | 'restart'
  | 'start'
  | 'stop'
  | 'update'

export interface McpToolCatalogState {
  catalogCompleteness?: McpCatalogCompletenessView
  catalogGeneration?: number
  errorMessage: string | null
  isRefreshing: boolean
  nextCursor?: string
  serverConfigDigest?: string
  serverConfigEpoch?: string
  serverRegistryRevision?: number
  status: 'idle' | 'loading' | 'ready' | 'error'
  tools: readonly McpToolSummaryView[]
}

const INITIAL_STATE: McpManagementViewState = {
  errorMessage: null,
  isRefreshing: false,
  output: null,
  status: 'loading'
}

const IDLE_CATALOG: McpToolCatalogState = {
  errorMessage: null,
  isRefreshing: false,
  status: 'idle',
  tools: []
}

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

interface ServerIdentity {
  catalogGeneration: number
  configDigest: string
  configEpoch: string
  registryRevision: number
  serverId: string
  updatedAtMs: number
}

function serverIdentity(server: McpServerListItem): ServerIdentity {
  return {
    catalogGeneration: server.catalogGeneration,
    configDigest: server.configDigest,
    configEpoch: server.configEpoch,
    registryRevision: server.registryRevision,
    serverId: server.serverId,
    updatedAtMs: server.updatedAtMs
  }
}

function matchesCurrentServer(
  output: McpServerListOutput | null,
  identity: ServerIdentity
): boolean {
  const current = output?.servers.find((server) => server.serverId === identity.serverId)
  return Boolean(
    current &&
    current.registryRevision === identity.registryRevision &&
    current.configEpoch === identity.configEpoch &&
    current.configDigest === identity.configDigest
  )
}

function currentServer(
  output: McpServerListOutput | null,
  serverId: string
): McpServerListItem | undefined {
  return output?.servers.find((server) => server.serverId === serverId)
}

function isExactDetailsResponseForIdentity(
  output: McpServerDetailsOutput,
  identity: ServerIdentity
): boolean {
  const server = output.server
  return (
    output.registryRevision >= server.registryRevision &&
    server.serverId === identity.serverId &&
    server.registryRevision === identity.registryRevision &&
    server.configEpoch === identity.configEpoch &&
    server.configDigest === identity.configDigest &&
    server.updatedAtMs >= identity.updatedAtMs &&
    server.catalogGeneration >= identity.catalogGeneration
  )
}

function isMutationResponseValidForIdentity(
  output: McpServerDetailsOutput,
  identity: ServerIdentity,
  allowConfigChange: boolean
): boolean {
  const server = output.server
  if (
    output.registryRevision < server.registryRevision ||
    server.serverId !== identity.serverId ||
    server.registryRevision < identity.registryRevision ||
    server.updatedAtMs < identity.updatedAtMs
  ) {
    return false
  }

  const sameConfig =
    server.configEpoch === identity.configEpoch && server.configDigest === identity.configDigest
  if (sameConfig) return server.catalogGeneration >= identity.catalogGeneration
  return allowConfigChange && server.registryRevision > identity.registryRevision
}

function isMutationResponseValidForOperation(
  output: McpServerDetailsOutput,
  identity: ServerIdentity,
  operation: McpServerPendingOperation
): boolean {
  const allowConfigChange =
    operation === 'disable' || operation === 'enable' || operation === 'update'
  if (!isMutationResponseValidForIdentity(output, identity, allowConfigChange)) return false
  if (
    operation === 'enable' &&
    (!output.server.enabled ||
      output.server.trust !== 'userApproved' ||
      output.server.launchAuthorizationState !== 'authorized')
  ) {
    return false
  }
  if (operation === 'disable' && output.server.enabled) return false
  if ((operation === 'start' || operation === 'restart') && !output.server.enabled) return false
  return true
}

function mergeDetailsWithFreshSummary(
  details: McpServerDetailsView,
  summary: McpServerListItem
): McpServerDetailsView {
  const detailsSummary = detailsToListItem(details)
  return shouldUseIncomingServerSnapshot(summary, detailsSummary)
    ? details
    : mergeServerSummaryIntoDetails(details, summary)
}

function sameServerConfigIdentity(left: McpServerListItem, right: McpServerListItem): boolean {
  return left.configEpoch === right.configEpoch && left.configDigest === right.configDigest
}

function sameServerRegistryIdentity(left: McpServerListItem, right: McpServerListItem): boolean {
  return sameServerConfigIdentity(left, right) && left.registryRevision === right.registryRevision
}

function shouldUseIncomingServerSnapshot(
  current: McpServerListItem,
  incoming: McpServerListItem
): boolean {
  if (!sameServerConfigIdentity(current, incoming)) {
    return (
      incoming.registryRevision > current.registryRevision &&
      incoming.updatedAtMs >= current.updatedAtMs
    )
  }
  if (
    incoming.registryRevision < current.registryRevision ||
    incoming.updatedAtMs < current.updatedAtMs ||
    incoming.catalogGeneration < current.catalogGeneration
  ) {
    return false
  }
  return (
    incoming.registryRevision > current.registryRevision ||
    incoming.updatedAtMs > current.updatedAtMs ||
    incoming.catalogGeneration > current.catalogGeneration
  )
}

function canUseAuthoritativeRuntimeSnapshot(
  current: McpServerListItem,
  incoming: McpServerListItem
): boolean {
  return (
    sameServerRegistryIdentity(current, incoming) &&
    incoming.updatedAtMs >= current.updatedAtMs &&
    incoming.catalogGeneration >= current.catalogGeneration
  )
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

function applyDetailsOutput(
  outputRef: MutableRefObject<McpServerListOutput | null>,
  setState: Dispatch<SetStateAction<McpManagementViewState>>,
  detailsRef: MutableRefObject<ReadonlyMap<string, McpServerDetailsView>>,
  setDetails: Dispatch<SetStateAction<ReadonlyMap<string, McpServerDetailsView>>>,
  catalogsRef: MutableRefObject<ReadonlyMap<string, McpToolCatalogState>>,
  setCatalogs: Dispatch<SetStateAction<ReadonlyMap<string, McpToolCatalogState>>>,
  detailsOutput: McpServerDetailsOutput,
  acceptAuthoritativeRuntimeSnapshot = false
): McpServerDetailsView | null {
  if (detailsOutput.registryRevision < detailsOutput.server.registryRevision) return null

  const current = outputRef.current
  const previous = currentServer(current, detailsOutput.server.serverId)
  if (current && !previous && detailsOutput.registryRevision < current.registryRevision) {
    return null
  }
  const incomingListItem = detailsToListItem(detailsOutput.server)
  if (
    previous &&
    !shouldUseIncomingServerSnapshot(previous, incomingListItem) &&
    !(
      acceptAuthoritativeRuntimeSnapshot &&
      canUseAuthoritativeRuntimeSnapshot(previous, incomingListItem)
    )
  ) {
    if (!sameServerConfigIdentity(previous, incomingListItem)) {
      return detailsRef.current.get(previous.serverId) ?? null
    }
    const existingDetails = detailsRef.current.get(previous.serverId)
    const merged = mergeServerSummaryIntoDetails(existingDetails ?? detailsOutput.server, previous)
    setServerDetails(detailsRef, setDetails, merged)
    return merged
  }

  if (previous && !sameServerRegistryIdentity(previous, incomingListItem)) {
    const catalogs = new Map(catalogsRef.current)
    catalogs.delete(detailsOutput.server.serverId)
    catalogsRef.current = catalogs
    setCatalogs(catalogs)
  }
  setServerDetails(detailsRef, setDetails, detailsOutput.server)
  const servers = current?.servers.some((server) => server.serverId === incomingListItem.serverId)
    ? current.servers.map((server) =>
        server.serverId === incomingListItem.serverId ? incomingListItem : server
      )
    : [...(current?.servers ?? []), incomingListItem]
  const output: McpServerListOutput = {
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    registryRevision: Math.max(current?.registryRevision ?? 0, detailsOutput.registryRevision),
    servers
  }
  outputRef.current = output
  setState({ errorMessage: null, isRefreshing: false, output, status: 'ready' })
  return detailsOutput.server
}

function detailsToListItem(server: McpServerDetailsView): McpServerListItem {
  return {
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    serverId: server.serverId,
    displayName: server.displayName,
    scope: server.scope,
    source: server.source,
    transport: server.transport,
    trust: server.trust,
    approvalMode: server.approvalMode,
    registryRevision: server.registryRevision,
    configEpoch: server.configEpoch,
    configDigest: server.configDigest,
    state: server.state,
    enabled: server.enabled,
    launchAuthorizationState: server.launchAuthorizationState,
    catalogGeneration: server.catalogGeneration,
    catalogCompleteness: server.catalogCompleteness,
    toolCount: server.toolCount,
    activeCallCount: server.activeCallCount,
    ...(server.lastError === undefined
      ? {}
      : {
          lastError: {
            code: server.lastError.code,
            message: server.lastError.message
          }
        }),
    updatedAtMs: server.updatedAtMs
  }
}

function setServerDetails(
  detailsRef: MutableRefObject<ReadonlyMap<string, McpServerDetailsView>>,
  setDetails: Dispatch<SetStateAction<ReadonlyMap<string, McpServerDetailsView>>>,
  server: McpServerDetailsView
): void {
  const next = new Map(detailsRef.current).set(server.serverId, server)
  detailsRef.current = next
  setDetails(next)
}

function setCatalogState(
  catalogsRef: MutableRefObject<ReadonlyMap<string, McpToolCatalogState>>,
  setCatalogs: Dispatch<SetStateAction<ReadonlyMap<string, McpToolCatalogState>>>,
  serverId: string,
  value: McpToolCatalogState
): void {
  const next = new Map(catalogsRef.current).set(serverId, value)
  catalogsRef.current = next
  setCatalogs(next)
}

function failClosedCatalog(
  catalogsRef: MutableRefObject<ReadonlyMap<string, McpToolCatalogState>>,
  setCatalogs: Dispatch<SetStateAction<ReadonlyMap<string, McpToolCatalogState>>>,
  identity: ServerIdentity
): void {
  setCatalogState(catalogsRef, setCatalogs, identity.serverId, {
    ...IDLE_CATALOG,
    serverConfigDigest: identity.configDigest,
    serverConfigEpoch: identity.configEpoch,
    serverRegistryRevision: identity.registryRevision
  })
}

function catalogMatchesIdentity(catalog: McpToolCatalogState, identity: ServerIdentity): boolean {
  return (
    catalog.serverConfigEpoch === identity.configEpoch &&
    catalog.serverConfigDigest === identity.configDigest &&
    catalog.serverRegistryRevision === identity.registryRevision
  )
}

function isCatalogPageProjectionValid(
  page: McpCatalogToolsPageOutput,
  expectedServerId: string
): boolean {
  return (
    page.serverId === expectedServerId &&
    page.tools.every(
      (tool) =>
        tool.serverId === expectedServerId &&
        tool.catalogGeneration === page.catalogGeneration &&
        tool.catalogCompleteness === page.catalogCompleteness
    )
  )
}

function catalogPageToState(
  page: McpCatalogToolsPageOutput,
  identity: ServerIdentity,
  previous?: McpToolCatalogState
): McpToolCatalogState {
  const previousTools =
    previous?.catalogGeneration === page.catalogGeneration &&
    previous.serverConfigEpoch === identity.configEpoch &&
    previous.serverConfigDigest === identity.configDigest &&
    previous.serverRegistryRevision === identity.registryRevision
      ? previous.tools
      : []
  const known = new Set(previousTools.map((tool) => `${tool.rawName}\u0000${tool.modelName}`))
  const appended = page.tools.filter((tool) => !known.has(`${tool.rawName}\u0000${tool.modelName}`))
  return {
    catalogCompleteness: page.catalogCompleteness,
    catalogGeneration: page.catalogGeneration,
    errorMessage: null,
    isRefreshing: false,
    ...(page.nextCursor === undefined ? {} : { nextCursor: page.nextCursor }),
    serverConfigDigest: identity.configDigest,
    serverConfigEpoch: identity.configEpoch,
    serverRegistryRevision: identity.registryRevision,
    status: 'ready',
    tools: [...previousTools, ...appended]
  }
}

function pruneServerState(
  output: McpServerListOutput,
  detailsRef: MutableRefObject<ReadonlyMap<string, McpServerDetailsView>>,
  setDetails: Dispatch<SetStateAction<ReadonlyMap<string, McpServerDetailsView>>>,
  catalogsRef: MutableRefObject<ReadonlyMap<string, McpToolCatalogState>>,
  setCatalogs: Dispatch<SetStateAction<ReadonlyMap<string, McpToolCatalogState>>>
): void {
  const ids = new Set(output.servers.map((server) => server.serverId))
  const details = new Map<string, McpServerDetailsView>()
  for (const [serverId, value] of detailsRef.current) {
    const server = output.servers.find((item) => item.serverId === serverId)
    if (
      ids.has(serverId) &&
      server &&
      server.configEpoch === value.configEpoch &&
      server.configDigest === value.configDigest &&
      server.registryRevision === value.registryRevision
    ) {
      details.set(serverId, mergeServerSummaryIntoDetails(value, server))
    }
  }
  const catalogs = new Map(
    [...catalogsRef.current].filter(([serverId, catalog]) =>
      output.servers.some(
        (server) =>
          server.serverId === serverId &&
          server.configEpoch === catalog.serverConfigEpoch &&
          server.configDigest === catalog.serverConfigDigest &&
          server.registryRevision === catalog.serverRegistryRevision
      )
    )
  )
  detailsRef.current = details
  catalogsRef.current = catalogs
  setDetails(details)
  setCatalogs(catalogs)
}

function mergeServerSummaryIntoDetails(
  details: McpServerDetailsView,
  summary: McpServerListItem
): McpServerDetailsView {
  return {
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    serverId: summary.serverId,
    displayName: summary.displayName,
    scope: summary.scope,
    source: summary.source,
    transport: summary.transport,
    trust: summary.trust,
    approvalMode: summary.approvalMode,
    registryRevision: summary.registryRevision,
    configEpoch: summary.configEpoch,
    configDigest: summary.configDigest,
    state: summary.state,
    enabled: summary.enabled,
    launchAuthorizationState: summary.launchAuthorizationState,
    catalogGeneration: summary.catalogGeneration,
    catalogCompleteness: summary.catalogCompleteness,
    toolCount: summary.toolCount,
    activeCallCount: summary.activeCallCount,
    ...(summary.lastError === undefined
      ? {}
      : {
          lastError: {
            code: summary.lastError.code,
            message: summary.lastError.message
          }
        }),
    updatedAtMs: summary.updatedAtMs,
    executable: details.executable,
    arguments: [...details.arguments],
    cwd: details.cwd,
    ...(details.protocol === undefined
      ? {}
      : {
          protocol: {
            protocolVersion: details.protocol.protocolVersion,
            lifecycle: details.protocol.lifecycle
          }
        }),
    ...(details.capabilities === undefined
      ? {}
      : {
          capabilities: {
            tools: details.capabilities.tools,
            resources: details.capabilities.resources,
            prompts: details.capabilities.prompts,
            logging: details.capabilities.logging,
            completion: details.capabilities.completion
          }
        }),
    createdAtMs: details.createdAtMs
  }
}

function mergeServerListOutput(
  current: McpServerListOutput | null,
  incoming: McpServerListOutput
): McpServerListOutput {
  if (!current) return incoming
  if (incoming.registryRevision < current.registryRevision) return current

  const incomingIds = new Set(incoming.servers.map((server) => server.serverId))
  const servers = incoming.servers.map((server) => {
    const existing = current.servers.find((item) => item.serverId === server.serverId)
    if (!existing) return server
    return shouldUseIncomingServerSnapshot(existing, server) ||
      canUseAuthoritativeRuntimeSnapshot(existing, server)
      ? server
      : existing
  })
  if (incoming.registryRevision === current.registryRevision) {
    for (const existing of current.servers) {
      if (!incomingIds.has(existing.serverId)) servers.push(existing)
    }
  }
  return {
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    registryRevision: incoming.registryRevision,
    servers
  }
}

function removeServerState(
  outputRef: MutableRefObject<McpServerListOutput | null>,
  setState: Dispatch<SetStateAction<McpManagementViewState>>,
  detailsRef: MutableRefObject<ReadonlyMap<string, McpServerDetailsView>>,
  setDetails: Dispatch<SetStateAction<ReadonlyMap<string, McpServerDetailsView>>>,
  catalogsRef: MutableRefObject<ReadonlyMap<string, McpToolCatalogState>>,
  setCatalogs: Dispatch<SetStateAction<ReadonlyMap<string, McpToolCatalogState>>>,
  serverId: string,
  registryRevision: number
): void {
  const current = outputRef.current
  if (current) {
    const output: McpServerListOutput = {
      schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
      registryRevision: Math.max(current.registryRevision, registryRevision),
      servers: current.servers.filter((server) => server.serverId !== serverId)
    }
    outputRef.current = output
    setState({ errorMessage: null, isRefreshing: false, output, status: 'ready' })
  }
  const details = new Map(detailsRef.current)
  details.delete(serverId)
  detailsRef.current = details
  setDetails(details)
  const catalogs = new Map(catalogsRef.current)
  catalogs.delete(serverId)
  catalogsRef.current = catalogs
  setCatalogs(catalogs)
}
