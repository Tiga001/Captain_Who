import { BrowserWindow, dialog } from 'electron'
import type {
  IpcMainInvokeEvent,
  MessageBoxOptions,
  OpenDialogOptions,
  OpenDialogReturnValue
} from 'electron'
import { resolve } from 'node:path'
import {
  captureHostInvocation,
  HOST_CHANNELS,
  type HostInvocationResult
} from '@mycopilot/host-api'
import {
  MCP_MANAGEMENT_ERROR_CODE,
  MCP_MANAGEMENT_SCHEMA_VERSION,
  parseMcpCatalogToolsPageInput,
  parseMcpCatalogToolsPageOutput,
  parseMcpChangedNotification,
  parseMcpLaunchAuthorizationResult,
  parseMcpManagementErrorData,
  parseMcpServerCreateInput,
  parseMcpServerDetailsOutput,
  parseMcpServerIdInput,
  parseMcpServerListOutput,
  parseMcpServerMutationInput,
  parseMcpServerUpdateInput,
  type McpChangedNotification,
  type McpLaunchAuthorizationPreview,
  type McpLaunchAuthorizationResult,
  type McpManagementOperation,
  type McpServerMutationInput
} from '@mycopilot/protocol'
import type { CoreServer } from '../core/coreServer'
import type { TrustedIpcMain } from './trustedIpc'

const MCP_NOTIFICATION_COALESCE_MS = 25
const MCP_NOTIFICATION_PENDING_LIMIT = 1024
const UNSAFE_LAUNCH_DISPLAY_CHARACTER = /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu

type ShowOpenDialog = (
  event: IpcMainInvokeEvent,
  options: OpenDialogOptions
) => Promise<OpenDialogReturnValue>
type ConfirmLaunch = (
  event: IpcMainInvokeEvent,
  preview: McpLaunchAuthorizationPreview
) => Promise<boolean>

export interface McpIpcNativeDialogs {
  selectExecutable(event: IpcMainInvokeEvent): Promise<string | null>
  selectWorkingDirectory(event: IpcMainInvokeEvent): Promise<string | null>
  confirmLaunch(event: IpcMainInvokeEvent, preview: McpLaunchAuthorizationPreview): Promise<boolean>
}

