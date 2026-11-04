// Identity and version checks shared by reads, mutations, and state projections.
import type {
  McpServerDetailsOutput,
  McpServerListItem,
  McpServerListOutput
} from '@mycopilot/protocol'
import type { McpServerPendingOperation } from './mcpManagementTypes'

export interface ServerIdentity {
  catalogGeneration: number
  configDigest: string
  configEpoch: string
  registryRevision: number
  serverId: string
  updatedAtMs: number
}

export function serverIdentity(server: McpServerListItem): ServerIdentity {
  return {
    catalogGeneration: server.catalogGeneration,
    configDigest: server.configDigest,
    configEpoch: server.configEpoch,
    registryRevision: server.registryRevision,
    serverId: server.serverId,
    updatedAtMs: server.updatedAtMs
  }
}

export function matchesCurrentServer(
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

export function currentServer(
  output: McpServerListOutput | null,
  serverId: string
): McpServerListItem | undefined {
  return output?.servers.find((server) => server.serverId === serverId)
}

export function isExactDetailsResponseForIdentity(
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

export function isMutationResponseValidForIdentity(
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

export function isMutationResponseValidForOperation(
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

export function sameServerConfigIdentity(
  left: McpServerListItem,
  right: McpServerListItem
): boolean {
  return left.configEpoch === right.configEpoch && left.configDigest === right.configDigest
}

export function sameServerRegistryIdentity(
  left: McpServerListItem,
  right: McpServerListItem
): boolean {
  return sameServerConfigIdentity(left, right) && left.registryRevision === right.registryRevision
}

export function shouldUseIncomingServerSnapshot(
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

export function canUseAuthoritativeRuntimeSnapshot(
  current: McpServerListItem,
  incoming: McpServerListItem
): boolean {
  return (
    sameServerRegistryIdentity(current, incoming) &&
    incoming.updatedAtMs >= current.updatedAtMs &&
    incoming.catalogGeneration >= current.catalogGeneration
  )
}
