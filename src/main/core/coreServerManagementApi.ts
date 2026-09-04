import { createHash } from 'node:crypto'
import type { ImageGenerationArtifactContent } from '@mycopilot/host-api'
import type {
  BrowserRiskAuthorizeInput,
  BrowserRiskAuthorizeOutput,
  BrowserRiskCancelInput,
  ImageGenerationArtifactReadInput,
  ImageGenerationArtifactReadOutput,
  ImageGenerationGetConfigurationOutput,
  ImageGenerationSetEnabledInput,
  ImageGenerationSetEnabledOutput,
  ImageGenerationStatus,
  ImageGenerationUpdateConfigurationInput,
  ImageGenerationUpdateConfigurationOutput,
  ManagedArtifactReadIdentity,
  ManagedPlaywrightCancelNotification,
  ManagedPlaywrightCommandNotification,
  ManagedPlaywrightCompletionInput,
  ManagedPlaywrightDispatchPhaseInput,
  McpBuiltinCapabilityListOutput,
  McpBuiltinCapabilityMutationOutput,
  McpBuiltinCapabilitySetAllowedInput,
  McpCatalogToolsPageInput,
  McpCatalogToolsPageOutput,
  McpChangedNotification,
  McpLaunchAuthorizationCommitInput,
  McpLaunchAuthorizationPreview,
  McpLaunchAuthorizationResult,
  McpManagementErrorData,
  McpServerCreateInput,
  McpServerDetailsOutput,
  McpServerIdInput,
  McpServerListOutput,
  McpServerMutationInput,
  McpServerUpdateInput,
  OfficeEngineStatus
} from '@mycopilot/protocol'
import {
  BROWSER_RISK_AUTHORIZE_METHOD,
  BROWSER_RISK_CANCEL_METHOD,
  IMAGE_GENERATION_ARTIFACT_ERROR_CODE,
  IMAGE_GENERATION_CONFIGURATION_ERROR_CODE,
  IMAGE_GENERATION_GET_CONFIGURATION_METHOD,
  IMAGE_GENERATION_GET_STATUS_METHOD,
  IMAGE_GENERATION_READ_ARTIFACT_METHOD,
  IMAGE_GENERATION_SET_ENABLED_METHOD,
  IMAGE_GENERATION_UPDATE_CONFIGURATION_METHOD,
  MANAGED_PLAYWRIGHT_CANCEL_NOTIFICATION_METHOD,
  MANAGED_PLAYWRIGHT_COMMAND_NOTIFICATION_METHOD,
  MANAGED_PLAYWRIGHT_COMPLETE_METHOD,
  MANAGED_PLAYWRIGHT_DISPATCH_PHASE_METHOD,
  MCP_BUILTIN_CAPABILITY_LIST_METHOD,
  MCP_BUILTIN_CAPABILITY_SET_ALLOWED_METHOD,
  MCP_CATALOG_REFRESH_METHOD,
  MCP_CATALOG_TOOLS_METHOD,
  MCP_CHANGED_NOTIFICATION_METHOD,
  MCP_MANAGEMENT_ERROR_CODE,
  MCP_MANAGEMENT_SCHEMA_VERSION,
  MCP_SERVER_ADD_METHOD,
  MCP_SERVER_AUTHORIZE_LAUNCH_COMMIT_METHOD,
  MCP_SERVER_AUTHORIZE_LAUNCH_PREPARE_METHOD,
  MCP_SERVER_DELETE_METHOD,
  MCP_SERVER_DISABLE_METHOD,
  MCP_SERVER_ENABLE_METHOD,
  MCP_SERVER_GET_METHOD,
  MCP_SERVER_LIST_METHOD,
  MCP_SERVER_RESTART_METHOD,
  MCP_SERVER_START_METHOD,
  MCP_SERVER_STATUS_METHOD,
  MCP_SERVER_STOP_METHOD,
  MCP_SERVER_UPDATE_METHOD,
  OFFICE_GET_STATUS_METHOD,
  parseBrowserRiskAuthorizeInput,
  parseBrowserRiskAuthorizeOutput,
  parseBrowserRiskCancelInput,
  parseBrowserRiskCancelOutput,
  parseImageGenerationArtifactErrorData,
  parseImageGenerationArtifactReadInput,
  parseImageGenerationArtifactReadOutput,
  parseImageGenerationConfigurationErrorData,
  parseImageGenerationGetConfigurationOutput,
  parseImageGenerationSetEnabledInput,
  parseImageGenerationSetEnabledOutput,
  parseImageGenerationStatus,
  parseImageGenerationUpdateConfigurationInput,
  parseImageGenerationUpdateConfigurationOutput,
  parseManagedPlaywrightCancelNotification,
  parseManagedPlaywrightCommandNotification,
  parseManagedPlaywrightCompletionInput,
  parseManagedPlaywrightCompletionOutput,
  parseManagedPlaywrightDispatchPhaseInput,
  parseManagedPlaywrightDispatchPhaseOutput,
  parseMcpBuiltinCapabilityListOutput,
  parseMcpBuiltinCapabilityMutationOutput,
  parseMcpBuiltinCapabilitySetAllowedInput,
  parseMcpCatalogToolsPageInput,
  parseMcpCatalogToolsPageOutput,
  parseMcpChangedNotification,
  parseMcpLaunchAuthorizationCommitInput,
  parseMcpLaunchAuthorizationPreview,
  parseMcpLaunchAuthorizationResult,
  parseMcpManagementErrorData,
  parseMcpServerCreateInput,
  parseMcpServerDetailsOutput,
  parseMcpServerIdInput,
  parseMcpServerListOutput,
  parseMcpServerMutationInput,
  parseMcpServerUpdateInput,
  parseOfficeEngineStatus
} from '@mycopilot/protocol'

