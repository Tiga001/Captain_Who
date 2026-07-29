import { ipcMain as electronIpcMain } from 'electron'
import type { IpcMainEvent, IpcMainInvokeEvent } from 'electron'

type InvokeHandler = Parameters<typeof electronIpcMain.handle>[1]
type OneWayHandler = (event: IpcMainEvent, ...args: unknown[]) => void

export interface TrustedIpcMain {
  handle(channel: string, handler: InvokeHandler): void
  on(channel: string, handler: OneWayHandler): void
}

export function createTrustedIpcMain(
  isTrustedRenderer: (event: IpcMainInvokeEvent) => boolean
): TrustedIpcMain {
  return {
    handle(channel, handler): void {
      electronIpcMain.handle(channel, (event, ...args) => {
        if (!isTrustedRenderer(event)) {
          throw new Error(`Blocked untrusted IPC sender for ${channel}`)
        }
        return handler(event, ...args)
      })
    },
    on(channel, handler): void {
      electronIpcMain.on(channel, (event, ...args) => {
        if (!isTrustedRenderer(event as unknown as IpcMainInvokeEvent)) {
          console.warn(`Blocked untrusted one-way IPC sender for ${channel}`)
          return
        }
        handler(event, ...args)
      })
    }
  }
}
