import { describe, expect, it } from 'vitest'
import {
  parseAgentCommandSessionEvent,
  parseAgentCommandSessionGetInput,
  parseAgentCommandSessionGetOutput,
  parseAgentCommandSessionListOutput,
  parseAgentCommandSessionTranscript
} from './agentCommandSessionParsers'
import { AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS } from './agent'
import { parseAgentEventForHost } from './agentMcpParsers'

const sessionId = 'cmd_1234567890abcdef1234567890abcdef'
const snapshot = {
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
  latestSequence: 2,
  outputTruncated: false
} as const

describe('Agent command Session protocol', () => {
  it('strictly parses all lifecycle events with durable routing identity', () => {
    const started = {
      type: 'command_started',
      runId: 'run-1',
      conversationId: 'conversation-1',
      assistantMessageId: 'assistant-1',
      callId: 'call-1',
      sessionId,
      startedAt: 10
    } as const
    expect(parseAgentCommandSessionEvent(started)).toMatchObject({
      type: 'command_started',
      sessionId
    })
    expect(parseAgentEventForHost(started)).toMatchObject({ type: 'command_started', sessionId })

    expect(
      parseAgentCommandSessionEvent({
        type: 'command_output',
        runId: 'run-1',
        conversationId: 'conversation-1',
        assistantMessageId: 'assistant-1',
        callId: 'call-1',
        sessionId,
        sequence: 2,
        stream: 'stdout',
        output: 'ready\n'
      })
    ).toMatchObject({ type: 'command_output', sequence: 2 })

    expect(
      parseAgentCommandSessionEvent({
        type: 'command_exited',
        runId: 'run-1',
        conversationId: 'conversation-1',
        assistantMessageId: 'assistant-1',
        callId: 'call-1',
        sessionId,
        status: 'exited',
        exitCode: 0,
        endedAt: 20,
        latestSequence: 2,
        outputTruncated: false
      })
    ).toMatchObject({ type: 'command_exited', exitCode: 0 })

    expect(
      parseAgentCommandSessionEvent({
        type: 'command_interrupted',
        runId: 'run-1',
        conversationId: 'conversation-1',
        assistantMessageId: 'assistant-1',
        callId: 'call-1',
        sessionId,
        endedAt: 20,
        latestSequence: 2,
        outputTruncated: false
      })
    ).toMatchObject({ type: 'command_interrupted', sessionId })
  })

  it('parses bounded list and cursor-addressed transcript responses', () => {
    expect(parseAgentCommandSessionListOutput({ sessions: [snapshot] })).toEqual({
      sessions: [snapshot]
    })
    expect(
      parseAgentCommandSessionGetOutput({
        session: snapshot,
        transcript: {
          requestedAfterSequence: 0,
          firstAvailableSequence: 1,
          latestSequence: 2,
          truncatedBefore: false,
          outputCaptureTruncated: false,
          chunks: [
            { sequence: 1, stream: 'stdout', output: 'boot\n' },
            { sequence: 2, stream: 'stderr', output: 'ready\n' }
          ]
        }
      })
    ).toMatchObject({ session: { sessionId }, transcript: { latestSequence: 2 } })
  })

  it('accepts the Host transcript chunk boundary and rejects one item beyond it', () => {
    const chunks = Array.from(
      { length: AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS },
      (_, index) => ({ sequence: index + 1, stream: 'stdout' as const, output: 'x' })
    )
    expect(
      parseAgentCommandSessionGetOutput({
        session: {
          ...snapshot,
          latestSequence: AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS
        },
        transcript: {
          requestedAfterSequence: 0,
          firstAvailableSequence: 1,
          latestSequence: AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS,
          truncatedBefore: false,
          outputCaptureTruncated: false,
          chunks
        }
      }).transcript.chunks
    ).toHaveLength(AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS)

    expect(() =>
      parseAgentCommandSessionTranscript({
        requestedAfterSequence: 0,
        firstAvailableSequence: 1,
        latestSequence: AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS + 1,
        truncatedBefore: false,
        outputCaptureTruncated: false,
        chunks: [
          ...chunks,
          {
            sequence: AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS + 1,
            stream: 'stderr',
            output: 'y'
          }
        ]
      })
    ).toThrow(/chunks exceeded 2048 items/)
  })

  it('validates Host query identities and bounds', () => {
    expect(
      parseAgentCommandSessionGetInput({
        conversationId: 'conversation-1',
        sessionId,
        afterSequence: 3,
        maxBytes: 4096
      })
    ).toMatchObject({ sessionId, maxBytes: 4096 })
    expect(() =>
      parseAgentCommandSessionGetInput({ conversationId: 'conversation-1', sessionId: 'guessable' })
    ).toThrow(/managed command Session id/)
    expect(() =>
      parseAgentCommandSessionGetInput({
        conversationId: 'conversation-1',
        sessionId,
        maxBytes: 1024 * 1024 + 1
      })
    ).toThrow(/exceeded maximum/)
  })

  it('rejects malformed events, unbounded output and inconsistent snapshots', () => {
    expect(() =>
      parseAgentCommandSessionEvent({
        type: 'command_output',
        runId: 'run-1',
        conversationId: 'conversation-1',
        assistantMessageId: 'assistant-1',
        callId: 'call-1',
        sessionId,
        sequence: 1,
        stream: 'stdout',
        output: 'x'.repeat(64 * 1024 + 1)
      })
    ).toThrow(/exceeded 65536 UTF-8 bytes/)
    expect(() =>
      parseAgentCommandSessionListOutput({
        sessions: [{ ...snapshot, status: 'exited', endedAt: undefined }]
      })
    ).toThrow(/endedAt presence/)
    expect(() =>
      parseAgentCommandSessionListOutput({
        sessions: [
          {
            ...snapshot,
            status: 'outcome_unknown',
            endedAt: 20,
            exitCode: 0
          }
        ]
      })
    ).toThrow(/exitCode is valid only for exited status/)
    expect(() =>
      parseAgentCommandSessionListOutput({
        sessions: [{ ...snapshot, commandDigest: 'a'.repeat(64) }]
      })
    ).toThrow(/SHA-256 command digest/)
    expect(() =>
      parseAgentCommandSessionEvent({
        type: 'command_started',
        runId: 'run-1',
        conversationId: 'conversation-1',
        assistantMessageId: 'assistant-1',
        callId: 'call-1',
        sessionId,
        startedAt: 10,
        rawProcessId: 123
      })
    ).toThrow(/unexpected field rawProcessId/)
  })
})