export function registerMcpIpc(
  ipcMain: TrustedIpcMain,
  coreServer: CoreServer,
  nativeDialogs: McpIpcNativeDialogs = createMcpNativeDialogs()
): () => void {
  const broadcaster = new McpChangedBroadcaster()
  const unsubscribe = coreServer.onMcpChanged((event) => broadcaster.enqueue(event))

  ipcMain.handle(HOST_CHANNELS.mcp.listServers, () =>
    captureMcpInvocation('list', () => coreServer.listMcpServers(), parseMcpServerListOutput)
  )
  ipcMain.handle(HOST_CHANNELS.mcp.getServer, (_event, input) =>
    captureMcpInputInvocation(
      'get',
      () => parseMcpServerIdInput(input),
      (request) => coreServer.getMcpServer(request),
      parseMcpServerDetailsOutput
    )
  )
  ipcMain.handle(HOST_CHANNELS.mcp.addServer, (_event, input) =>
    captureMcpInputInvocation(
      'add',
      () => parseMcpServerCreateInput(input),
      (request) => coreServer.addMcpServer(request),
      parseMcpServerDetailsOutput
    )
  )
  ipcMain.handle(HOST_CHANNELS.mcp.updateServer, (_event, input) =>
    captureMcpInputInvocation(
      'update',
      () => parseMcpServerUpdateInput(input),
      (request) => coreServer.updateMcpServer(request),
      parseMcpServerDetailsOutput
    )
  )
  ipcMain.handle(HOST_CHANNELS.mcp.deleteServer, (_event, input) =>
    captureMcpInputInvocation(
      'delete',
      () => parseMcpServerMutationInput(input),
      (request) => coreServer.deleteMcpServer(request),
      parseMcpServerDetailsOutput
    )
  )
  ipcMain.handle(HOST_CHANNELS.mcp.requestLaunchAuthorization, (event, input) =>
    captureMcpInputInvocation(
      'prepareLaunchAuthorization',
      () => parseMcpServerMutationInput(input),
      (request) =>
        requestMcpLaunchAuthorization(event, coreServer, nativeDialogs.confirmLaunch, request),
      (value) => (value === null ? null : parseMcpLaunchAuthorizationResult(value))
    )
  )
  ipcMain.handle(HOST_CHANNELS.mcp.enableServer, (_event, input) =>
    captureMcpInputInvocation(
      'enable',
      () => parseMcpServerMutationInput(input),
      (request) => coreServer.enableMcpServer(request),
      parseMcpServerDetailsOutput
    )
  )
  ipcMain.handle(HOST_CHANNELS.mcp.disableServer, (_event, input) =>
    captureMcpInputInvocation(
      'disable',
      () => parseMcpServerMutationInput(input),
      (request) => coreServer.disableMcpServer(request),
      parseMcpServerDetailsOutput
    )
  )
  ipcMain.handle(HOST_CHANNELS.mcp.startServer, (_event, input) =>
    captureMcpInputInvocation(
      'start',
      () => parseMcpServerMutationInput(input),
      (request) => coreServer.startMcpServer(request),
      parseMcpServerDetailsOutput
    )
  )
  ipcMain.handle(HOST_CHANNELS.mcp.stopServer, (_event, input) =>
    captureMcpInputInvocation(
      'stop',
      () => parseMcpServerMutationInput(input),
      (request) => coreServer.stopMcpServer(request),
      parseMcpServerDetailsOutput
    )
  )
  ipcMain.handle(HOST_CHANNELS.mcp.restartServer, (_event, input) =>
    captureMcpInputInvocation(
      'restart',
      () => parseMcpServerMutationInput(input),
      (request) => coreServer.restartMcpServer(request),
      parseMcpServerDetailsOutput
    )
  )
  ipcMain.handle(HOST_CHANNELS.mcp.getStatus, (_event, input) =>
    captureMcpInputInvocation(
      'status',
      () => parseMcpServerIdInput(input),
      (request) => coreServer.getMcpServerStatus(request),
      parseMcpServerDetailsOutput
    )
  )
  ipcMain.handle(HOST_CHANNELS.mcp.listTools, (_event, input) =>
    captureMcpInputInvocation(
      'listTools',
      () => parseMcpCatalogToolsPageInput(input),
      (request) => coreServer.listMcpTools(request),
      parseMcpCatalogToolsPageOutput
    )
  )
  ipcMain.handle(HOST_CHANNELS.mcp.refreshCatalog, (_event, input) =>
    captureMcpInputInvocation(
      'refreshCatalog',
      () => parseMcpServerMutationInput(input),
      (request) => coreServer.refreshMcpCatalog(request),
      parseMcpCatalogToolsPageOutput
    )
  )
  ipcMain.handle(HOST_CHANNELS.mcp.selectExecutable, (event) =>
    nativeDialogs.selectExecutable(event)
  )
  ipcMain.handle(HOST_CHANNELS.mcp.selectWorkingDirectory, (event) =>
    nativeDialogs.selectWorkingDirectory(event)
  )

  return () => {
    unsubscribe()
    broadcaster.dispose()
  }
}

export function createMcpNativeDialogs(
  showOpenDialog: ShowOpenDialog = showNativeOpenDialog,
  confirmLaunch: ConfirmLaunch = showNativeLaunchConfirmation
): McpIpcNativeDialogs {
  return {
    selectExecutable: async (event) => {
      const result = await showOpenDialog(event, {
        title: 'Select MCP server executable',
        properties: ['openFile']
      })
      const selected = result.filePaths[0]
      return result.canceled || !selected ? null : resolve(selected)
    },
    selectWorkingDirectory: async (event) => {
      const result = await showOpenDialog(event, {
        title: 'Select MCP server working directory',
        properties: ['openDirectory']
      })
      const selected = result.filePaths[0]
      return result.canceled || !selected ? null : resolve(selected)
    },
    confirmLaunch
  }
}

