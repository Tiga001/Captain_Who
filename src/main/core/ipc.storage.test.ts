import type { IpcMainInvokeEvent } from 'electron'
import { describe, expect, it, vi } from 'vitest'
import { registerStorageIpc } from '../ipc/storageIpc'

const forkInput = {
  requestId: 'conversation-fork-request-1',
  sourceConversationId: 'conversation-1',
  forkPoint: { kind: 'assistant_reply', assistantMessageId: 'assistant-1' } as const
}

function registerForkHandler(forkConversation: ReturnType<typeof vi.fn>) {
  const handlers = new Map<string, (...args: unknown[]) => unknown>()
  const ipcMain = {
    handle: vi.fn((channel: string, handler: (...args: unknown[]) => unknown) => {
      handlers.set(channel, handler)
    }),
    on: vi.fn()
  }
  registerStorageIpc(ipcMain as never, { forkConversation } as never, {} as never)

  const handler = handlers.get('host:storage.forkConversation')
  expect(handler).toBeTypeOf('function')
  return handler as (...args: unknown[]) => unknown
}

describe('conversation fork IPC', () => {
  it('forwards a validated provider-transition boundary fork point', async () => {
    const forkConversation = vi.fn().mockResolvedValue({ id: 'conversation-2' })
    const handler = registerForkHandler(forkConversation)
    const input = {
      requestId: 'conversation-fork-request-2',
      sourceConversationId: 'conversation-1',
      forkPoint: { kind: 'provider_transition_boundary', operationId: 'operation-1' }
    }

    await expect(handler({} as IpcMainInvokeEvent, input)).resolves.toEqual({
      ok: true,
      value: { id: 'conversation-2' }
    })
    expect(forkConversation).toHaveBeenCalledWith(input)
  })

  it('rejects the retired assistant cutoff before calling Core', async () => {
    const forkConversation = vi.fn()
    const handler = registerForkHandler(forkConversation)

    await expect(
      handler({} as IpcMainInvokeEvent, {
        requestId: forkInput.requestId,
        sourceConversationId: forkInput.sourceConversationId,
        throughAssistantMessageId: 'assistant-1'
      })
    ).resolves.toEqual({
      ok: false,
      error: { message: 'Conversation fork failed.' }
    })
    expect(forkConversation).not.toHaveBeenCalled()
  })

  it('preserves the stable active-command code without leaking Core error text', async () => {
    const data = {
      type: 'conversation_fork',
      code: 'active_command_session',
      conversationId: 'conversation-1',
      activeSessionCount: 2
    }
    const forkConversation = vi
      .fn()
      .mockRejectedValue(
        Object.assign(
          new Error(
            'CoreJsonRpcError: UNIQUE constraint failed: conversation_history_blobs.call_id'
          ),
          { code: -32000, data }
        )
      )
    const handler = registerForkHandler(forkConversation)

    await expect(handler({} as IpcMainInvokeEvent, forkInput)).resolves.toEqual({
      ok: false,
      error: {
        message: 'Conversation fork was rejected.',
        code: -32000,
        data
      }
    })
  })

  it('redacts unknown Core and database failures', async () => {
    const forkConversation = vi
      .fn()
      .mockRejectedValue(
        new Error('UNIQUE constraint failed: conversation_history_blobs.conversation_id')
      )
    const handler = registerForkHandler(forkConversation)

    await expect(handler({} as IpcMainInvokeEvent, forkInput)).resolves.toEqual({
      ok: false,
      error: { message: 'Conversation fork failed.' }
    })
  })

  it('redacts malformed recovery data instead of forwarding it to the renderer', async () => {
    const forkConversation = vi.fn().mockRejectedValue(
      Object.assign(new Error('safe-looking message with unsafe recovery data'), {
        code: -32000,
        data: {
          type: 'conversation_fork',
          code: 'active_command_session',
          conversationId: 'conversation-1',
          activeSessionCount: 1,
          sql: 'private schema detail'
        }
      })
    )
    const handler = registerForkHandler(forkConversation)

    await expect(handler({} as IpcMainInvokeEvent, forkInput)).resolves.toEqual({
      ok: false,
      error: { message: 'Conversation fork failed.' }
    })
  })
})

describe('model settings IPC', () => {
  it('routes the safe descriptor projection and returns the authoritative save result', async () => {
    const handlers = new Map<string, (...args: unknown[]) => unknown>()
    const ipcMain = {
      handle: vi.fn((channel: string, handler: (...args: unknown[]) => unknown) => {
        handlers.set(channel, handler)
      }),
      on: vi.fn()
    }
    const descriptors = [{ profileId: 'deepseek_v4_chat', profileVersion: 1 }]
    const settings = {
      apiUrl: '',
      apiToken: '',
      searchMode: 'auto',
      tavilyApiKey: '',
      models: []
    }
    const coreServer = {
      loadProviderProfileUiDescriptors: vi.fn().mockResolvedValue(descriptors),
      saveModelSettings: vi.fn().mockResolvedValue(settings)
    }
    registerStorageIpc(ipcMain as never, coreServer as never, {} as never)

    await expect(handlers.get('host:storage.loadProviderProfileUiDescriptors')?.({})).resolves.toBe(
      descriptors
    )
    await expect(handlers.get('host:storage.saveModelSettings')?.({}, settings)).resolves.toBe(
      settings
    )
    expect(coreServer.saveModelSettings).toHaveBeenCalledWith(settings)
  })
})