import { CoreServerAgentApi } from './coreServerAgentApi'

function rethrowValidatedImageGenerationConfigurationError(error: unknown): never {
  if (
    typeof error !== 'object' ||
    error === null ||
    Array.isArray(error) ||
    !('code' in error) ||
    error.code !== IMAGE_GENERATION_CONFIGURATION_ERROR_CODE
  ) {
    throw error
  }

  const data = parseImageGenerationConfigurationErrorData('data' in error ? error.data : undefined)
  // The validated, bounded domain message is authoritative. A provider or transport error message
  // must never be forwarded because it could contain a URL, response body, or credential material.
  throw Object.assign(new Error(data.message), {
    name: 'ImageGenerationConfigurationError',
    code: IMAGE_GENERATION_CONFIGURATION_ERROR_CODE,
    data
  })
}

function rethrowValidatedImageGenerationArtifactError(error: unknown): never {
  if (
    typeof error !== 'object' ||
    error === null ||
    Array.isArray(error) ||
    !('code' in error) ||
    error.code !== IMAGE_GENERATION_ARTIFACT_ERROR_CODE
  ) {
    throw error
  }

  const data = parseImageGenerationArtifactErrorData('data' in error ? error.data : undefined)
  throw Object.assign(new Error(data.message), {
    name: 'ImageGenerationArtifactError',
    code: IMAGE_GENERATION_ARTIFACT_ERROR_CODE,
    data
  })
}

function rethrowValidatedMcpManagementError(error: unknown): never {
  if (
    typeof error !== 'object' ||
    error === null ||
    Array.isArray(error) ||
    !('code' in error) ||
    error.code !== MCP_MANAGEMENT_ERROR_CODE
  ) {
    throw error
  }

  const data: McpManagementErrorData = parseMcpManagementErrorData(
    'data' in error ? error.data : undefined
  )
  // Only the bounded, protocol-validated Host projection may cross Main. Core/Server messages can
  // contain process details and are deliberately not forwarded.
  throw Object.assign(new Error(data.message), {
    name: 'McpManagementError',
    code: MCP_MANAGEMENT_ERROR_CODE,
    data
  })
}

function validatedImageGenerationArtifactContent(
  output: ImageGenerationArtifactReadOutput,
  expected: ManagedArtifactReadIdentity
): ImageGenerationArtifactContent {
  if (!sameManagedArtifact(output.artifact, expected)) {
    throw new Error('Invalid Image generation Artifact read response: frozen identity changed')
  }
  const bytes = Buffer.from(output.dataBase64, 'base64')
  if (
    bytes.byteLength !== output.artifact.sizeBytes ||
    bytes.toString('base64') !== output.dataBase64
  ) {
    throw new Error('Invalid Image generation Artifact read response: byte length changed')
  }
  const sha256 = createHash('sha256').update(bytes).digest('hex')
  if (sha256 !== output.artifact.sha256) {
    throw new Error('Invalid Image generation Artifact read response: content digest changed')
  }
  return {
    schemaVersion: output.schemaVersion,
    artifact: output.artifact,
    fileName: output.fileName,
    bytes: Uint8Array.from(bytes)
  }
}

