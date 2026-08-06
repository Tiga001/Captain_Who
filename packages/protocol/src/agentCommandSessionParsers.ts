import type {
  AgentCommandSessionGetInput,
  AgentCommandSessionGetOutput,
  AgentCommandSessionListInput,
  AgentCommandSessionListOutput,
  AgentCommandSessionOutputChunk,
  AgentCommandSessionSnapshot,
  AgentCommandSessionStatus,
  AgentCommandSessionTranscript,
  AgentEvent
} from './agent'
import { AGENT_COMMAND_SESSION_SCHEMA_VERSION } from './agent'
import {
  expectArray,
  expectBoolean,
  expectEnum,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  expectString,
  invalidProtocolValue,
  optionalNonEmptyString
} from './skills/validation'

const COMMAND_SESSION_ID_PATTERN = /^cmd_[0-9a-f]{32}$/
const COMMAND_DIGEST_PATTERN = /^sha256:[0-9a-f]{64}$/
const MAX_ID_BYTES = 512
const MAX_COMMAND_BYTES = 8 * 1024
const MAX_CWD_BYTES = 16 * 1024
const MAX_EVENT_OUTPUT_BYTES = 64 * 1024
const MAX_TRANSCRIPT_BYTES = 1024 * 1024
const MAX_TRANSCRIPT_CHUNKS = 2048
const MAX_LISTED_SESSIONS = 512

const SESSION_STATUSES = [
  'starting',
  'running',
  'exited',
  'interrupted',
  'timed_out',
  'failed',
  'outcome_unknown'
] as const

const COMMAND_EVENT_TYPES = [
  'command_started',
  'command_output',
  'command_exited',
  'command_interrupted'
] as const

export function isAgentCommandSessionEventType(
  value: unknown
): value is (typeof COMMAND_EVENT_TYPES)[number] {
  return typeof value === 'string' && (COMMAND_EVENT_TYPES as readonly string[]).includes(value)
}

export function parseAgentCommandSessionEvent(value: unknown): AgentEvent {
  const record = expectRecord(value, 'Agent command Session event')
  const type = expectEnum(record.type, COMMAND_EVENT_TYPES, 'Agent command Session event.type')
  const identity = parseEventIdentity(record)

  if (type === 'command_started') {
    expectOnlyKeys(
      record,
      [
        'type',
        'runId',
        'conversationId',
        'assistantMessageId',
        'projectId',
        'callId',
        'sessionId',
        'startedAt'
      ] as const,
      'command_started event'
    )
    return {
      type,
      ...identity,
      startedAt: expectSafeInteger(record.startedAt, 'command_started event.startedAt', 0)
    }
  }

  if (type === 'command_output') {
    expectOnlyKeys(
      record,
      [
        'type',
        'runId',
        'conversationId',
        'assistantMessageId',
        'projectId',
        'callId',
        'sessionId',
        'sequence',
        'stream',
        'output'
      ] as const,
      'command_output event'
    )
    return {
      type,
      ...identity,
      sequence: expectSafeInteger(record.sequence, 'command_output event.sequence', 1),
      stream: expectEnum(
        record.stream,
        ['stdout', 'stderr'] as const,
        'command_output event.stream'
      ),
      output: boundedString(record.output, 'command_output event.output', MAX_EVENT_OUTPUT_BYTES)
    }
  }

  if (type === 'command_exited') {
    expectOnlyKeys(
      record,
      [
        'type',
        'runId',
        'conversationId',
        'assistantMessageId',
        'projectId',
        'callId',
        'sessionId',
        'status',
        'exitCode',
        'endedAt',
        'latestSequence',
        'outputTruncated'
      ] as const,
      'command_exited event'
    )
    const status = expectEnum(
      record.status,
      ['exited', 'timed_out', 'failed', 'outcome_unknown'] as const,
      'command_exited event.status'
    )
    const exitCode =
      record.exitCode === undefined
        ? undefined
        : expectI32(record.exitCode, 'command_exited event.exitCode')
    if (status !== 'exited' && exitCode !== undefined) {
      throw invalidProtocolValue('command_exited event', 'exitCode is valid only for exited status')
    }
    return {
      type,
      ...identity,
      status,
      ...(exitCode === undefined ? {} : { exitCode }),
      endedAt: expectSafeInteger(record.endedAt, 'command_exited event.endedAt', 0),
      latestSequence: expectSafeInteger(
        record.latestSequence,
        'command_exited event.latestSequence',
        0
      ),
      outputTruncated: expectBoolean(record.outputTruncated, 'command_exited event.outputTruncated')
    }
  }

  expectOnlyKeys(
    record,
    [
      'type',
      'runId',
      'conversationId',
      'assistantMessageId',
      'projectId',
      'callId',
      'sessionId',
      'endedAt',
      'latestSequence',
      'outputTruncated'
    ] as const,
    'command_interrupted event'
  )
  return {
    type,
    ...identity,
    endedAt: expectSafeInteger(record.endedAt, 'command_interrupted event.endedAt', 0),
    latestSequence: expectSafeInteger(
      record.latestSequence,
      'command_interrupted event.latestSequence',
      0
    ),
    outputTruncated: expectBoolean(
      record.outputTruncated,
      'command_interrupted event.outputTruncated'
    )
  }
}

