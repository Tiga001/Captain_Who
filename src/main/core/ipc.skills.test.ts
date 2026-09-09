import { resolve } from 'node:path'

import type { IpcMainInvokeEvent } from 'electron'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const showOpenDialog = vi.hoisted(() => vi.fn())
const fromWebContents = vi.hoisted(() => vi.fn(() => undefined))
const ipcMainHandle = vi.hoisted(() => vi.fn())

vi.mock('electron', () => ({
  BrowserWindow: {
    fromWebContents,
    getAllWindows: vi.fn(() => [])
  },
  Notification: class {
    static isSupported(): boolean {
      return false
    }
  },
  clipboard: { writeText: vi.fn() },
  dialog: { showOpenDialog },
  ipcMain: { handle: ipcMainHandle, on: vi.fn() },
  nativeTheme: { themeSource: 'system' },
  shell: {
    openExternal: vi.fn(),
    showItemInFolder: vi.fn()
  }
}))

vi.mock('../terminal/TerminalBridge', () => ({ TerminalBridge: class {} }))
vi.mock('../attachments/AttachmentDialogBridge', () => ({ AttachmentDialogBridge: class {} }))
vi.mock('../resources/FaviconResourceCache', () => ({ FaviconResourceCache: class {} }))
vi.mock('../workspaceFiles/WorkspaceFilesService', () => ({ WorkspaceFilesService: class {} }))
vi.mock('../webviews/managedWebviewSecurity', () => ({ clearManagedWebviewData: vi.fn() }))

import { registerHostIpc, selectInstallationDirectory } from '../ipc'

const event = { sender: {} } as IpcMainInvokeEvent

function automationCoreStubs() {
  return {
    onConfigurationInvalidated: vi.fn(() => vi.fn()),
    onAutomationEvent: vi.fn(() => vi.fn()),
    onAutomationResync: vi.fn(() => vi.fn()),
    onNotificationEvent: vi.fn(() => vi.fn()),
    onNotificationResync: vi.fn(() => vi.fn()),
    onHumanInteractionSettingsChanged: vi.fn(() => vi.fn()),
    onHumanInteractionRequestChanged: vi.fn(() => vi.fn())
  }
}

describe('Skill installation directory selection', () => {
  beforeEach(() => {
    showOpenDialog.mockReset()
    fromWebContents.mockClear()
    ipcMainHandle.mockReset()
  })

  it('selects a single directory and always returns an absolute path', async () => {
    showOpenDialog.mockResolvedValue({ canceled: false, filePaths: ['fixtures/local-skill'] })

    await expect(selectInstallationDirectory(event)).resolves.toBe(resolve('fixtures/local-skill'))
    expect(showOpenDialog).toHaveBeenCalledWith({
      title: 'Select Skill installation directory',
      properties: ['openDirectory']
    })
  })

  it.each([
    { canceled: true, filePaths: ['/tmp/ignored'] },
    { canceled: false, filePaths: [] }
  ])('returns null when no directory is selected', async (dialogResult) => {
    showOpenDialog.mockResolvedValue(dialogResult)

    await expect(selectInstallationDirectory(event)).resolves.toBeNull()
  })
})