function sameManagedArtifact(
  left: ManagedArtifactReadIdentity,
  right: ManagedArtifactReadIdentity
): boolean {
  if (
    left.artifactId !== right.artifactId ||
    left.uri !== right.uri ||
    left.kind !== right.kind ||
    left.format !== right.format ||
    left.mimeType !== right.mimeType ||
    left.sizeBytes !== right.sizeBytes ||
    left.sha256 !== right.sha256
  ) {
    return false
  }
  if (left.kind === 'image' && right.kind === 'image') {
    return left.width === right.width && left.height === right.height
  }
  return left.kind === 'document' && right.kind === 'document'
}

/** Host-management request facade; JSON-RPC process lifecycle remains owned by CoreServer. */
export class CoreServerManagementApi extends CoreServerAgentApi {
  getOfficeStatus(): Promise<OfficeEngineStatus> {
    return this.rpc.request<unknown>(OFFICE_GET_STATUS_METHOD).then(parseOfficeEngineStatus)
  }

  getImageGenerationConfiguration(): Promise<ImageGenerationGetConfigurationOutput> {
    return this.rpc
      .request<unknown>(IMAGE_GENERATION_GET_CONFIGURATION_METHOD)
      .then(parseImageGenerationGetConfigurationOutput)
      .catch(rethrowValidatedImageGenerationConfigurationError)
  }

  updateImageGenerationConfiguration(
    input: ImageGenerationUpdateConfigurationInput
  ): Promise<ImageGenerationUpdateConfigurationOutput> {
    const request = parseImageGenerationUpdateConfigurationInput(input)
    return this.rpc
      .request<unknown, ImageGenerationUpdateConfigurationInput>(
        IMAGE_GENERATION_UPDATE_CONFIGURATION_METHOD,
        request
      )
      .then(parseImageGenerationUpdateConfigurationOutput)
      .catch(rethrowValidatedImageGenerationConfigurationError)
  }

  setImageGenerationEnabled(
    input: ImageGenerationSetEnabledInput
  ): Promise<ImageGenerationSetEnabledOutput> {
    const request = parseImageGenerationSetEnabledInput(input)
    return this.rpc
      .request<unknown, ImageGenerationSetEnabledInput>(
        IMAGE_GENERATION_SET_ENABLED_METHOD,
        request
      )
      .then(parseImageGenerationSetEnabledOutput)
      .catch(rethrowValidatedImageGenerationConfigurationError)
  }

  getImageGenerationStatus(): Promise<ImageGenerationStatus> {
    return this.rpc
      .request<unknown>(IMAGE_GENERATION_GET_STATUS_METHOD)
      .then(parseImageGenerationStatus)
      .catch(rethrowValidatedImageGenerationConfigurationError)
  }

  readImageGenerationArtifact(
    input: ImageGenerationArtifactReadInput
  ): Promise<ImageGenerationArtifactContent> {
    const request = parseImageGenerationArtifactReadInput(input)
    return this.rpc
      .request<unknown, ImageGenerationArtifactReadInput>(
        IMAGE_GENERATION_READ_ARTIFACT_METHOD,
        request
      )
      .then(parseImageGenerationArtifactReadOutput)
      .then((output) => validatedImageGenerationArtifactContent(output, request.artifact))
      .catch(rethrowValidatedImageGenerationArtifactError)
  }

  listMcpServers(): Promise<McpServerListOutput> {
    return this.rpc
      .request<unknown, { schemaVersion: typeof MCP_MANAGEMENT_SCHEMA_VERSION }>(
        MCP_SERVER_LIST_METHOD,
        { schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION }
      )
      .then(parseMcpServerListOutput)
      .catch(rethrowValidatedMcpManagementError)
  }

  listMcpBuiltinCapabilities(): Promise<McpBuiltinCapabilityListOutput> {
    return this.rpc
      .request<unknown, { schemaVersion: typeof MCP_MANAGEMENT_SCHEMA_VERSION }>(
        MCP_BUILTIN_CAPABILITY_LIST_METHOD,
        { schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION }
      )
      .then(parseMcpBuiltinCapabilityListOutput)
      .catch(rethrowValidatedMcpManagementError)
  }

