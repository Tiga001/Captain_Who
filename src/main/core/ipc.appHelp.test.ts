import type { IpcMainInvokeEvent } from 'electron'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const { ipcMainHandle, showAboutPanel, openExternal } = vi.hoisted(() => ({
  ipcMainHandle: vi.fn(),
  showAboutPanel: vi.fn(),
  openExternal: vi.fn()
}))

vi.mock('electron', () => ({
  app: { showAboutPanel },
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
  shell: { openExternal, showItemInFolder: vi.fn() }
}))

vi.mock('../terminal/TerminalBridge', () => ({ TerminalBridge: class {} }))
vi.mock('../attachments/AttachmentDialogBridge', () => ({ AttachmentDialogBridge: class {} }))
vi.mock('../resources/FaviconResourceCache', () => ({ FaviconResourceCache: class {} }))
vi.mock('../workspaceFiles/WorkspaceFilesService', () => ({ WorkspaceFilesService: class {} }))
vi.mock('../webviews/managedWebviewSecurity', () => ({ clearManagedWebviewData: vi.fn() }))
vi.mock('../appIcon', () => ({ applyAdaptiveAppIcon: vi.fn() }))

import { registerHostIpc } from '../ipc'

const event = { sender: {} } as IpcMainInvokeEvent

function registerHelpHandlers(trusted = true) {
  const coreServer = {
    onConfigurationInvalidated: vi.fn(() => vi.fn()),
    onAutomationEvent: vi.fn(() => vi.fn()),
    onAutomationResync: vi.fn(() => vi.fn()),
    onNotificationEvent: vi.fn(() => vi.fn()),
    onNotificationResync: vi.fn(() => vi.fn()),
    onHumanInteractionSettingsChanged: vi.fn(() => vi.fn()),
    onHumanInteractionRequestChanged: vi.fn(() => vi.fn()),
    onAgentEvent: vi.fn(),
    onProviderTransition: vi.fn(() => vi.fn()),
    onSkillsChanged: vi.fn(),
    onMcpChanged: vi.fn(() => vi.fn())
  }
  const openAppUrl = vi.fn()
  registerHostIpc(
    coreServer as never,
    { setProjectLoader: vi.fn() } as never,
    {} as never,
    () => trusted,
    undefined,
    undefined,
    undefined,
    undefined,
    {
      coreServer,
      historyService: { onChanged: vi.fn(() => vi.fn()) },
      linkRouter: { openAppUrl }
    } as never
  )

  const handler = (channel: string): ((...args: unknown[]) => unknown) => {
    const registration = ipcMainHandle.mock.calls.find(([registered]) => registered === channel)
    expect(registration?.[1]).toBeTypeOf('function')
    return registration?.[1]
  }
  return { handler, openAppUrl }
}

describe('App help IPC', () => {
  beforeEach(() => {
    ipcMainHandle.mockReset()
    showAboutPanel.mockReset()
    openExternal.mockReset().mockResolvedValue(undefined)
  })

  it('opens the existing native About panel without renderer-controlled options', () => {
    const { handler } = registerHelpHandlers()

    handler(HOST_CHANNELS.app.showAbout)(event, { applicationName: 'Ignored' })

    expect(showAboutPanel).toHaveBeenCalledExactlyOnceWith()
    expect(openExternal).not.toHaveBeenCalled()
  })

  it('opens the fixed documentation URL in the system browser, bypassing app link routing', async () => {
    const { handler, openAppUrl } = registerHelpHandlers()

    await handler(HOST_CHANNELS.app.openDocumentation)(event, 'https://example.com', {
      activate: false
    })

    expect(openExternal).toHaveBeenCalledExactlyOnceWith('https://captainwhoagent.com/docs')
    expect(openAppUrl).not.toHaveBeenCalled()
    expect(showAboutPanel).not.toHaveBeenCalled()
  })

  it('propagates system browser failures so the renderer can handle them', async () => {
    const { handler } = registerHelpHandlers()
    openExternal.mockRejectedValue(new Error('System browser unavailable'))

    await expect(handler(HOST_CHANNELS.app.openDocumentation)(event)).rejects.toThrow(
      'System browser unavailable'
    )
  })

  it.each([HOST_CHANNELS.app.showAbout, HOST_CHANNELS.app.openDocumentation])(
    'blocks untrusted senders on %s',
    (channel) => {
      const { handler, openAppUrl } = registerHelpHandlers(false)

      expect(() => handler(channel)(event)).toThrow('Blocked untrusted IPC sender')
      expect(showAboutPanel).not.toHaveBeenCalled()
      expect(openExternal).not.toHaveBeenCalled()
      expect(openAppUrl).not.toHaveBeenCalled()
    }
  )
})
