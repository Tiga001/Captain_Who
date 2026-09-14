import { describe, expect, it, vi } from 'vitest'
import { HOST_CHANNELS } from '@mycopilot/host-api'
vi.mock('electron', () => ({ BrowserWindow: { getAllWindows: () => [] } }))
import { registerAgentIpc } from '../ipc/agentIpc'

describe('account gate only protects new conversation turns', () => {
  it('awaits a refreshed Core grant before starting or rewriting', async () => {
    const handlers = new Map<string, (...args: unknown[]) => unknown>()
    const core = {
      onAgentEvent: vi.fn(),
      onProviderTransition: vi.fn(),
      startConversationTurn: vi.fn().mockResolvedValue({ runId: 'new' }),
      rewriteConversationTurn: vi.fn().mockResolvedValue({ runId: 'rewrite' })
    }
    let release!: () => void
    const gate = new Promise<void>((resolve) => {
      release = resolve
    })
    registerAgentIpc(
      {
        handle: (channel: string, handler: (...args: unknown[]) => unknown) =>
          handlers.set(channel, handler),
        on: vi.fn()
      } as never,
      core as never,
      () => gate
    )
    const start = handlers.get(HOST_CHANNELS.agent.startConversationTurn)!({}, {})
    const rewrite = handlers.get(HOST_CHANNELS.agent.rewriteConversationTurn)!({}, {})
    expect(core.startConversationTurn).not.toHaveBeenCalled()
    expect(core.rewriteConversationTurn).not.toHaveBeenCalled()
    release()
    await Promise.all([start, rewrite])
    expect(core.startConversationTurn).toHaveBeenCalledOnce()
    expect(core.rewriteConversationTurn).toHaveBeenCalledOnce()
  })
  it('blocks new/rewrite turns after logout but leaves steering, approval, cancellation and events intact', async () => {
    const handlers = new Map<string, (...args: unknown[]) => unknown>()
    const ipc = {
      handle: (channel: string, handler: (...args: unknown[]) => unknown) =>
        handlers.set(channel, handler),
      on: vi.fn()
    }
    const core = {
      onAgentEvent: vi.fn(),
      onProviderTransition: vi.fn(),
      startConversationTurn: vi.fn().mockResolvedValue({ runId: 'run-1' }),
      rewriteConversationTurn: vi.fn().mockResolvedValue({ runId: 'run-2' }),
      steerRun: vi.fn().mockResolvedValue({ accepted: true }),
      cancelRun: vi.fn().mockResolvedValue(undefined),
      listPendingActions: vi.fn().mockResolvedValue([]),
      shutdown: vi.fn()
    }
    let signedIn = true
    registerAgentIpc(ipc as never, core as never, () => {
      if (!signedIn) throw new Error('ACCOUNT_LOGIN_REQUIRED')
    })
    const invoke = (channel: string) => handlers.get(channel)!({}, { runId: 'run-1' })
    await expect(invoke(HOST_CHANNELS.agent.startConversationTurn)).resolves.toMatchObject({
      ok: true
    })
    signedIn = false
    await expect(invoke(HOST_CHANNELS.agent.startConversationTurn)).resolves.toMatchObject({
      ok: false,
      error: { message: 'ACCOUNT_LOGIN_REQUIRED' }
    })
    await expect(invoke(HOST_CHANNELS.agent.rewriteConversationTurn)).resolves.toMatchObject({
      ok: false
    })
    expect(core.startConversationTurn).toHaveBeenCalledTimes(1)
    expect(core.rewriteConversationTurn).not.toHaveBeenCalled()
    expect(core.cancelRun).not.toHaveBeenCalled()
    expect(core.shutdown).not.toHaveBeenCalled()
    await expect(invoke(HOST_CHANNELS.agent.steerRun)).resolves.toEqual({ accepted: true })
    await expect(invoke(HOST_CHANNELS.agent.listPendingActions)).resolves.toEqual([])
    await invoke(HOST_CHANNELS.agent.cancelRun)
    expect(core.cancelRun).toHaveBeenCalledTimes(1)
    signedIn = true
    await expect(invoke(HOST_CHANNELS.agent.startConversationTurn)).resolves.toMatchObject({
      ok: true
    })
  })
  it('blocks unlicensed new turns while keeping local Token reads and running work available', async () => {
    const handlers = new Map<string, (...args: unknown[]) => unknown>()
    const core = {
      onAgentEvent: vi.fn(),
      onProviderTransition: vi.fn(),
      startConversationTurn: vi.fn(),
      rewriteConversationTurn: vi.fn(),
      getLocalTokenUsage: vi.fn().mockResolvedValue({ totalTokens: '42' }),
      steerRun: vi.fn().mockResolvedValue({ accepted: true }),
      cancelRun: vi.fn(),
      shutdown: vi.fn()
    }
    registerAgentIpc(
      {
        handle: (channel: string, handler: (...args: unknown[]) => unknown) =>
          handlers.set(channel, handler),
        on: vi.fn()
      } as never,
      core as never,
      () => {
        throw new Error('ACCOUNT_LICENSE_REQUIRED')
      }
    )
    const invoke = (channel: string) => handlers.get(channel)!({}, { runId: 'running' })
    for (const channel of [
      HOST_CHANNELS.agent.startConversationTurn,
      HOST_CHANNELS.agent.rewriteConversationTurn
    ]) {
      await expect(invoke(channel)).resolves.toMatchObject({
        ok: false,
        error: { message: 'ACCOUNT_LICENSE_REQUIRED' }
      })
    }
    await expect(invoke(HOST_CHANNELS.agent.getLocalTokenUsage)).resolves.toEqual({
      totalTokens: '42'
    })
    await expect(invoke(HOST_CHANNELS.agent.steerRun)).resolves.toEqual({ accepted: true })
    expect(core.startConversationTurn).not.toHaveBeenCalled()
    expect(core.rewriteConversationTurn).not.toHaveBeenCalled()
    expect(core.cancelRun).not.toHaveBeenCalled()
    expect(core.shutdown).not.toHaveBeenCalled()
  })
  it('fails closed when a caller forgets to supply the account guard', async () => {
    const handlers = new Map<string, (...args: unknown[]) => unknown>()
    registerAgentIpc(
      {
        handle: (channel, handler) => {
          handlers.set(channel, handler)
        },
        on: vi.fn()
      } as never,
      { onAgentEvent: vi.fn(), onProviderTransition: vi.fn() } as never
    )
    await expect(
      handlers.get(HOST_CHANNELS.agent.startConversationTurn)!({}, {})
    ).resolves.toMatchObject({ ok: false })
  })
})
