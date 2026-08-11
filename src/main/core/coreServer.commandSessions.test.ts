import { beforeEach, describe, expect, it, vi } from 'vitest'

const rpcRequest = vi.hoisted(() => vi.fn())

vi.mock('./jsonRpcClient', () => ({
  CoreJsonRpcClient: class {
    readonly request = rpcRequest
  }
}))

import { CoreServer } from './coreServer'

const sessionId = 'cmd_1234567890abcdef1234567890abcdef'
const session = {
  schemaVersion: 1,
  sessionId,
  conversationId: 'conversation-1',
  assistantMessageId: 'assistant-1',
  originRunId: 'run-1',
  callId: 'call-1',
  projectId: 'project-1',
  command: 'python3 app.py',
  cwd: '/workspace',
  commandDigest: `sha256:${'a'.repeat(64)}`,
  status: 'running',
  startedAt: 10,
  latestSequence: 1,
  outputTruncated: false
}

describe('CoreServer command Session client', () => {
  beforeEach(() => rpcRequest.mockReset())

  it('routes conversation-scoped Session listing through the strict response parser', async () => {
    rpcRequest.mockResolvedValue({ sessions: [session] })

    await expect(
      new CoreServer().listCommandSessions({ conversationId: 'conversation-1' })
    ).resolves.toEqual({ sessions: [session] })
    expect(rpcRequest).toHaveBeenCalledWith('agent.commandSessions.list', {
      conversationId: 'conversation-1'
    })
  })

  it('preserves a multiline command across the Core RPC boundary', async () => {
    const command = "python3 <<'PY'\nif True:\n    print('Aspen PDF')\nPY\n"
    rpcRequest.mockResolvedValue({ sessions: [{ ...session, command }] })

    await expect(
      new CoreServer().listCommandSessions({ conversationId: 'conversation-1' })
    ).resolves.toMatchObject({ sessions: [{ command }] })
  })

  it('routes an explicit transcript cursor without consuming the model poll cursor', async () => {
    rpcRequest.mockResolvedValue({
      session,
      transcript: {
        requestedAfterSequence: 0,
        firstAvailableSequence: 1,
        latestSequence: 1,
        truncatedBefore: false,
        outputCaptureTruncated: false,
        chunks: [{ sequence: 1, stream: 'stdout', output: 'ready\n' }]
      }
    })

    await expect(
      new CoreServer().getCommandSession({
        conversationId: 'conversation-1',
        sessionId,
        afterSequence: 0,
        maxBytes: 4096
      })
    ).resolves.toMatchObject({ session: { sessionId }, transcript: { latestSequence: 1 } })
    expect(rpcRequest).toHaveBeenCalledWith('agent.commandSessions.get', {
      conversationId: 'conversation-1',
      sessionId,
      afterSequence: 0,
      maxBytes: 4096
    })
  })

  it('rejects malformed Core output before it reaches the Host API', async () => {
    rpcRequest.mockResolvedValue({ sessions: [{ ...session, sessionId: 'predictable' }] })
    await expect(
      new CoreServer().listCommandSessions({ conversationId: 'conversation-1' })
    ).rejects.toThrow(/managed command Session id/)
  })
})