describe('Skill management IPC registration', () => {
  beforeEach(() => {
    ipcMainHandle.mockReset()
  })

  it('preserves structured source resolution errors returned by CoreServer', async () => {
    const input = {
      resolutionId: '11111111-1111-4111-8111-111111111111',
      locator: {
        kind: 'url',
        url: 'https://github.com/openai/example-skills'
      }
    }
    const data = {
      type: 'skillSourceResolution',
      phase: 'resolve',
      code: 'rateLimited',
      recovery: 'retryLater',
      message: 'GitHub temporarily refused the resolution request.',
      provider: 'github',
      retryAfterMs: 60_000
    }
    const resolveSkillInstallationSource = vi.fn().mockRejectedValue(
      Object.assign(new Error(data.message), {
        code: -32013,
        data
      })
    )
    const coreServer = {
      ...automationCoreStubs(),
      onAgentEvent: vi.fn(),
      onProviderTransition: vi.fn(() => vi.fn()),
      onSkillsChanged: vi.fn(),
      onMcpChanged: vi.fn(() => vi.fn()),
      resolveSkillInstallationSource
    }
    registerHostIpc(coreServer as never, {} as never, {} as never, () => true)
    const registration = ipcMainHandle.mock.calls.find(
      ([channel]) => channel === 'host:skills.resolveInstallationSource'
    )
    const handler = registration?.[1] as
      ((event: IpcMainInvokeEvent, input: unknown) => Promise<unknown>) | undefined

    expect(handler).toBeTypeOf('function')
    await expect(handler?.(event, input)).resolves.toEqual({
      ok: false,
      error: {
        message: data.message,
        code: -32013,
        data
      }
    })
    expect(resolveSkillInstallationSource).toHaveBeenCalledWith(input)
  })

  it('wraps source resolution cancellation through the Host invocation envelope', async () => {
    const input = { resolutionId: '11111111-1111-4111-8111-111111111111' }
    const output = { schemaVersion: 2, resolutionId: input.resolutionId, outcome: 'cancelled' }
    const cancelSkillSourceResolution = vi.fn().mockResolvedValue(output)
    const coreServer = {
      ...automationCoreStubs(),
      onAgentEvent: vi.fn(),
      onProviderTransition: vi.fn(() => vi.fn()),
      onSkillsChanged: vi.fn(),
      onMcpChanged: vi.fn(() => vi.fn()),
      cancelSkillSourceResolution
    }
    registerHostIpc(coreServer as never, {} as never, {} as never, () => true)
    const registration = ipcMainHandle.mock.calls.find(
      ([channel]) => channel === 'host:skills.cancelSourceResolution'
    )
    const handler = registration?.[1] as
      ((event: IpcMainInvokeEvent, input: unknown) => Promise<unknown>) | undefined

    await expect(handler?.(event, input)).resolves.toEqual({ ok: true, value: output })
    expect(cancelSkillSourceResolution).toHaveBeenCalledWith(input)
  })

  it('preserves structured uninstall errors returned by CoreServer', async () => {
    const input = {
      skillId: 'installed:user:skill-1',
      expectedRevision:
        'skill-installation-sha256-v1:1111111111111111111111111111111111111111111111111111111111111111'
    }
    const data = {
      type: 'skillInstallation',
      operation: 'uninstall',
      code: 'revisionConflict',
      recovery: 'refreshCatalog',
      message: 'The installed Skill changed. Refresh and try again.',
      commitMayHaveSucceeded: false,
      skillId: input.skillId,
      expectedRevision: input.expectedRevision,
      actualRevision:
        'skill-installation-sha256-v1:2222222222222222222222222222222222222222222222222222222222222222'
    }
    const uninstallSkill = vi.fn().mockRejectedValue(
      Object.assign(new Error(data.message), {
        code: -32010,
        data
      })
    )
    const coreServer = {
      ...automationCoreStubs(),
      onAgentEvent: vi.fn(),
      onProviderTransition: vi.fn(() => vi.fn()),
      onSkillsChanged: vi.fn(),
      onMcpChanged: vi.fn(() => vi.fn()),
      uninstallSkill
    }
    registerHostIpc(coreServer as never, {} as never, {} as never, () => true)
    const registration = ipcMainHandle.mock.calls.find(
      ([channel]) => channel === 'host:skills.uninstall'
    )
    const handler = registration?.[1] as
      ((event: IpcMainInvokeEvent, input: unknown) => Promise<unknown>) | undefined

    expect(handler).toBeTypeOf('function')
    await expect(handler?.(event, input)).resolves.toEqual({
      ok: false,
      error: {
        message: data.message,
        code: -32010,
        data
      }
    })
    expect(uninstallSkill).toHaveBeenCalledWith(input)
  })
})

describe('Office status IPC registration', () => {
  beforeEach(() => {
    ipcMainHandle.mockReset()
  })

  it('registers a trusted no-parameter bridge to CoreServer', async () => {
    const output = { schemaVersion: 1, providerId: 'officecli', availability: 'unavailable' }
    const getOfficeStatus = vi.fn().mockResolvedValue(output)
    const coreServer = {
      ...automationCoreStubs(),
      onAgentEvent: vi.fn(),
      onProviderTransition: vi.fn(() => vi.fn()),
      onSkillsChanged: vi.fn(),
      onMcpChanged: vi.fn(() => vi.fn()),
      getOfficeStatus
    }
    registerHostIpc(coreServer as never, {} as never, {} as never, () => true)
    const registration = ipcMainHandle.mock.calls.find(
      ([channel]) => channel === 'host:office.getStatus'
    )
    const handler = registration?.[1] as
      ((event: IpcMainInvokeEvent) => Promise<unknown>) | undefined

    expect(handler).toBeTypeOf('function')
    await expect(handler?.(event)).resolves.toBe(output)
    expect(getOfficeStatus).toHaveBeenCalledWith()
  })
})
