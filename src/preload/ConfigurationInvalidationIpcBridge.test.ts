import { describe, expect, it, vi } from 'vitest'
import { createStorageIpcBridge } from './StorageIpcBridge'
import { createImageGenerationIpcBridge } from './ImageGenerationIpcBridge'
import { createMcpIpcBridge } from './McpIpcBridge'

describe('configuration invalidation bridges', () => {
  it.each(['models', 'images', 'builtins'] as const)(
    'subscribes and cleans up %s without exposing payload data',
    (domain) => {
      const ipc = { invoke: vi.fn(), on: vi.fn(), removeListener: vi.fn() }
      const listener = vi.fn()
      const subscribe = {
        models: createStorageIpcBridge(ipc).onModelSettingsChanged,
        images: createImageGenerationIpcBridge(ipc).onChanged,
        builtins: createMcpIpcBridge(ipc).onBuiltinCapabilitiesChanged
      }[domain]
      const channel = {
        models: 'host:storage.modelSettingsChanged',
        images: 'host:imageGeneration.changed',
        builtins: 'host:mcp.builtinCapabilitiesChanged'
      }[domain]
      const unsubscribe = subscribe(listener)
      const callback = ipc.on.mock.calls[0][1]
      callback({}, { credential: 'must-not-be-forwarded' })
      expect(ipc.on).toHaveBeenCalledWith(channel, callback)
      expect(listener.mock.calls).toEqual([[]])
      unsubscribe()
      expect(ipc.removeListener).toHaveBeenCalledWith(channel, callback)
      expect(ipc.invoke).not.toHaveBeenCalled()
    }
  )
})
