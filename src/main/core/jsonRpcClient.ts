import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process'
import { createInterface } from 'node:readline'
import { app } from 'electron'
import { isAbsolute, join, normalize } from 'node:path'

import type {
  JsonRpcErrorResponse,
  JsonRpcErrorObject,
  JsonRpcId,
  JsonRpcNotification,
  JsonRpcRequest,
  JsonRpcResponse,
  JsonRpcSuccessResponse
} from '@mycopilot/protocol'

interface PendingRequest {
  resolve(value: unknown): void
  reject(reason: Error): void
}

type NotificationHandler = (params: unknown) => void

function deleteEnvironmentVariableCaseInsensitively(
  environment: NodeJS.ProcessEnv,
  variableName: string
): void {
  const normalizedName = variableName.toUpperCase()
  for (const existingName of Object.keys(environment)) {
    if (existingName.toUpperCase() === normalizedName) {
      delete environment[existingName]
    }
  }
}

export interface CoreJsonRpcClientOptions {
  /**
   * The application data root selected by Electron Host. It is normalized and frozen when the
   * client is constructed; spawning the Core remains lazy.
   */
  appDataRoot?: string
}

/** Preserves JSON-RPC error metadata for callers that can apply typed recovery policies. */
export class CoreJsonRpcError extends Error {
  readonly code: number
  readonly data: unknown

  constructor(error: JsonRpcErrorObject) {
    super(error.message)
    this.name = 'CoreJsonRpcError'
    this.code = error.code
    this.data = error.data
  }
}

export class CoreJsonRpcClient {
  private readonly appDataRoot: string
  private child: ChildProcessWithoutNullStreams | null = null
  private acceptingRequests = true
  private nextId = 1
  private readonly notificationHandlers = new Map<string, Set<NotificationHandler>>()
  private readonly pendingRequests = new Map<JsonRpcId, PendingRequest>()
  private readonly startedHandlers = new Set<() => void>()

  constructor(options: CoreJsonRpcClientOptions = {}) {
    const appDataRoot = normalize(options.appDataRoot ?? app.getPath('userData'))
    if (!isAbsolute(appDataRoot)) {
      throw new Error('Core application data root must be an absolute path')
    }
    this.appDataRoot = appDataRoot
  }

  start(): void {
    if (!this.acceptingRequests) {
      throw new Error('core-server request admission is closed')
    }
    if (this.child) {
      return
    }

    const command = this.resolveCommand()
    this.child = spawn(command.executable, command.args, {
      cwd: command.cwd,
      env: this.resolveEnvironment(),
      stdio: 'pipe'
    })

    const stdout = createInterface({ input: this.child.stdout })
    stdout.on('line', (line) => this.handleLine(line))

    this.child.stderr.on('data', (chunk) => {
      console.error(`[core-server] ${chunk.toString()}`)
    })

    const child = this.child
    const handleChildFailure = (error: Error): void => {
      // The child can close stdin just before Node delivers its `exit` event. Treat both
      // notifications as the same failure and, importantly, consume stdin's `error` event so an
      // EPIPE cannot become an uncaught main-process exception while a startup request is in flight.
      if (this.child !== child) return
      this.child = null
      this.rejectAll(error)
    }

    child.stdin.on('error', (error) => handleChildFailure(error))
    child.on('error', (error) => handleChildFailure(error))
    child.on('exit', (code, signal) => {
      handleChildFailure(
        new Error(`core-server exited with code ${code ?? 'null'} and signal ${signal ?? 'null'}`)
      )
    })
    for (const handler of this.startedHandlers) handler()
  }

  /** Host-only lifecycle hook, including lazy restarts. */
  onStarted(handler: () => void): () => void {
    this.startedHandlers.add(handler)
    return () => {
      this.startedHandlers.delete(handler)
    }
  }

  isRunning(): boolean {
    return this.child !== null
  }

  /**
   * Permanently fences this Host-owned client before application shutdown. Ordinary child-process
   * exits do not close admission, so the next user request can still restart Core while the app is
   * running. Once shutdown begins, however, a notifier or other late producer must not spawn a new
   * Core process behind the shutdown coordinator.
   */
  beginShutdown(): void {
    this.acceptingRequests = false
  }