export function parseAgentCommandSessionListInput(value: unknown): AgentCommandSessionListInput {
  const context = 'Agent command Session list input'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['conversationId'] as const, context)
  return { conversationId: boundedId(record.conversationId, `${context}.conversationId`) }
}

export function parseAgentCommandSessionGetInput(value: unknown): AgentCommandSessionGetInput {
  const context = 'Agent command Session get input'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['conversationId', 'sessionId', 'afterSequence', 'maxBytes'] as const,
    context
  )
  return {
    conversationId: boundedId(record.conversationId, `${context}.conversationId`),
    sessionId: commandSessionId(record.sessionId, `${context}.sessionId`),
    ...(record.afterSequence === undefined
      ? {}
      : {
          afterSequence: expectSafeInteger(record.afterSequence, `${context}.afterSequence`, 0)
        }),
    ...(record.maxBytes === undefined
      ? {}
      : {
          maxBytes: boundedInteger(record.maxBytes, `${context}.maxBytes`, 1, MAX_TRANSCRIPT_BYTES)
        })
  }
}

export function parseAgentCommandSessionListOutput(value: unknown): AgentCommandSessionListOutput {
  const context = 'Agent command Session list output'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['sessions'] as const, context)
  const sessions = expectArray(record.sessions, `${context}.sessions`)
  if (sessions.length > MAX_LISTED_SESSIONS) {
    throw invalidProtocolValue(context, `sessions exceeded ${MAX_LISTED_SESSIONS} items`)
  }
  return { sessions: sessions.map((session) => parseAgentCommandSessionSnapshot(session)) }
}

export function parseAgentCommandSessionGetOutput(value: unknown): AgentCommandSessionGetOutput {
  const context = 'Agent command Session get output'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['session', 'transcript'] as const, context)
  const session = parseAgentCommandSessionSnapshot(record.session)
  const transcript = parseAgentCommandSessionTranscript(record.transcript)
  if (transcript.latestSequence !== session.latestSequence) {
    throw invalidProtocolValue(context, 'Session and transcript latestSequence must match')
  }
  return { session, transcript }
}