async function requestMcpLaunchAuthorization(
  event: IpcMainInvokeEvent,
  coreServer: CoreServer,
  confirmLaunch: ConfirmLaunch,
  input: McpServerMutationInput
): Promise<McpLaunchAuthorizationResult | null> {
  const preview = await coreServer.prepareMcpLaunchAuthorization(input)
  assertPreviewMatchesRequest(preview, input)
  if (!(await confirmLaunch(event, preview))) {
    return null
  }
  return coreServer.commitMcpLaunchAuthorization({
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    authorizationId: preview.authorizationId,
    precondition: preview.precondition
  })
}

function assertPreviewMatchesRequest(
  preview: McpLaunchAuthorizationPreview,
  input: McpServerMutationInput
): void {
  if (
    preview.serverId !== input.serverId ||
    preview.precondition.expectedRegistryRevision !== input.precondition.expectedRegistryRevision ||
    preview.precondition.expectedConfigEpoch !== input.precondition.expectedConfigEpoch ||
    preview.precondition.expectedConfigDigest !== input.precondition.expectedConfigDigest
  ) {
    throw new Error('MCP launch authorization preview identity changed')
  }
}

async function captureMcpInvocation<T>(
  operationName: McpManagementOperation,
  operation: () => Promise<T>,
  parseOutput: (value: unknown) => T
): Promise<HostInvocationResult<T>> {
  const result = await captureHostInvocation(async () => parseOutput(await operation()))
  if (result.ok) return result
  if (result.error.code === MCP_MANAGEMENT_ERROR_CODE) {
    try {
      const data = parseMcpManagementErrorData(result.error.data)
      return {
        ok: false,
        error: {
          message: data.message,
          code: MCP_MANAGEMENT_ERROR_CODE,
          data
        }
      }
    } catch {
      // Fall through to a fixed projection; never echo rejected data or parser diagnostics.
    }
  }
  const data = {
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    type: 'mcpManagement',
    operation: operationName,
    code: 'internalSafeError',
    recovery: 'doNotRetry',
    message: 'MCP management operation failed.'
  } as const
  return {
    ok: false,
    error: {
      message: data.message,
      code: MCP_MANAGEMENT_ERROR_CODE,
      data
    }
  }
}

function captureMcpInputInvocation<TInput, TOutput>(
  operationName: McpManagementOperation,
  parseInput: () => TInput,
  operation: (input: TInput) => Promise<TOutput>,
  parseOutput: (value: unknown) => TOutput
): Promise<HostInvocationResult<TOutput>> {
  let input: TInput
  try {
    input = parseInput()
  } catch {
    const data = {
      schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
      type: 'mcpManagement',
      operation: operationName,
      code: 'invalidInput',
      recovery: 'fixInput',
      message: 'The MCP management request is invalid.'
    } as const
    return Promise.resolve({
      ok: false,
      error: {
        message: data.message,
        code: MCP_MANAGEMENT_ERROR_CODE,
        data
      }
    })
  }
  return captureMcpInvocation(operationName, () => operation(input), parseOutput)
}

async function showNativeOpenDialog(
  event: IpcMainInvokeEvent,
  options: OpenDialogOptions
): Promise<OpenDialogReturnValue> {
  const window = BrowserWindow.fromWebContents(event.sender)
  return window ? dialog.showOpenDialog(window, options) : dialog.showOpenDialog(options)
}

async function showNativeLaunchConfirmation(
  event: IpcMainInvokeEvent,
  preview: McpLaunchAuthorizationPreview
): Promise<boolean> {
  const options: MessageBoxOptions = {
    type: 'warning',
    title: 'Authorize local MCP server',
    message: 'Allow MyCopilot to start this exact local MCP server configuration?',
    detail: formatLaunchAuthorizationDetail(preview),
    buttons: ['Cancel', 'Authorize'],
    defaultId: 0,
    cancelId: 0,
    noLink: true
  }
  const window = BrowserWindow.fromWebContents(event.sender)
  const result = window
    ? await dialog.showMessageBox(window, options)
    : await dialog.showMessageBox(options)
  return result.response === 1
}