  setMcpBuiltinCapabilityAllowed(
    input: McpBuiltinCapabilitySetAllowedInput
  ): Promise<McpBuiltinCapabilityMutationOutput> {
    const request = parseMcpBuiltinCapabilitySetAllowedInput(input)
    return this.rpc
      .request<unknown, McpBuiltinCapabilitySetAllowedInput>(
        MCP_BUILTIN_CAPABILITY_SET_ALLOWED_METHOD,
        request
      )
      .then(parseMcpBuiltinCapabilityMutationOutput)
      .catch(rethrowValidatedMcpManagementError)
  }

  getMcpServer(input: McpServerIdInput): Promise<McpServerDetailsOutput> {
    const request = parseMcpServerIdInput(input)
    return this.rpc
      .request<unknown, McpServerIdInput>(MCP_SERVER_GET_METHOD, request)
      .then(parseMcpServerDetailsOutput)
      .catch(rethrowValidatedMcpManagementError)
  }

  addMcpServer(input: McpServerCreateInput): Promise<McpServerDetailsOutput> {
    const request = parseMcpServerCreateInput(input)
    return this.rpc
      .request<unknown, McpServerCreateInput>(MCP_SERVER_ADD_METHOD, request)
      .then(parseMcpServerDetailsOutput)
      .catch(rethrowValidatedMcpManagementError)
  }

  updateMcpServer(input: McpServerUpdateInput): Promise<McpServerDetailsOutput> {
    const request = parseMcpServerUpdateInput(input)
    return this.rpc
      .request<unknown, McpServerUpdateInput>(MCP_SERVER_UPDATE_METHOD, request)
      .then(parseMcpServerDetailsOutput)
      .catch(rethrowValidatedMcpManagementError)
  }

  deleteMcpServer(input: McpServerMutationInput): Promise<McpServerDetailsOutput> {
    const request = parseMcpServerMutationInput(input)
    return this.rpc
      .request<unknown, McpServerMutationInput>(MCP_SERVER_DELETE_METHOD, request)
      .then(parseMcpServerDetailsOutput)
      .catch(rethrowValidatedMcpManagementError)
  }

  prepareMcpLaunchAuthorization(
    input: McpServerMutationInput
  ): Promise<McpLaunchAuthorizationPreview> {
    const request = parseMcpServerMutationInput(input)
    return this.rpc
      .request<unknown, McpServerMutationInput>(MCP_SERVER_AUTHORIZE_LAUNCH_PREPARE_METHOD, request)
      .then(parseMcpLaunchAuthorizationPreview)
      .catch(rethrowValidatedMcpManagementError)
  }

  commitMcpLaunchAuthorization(
    input: McpLaunchAuthorizationCommitInput
  ): Promise<McpLaunchAuthorizationResult> {
    const request = parseMcpLaunchAuthorizationCommitInput(input)
    return this.rpc
      .request<unknown, McpLaunchAuthorizationCommitInput>(
        MCP_SERVER_AUTHORIZE_LAUNCH_COMMIT_METHOD,
        request
      )
      .then(parseMcpLaunchAuthorizationResult)
      .catch(rethrowValidatedMcpManagementError)
  }

  enableMcpServer(input: McpServerMutationInput): Promise<McpServerDetailsOutput> {
    return this.mutateMcpServer(MCP_SERVER_ENABLE_METHOD, input)
  }

  disableMcpServer(input: McpServerMutationInput): Promise<McpServerDetailsOutput> {
    return this.mutateMcpServer(MCP_SERVER_DISABLE_METHOD, input)
  }

  startMcpServer(input: McpServerMutationInput): Promise<McpServerDetailsOutput> {
    return this.mutateMcpServer(MCP_SERVER_START_METHOD, input)
  }

  stopMcpServer(input: McpServerMutationInput): Promise<McpServerDetailsOutput> {
    return this.mutateMcpServer(MCP_SERVER_STOP_METHOD, input)
  }

  restartMcpServer(input: McpServerMutationInput): Promise<McpServerDetailsOutput> {
    return this.mutateMcpServer(MCP_SERVER_RESTART_METHOD, input)
  }

  getMcpServerStatus(input: McpServerIdInput): Promise<McpServerDetailsOutput> {
    const request = parseMcpServerIdInput(input)
    return this.rpc
      .request<unknown, McpServerIdInput>(MCP_SERVER_STATUS_METHOD, request)
      .then(parseMcpServerDetailsOutput)
      .catch(rethrowValidatedMcpManagementError)
  }

