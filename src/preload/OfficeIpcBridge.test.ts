import type { IpcRenderer } from 'electron'
import type { OfficeEngineStatus } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { createOfficeIpcBridge, OFFICE_GET_STATUS_CHANNEL } from './OfficeIpcBridge'

describe('Office IPC bridge', () => {
  it('invokes the no-parameter status channel and preserves the parsed value', async () => {
    const status = {
      schemaVersion: 1,
      providerId: 'officecli',
      availability: 'available',
      source: 'packagedComponent',
      version: 'OfficeCLI 1.0.139',
      engineRevision: 'office-engine-sha256-v1:test',
      capabilities: {
        providerId: 'officecli',
        documentKinds: ['document', 'spreadsheet', 'presentation'],
        operations: ['create', 'validate'],
        supportsRendering: true,
        supportsValidation: true,
        supportsStructuredOutput: true
      }
    } satisfies OfficeEngineStatus
    const invoke = vi.fn().mockResolvedValue(status)
    const bridge = createOfficeIpcBridge({ invoke } as unknown as Pick<IpcRenderer, 'invoke'>)

    await expect(bridge.getStatus()).resolves.toBe(status)
    expect(invoke).toHaveBeenCalledWith(OFFICE_GET_STATUS_CHANNEL)
  })
})
