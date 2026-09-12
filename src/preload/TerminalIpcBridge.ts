import type { IpcRenderer, IpcRendererEvent } from 'electron'
import { HOST_CHANNELS, type TerminalHostApi } from '@mycopilot/host-api'
import type { TerminalExitEvent, TerminalOutputEvent } from '@mycopilot/protocol'
import { TerminalEventRouter } from './TerminalEventRouter'

const TERMINAL_INPUT_CHUNK_LENGTH = 64 * 1024

type TerminalIpcRenderer = Pick<IpcRenderer, 'invoke' | 'on' | 'send'>

export function createTerminalIpcBridge(ipcRenderer: TerminalIpcRenderer): TerminalHostApi {
  const eventRouter = new TerminalEventRouter()
  ipcRenderer.on(
    HOST_CHANNELS.terminal.output,
    (_event: IpcRendererEvent, payload: TerminalOutputEvent): void =>
      eventRouter.dispatchOutput(payload)
  )
  ipcRenderer.on(
    HOST_CHANNELS.terminal.exit,
    (_event: IpcRendererEvent, payload: TerminalExitEvent): void =>
      eventRouter.dispatchExit(payload)
  )

  return {
    acknowledgeOutput: (sessionId, sequence) =>
      ipcRenderer.send(HOST_CHANNELS.terminal.acknowledgeOutput, sessionId, sequence),
    createSession: (request) => ipcRenderer.invoke(HOST_CHANNELS.terminal.createSession, request),
    killSession: (sessionId) => ipcRenderer.invoke(HOST_CHANNELS.terminal.killSession, sessionId),
    resizeSession: (sessionId, cols, rows) =>
      ipcRenderer.invoke(HOST_CHANNELS.terminal.resizeSession, sessionId, cols, rows),
    subscribeSession: (sessionId, handlers) => eventRouter.subscribe(sessionId, handlers),
    markUserInput: (sessionId) => ipcRenderer.send(HOST_CHANNELS.terminal.markUserInput, sessionId),
    selectSourceDirectory: (sessionId, folderId) =>
      ipcRenderer.invoke(HOST_CHANNELS.terminal.selectSourceDirectory, sessionId, folderId),
    writeInput: (sessionId, data, userInitiated = true) => {
      for (let offset = 0; offset < data.length;) {
        let end = Math.min(data.length, offset + TERMINAL_INPUT_CHUNK_LENGTH)
        const lastCodeUnit = data.charCodeAt(end - 1)
        if (end < data.length && lastCodeUnit >= 0xd800 && lastCodeUnit <= 0xdbff) end -= 1
        ipcRenderer.send(
          HOST_CHANNELS.terminal.writeInput,
          sessionId,
          data.slice(offset, end),
          userInitiated
        )
        offset = end
      }
    }
  }
}
