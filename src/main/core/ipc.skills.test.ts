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
  it('preserves structured uninstall errors returned by CoreServer', async () => {
    const input = {
      skillId: 'installed:user:skill-1',
      expectedRevision: 'skill-package-sha256-v1:expected'
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
      actualRevision: 'skill-package-sha256-v1:actual'
    }
    const uninstallSkill = vi.fn().mockRejectedValue(
      Object.assign(new Error(data.message), {
        code: -32010,
        data
      })
    )
    const coreServer = {
      onAgentEvent: vi.fn(),
      onSkillsChanged: vi.fn(),
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
