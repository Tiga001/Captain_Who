import { HOST_CHANNELS, HostInvocationError, unwrapHostInvocation } from '@mycopilot/host-api'
import { describe, expect, it, vi } from 'vitest'

vi.mock('electron', () => ({ BrowserWindow: { getAllWindows: () => [] } }))

import { registerAgentIpc } from '../ipc/agentIpc'

describe('Provider transition IPC failure envelope', () => {
  it.each([undefined, -32000])('preserves only the received RPC code: %s', async (code) => {
    const source = Object.assign(new Error('private Core diagnostic'), {
      ...(code === undefined ? {} : { code }),
      data: { continuation: 'private-provider-state' }
    })
    const server = {
      onAgentEvent: vi.fn(),
      onProviderTransition: vi.fn(),
      startProviderTransition: vi.fn().mockRejectedValue(source)
    }
    const ipc = { handle: vi.fn(), on: vi.fn() }
    registerAgentIpc(ipc as never, server as never)
    const handler = ipc.handle.mock.calls.find(
      ([channel]) => channel === HOST_CHANNELS.agent.startProviderTransition
    )?.[1]
    const envelope = await handler({}, {})
    expect(envelope).toEqual({
      ok: false,
      error: {
        message: 'The model-switch check expired. Please try again.',
        ...(code === undefined ? {} : { code })
      }
    })
    try {
      unwrapHostInvocation(envelope)
      expect.fail('a failed Host envelope must reject in Renderer')
    } catch (error) {
      expect(error).toBeInstanceOf(HostInvocationError)
      expect(error).toMatchObject({ code, data: undefined })
    }
  })
})