  listMcpTools(input: McpCatalogToolsPageInput): Promise<McpCatalogToolsPageOutput> {
    const request = parseMcpCatalogToolsPageInput(input)
    return this.rpc
      .request<unknown, McpCatalogToolsPageInput>(MCP_CATALOG_TOOLS_METHOD, request)
      .then(parseMcpCatalogToolsPageOutput)
      .catch(rethrowValidatedMcpManagementError)
  }

  refreshMcpCatalog(input: McpServerMutationInput): Promise<McpCatalogToolsPageOutput> {
    const request = parseMcpServerMutationInput(input)
    return this.rpc
      .request<unknown, McpServerMutationInput>(MCP_CATALOG_REFRESH_METHOD, request)
      .then(parseMcpCatalogToolsPageOutput)
      .catch(rethrowValidatedMcpManagementError)
  }

  onMcpChanged(handler: (event: McpChangedNotification) => void): () => void {
    return this.rpc.onNotification(MCP_CHANGED_NOTIFICATION_METHOD, (params) => {
      try {
        handler(parseMcpChangedNotification(params))
      } catch {
        // Do not log protocol bodies or parser errors: either may contain rejected private data.
        console.warn('Ignored invalid mcp.changed notification')
      }
    })
  }

  onManagedPlaywrightCommand(
    handler: (event: ManagedPlaywrightCommandNotification) => void
  ): () => void {
    return this.rpc.onNotification(MANAGED_PLAYWRIGHT_COMMAND_NOTIFICATION_METHOD, (params) => {
      try {
        handler(parseManagedPlaywrightCommandNotification(params))
      } catch {
        console.warn('Ignored invalid managed Playwright command notification')
      }
    })
  }

  onManagedPlaywrightCancel(
    handler: (event: ManagedPlaywrightCancelNotification) => void
  ): () => void {
    return this.rpc.onNotification(MANAGED_PLAYWRIGHT_CANCEL_NOTIFICATION_METHOD, (params) => {
      try {
        handler(parseManagedPlaywrightCancelNotification(params))
      } catch {
        console.warn('Ignored invalid managed Playwright cancel notification')
      }
    })
  }

  completeManagedPlaywright(input: ManagedPlaywrightCompletionInput): Promise<boolean> {
    const request = parseManagedPlaywrightCompletionInput(input)
    return this.rpc
      .request<unknown, ManagedPlaywrightCompletionInput>(
        MANAGED_PLAYWRIGHT_COMPLETE_METHOD,
        request
      )
      .then(parseManagedPlaywrightCompletionOutput)
      .then((output) => output.accepted)
  }

  acknowledgeManagedPlaywrightDispatchPhase(
    input: ManagedPlaywrightDispatchPhaseInput
  ): Promise<boolean> {
    const request = parseManagedPlaywrightDispatchPhaseInput(input)
    return this.rpc
      .request<unknown, ManagedPlaywrightDispatchPhaseInput>(
        MANAGED_PLAYWRIGHT_DISPATCH_PHASE_METHOD,
        request
      )
      .then(parseManagedPlaywrightDispatchPhaseOutput)
      .then((output) => output.accepted)
  }

  authorizeBrowserRisk(input: BrowserRiskAuthorizeInput): Promise<BrowserRiskAuthorizeOutput> {
    const request = parseBrowserRiskAuthorizeInput(input)
    return this.rpc
      .request<unknown, BrowserRiskAuthorizeInput>(BROWSER_RISK_AUTHORIZE_METHOD, request)
      .then(parseBrowserRiskAuthorizeOutput)
  }

  cancelBrowserRisk(input: BrowserRiskCancelInput): Promise<boolean> {
    const request = parseBrowserRiskCancelInput(input)
    return this.rpc
      .request<unknown, BrowserRiskCancelInput>(BROWSER_RISK_CANCEL_METHOD, request)
      .then(parseBrowserRiskCancelOutput)
      .then((output) => output.accepted)
  }

  private mutateMcpServer(
    method:
      | typeof MCP_SERVER_ENABLE_METHOD
      | typeof MCP_SERVER_DISABLE_METHOD
      | typeof MCP_SERVER_START_METHOD
      | typeof MCP_SERVER_STOP_METHOD
      | typeof MCP_SERVER_RESTART_METHOD,
    input: McpServerMutationInput
  ): Promise<McpServerDetailsOutput> {
    const request = parseMcpServerMutationInput(input)
    return this.rpc
      .request<unknown, McpServerMutationInput>(method, request)
      .then(parseMcpServerDetailsOutput)
      .catch(rethrowValidatedMcpManagementError)
  }
}
