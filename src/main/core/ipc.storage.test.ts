import type { IpcMainInvokeEvent } from 'electron'
import { describe, expect, it, vi } from 'vitest'
import { registerStorageIpc } from '../ipc/storageIpc'

const forkInput = {
  requestId: 'conversation-fork-request-1',
  sourceConversationId: 'conversation-1',
  throughAssistantMessageId: 'assistant-1'
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