  stop(): void {
    if (!this.child) {
      return
    }

    this.child.kill()
    this.child = null
    this.rejectAll(new Error('core-server stopped'))
  }

  async request<TResult, TParams = unknown>(method: string, params?: TParams): Promise<TResult> {
    if (!this.acceptingRequests) {
      throw new Error('core-server request admission is closed')
    }
    this.start()

    return this.requestOnRunningChild<TResult, TParams>(method, params)
  }

  /**
   * Sends the bounded Core shutdown RPC after request admission has closed. This method never
   * starts a child, so an exit race can only reject the shutdown request; it cannot resurrect Core.
   */
  async requestDuringShutdown<TResult, TParams = unknown>(
    method: string,
    params?: TParams
  ): Promise<TResult> {
    if (this.acceptingRequests) {
      throw new Error('core-server shutdown request requires closed admission')
    }
    return this.requestOnRunningChild<TResult, TParams>(method, params)
  }

  private async requestOnRunningChild<TResult, TParams = unknown>(
    method: string,
    params?: TParams
  ): Promise<TResult> {
    if (!this.child) {
      throw new Error('core-server is not running')
    }

    const child = this.child
    if (!child) {
      throw new Error('core-server is not running')
    }

    const id = this.nextId++
    const request: JsonRpcRequest<TParams> = {
      jsonrpc: '2.0',
      id,
      method,
      params
    }

    return new Promise<TResult>((resolve, reject) => {
      this.pendingRequests.set(id, {
        resolve: (value) => resolve(value as TResult),
        reject
      })

      child.stdin.write(`${JSON.stringify(request)}\n`, (error) => {
        if (!error) {
          return
        }

        this.pendingRequests.delete(id)
        reject(error)
      })
    })
  }

  onNotification(method: string, handler: NotificationHandler): () => void {
    const handlers = this.notificationHandlers.get(method) ?? new Set<NotificationHandler>()
    handlers.add(handler)
    this.notificationHandlers.set(method, handlers)

    return () => {
      handlers.delete(handler)
      if (handlers.size === 0) {
        this.notificationHandlers.delete(method)
      }
    }
  }

  private handleLine(line: string): void {
    let message: JsonRpcResponse | JsonRpcNotification

    try {
      message = JSON.parse(line) as JsonRpcResponse | JsonRpcNotification
    } catch (error) {
      console.error('[core-server] invalid JSON-RPC response', error)
      return
    }

    if (this.isNotification(message)) {
      this.handleNotification(message)
      return
    }

    const response = message

    if (response.id === null) {
      return
    }

    const pending = this.pendingRequests.get(response.id)
    if (!pending) {
      return
    }

    this.pendingRequests.delete(response.id)

    if (this.isErrorResponse(response)) {
      pending.reject(new CoreJsonRpcError(response.error))
      return
    }

    pending.resolve((response as JsonRpcSuccessResponse).result)
  }

  private rejectAll(error: Error): void {
    for (const pending of this.pendingRequests.values()) {
      pending.reject(error)
    }
    this.pendingRequests.clear()
  }

  private isErrorResponse(response: JsonRpcResponse): response is JsonRpcErrorResponse {
    return 'error' in response
  }

  private isNotification(
    message: JsonRpcResponse | JsonRpcNotification
  ): message is JsonRpcNotification {
    return 'method' in message && !('id' in message)
  }

  private handleNotification(notification: JsonRpcNotification): void {
    const handlers = this.notificationHandlers.get(notification.method)
    if (!handlers) {
      return
    }

    for (const handler of handlers) {
      handler(notification.params)
    }
  }

  private resolveCommand(): { executable: string; args: string[]; cwd: string } {
    if (!app.isPackaged) {
      return {
        executable: 'cargo',
        args: ['run', '-p', 'mycopilot-core-server', '--bin', 'core-server', '--quiet'],
        cwd: join(__dirname, '../..')
      }
    }

    const executableName = process.platform === 'win32' ? 'core-server.exe' : 'core-server'
    return {
      executable: join(process.resourcesPath, executableName),
      args: [],
      cwd: process.resourcesPath
    }
  }