function formatLaunchAuthorizationDetail(preview: McpLaunchAuthorizationPreview): string {
  const argumentsText =
    preview.arguments.length === 0
      ? '(none)'
      : preview.arguments
          .map((argument, index) => `[${index}] ${escapeLaunchDisplayValue(argument)}`)
          .join('\n')
  return [
    `Server: ${escapeLaunchDisplayValue(preview.displayName)}`,
    `Executable: ${escapeLaunchDisplayValue(preview.executable)}`,
    'Arguments:',
    argumentsText,
    `Working directory: ${escapeLaunchDisplayValue(preview.cwd)}`,
    `Launch-spec digest: ${preview.launchSpecDigest}`
  ].join('\n')
}

function escapeLaunchDisplayValue(value: string): string {
  return (JSON.stringify(value) ?? '""').replace(UNSAFE_LAUNCH_DISPLAY_CHARACTER, (character) => {
    const codePoint = character.codePointAt(0) ?? 0xfffd
    return codePoint <= 0xffff
      ? `\\u${codePoint.toString(16).padStart(4, '0')}`
      : `\\u{${codePoint.toString(16)}}`
  })
}

class McpChangedBroadcaster {
  private readonly pending = new Map<string, McpChangedNotification>()
  private pendingResync: McpChangedNotification | null = null
  private timer: ReturnType<typeof setTimeout> | null = null
  private sourceEpoch: string | null = null
  private lastAcceptedSequence = -1
  private lastDeliveredSequence = -1

  enqueue(value: unknown): void {
    let event: McpChangedNotification
    try {
      event = parseMcpChangedNotification(value)
    } catch {
      return
    }
    if (event.sourceEpoch !== this.sourceEpoch) {
      this.pending.clear()
      this.pendingResync = null
      this.lastAcceptedSequence = -1
      this.lastDeliveredSequence = -1
      this.sourceEpoch = event.sourceEpoch
    }
    if (
      event.sequence <= this.lastDeliveredSequence ||
      event.sequence <= this.lastAcceptedSequence
    ) {
      return
    }
    this.lastAcceptedSequence = event.sequence
    if (event.kind === 'resyncRequired') {
      this.pending.clear()
      this.pendingResync = event
    } else if (this.pendingResync) {
      if (event.sequence >= this.pendingResync.sequence) {
        this.pendingResync = {
          schemaVersion: event.schemaVersion,
          sourceEpoch: event.sourceEpoch,
          sequence: event.sequence,
          registryRevision: event.registryRevision,
          kind: 'resyncRequired'
        }
      }
    } else {
      const previous = this.pending.get(event.serverId)
      if (!previous && this.pending.size >= MCP_NOTIFICATION_PENDING_LIMIT) {
        this.pending.clear()
        this.pendingResync = {
          schemaVersion: event.schemaVersion,
          sourceEpoch: event.sourceEpoch,
          sequence: event.sequence,
          registryRevision: event.registryRevision,
          kind: 'resyncRequired'
        }
      } else if (!previous || event.sequence >= previous.sequence) {
        this.pending.set(event.serverId, event)
      }
    }
    if (this.timer) return
    this.timer = setTimeout(() => this.flush(), MCP_NOTIFICATION_COALESCE_MS)
    this.timer.unref()
  }

  dispose(): void {
    if (this.timer) clearTimeout(this.timer)
    this.timer = null
    this.pending.clear()
    this.pendingResync = null
  }

  private flush(): void {
    this.timer = null
    const events = this.pendingResync
      ? [this.pendingResync]
      : [...this.pending.values()].sort((left, right) => left.sequence - right.sequence)
    this.pending.clear()
    this.pendingResync = null
    for (const event of events) {
      if (event.sequence <= this.lastDeliveredSequence) {
        continue
      }
      for (const window of BrowserWindow.getAllWindows()) {
        if (!window.isDestroyed() && !window.webContents.isDestroyed()) {
          window.webContents.send(HOST_CHANNELS.mcp.changed, event)
        }
      }
      this.lastDeliveredSequence = event.sequence
    }
  }
}