export function parseAgentCommandSessionSnapshot(value: unknown): AgentCommandSessionSnapshot {
  const context = 'Agent command Session snapshot'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'sessionId',
      'conversationId',
      'assistantMessageId',
      'originRunId',
      'callId',
      'projectId',
      'command',
      'cwd',
      'commandDigest',
      'status',
      'startedAt',
      'endedAt',
      'exitCode',
      'latestSequence',
      'outputTruncated',
      'archiveRef'
    ] as const,
    context
  )
  if (record.schemaVersion !== AGENT_COMMAND_SESSION_SCHEMA_VERSION) {
    throw invalidProtocolValue(
      context,
      `unsupported schema version ${String(record.schemaVersion)}`
    )
  }
  const status = expectEnum(record.status, SESSION_STATUSES, `${context}.status`)
  const startedAt = expectSafeInteger(record.startedAt, `${context}.startedAt`, 0)
  const endedAt = optionalSafeInteger(record.endedAt, `${context}.endedAt`)
  const exitCode =
    record.exitCode === undefined ? undefined : expectI32(record.exitCode, `${context}.exitCode`)
  if (statusIsTerminal(status) !== (endedAt !== undefined)) {
    throw invalidProtocolValue(context, 'endedAt presence must match terminal status')
  }
  if (endedAt !== undefined && endedAt < startedAt) {
    throw invalidProtocolValue(context, 'endedAt must not precede startedAt')
  }
  if (status !== 'exited' && exitCode !== undefined) {
    throw invalidProtocolValue(context, 'exitCode is valid only for exited status')
  }

  return {
    schemaVersion: AGENT_COMMAND_SESSION_SCHEMA_VERSION,
    sessionId: commandSessionId(record.sessionId, `${context}.sessionId`),
    conversationId: boundedId(record.conversationId, `${context}.conversationId`),
    assistantMessageId: boundedId(record.assistantMessageId, `${context}.assistantMessageId`),
    originRunId: boundedId(record.originRunId, `${context}.originRunId`),
    callId: boundedId(record.callId, `${context}.callId`),
    ...(record.projectId === undefined
      ? {}
      : { projectId: boundedId(record.projectId, `${context}.projectId`) }),
    command: boundedString(record.command, `${context}.command`, MAX_COMMAND_BYTES),
    cwd: boundedString(record.cwd, `${context}.cwd`, MAX_CWD_BYTES),
    commandDigest: commandDigest(record.commandDigest, `${context}.commandDigest`),
    status,
    startedAt,
    ...(endedAt === undefined ? {} : { endedAt }),
    ...(exitCode === undefined ? {} : { exitCode }),
    latestSequence: expectSafeInteger(record.latestSequence, `${context}.latestSequence`, 0),
    outputTruncated: expectBoolean(record.outputTruncated, `${context}.outputTruncated`),
    ...(record.archiveRef === undefined
      ? {}
      : { archiveRef: boundedId(record.archiveRef, `${context}.archiveRef`) })
  }
}

export function parseAgentCommandSessionTranscript(value: unknown): AgentCommandSessionTranscript {
  const context = 'Agent command Session transcript'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'requestedAfterSequence',
      'firstAvailableSequence',
      'latestSequence',
      'truncatedBefore',
      'outputCaptureTruncated',
      'chunks'
    ] as const,
    context
  )
  const requestedAfterSequence = expectSafeInteger(
    record.requestedAfterSequence,
    `${context}.requestedAfterSequence`,
    0
  )
  const latestSequence = expectSafeInteger(record.latestSequence, `${context}.latestSequence`, 0)
  const firstAvailableSequence = optionalSafeInteger(
    record.firstAvailableSequence,
    `${context}.firstAvailableSequence`
  )
  const rawChunks = expectArray(record.chunks, `${context}.chunks`)
  if (rawChunks.length > MAX_TRANSCRIPT_CHUNKS) {
    throw invalidProtocolValue(context, `chunks exceeded ${MAX_TRANSCRIPT_CHUNKS} items`)
  }
  const chunks = rawChunks.map((chunk) => parseOutputChunk(chunk))
  let outputBytes = 0
  for (let index = 0; index < chunks.length; index += 1) {
    const chunk = chunks[index]
    outputBytes += new TextEncoder().encode(chunk.output).byteLength
    if (chunk.sequence <= requestedAfterSequence || chunk.sequence > latestSequence) {
      throw invalidProtocolValue(context, 'chunk sequence lies outside the requested range')
    }
    if (index > 0 && chunks[index - 1].sequence >= chunk.sequence) {
      throw invalidProtocolValue(context, 'chunk sequences must be strictly increasing')
    }
  }
  if (outputBytes > MAX_TRANSCRIPT_BYTES) {
    throw invalidProtocolValue(context, `output exceeded ${MAX_TRANSCRIPT_BYTES} UTF-8 bytes`)
  }
  if (firstAvailableSequence !== undefined && firstAvailableSequence > latestSequence) {
    throw invalidProtocolValue(context, 'firstAvailableSequence exceeds latestSequence')
  }
  return {
    requestedAfterSequence,
    ...(firstAvailableSequence === undefined ? {} : { firstAvailableSequence }),
    latestSequence,
    truncatedBefore: expectBoolean(record.truncatedBefore, `${context}.truncatedBefore`),
    outputCaptureTruncated: expectBoolean(
      record.outputCaptureTruncated,
      `${context}.outputCaptureTruncated`
    ),
    chunks
  }
}

