import { HostInvocationError } from '@mycopilot/host-api'
import type {
  McpCatalogToolsPageInput,
  McpCatalogToolsPageOutput,
  McpChangedNotification,
  McpLaunchAuthorizationResult,
  McpServerCreateInput,
  McpServerDetailsOutput,
  McpServerIdInput,
  McpServerListOutput,
  McpServerMutationInput,
  McpServerUpdateInput
} from '@mycopilot/protocol'
import { hostClient } from '../../host/hostClient'

export async function listMcpServers(): Promise<McpServerListOutput> {
  const result = await hostClient.mcp.listServers()
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}

export async function getMcpServer(input: McpServerIdInput): Promise<McpServerDetailsOutput> {
  const result = await hostClient.mcp.getServer(input)
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}

export async function addMcpServer(input: McpServerCreateInput): Promise<McpServerDetailsOutput> {
  const result = await hostClient.mcp.addServer(input)
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}

export async function updateMcpServer(
  input: McpServerUpdateInput
): Promise<McpServerDetailsOutput> {
  const result = await hostClient.mcp.updateServer(input)
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}

export async function deleteMcpServer(
  input: McpServerMutationInput
): Promise<McpServerDetailsOutput> {
  const result = await hostClient.mcp.deleteServer(input)
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}

export async function requestMcpLaunchAuthorization(
  input: McpServerMutationInput
): Promise<McpLaunchAuthorizationResult> {
  const result = await hostClient.mcp.requestLaunchAuthorization(input)
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}

export async function enableMcpServer(
  input: McpServerMutationInput
): Promise<McpServerDetailsOutput> {
  const result = await hostClient.mcp.enableServer(input)
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}

export async function disableMcpServer(
  input: McpServerMutationInput
): Promise<McpServerDetailsOutput> {
  const result = await hostClient.mcp.disableServer(input)
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}

export async function startMcpServer(
  input: McpServerMutationInput
): Promise<McpServerDetailsOutput> {
  const result = await hostClient.mcp.startServer(input)
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}

export async function stopMcpServer(
  input: McpServerMutationInput
): Promise<McpServerDetailsOutput> {
  const result = await hostClient.mcp.stopServer(input)
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}

export async function restartMcpServer(
  input: McpServerMutationInput
): Promise<McpServerDetailsOutput> {
  const result = await hostClient.mcp.restartServer(input)
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}

export async function getMcpServerStatus(input: McpServerIdInput): Promise<McpServerDetailsOutput> {
  const result = await hostClient.mcp.getStatus(input)
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}

export async function listMcpTools(
  input: McpCatalogToolsPageInput
): Promise<McpCatalogToolsPageOutput> {
  const result = await hostClient.mcp.listTools(input)
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}

export async function refreshMcpCatalog(
  input: McpServerMutationInput
): Promise<McpCatalogToolsPageOutput> {
  const result = await hostClient.mcp.refreshCatalog(input)
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}

export function selectMcpExecutable(): Promise<string | null> {
  return hostClient.mcp.selectExecutable()
}

export function selectMcpWorkingDirectory(): Promise<string | null> {
  return hostClient.mcp.selectWorkingDirectory()
}

export function onMcpChanged(handler: (event: McpChangedNotification) => void): () => void {
  return hostClient.mcp.onChanged(handler)
}
