import { EventEmitter } from 'node:events'
import type { UtilityProcess, WebContents } from 'electron'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type {
  TerminalServiceOutboundMessage,
  TerminalServiceRequest
} from './terminalTransportProtocol'
import { TerminalBridge } from './TerminalBridge'

vi.mock('electron', () => ({
  utilityProcess: {
    fork: vi.fn()
  }
}))

class FakeUtilityProcess extends EventEmitter {
  readonly kill = vi.fn()
  readonly postMessage = vi.fn()
  readonly stderr = new EventEmitter()

  constructor(readonly pid: number | undefined = 101) {
    super()
  }

  respond(request: TerminalServiceRequest, result?: unknown): void {
    this.emit('message', {
      id: request.id,
      result,
      success: true,
      type: 'response'
    } satisfies TerminalServiceOutboundMessage)
  }
}

class FakeWebContents extends EventEmitter {
  destroyed = false
  readonly send = vi.fn()

  constructor(readonly id: number) {
    super()
  }

  isDestroyed(): boolean {
    return this.destroyed
  }

  destroyOwner(): void {
    this.destroyed = true
    this.emit('destroyed')
  }
}

const bridges: TerminalBridge[] = []

afterEach(() => {
  vi.useRealTimers()
  for (const bridge of bridges.splice(0)) bridge.killNow()
  vi.restoreAllMocks()
})

function createHarness(requestTimeoutMs = 5000, utility = new FakeUtilityProcess()) {
  const forkUtility = vi.fn(() => utility as unknown as UtilityProcess)
  const bridge = new TerminalBridge({
    forkUtility,
    requestTimeoutMs
  })
  bridges.push(bridge)
  return { bridge, forkUtility, utility }
}

function asWebContents(owner: FakeWebContents): WebContents {
  return owner as unknown as WebContents
}

function findRequest(
  utility: FakeUtilityProcess,
  method: TerminalServiceRequest['method']
): TerminalServiceRequest {
  const request = utility.postMessage.mock.calls
    .map(([message]) => message as TerminalServiceRequest)
    .find((message) => message.type === 'request' && message.method === method)
  if (!request) throw new Error(`Missing terminal service request: ${method}`)
  return request
}

async function createSession(
  bridge: TerminalBridge,
  utility: FakeUtilityProcess,
  owner: FakeWebContents,
  sessionId: string
): Promise<void> {
  const createPromise = bridge.createSession(asWebContents(owner), {
    cols: 80,
    cwd: '/workspace',
    rows: 24,
    sessionId
  })
  const request = findRequest(utility, 'terminal.createSession')
  utility.respond(request, {
    cols: 80,
    cwd: '/workspace',
    processId: 123,
    rows: 24,
    sessionId,
    shell: '/bin/sh'
  })
  await createPromise
}

