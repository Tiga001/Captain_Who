// Electron main client.
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process'
import { createInterface } from 'node:readline'
import { app } from 'electron'
import { join } from 'node:path'

import type {
  JsonRpcErrorResponse,
  JsonRpcId,
  JsonRpcRequest,
  JsonRpcResponse,
  JsonRpcSuccessResponse
} from '@mycopilot/protocol'

interface PendingRequest {
  resolve(value: unknown): void
  reject(reason: Error): void
}

export class CoreJsonRpcClient {
  private child: ChildProcessWithoutNullStreams | null = null
  private nextId = 1
  private readonly pendingRequests = new Map<JsonRpcId, PendingRequest>()

  start(): void {
    if (this.child) {
      return
    }

    const command = this.resolveCommand()
    this.child = spawn(command.executable, command.args, {
      cwd: command.cwd,
      env: process.env,
      stdio: 'pipe'
    })

    const stdout = createInterface({ input: this.child.stdout })
    stdout.on('line', (line) => this.handleLine(line))

    this.child.stderr.on('data', (chunk) => {
      console.error(`[core-server] ${chunk.toString()}`)
    })

    this.child.on('error', (error) => this.rejectAll(error))
    this.child.on('exit', (code, signal) => {
      this.child = null
      this.rejectAll(new Error(`core-server exited with code ${code ?? 'null'} and signal ${signal ?? 'null'}`))
    })
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
    this.start()

    if (!this.child) {
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

      this.child?.stdin.write(`${JSON.stringify(request)}\n`, (error) => {
        if (!error) {
          return
        }

        this.pendingRequests.delete(id)
        reject(error)
      })
    })
  }

  private handleLine(line: string): void {
    let response: JsonRpcResponse

    try {
      response = JSON.parse(line) as JsonRpcResponse
    } catch (error) {
      console.error('[core-server] invalid JSON-RPC response', error)
      return
    }

    if (response.id === null) {
      return
    }

    const pending = this.pendingRequests.get(response.id)
    if (!pending) {
      return
    }

    this.pendingRequests.delete(response.id)

    if (this.isErrorResponse(response)) {
      pending.reject(new Error(response.error.message))
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

  private resolveCommand(): { executable: string; args: string[]; cwd: string } {
    if (!app.isPackaged) {
      return {
        executable: 'cargo',
        args: ['run', '-p', 'mycopilot-core-server', '--bin', 'core-server', '--quiet'],
        cwd: join(__dirname, '../..')
      }
    }

    return {
      executable: join(process.resourcesPath, 'core-server'),
      args: [],
      cwd: process.resourcesPath
    }
  }
}
