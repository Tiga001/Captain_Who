import { mkdtempSync, mkdirSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import type { StorageProjectRecord, TerminalSessionSnapshot } from '@mycopilot/protocol'
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

function findRequest<TMethod extends TerminalServiceRequest['method']>(
  utility: FakeUtilityProcess,
  method: TMethod
): Extract<TerminalServiceRequest, { method: TMethod }> {
  const request = utility.postMessage.mock.calls
    .map(([message]) => message as TerminalServiceRequest)
    .find((message) => message.type === 'request' && message.method === method)
  if (!request) throw new Error(`Missing terminal service request: ${method}`)
  return request as Extract<TerminalServiceRequest, { method: TMethod }>
}

function serviceSessionId(request: TerminalServiceRequest): string {
  if (request.method === 'terminal.shutdown' || !request.params.sessionId) {
    throw new Error('Missing internal terminal instance id')
  }
  return request.params.sessionId
}

function createdSnapshot(
  request: TerminalServiceRequest,
  overrides: Partial<TerminalSessionSnapshot> = {}
): TerminalSessionSnapshot {
  return {
    cols: 80,
    cwd: '/workspace',
    processId: 123,
    rows: 24,
    sessionId: serviceSessionId(request),
    shell: '/bin/sh',
    ...overrides
  }
}

async function createSession(
  bridge: TerminalBridge,
  utility: FakeUtilityProcess,
  owner: FakeWebContents,
  sessionId: string
): Promise<string> {
  const createPromise = bridge.createSession(asWebContents(owner), {
    cols: 80,
    cwd: '/workspace',
    rows: 24,
    sessionId
  })
  const request = findRequest(utility, 'terminal.createSession')
  const internalId = serviceSessionId(request)
  expect(internalId).toBe(`terminal-instance-${request.id}`)
  utility.respond(request, createdSnapshot(request))
  await expect(createPromise).resolves.toMatchObject({
    status: 'created',
    session: { sessionId }
  })
  return internalId
}

describe('TerminalBridge', () => {
  it('cancels before starting the utility when the requesting owner is already destroyed', async () => {
    const { bridge, forkUtility, utility } = createHarness()
    const owner = new FakeWebContents(1)
    owner.destroyOwner()

    await expect(
      bridge.createSession(asWebContents(owner), { sessionId: 'already-closed' })
    ).resolves.toEqual({ status: 'cancelled' })

    expect(forkUtility).not.toHaveBeenCalled()
    expect(utility.postMessage).not.toHaveBeenCalled()
    await expect(bridge.killSession(asWebContents(owner), 'already-closed')).resolves.toBe(false)
  })

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

    expect(serviceSessionId(requests[0])).not.toBe(serviceSessionId(requests[1]))
    utility.respond(requests[0], createdSnapshot(requests[0]))
    utility.respond(requests[1], createdSnapshot(requests[1]))
    await expect(firstPromise).resolves.toMatchObject({
      status: 'created',
      session: { sessionId: 'session-a' }
    })
    await expect(secondPromise).resolves.toMatchObject({
      status: 'created',
      session: { sessionId: 'session-b' }
    })
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
    const internalId = await createSession(bridge, utility, owner, 'session-a')

    utility.emit('message', {
      event: { data: 'hello', sequence: 1, sessionId: internalId },
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
    const internalId = await createSession(bridge, utility, owner, 'session-a')
    utility.postMessage.mockClear()

    bridge.writeInput(asWebContents(owner), 'session-a', 'first')
    bridge.writeInput(asWebContents(owner), 'session-a', 'second')
    bridge.acknowledgeOutput(asWebContents(owner), 'session-a', 2)
    bridge.writeInput(asWebContents(otherRenderer), 'session-a', 'foreign')
    bridge.acknowledgeOutput(asWebContents(otherRenderer), 'session-a', 3)

    expect(utility.postMessage.mock.calls.map(([message]) => message)).toEqual([
      {
        method: 'terminal.writeInput',
        params: { data: 'first', sessionId: internalId, userInitiated: true },
        type: 'command'
      },
      {
        method: 'terminal.writeInput',
        params: { data: 'second', sessionId: internalId, userInitiated: true },
        type: 'command'
      },
      {
        method: 'terminal.acknowledgeOutput',
        params: { sequence: 2, sessionId: internalId },
        type: 'command'
      }
    ])
  })

  it("does not let a foreign renderer remove another renderer's session", async () => {
    const { bridge, utility } = createHarness()
    const owner = new FakeWebContents(1)
    const otherRenderer = new FakeWebContents(2)
    const internalId = await createSession(bridge, utility, owner, 'session-a')
    utility.postMessage.mockClear()

    await expect(bridge.killSession(asWebContents(otherRenderer), 'session-a')).resolves.toBe(false)
    bridge.writeInput(asWebContents(owner), 'session-a', 'still-owned')

    expect(utility.postMessage).toHaveBeenCalledOnce()
    expect(utility.postMessage).toHaveBeenCalledWith({
      method: 'terminal.writeInput',
      params: { data: 'still-owned', sessionId: internalId, userInitiated: true },
      type: 'command'
    })
  })

  it('cancels a pending create and disposes it once when its owner disappears', async () => {
    const { bridge, utility } = createHarness()
    const owner = new FakeWebContents(1)
    const createPromise = bridge.createSession(asWebContents(owner), {
      sessionId: 'session-a'
    })
    const cancellation = expect(createPromise).resolves.toEqual({ status: 'cancelled' })
    const request = findRequest(utility, 'terminal.createSession')

    owner.destroyOwner()
    await cancellation
    utility.respond(request, createdSnapshot(request))

    const disposeMessages = utility.postMessage.mock.calls
      .map(([message]) => message)
      .filter(
        (message) => message.type === 'command' && message.method === 'terminal.disposeSession'
      )
    expect(disposeMessages).toEqual([
      {
        method: 'terminal.disposeSession',
        params: { sessionId: serviceSessionId(request) },
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
    const internalId = await createSession(bridge, utility, owner, 'session-a')
    utility.postMessage.mockClear()

    releaseOwner(owner)
    releaseOwner(owner)

    expect(utility.postMessage).toHaveBeenCalledTimes(1)
    expect(utility.postMessage).toHaveBeenCalledWith({
      method: 'terminal.disposeSession',
      params: { sessionId: internalId },
      type: 'command'
    })

    utility.emit('message', {
      event: { data: 'late', sequence: 1, sessionId: internalId },
      method: 'terminal.output',
      type: 'notification'
    } satisfies TerminalServiceOutboundMessage)
    expect(owner.send).not.toHaveBeenCalled()
  })

  it('does not release a session for subframe or same-document navigation', async () => {
    const { bridge, utility } = createHarness()
    const owner = new FakeWebContents(1)
    const internalId = await createSession(bridge, utility, owner, 'session-a')
    utility.postMessage.mockClear()

    owner.emit('did-start-navigation', { isMainFrame: false, isSameDocument: false })
    owner.emit('did-start-navigation', { isMainFrame: true, isSameDocument: true })
    bridge.writeInput(asWebContents(owner), 'session-a', 'still-owned')

    expect(utility.postMessage).toHaveBeenCalledOnce()
    expect(utility.postMessage).toHaveBeenCalledWith({
      method: 'terminal.writeInput',
      params: { data: 'still-owned', sessionId: internalId, userInitiated: true },
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

describe('TerminalBridge project sources', () => {
  it.each(['kill-ack-first', 'load-first'] as const)(
    'cancels during project loading without creating or killing an unstarted PTY (%s)',
    async (order) => {
      const { bridge, utility } = createHarness()
      let finishLoad!: (projects: StorageProjectRecord[]) => void
      bridge.setProjectLoader(
        () =>
          new Promise((resolve) => {
            finishLoad = resolve
          })
      )
      const owner = new FakeWebContents(1)
      const start = bridge.createSession(asWebContents(owner), {
        projectId: 'empty',
        sessionId: 'loading'
      })
      const outcome = start.then(
        (result) => ({ result }),
        (error: unknown) => ({ error })
      )
      const kill = bridge.killSession(asWebContents(owner), 'loading')
      const projects: StorageProjectRecord[] = [
        { id: 'empty', name: 'Empty', createdAt: 1, folders: [] }
      ]
      if (order === 'kill-ack-first') {
        await expect(kill).resolves.toBe(true)
        finishLoad(projects)
      } else {
        finishLoad(projects)
        await expect(kill).resolves.toBe(true)
      }
      await expect(outcome).resolves.toEqual({ result: { status: 'cancelled' } })
      expect(utility.postMessage).not.toHaveBeenCalled()
      await expect(bridge.killSession(asWebContents(owner), 'loading')).resolves.toBe(false)
    }
  )

  it('settles a cancelled pending create before either utility create or kill acknowledges', async () => {
    const { bridge, utility } = createHarness()
    const owner = new FakeWebContents(1)
    let settled = false
    const start = bridge.createSession(asWebContents(owner), { sessionId: 'pending' })
    const outcome = start.then(
      (result) => {
        settled = true
        return { result }
      },
      (error: unknown) => {
        settled = true
        return { error }
      }
    )
    const create = findRequest(utility, 'terminal.createSession')
    const kill = bridge.killSession(asWebContents(owner), 'pending')
    const killOutcome = kill.then(
      (result) => ({ result }),
      (error: unknown) => ({ error })
    )
    const killRequest = findRequest(utility, 'terminal.killSession')
    expect(serviceSessionId(killRequest)).toBe(serviceSessionId(create))
    await vi.waitFor(() => expect(settled).toBe(true), { timeout: 250, interval: 1 })
    await expect(outcome).resolves.toEqual({ result: { status: 'cancelled' } })
    bridge.writeInput(asWebContents(owner), 'pending', 'must not reach PTY')
    bridge.acknowledgeOutput(asWebContents(owner), 'pending', 1)
    expect(utility.postMessage.mock.calls.map(([message]) => message.method)).toEqual([
      'terminal.createSession',
      'terminal.killSession'
    ])
    utility.respond(killRequest, true)
    await expect(killOutcome).resolves.toEqual({ result: true })
    utility.respond(create, createdSnapshot(create))
    expect(owner.send).not.toHaveBeenCalled()
    expect(utility.postMessage).toHaveBeenCalledTimes(2)
  })

  it.each(['before-new-create', 'after-new-create'] as const)(
    'isolates a reused client id from old create, kill, output, and exit responses (%s)',
    async (lateOrder) => {
      const { bridge, utility } = createHarness()
      const owner = new FakeWebContents(1)
      const oldStart = bridge.createSession(asWebContents(owner), { sessionId: 'reused' })
      const oldOutcome = oldStart.then(
        (result) => ({ result }),
        (error: unknown) => ({ error })
      )
      const oldCreate = findRequest(utility, 'terminal.createSession')
      const oldId = serviceSessionId(oldCreate)
      const oldKill = bridge.killSession(asWebContents(owner), 'reused')
      const killOutcome = oldKill.then(
        (result) => ({ result }),
        (error: unknown) => ({ error })
      )
      const oldKillRequest = findRequest(utility, 'terminal.killSession')
      expect(serviceSessionId(oldKillRequest)).toBe(oldId)
      utility.postMessage.mockClear()

      // Reuse is allowed before the old utility instance acknowledges its kill.
      const nextStart = bridge.createSession(asWebContents(owner), { sessionId: 'reused' })
      const nextOutcome = nextStart.then(
        (result) => ({ result }),
        (error: unknown) => ({ error })
      )
      const nextCreate = findRequest(utility, 'terminal.createSession')
      const nextId = serviceSessionId(nextCreate)
      expect(nextId).not.toBe(oldId)
      const emitOldReplies = () => {
        utility.respond(oldKillRequest, true)
        utility.respond(oldCreate, createdSnapshot(oldCreate, { cwd: '/old-instance' }))
        utility.emit('message', {
          type: 'notification',
          method: 'terminal.output',
          event: { sessionId: oldId, data: 'old output', sequence: 900 }
        } satisfies TerminalServiceOutboundMessage)
        utility.emit('message', {
          type: 'notification',
          method: 'terminal.exit',
          event: { sessionId: oldId, exitCode: 0, finalOutputSequence: 900 }
        } satisfies TerminalServiceOutboundMessage)
      }
      if (lateOrder === 'before-new-create') emitOldReplies()
      utility.respond(nextCreate, createdSnapshot(nextCreate, { cwd: '/new-instance' }))
      await expect(nextOutcome).resolves.toMatchObject({
        result: { status: 'created', session: { sessionId: 'reused', cwd: '/new-instance' } }
      })
      if (lateOrder === 'after-new-create') emitOldReplies()
      await expect(oldOutcome).resolves.toEqual({ result: { status: 'cancelled' } })
      await expect(killOutcome).resolves.toEqual({ result: true })
      expect(owner.send).not.toHaveBeenCalled()
      utility.postMessage.mockClear()
      bridge.writeInput(asWebContents(owner), 'reused', 'new input')
      bridge.markUserInput(asWebContents(owner), 'reused')
      bridge.acknowledgeOutput(asWebContents(owner), 'reused', 1)
      const resize = bridge.resizeSession(asWebContents(owner), 'reused', 100, 40)
      const select = bridge.selectSourceDirectory(asWebContents(owner), 'reused', 'aux')
      const messages = utility.postMessage.mock.calls.map(([message]) => message)
      expect(messages.map((message) => message.method)).toEqual([
        'terminal.writeInput',
        'terminal.markUserInput',
        'terminal.acknowledgeOutput',
        'terminal.resizeSession',
        'terminal.selectSourceDirectory'
      ])
      expect(messages.every((message) => message.params.sessionId === nextId)).toBe(true)
      utility.respond(findRequest(utility, 'terminal.resizeSession'))
      utility.respond(findRequest(utility, 'terminal.selectSourceDirectory'))
      await Promise.all([resize, select])
      utility.emit('message', {
        type: 'notification',
        method: 'terminal.output',
        event: { sessionId: nextId, data: 'new output', sequence: 1 }
      } satisfies TerminalServiceOutboundMessage)
      expect(owner.send).toHaveBeenCalledExactlyOnceWith('host:terminal.output', {
        sessionId: 'reused',
        data: 'new output',
        sequence: 1
      })
      utility.emit('message', {
        type: 'notification',
        method: 'terminal.exit',
        event: { sessionId: nextId, exitCode: 0, finalOutputSequence: 1 }
      } satisfies TerminalServiceOutboundMessage)
      expect(owner.send).toHaveBeenLastCalledWith('host:terminal.exit', {
        sessionId: 'reused',
        exitCode: 0,
        finalOutputSequence: 1
      })
      utility.postMessage.mockClear()
      bridge.writeInput(asWebContents(owner), 'reused', 'after exit')
      expect(utility.postMessage).not.toHaveBeenCalled()
    }
  )

  it('preserves a genuine project-loader failure as a rejected create', async () => {
    const { bridge, utility } = createHarness()
    bridge.setProjectLoader(async () => {
      throw new Error('Project database unavailable')
    })
    await expect(
      bridge.createSession(asWebContents(new FakeWebContents(1)), {
        projectId: 'project',
        sessionId: 'loader-failure'
      })
    ).rejects.toThrow('Project database unavailable')
    expect(
      utility.postMessage.mock.calls.some(
        ([message]) => message.method === 'terminal.createSession'
      )
    ).toBe(false)
  })

  it('preserves a genuine PTY creation error as a rejected create', async () => {
    const { bridge, utility } = createHarness()
    const start = bridge.createSession(asWebContents(new FakeWebContents(1)), {
      sessionId: 'pty-failure'
    })
    const rejection = expect(start).rejects.toThrow('spawn ENOENT')
    const create = findRequest(utility, 'terminal.createSession')
    utility.emit('message', {
      id: create.id,
      type: 'response',
      success: false,
      error: 'spawn ENOENT'
    } satisfies TerminalServiceOutboundMessage)
    await rejection
  })

  it.each(['project-load', 'utility-create'] as const)(
    'preserves a utility exit during %s as a rejected create',
    async (phase) => {
      const { bridge, utility } = createHarness()
      let finishLoad: ((projects: StorageProjectRecord[]) => void) | undefined
      bridge.setProjectLoader(
        () =>
          new Promise((resolve) => {
            finishLoad = resolve
          })
      )
      const start = bridge.createSession(asWebContents(new FakeWebContents(1)), {
        ...(phase === 'project-load' ? { projectId: 'empty' } : {}),
        sessionId: 'utility-exit'
      })
      const rejection = expect(start).rejects.toThrow('Terminal service exited with code 17')
      utility.emit('exit', 17)
      finishLoad?.([{ id: 'empty', name: 'Empty', createdAt: 1, folders: [] }])
      await rejection
    }
  )

  it('loads primary and immutable source identities from Host records, ignoring supplied cwd', async () => {
    const base = mkdtempSync(join(tmpdir(), 'terminal-host-project-'))
    try {
      const primary = join(base, 'primary')
      const aux = join(base, 'aux')
      mkdirSync(primary)
      mkdirSync(aux)
      const project: StorageProjectRecord = {
        id: 'project',
        name: 'Project',
        createdAt: 1,
        folders: [
          { id: 'main', alias: 'main', path: primary, role: 'primary', sortOrder: 0, createdAt: 1 },
          { id: 'aux', alias: 'aux', path: aux, role: 'auxiliary', sortOrder: 1, createdAt: 1 }
        ]
      }
      const { bridge, utility } = createHarness()
      bridge.setProjectLoader(async () => [project])
      const owner = new FakeWebContents(1)
      const start = bridge.createSession(asWebContents(owner), {
        projectId: 'project',
        cwd: '/untrusted/path',
        sessionId: 'project-terminal'
      })
      await vi.waitFor(() => expect(utility.postMessage).toHaveBeenCalled())
      const request = findRequest(utility, 'terminal.createSession')
      if (request.method !== 'terminal.createSession') throw new Error('Wrong request')
      expect(request.params.cwd).toBe(primary)
      expect(request.params.projectSources?.projectId).toBe('project')
      expect(request.params.projectSources?.folders.map((folder) => folder.id)).toEqual([
        'main',
        'aux'
      ])
      expect(request.params.projectSources?.folders[1].identity?.inode).toMatch(/^\d+$/)
      utility.respond(request, createdSnapshot(request, { cwd: primary }))
      await expect(start).resolves.toMatchObject({
        status: 'created',
        session: { sessionId: 'project-terminal' }
      })
      utility.postMessage.mockClear()
      const selection = bridge.selectSourceDirectory(
        asWebContents(owner),
        'project-terminal',
        'aux'
      )
      bridge.writeInput(asWebContents(owner), 'project-terminal', 'pwd\r')
      const messages = utility.postMessage.mock.calls.map(([message]) => message)
      expect(messages.map((message) => message.method)).toEqual([
        'terminal.selectSourceDirectory',
        'terminal.writeInput'
      ])
      expect(messages[0].params).toEqual({ sessionId: serviceSessionId(request), folderId: 'aux' })
      expect(messages[1].params.sessionId).toBe(serviceSessionId(request))
      utility.respond(messages[0])
      await selection
      expect(() =>
        bridge.selectSourceDirectory(
          asWebContents(new FakeWebContents(2)),
          'project-terminal',
          'aux'
        )
      ).toThrow()
      bridge.markUserInput(asWebContents(new FakeWebContents(2)), 'project-terminal')
      expect(utility.postMessage).toHaveBeenCalledTimes(2)
    } finally {
      rmSync(base, { recursive: true, force: true })
    }
  })

  it('fails a missing project and preserves HOME fallback for a project without folders', async () => {
    const { bridge, utility } = createHarness()
    bridge.setProjectLoader(async () => [{ id: 'empty', name: 'Empty', createdAt: 1, folders: [] }])
    const owner = new FakeWebContents(1)
    await expect(
      bridge.createSession(asWebContents(owner), { projectId: 'missing', sessionId: 'missing' })
    ).rejects.toThrow('no longer exists')
    utility.postMessage.mockClear()
    const start = bridge.createSession(asWebContents(owner), {
      projectId: 'empty',
      cwd: '/ignored',
      sessionId: 'empty-terminal'
    })
    await vi.waitFor(() => expect(utility.postMessage).toHaveBeenCalled())
    const request = findRequest(utility, 'terminal.createSession')
    if (request.method !== 'terminal.createSession') throw new Error('Wrong request')
    expect(request.params.cwd).toBeUndefined()
    expect(request.params.projectSources).toBeUndefined()
    utility.respond(request, createdSnapshot(request, { cwd: '/home/test' }))
    await expect(start).resolves.toMatchObject({
      status: 'created',
      session: { sessionId: 'empty-terminal' }
    })
  })

  it('does not attach a stale project load after its owner has navigated away', async () => {
    const { bridge, utility } = createHarness()
    let resolveLoad!: (projects: StorageProjectRecord[]) => void
    bridge.setProjectLoader(
      () =>
        new Promise((resolve) => {
          resolveLoad = resolve
        })
    )
    const owner = new FakeWebContents(1)
    const start = bridge.createSession(asWebContents(owner), {
      projectId: 'empty',
      sessionId: 'stale'
    })
    const cancelled = expect(start).resolves.toEqual({ status: 'cancelled' })
    owner.destroyOwner()
    resolveLoad([{ id: 'empty', name: 'Empty', createdAt: 1, folders: [] }])
    await cancelled
    expect(
      utility.postMessage.mock.calls.some(
        ([message]) => message.method === 'terminal.createSession'
      )
    ).toBe(false)
  })
})

it('does not let a stale project load remove a replacement session with the same local id', async () => {
  const { bridge, utility } = createHarness()
  let finishOld!: (projects: StorageProjectRecord[]) => void
  const project: StorageProjectRecord = { id: 'empty', name: 'Empty', createdAt: 1, folders: [] }
  const loader = vi
    .fn<() => Promise<StorageProjectRecord[]>>()
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishOld = resolve
        })
    )
    .mockResolvedValue([project])
  bridge.setProjectLoader(loader)
  const previousOwner = new FakeWebContents(1)
  const oldStart = bridge.createSession(asWebContents(previousOwner), {
    projectId: 'empty',
    sessionId: 'reused'
  })
  const oldCancelled = expect(oldStart).resolves.toEqual({ status: 'cancelled' })
  previousOwner.destroyOwner()
  utility.postMessage.mockClear()
  const nextOwner = new FakeWebContents(2)
  const nextStart = bridge.createSession(asWebContents(nextOwner), {
    projectId: 'empty',
    sessionId: 'reused'
  })
  await vi.waitFor(() => expect(utility.postMessage).toHaveBeenCalled())
  const nextRequest = findRequest(utility, 'terminal.createSession')
  utility.respond(nextRequest, createdSnapshot(nextRequest, { cwd: '/home/test' }))
  await expect(nextStart).resolves.toMatchObject({
    status: 'created',
    session: { sessionId: 'reused' }
  })
  utility.postMessage.mockClear()
  finishOld([project])
  await oldCancelled
  expect(utility.postMessage).not.toHaveBeenCalled()
  bridge.writeInput(asWebContents(nextOwner), 'reused', 'still-new')
  expect(utility.postMessage).toHaveBeenCalledExactlyOnceWith({
    method: 'terminal.writeInput',
    params: { data: 'still-new', sessionId: serviceSessionId(nextRequest), userInitiated: true },
    type: 'command'
  })
})

it('returns directory display fields from the same snapshot frozen after asynchronous project loading', async () => {
  const base = mkdtempSync(join(tmpdir(), 'terminal-project-race-'))
  try {
    const oldPath = join(base, 'before')
    const newPath = join(base, 'after')
    const addedPath = join(base, 'added')
    for (const path of [oldPath, newPath, addedPath]) mkdirSync(path)
    const project: StorageProjectRecord = {
      id: 'project',
      name: 'Project',
      createdAt: 1,
      folders: [
        { id: 'main', alias: 'before', path: oldPath, role: 'primary', sortOrder: 0, createdAt: 1 }
      ]
    }
    const { bridge, utility } = createHarness()
    let finishLoad!: (projects: StorageProjectRecord[]) => void
    bridge.setProjectLoader(
      () =>
        new Promise((resolve) => {
          finishLoad = resolve
        })
    )
    const start = bridge.createSession(asWebContents(new FakeWebContents(1)), {
      projectId: 'project',
      cwd: oldPath,
      sessionId: 'race'
    })
    // Reproduces editing a project before its pending load completes.
    project.folders[0] = { ...project.folders[0], path: newPath, alias: 'after' }
    project.folders.push({
      id: 'added',
      alias: 'added',
      path: addedPath,
      role: 'auxiliary',
      sortOrder: 1,
      createdAt: 1
    })
    finishLoad([project])
    await vi.waitFor(() => expect(utility.postMessage).toHaveBeenCalled())
    const request = findRequest(utility, 'terminal.createSession')
    if (request.method !== 'terminal.createSession') throw new Error('Wrong request')
    expect(request.params.cwd).toBe(newPath)
    const frozen = request.params.projectSources!.folders
    expect(frozen.map(({ id, alias, path }) => ({ id, alias, path }))).toEqual([
      { id: 'main', alias: 'after', path: newPath },
      { id: 'added', alias: 'added', path: addedPath }
    ])
    // Further changes must not leak into this terminal's response while PTY starts.
    project.folders[0].alias = 'later'
    project.folders.length = 1
    utility.respond(request, createdSnapshot(request, { cwd: newPath }))
    const result = await start
    expect(result.status).toBe('created')
    if (result.status !== 'created') throw new Error('Terminal was unexpectedly cancelled')
    const snapshot = result.session
    expect(snapshot.sessionId).toBe('race')
    expect(snapshot.sourceFolders).toEqual(
      frozen.map(({ id, alias, path, role }) => ({ id, alias, path, role }))
    )
    expect(snapshot.sourceFolders![0].alias).toBe('after')
    expect(snapshot.sourceFolders).toHaveLength(2)
    expect(snapshot.sourceFolders![0]).not.toHaveProperty('identity')
  } finally {
    rmSync(base, { recursive: true, force: true })
  }
})
