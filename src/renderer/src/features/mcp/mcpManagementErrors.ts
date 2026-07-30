import { HostInvocationError } from '@mycopilot/host-api'
import {
  parseMcpManagementErrorData,
  type McpManagementErrorCode,
  type McpManagementOperation,
  type McpManagementRecovery
} from '@mycopilot/protocol'

const FALLBACK_MESSAGE = ''

export interface McpManagementErrorDetails {
  code?: McpManagementErrorCode
  currentRegistryRevision?: number
  message: string
  operation?: McpManagementOperation
  recovery?: McpManagementRecovery
  serverId?: string
}

export function getMcpManagementErrorDetails(error: unknown): McpManagementErrorDetails {
  if (!(error instanceof HostInvocationError)) {
    return { message: FALLBACK_MESSAGE }
  }

  try {
    const data = parseMcpManagementErrorData(error.data)
    return {
      code: data.code,
      message: data.message,
      operation: data.operation,
      recovery: data.recovery,
      ...(data.serverId === undefined ? {} : { serverId: data.serverId }),
      ...(data.currentRegistryRevision === undefined
        ? {}
        : { currentRegistryRevision: data.currentRegistryRevision })
    }
  } catch {
    return { message: FALLBACK_MESSAGE }
  }
}

export function shouldRefreshMcpAfterError(details: McpManagementErrorDetails): boolean {
  return (
    details.code === 'conflict' ||
    details.code === 'notFound' ||
    details.code === 'authorizationStale' ||
    details.recovery === 'refresh'
  )
}

export function mcpOperationNeedsAuthoritativeConfirmation(
  details: McpManagementErrorDetails
): boolean {
  return details.code === 'cleanupIncomplete'
}