describe('TerminalBridge', () => {
  it('reuses a utility process while Electron is still waiting for its spawn event', async () => {
    const utility = new FakeUtilityProcess(undefined)
    const { bridge, forkUtility } = createHarness(5000, utility)
    const firstOwner = new FakeWebContents(1)
    const secondOwner = new FakeWebContents(2)
    const firstPromise = bridge.createSession(asWebContents(firstOwner), {
      sessionId: 'session-a'
    })
    const secondPromise = bridge.createSession(asWebContents(secondOwner), {
      sessionId: 'session-b'
    })

    const requests = utility.postMessage.mock.calls
      .map(([message]) => message as TerminalServiceRequest)
      .filter(
        (message) => message.type === 'request' && message.method === 'terminal.createSession'
      )
    expect(forkUtility).toHaveBeenCalledOnce()
    expect(requests).toHaveLength(2)

    utility.respond(requests[0], {
      cols: 80,
      cwd: '/workspace',
      rows: 24,
      sessionId: 'session-a',
      shell: '/bin/sh'
    })
    utility.respond(requests[1], {
      cols: 80,
      cwd: '/workspace',
      rows: 24,
      sessionId: 'session-b',
      shell: '/bin/sh'
    })
    await Promise.all([firstPromise, secondPromise])
  })

  it('fails sessions immediately after a fatal utility error without logging its report', async () => {
    const { bridge, utility } = createHarness()
    const owner = new FakeWebContents(1)
    await createSession(bridge, utility, owner, 'session-a')
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined)

    utility.emit('error', 'FatalError', 'native.cc:10', 'secret diagnostic report')

    expect(owner.send).toHaveBeenCalledWith('host:terminal.exit', {
      exitCode: null,
      finalOutputSequence: 0,
      sessionId: 'session-a',
      signal: 'terminal-service-exit'
    })
    expect(consoleError).toHaveBeenCalledWith('Terminal service FatalError at native.cc:10')
    expect(consoleError.mock.calls.flat().join(' ')).not.toContain('secret')
  })

  it('routes output only to the renderer that owns the session', async () => {
    const { bridge, utility } = createHarness()
    const owner = new FakeWebContents(1)
    const otherRenderer = new FakeWebContents(2)
    await createSession(bridge, utility, owner, 'session-a')

    utility.emit('message', {
      event: { data: 'hello', sequence: 1, sessionId: 'session-a' },
      method: 'terminal.output',
      type: 'notification'
    } satisfies TerminalServiceOutboundMessage)

    expect(owner.send).toHaveBeenCalledWith('host:terminal.output', {
      data: 'hello',
      sequence: 1,
      sessionId: 'session-a'
    })
    expect(otherRenderer.send).not.toHaveBeenCalled()
  })

  it('keeps one-way input and ACK commands ordered and ignores a foreign renderer', async () => {
    const { bridge, utility } = createHarness()
    const owner = new FakeWebContents(1)
    const otherRenderer = new FakeWebContents(2)
    await createSession(bridge, utility, owner, 'session-a')
    utility.postMessage.mockClear()

    bridge.writeInput(asWebContents(owner), 'session-a', 'first')
    bridge.writeInput(asWebContents(owner), 'session-a', 'second')
    bridge.acknowledgeOutput(asWebContents(owner), 'session-a', 2)
    bridge.writeInput(asWebContents(otherRenderer), 'session-a', 'foreign')
    bridge.acknowledgeOutput(asWebContents(otherRenderer), 'session-a', 3)

    expect(utility.postMessage.mock.calls.map(([message]) => message)).toEqual([
      {
        method: 'terminal.writeInput',
        params: { data: 'first', sessionId: 'session-a' },
        type: 'command'
      },
      {
        method: 'terminal.writeInput',
        params: { data: 'second', sessionId: 'session-a' },
        type: 'command'
      },
      {
        method: 'terminal.acknowledgeOutput',
        params: { sequence: 2, sessionId: 'session-a' },
        type: 'command'
      }
    ])
  })

  it("does not let a foreign renderer remove another renderer's session", async () => {
    const { bridge, utility } = createHarness()
    const owner = new FakeWebContents(1)
    const otherRenderer = new FakeWebContents(2)
    await createSession(bridge, utility, owner, 'session-a')
    utility.postMessage.mockClear()

    await expect(bridge.killSession(asWebContents(otherRenderer), 'session-a')).resolves.toBe(false)
    bridge.writeInput(asWebContents(owner), 'session-a', 'still-owned')

    expect(utility.postMessage).toHaveBeenCalledOnce()
    expect(utility.postMessage).toHaveBeenCalledWith({
      method: 'terminal.writeInput',
      params: { data: 'still-owned', sessionId: 'session-a' },
      type: 'command'
    })
  })

  it('cancels a pending create and disposes it once when its owner disappears', async () => {
    const { bridge, utility } = createHarness()
    const owner = new FakeWebContents(1)
    const createPromise = bridge.createSession(asWebContents(owner), {
      sessionId: 'session-a'
    })
    const rejection = expect(createPromise).rejects.toThrow(
      'Terminal owner disappeared while the session was starting'
    )
    const request = findRequest(utility, 'terminal.createSession')

    owner.destroyOwner()
    await rejection
    utility.respond(request, {
      cols: 80,
      cwd: '/workspace',
      rows: 24,
      sessionId: 'session-a',
      shell: '/bin/sh'
    })

    const disposeMessages = utility.postMessage.mock.calls
      .map(([message]) => message)
      .filter(
        (message) => message.type === 'command' && message.method === 'terminal.disposeSession'
      )
    expect(disposeMessages).toEqual([
      {
        method: 'terminal.disposeSession',
        params: { sessionId: 'session-a' },
        type: 'command'
      }
    ])
  })

  it.each([
    ['destroy', (owner: FakeWebContents) => owner.destroyOwner()],
    [
      'main-frame reload',
      (owner: FakeWebContents) =>
        owner.emit('did-start-navigation', { isMainFrame: true, isSameDocument: false })
    ],
    ['renderer crash', (owner: FakeWebContents) => owner.emit('render-process-gone')]
  ])('disposes owned sessions exactly once after %s', async (_label, releaseOwner) => {
    const { bridge, utility } = createHarness()
    const owner = new FakeWebContents(1)
    await createSession(bridge, utility, owner, 'session-a')
    utility.postMessage.mockClear()

    releaseOwner(owner)
    releaseOwner(owner)

    expect(utility.postMessage).toHaveBeenCalledTimes(1)
    expect(utility.postMessage).toHaveBeenCalledWith({
      method: 'terminal.disposeSession',
      params: { sessionId: 'session-a' },
      type: 'command'
    })

    utility.emit('message', {
      event: { data: 'late', sequence: 1, sessionId: 'session-a' },
      method: 'terminal.output',
      type: 'notification'
    } satisfies TerminalServiceOutboundMessage)
    expect(owner.send).not.toHaveBeenCalled()
  })

  it('does not release a session for subframe or same-document navigation', async () => {
    const { bridge, utility } = createHarness()
    const owner = new FakeWebContents(1)
    await createSession(bridge, utility, owner, 'session-a')
    utility.postMessage.mockClear()

    owner.emit('did-start-navigation', { isMainFrame: false, isSameDocument: false })
    owner.emit('did-start-navigation', { isMainFrame: true, isSameDocument: true })
    bridge.writeInput(asWebContents(owner), 'session-a', 'still-owned')

    expect(utility.postMessage).toHaveBeenCalledOnce()
    expect(utility.postMessage).toHaveBeenCalledWith({
      method: 'terminal.writeInput',
      params: { data: 'still-owned', sessionId: 'session-a' },
      type: 'command'
    })
  })

  it('times out a stalled request, kills the utility, and rejects pending work', async () => {
    vi.useFakeTimers()
    const { bridge, utility } = createHarness(25)
    const owner = new FakeWebContents(1)
    const createPromise = bridge.createSession(asWebContents(owner), {
      sessionId: 'session-a'
    })
    const rejection = expect(createPromise).rejects.toThrow(
      'Terminal service request timed out: terminal.createSession'
    )

    await vi.advanceTimersByTimeAsync(25)

    await rejection
    expect(utility.kill).toHaveBeenCalledOnce()
    expect(owner.send).toHaveBeenCalledWith('host:terminal.exit', {
      exitCode: null,
      finalOutputSequence: 0,
      sessionId: 'session-a',
      signal: 'terminal-service-exit'
    })
  })
})
