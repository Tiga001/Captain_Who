import type { IpcMainInvokeEvent } from 'electron'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const ipcMainHandle = vi.hoisted(() => vi.fn())

vi.mock('electron', () => ({
  BrowserWindow: {
    fromWebContents: vi.fn(() => undefined),
    getAllWindows: vi.fn(() => [])
  },
  Notification: class {
    static isSupported(): boolean {
      return false
    }
  },
  clipboard: { writeText: vi.fn() },
  dialog: { showOpenDialog: vi.fn() },
  ipcMain: { handle: ipcMainHandle, on: vi.fn() },
  nativeTheme: { themeSource: 'system' },
  shell: { openExternal: vi.fn(), showItemInFolder: vi.fn() }
}))

vi.mock('../terminal/TerminalBridge', () => ({ TerminalBridge: class {} }))
vi.mock('../attachments/AttachmentDialogBridge', () => ({ AttachmentDialogBridge: class {} }))
vi.mock('../resources/FaviconResourceCache', () => ({ FaviconResourceCache: class {} }))
vi.mock('../workspaceFiles/WorkspaceFilesService', () => ({ WorkspaceFilesService: class {} }))
vi.mock('../webviews/managedWebviewSecurity', () => ({ clearManagedWebviewData: vi.fn() }))

import { registerHostIpc } from '../ipc'

const event = { sender: {} } as IpcMainInvokeEvent

function findHandler(channel: string): (...args: unknown[]) => Promise<unknown> {
  const registration = ipcMainHandle.mock.calls.find(([registered]) => registered === channel)
  const handler = registration?.[1]
  expect(handler).toBeTypeOf('function')
  return handler as (...args: unknown[]) => Promise<unknown>
}

function automationCoreStubs() {
  return {
    onAutomationEvent: vi.fn(() => vi.fn()),
    onAutomationResync: vi.fn(() => vi.fn()),
    onNotificationEvent: vi.fn(() => vi.fn()),
    onNotificationResync: vi.fn(() => vi.fn())
  }
}

describe('Image generation configuration IPC registration', () => {
  beforeEach(() => ipcMainHandle.mockReset())

  it('wraps each operation in a serializable Host invocation envelope', async () => {
    const output = { schemaVersion: 1, marker: 'opaque' }
    const input = { expectedRevision: 'image-generation:v1:1', enabled: true }
    const artifactInput = { schemaVersion: 1, artifact: { artifactId: 'opaque' } }
    const coreServer = {
      ...automationCoreStubs(),
      onAgentEvent: vi.fn(),
      onProviderTransition: vi.fn(() => vi.fn()),
      onSkillsChanged: vi.fn(),
      onMcpChanged: vi.fn(() => vi.fn()),
      getImageGenerationConfiguration: vi.fn().mockResolvedValue(output),
      updateImageGenerationConfiguration: vi.fn().mockResolvedValue(output),
      setImageGenerationEnabled: vi.fn().mockResolvedValue(output),
      getImageGenerationStatus: vi.fn().mockResolvedValue(output),
      readImageGenerationArtifact: vi.fn().mockResolvedValue(output)
    }
    registerHostIpc(coreServer as never, {} as never, {} as never, () => true)

    await expect(findHandler('host:imageGeneration.getConfiguration')(event)).resolves.toEqual({
      ok: true,
      value: output
    })
    await expect(
      findHandler('host:imageGeneration.updateConfiguration')(event, input)
    ).resolves.toEqual({ ok: true, value: output })
    await expect(findHandler('host:imageGeneration.setEnabled')(event, input)).resolves.toEqual({
      ok: true,
      value: output
    })
    await expect(findHandler('host:imageGeneration.getStatus')(event)).resolves.toEqual({
      ok: true,
      value: output
    })
    await expect(
      findHandler('host:imageGeneration.readArtifact')(event, artifactInput)
    ).resolves.toEqual({
      ok: true,
      value: output
    })

    expect(coreServer.updateImageGenerationConfiguration).toHaveBeenCalledWith(input)
    expect(coreServer.setImageGenerationEnabled).toHaveBeenCalledWith(input)
    expect(coreServer.readImageGenerationArtifact).toHaveBeenCalledWith(artifactInput)
  })

  it('preserves structured failures while treating secret-bearing input as opaque', async () => {
    const input = {
      credentialMutation: { type: 'replace', value: 'secret-must-remain-opaque' }
    }
    const data = {
      type: 'imageGenerationConfiguration',
      operation: 'updateConfiguration',
      code: 'invalidCredential',
      recovery: 'reenterCredential',
      message: 'The credential is invalid.'
    }
    const updateImageGenerationConfiguration = vi.fn().mockRejectedValue(
      Object.assign(new Error(data.message), {
        code: -32020,
        data
      })
    )
    const coreServer = {
      ...automationCoreStubs(),
      onAgentEvent: vi.fn(),
      onProviderTransition: vi.fn(() => vi.fn()),
      onSkillsChanged: vi.fn(),
      onMcpChanged: vi.fn(() => vi.fn()),
      updateImageGenerationConfiguration
    }
    registerHostIpc(coreServer as never, {} as never, {} as never, () => true)

    await expect(
      findHandler('host:imageGeneration.updateConfiguration')(event, input)
    ).resolves.toEqual({
      ok: false,
      error: { message: data.message, code: -32020, data }
    })
    expect(updateImageGenerationConfiguration).toHaveBeenCalledWith(input)
  })

  it('does not let an untrusted Renderer read private Artifact bytes', () => {
    const readImageGenerationArtifact = vi.fn()
    const coreServer = {
      ...automationCoreStubs(),
      onAgentEvent: vi.fn(),
      onProviderTransition: vi.fn(() => vi.fn()),
      onSkillsChanged: vi.fn(),
      onMcpChanged: vi.fn(() => vi.fn()),
      readImageGenerationArtifact
    }
    registerHostIpc(coreServer as never, {} as never, {} as never, () => false)

    expect(() =>
      findHandler('host:imageGeneration.readArtifact')(event, {
        schemaVersion: 1,
        artifact: { artifactId: 'opaque' }
      })
    ).toThrow(/Blocked untrusted IPC sender/)
    expect(readImageGenerationArtifact).not.toHaveBeenCalled()
  })
})