  private resolveEnvironment(): NodeJS.ProcessEnv {
    const configuredOfficeCliPath = process.env.MYCOPILOT_OFFICECLI_PATH
    const configuredComponentsDirectory = process.env.MYCOPILOT_OFFICE_COMPONENTS_DIR
    const configuredOfficeRendererDirectory = process.env.MYCOPILOT_OFFICE_RENDERER_DIR
    const configuredWordPdfRendererDirectory = process.env.MYCOPILOT_WORD_PDF_RENDERER_DIR
    const configuredArtifactRuntimeDirectory = process.env.MYCOPILOT_ARTIFACT_RUNTIME_DIR
    const environment = { ...process.env }

    // Electron Host owns this location for both development and packaged applications.
    // Never allow a parent shell to redirect the Core to a separate database.
    // Windows environment keys are case-insensitive even though a copied JavaScript object is
    // not. Remove every casing before installing the single canonical Host capability.
    deleteEnvironmentVariableCaseInsensitively(environment, 'MYCOPILOT_APP_DATA_ROOT')
    deleteEnvironmentVariableCaseInsensitively(environment, 'MYCOPILOT_STORAGE_DB')
    environment.MYCOPILOT_APP_DATA_ROOT = this.appDataRoot

    // These variables are an internal, per-call capability between Rust and its browser proxy.
    // They must never be inherited by the normal long-lived core-server process.
    for (const name of [
      'MYCOPILOT_OFFICE_BROWSER_PROXY_MODE',
      'MYCOPILOT_OFFICE_BROWSER_EXECUTABLE',
      'MYCOPILOT_OFFICE_BROWSER_PROFILE',
      'MYCOPILOT_OFFICE_BROWSER_FAILURE_MARKER',
      'MYCOPILOT_OFFICE_BROWSER_MARKER_NONCE',
      'MYCOPILOT_OFFICE_BROWSER_MAX_INVOCATIONS'
    ]) {
      delete environment[name]
    }

    if (app.isPackaged) {
      // Let the Rust discovery layer resolve the canonical component layout so status reports the
      // provider as an application-packaged component rather than as a user override.
      if (configuredOfficeCliPath === undefined && configuredComponentsDirectory === undefined) {
        environment.MYCOPILOT_OFFICE_COMPONENTS_DIR = join(process.resourcesPath, 'components')
      }
      // The packaged renderer is part of the signed application component boundary. Always
      // replace an inherited development override with the application-owned component path;
      // this remains available even when OfficeCLI itself has an explicit configured path.
      environment.MYCOPILOT_OFFICE_RENDERER_DIR = join(
        process.resourcesPath,
        'components',
        'office-renderer'
      )
      environment.MYCOPILOT_WORD_PDF_RENDERER_DIR = join(
        process.resourcesPath,
        'components',
        'word-pdf-renderer'
      )
      // Production never accepts an inherited configured-component override.
      // The Rust layer treats this application resource root as a packaged,
      // code-signed trust boundary and appends `artifact-runtime` itself.
      delete environment.MYCOPILOT_ARTIFACT_RUNTIME_DIR
      environment.MYCOPILOT_ARTIFACT_RUNTIME_COMPONENTS_DIR = join(
        process.resourcesPath,
        'components'
      )
    } else {
      if (configuredOfficeCliPath === undefined && configuredComponentsDirectory === undefined) {
        const executableName = process.platform === 'win32' ? 'officecli.exe' : 'officecli'
        environment.MYCOPILOT_OFFICECLI_PATH = join(
          __dirname,
          '../..',
          '.cache',
          'officecli',
          'current',
          executableName
        )
      }
      if (configuredOfficeRendererDirectory === undefined) {
        environment.MYCOPILOT_OFFICE_RENDERER_DIR = join(
          __dirname,
          '../..',
          '.cache',
          'office-renderer',
          'current'
        )
      }
      if (configuredWordPdfRendererDirectory === undefined) {
        environment.MYCOPILOT_WORD_PDF_RENDERER_DIR = join(
          __dirname,
          '../..',
          '.cache',
          'word-pdf-renderer',
          'current'
        )
      }
      if (configuredArtifactRuntimeDirectory === undefined) {
        environment.MYCOPILOT_ARTIFACT_RUNTIME_DIR = join(
          __dirname,
          '../..',
          '.cache',
          'artifact-runtime',
          'current'
        )
      }
      delete environment.MYCOPILOT_ARTIFACT_RUNTIME_COMPONENTS_DIR
    }
    return environment
  }
}
