// Synchronous projections and publication; request lifetimes and mutation sequencing stay in the hook.
import type { Dispatch, MutableRefObject, SetStateAction } from 'react'
import {
  MCP_MANAGEMENT_SCHEMA_VERSION,
  type McpCatalogToolsPageOutput,
  type McpServerDetailsOutput,
  type McpServerDetailsView,
  type McpServerListItem,
  type McpServerListOutput
} from '@mycopilot/protocol'
import {
  canUseAuthoritativeRuntimeSnapshot,
  currentServer,
  sameServerConfigIdentity,
  sameServerRegistryIdentity,
  shouldUseIncomingServerSnapshot,
  type ServerIdentity
} from './mcpManagementIdentity'
import type { McpManagementViewState, McpToolCatalogState } from './mcpManagementTypes'

export const INITIAL_STATE: McpManagementViewState = {
  errorMessage: null,
  isRefreshing: false,
  output: null,
  status: 'loading'
}

export const IDLE_CATALOG: McpToolCatalogState = {
  errorMessage: null,
  isRefreshing: false,
  status: 'idle',
  tools: []
}

export function mergeDetailsWithFreshSummary(
  details: McpServerDetailsView,
  summary: McpServerListItem
): McpServerDetailsView {
  const detailsSummary = detailsToListItem(details)
  const merged = shouldUseIncomingServerSnapshot(summary, detailsSummary)
    ? details
    : mergeServerSummaryIntoDetails(details, summary)
  // A one-Server detail read performs the live executable/code identity
  // check. The list projection is intentionally structural and may still say
  // `authorized`, so it must never overwrite a fresher `stale` result here.
  return merged.launchAuthorizationState === details.launchAuthorizationState
    ? merged
    : { ...merged, launchAuthorizationState: details.launchAuthorizationState }
}

export function applyDetailsOutput(
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

export function setServerDetails(
  detailsRef: MutableRefObject<ReadonlyMap<string, McpServerDetailsView>>,
  setDetails: Dispatch<SetStateAction<ReadonlyMap<string, McpServerDetailsView>>>,
  server: McpServerDetailsView
): void {
  const next = new Map(detailsRef.current).set(server.serverId, server)
  detailsRef.current = next
  setDetails(next)
}

export function setCatalogState(
  catalogsRef: MutableRefObject<ReadonlyMap<string, McpToolCatalogState>>,
  setCatalogs: Dispatch<SetStateAction<ReadonlyMap<string, McpToolCatalogState>>>,
  serverId: string,
  value: McpToolCatalogState
): void {
  const next = new Map(catalogsRef.current).set(serverId, value)
  catalogsRef.current = next
  setCatalogs(next)
}

export function failClosedCatalog(
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

export function catalogMatchesIdentity(
  catalog: McpToolCatalogState,
  identity: ServerIdentity
): boolean {
  return (
    catalog.serverConfigEpoch === identity.configEpoch &&
    catalog.serverConfigDigest === identity.configDigest &&
    catalog.serverRegistryRevision === identity.registryRevision
  )
}

export function isCatalogPageProjectionValid(
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

export function catalogPageToState(
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

export function pruneServerState(
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

export function mergeServerListOutput(
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

export function removeServerState(
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