function parseEventIdentity(record: Record<string, unknown>) {
  return {
    runId: boundedId(record.runId, 'Agent command Session event.runId'),
    conversationId: boundedId(record.conversationId, 'Agent command Session event.conversationId'),
    assistantMessageId: boundedId(
      record.assistantMessageId,
      'Agent command Session event.assistantMessageId'
    ),
    ...(record.projectId === undefined
      ? {}
      : {
          projectId: boundedId(record.projectId, 'Agent command Session event.projectId')
        }),
    callId: boundedId(record.callId, 'Agent command Session event.callId'),
    sessionId: commandSessionId(record.sessionId, 'Agent command Session event.sessionId')
  }
}

function parseOutputChunk(value: unknown): AgentCommandSessionOutputChunk {
  const context = 'Agent command Session output chunk'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['sequence', 'stream', 'output'] as const, context)
  return {
    sequence: expectSafeInteger(record.sequence, `${context}.sequence`, 1),
    stream: expectEnum(record.stream, ['stdout', 'stderr'] as const, `${context}.stream`),
    output: boundedString(record.output, `${context}.output`, MAX_EVENT_OUTPUT_BYTES)
  }
}

function statusIsTerminal(status: AgentCommandSessionStatus): boolean {
  return status !== 'starting' && status !== 'running'
}

function boundedId(value: unknown, context: string): string {
  const parsed = optionalNonEmptyString(value, context)
  if (parsed === undefined) throw invalidProtocolValue(context, 'expected a non-empty string')
  return boundedString(parsed, context, MAX_ID_BYTES)
}

function commandSessionId(value: unknown, context: string): string {
  const parsed = boundedId(value, context)
  if (!COMMAND_SESSION_ID_PATTERN.test(parsed)) {
    throw invalidProtocolValue(context, 'expected a canonical managed command Session id')
  }
  return parsed
}

function commandDigest(value: unknown, context: string): string {
  const parsed = boundedId(value, context)
  if (!COMMAND_DIGEST_PATTERN.test(parsed)) {
    throw invalidProtocolValue(context, 'expected a SHA-256 command digest')
  }
  return parsed
}

function boundedString(value: unknown, context: string, maxBytes: number): string {
  const parsed = expectString(value, context)
  if (new TextEncoder().encode(parsed).byteLength > maxBytes) {
    throw invalidProtocolValue(context, `exceeded ${maxBytes} UTF-8 bytes`)
  }
  return parsed
}

function optionalSafeInteger(value: unknown, context: string): number | undefined {
  return value === undefined ? undefined : expectSafeInteger(value, context, 0)
}

function boundedInteger(value: unknown, context: string, minimum: number, maximum: number): number {
  const parsed = expectSafeInteger(value, context, minimum)
  if (parsed > maximum) throw invalidProtocolValue(context, `exceeded maximum ${maximum}`)
  return parsed
}

function expectI32(value: unknown, context: string): number {
  if (
    typeof value !== 'number' ||
    !Number.isSafeInteger(value) ||
    value < -2_147_483_648 ||
    value > 2_147_483_647
  ) {
    throw invalidProtocolValue(context, 'expected a signed 32-bit integer')
  }
  return value
}
