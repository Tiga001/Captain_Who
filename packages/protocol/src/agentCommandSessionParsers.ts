import type {
  AgentCommandArtifactChange,
  AgentCommandArtifactMetadata,
  AgentCommandArtifactObservation,
  AgentCommandArtifactObservationCoverage,
  AgentCommandArtifactSnapshotCoverage,
  AgentCommandArtifactValidation,
  AgentCommandArtifactObservationWarning,
  AgentCommandExpectedArtifactOutcome,
  AgentCommandSessionGetInput,
  AgentCommandSessionGetOutput,
  AgentCommandSessionListInput,
  AgentCommandSessionListOutput,
  AgentCommandSessionOutputChunk,
  AgentCommandPublishedOutput,
  AgentCommandSessionSnapshot,
  AgentCommandSessionStatus,
  AgentCommandSessionTranscript,
  AgentEvent
} from './agent'
import {
  AGENT_COMMAND_ARTIFACT_OBSERVATION_SCHEMA_VERSION,
  AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS,
  AGENT_COMMAND_SESSION_SCHEMA_VERSION
} from './agent'
import {
  expectArray,
  expectBoolean,
  expectEnum,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  expectString,
  hasAsciiControlCharacter,
  invalidProtocolValue,
  optionalNonEmptyString
} from './skills/validation'

const COMMAND_SESSION_ID_PATTERN = /^cmd_[0-9a-f]{32}$/
const COMMAND_DIGEST_PATTERN = /^sha256:[0-9a-f]{64}$/
const MAX_ID_BYTES = 512
// Rust accepts at most 16,000 Unicode scalar values. Four UTF-8 bytes per scalar is the exact
// cross-process upper bound; keep Session list/get and restart recovery lossless at that boundary.
const MAX_COMMAND_BYTES = 64 * 1024
const MAX_CWD_BYTES = 16 * 1024
const MAX_EVENT_OUTPUT_BYTES = 64 * 1024
const MAX_TRANSCRIPT_BYTES = 1024 * 1024
const MAX_LISTED_SESSIONS = 512
const MAX_PUBLISHED_OUTPUTS = 32
const MAX_PUBLISHED_OUTPUT_NAME_BYTES = 1024
const MAX_PUBLISHED_OUTPUT_BYTES = 128 * 1024 * 1024
const MAX_ARTIFACT_CHANGES = 256
const MAX_ARTIFACT_EXPECTED_OUTPUTS = 32
const MAX_ARTIFACT_WARNINGS = 64
const MAX_ARTIFACT_PATH_BYTES = 16 * 1024
const MAX_ARTIFACT_TEXT_BYTES = 16 * 1024
const SHA256_PATTERN = /^[0-9a-f]{64}$/u

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
        'outputTruncated',
        'outputs',
        'artifactObservation'
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
      outputTruncated: expectBoolean(
        record.outputTruncated,
        'command_exited event.outputTruncated'
      ),
      ...(record.outputs === undefined
        ? {}
        : { outputs: parsePublishedOutputs(record.outputs, 'command_exited event.outputs') }),
      ...(record.artifactObservation === undefined
        ? {}
        : {
            artifactObservation: parseAgentCommandArtifactObservation(
              record.artifactObservation,
              'command_exited event.artifactObservation'
            )
          })
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
      'outputTruncated',
      'outputs',
      'artifactObservation'
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
    ),
    ...(record.outputs === undefined
      ? {}
      : { outputs: parsePublishedOutputs(record.outputs, 'command_interrupted event.outputs') }),
    ...(record.artifactObservation === undefined
      ? {}
      : {
          artifactObservation: parseAgentCommandArtifactObservation(
            record.artifactObservation,
            'command_interrupted event.artifactObservation'
          )
        })
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
      'outputs',
      'artifactObservation',
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
  const outputs =
    record.outputs === undefined
      ? undefined
      : parsePublishedOutputs(record.outputs, `${context}.outputs`)
  if (!statusIsTerminal(status) && outputs && outputs.length > 0) {
    throw invalidProtocolValue(context, 'published outputs are valid only for terminal status')
  }
  const artifactObservation =
    record.artifactObservation === undefined
      ? undefined
      : parseAgentCommandArtifactObservation(
          record.artifactObservation,
          `${context}.artifactObservation`
        )
  if (!statusIsTerminal(status) && artifactObservation !== undefined) {
    throw invalidProtocolValue(context, 'artifact observation is valid only for terminal status')
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
    ...(outputs === undefined ? {} : { outputs }),
    ...(artifactObservation === undefined ? {} : { artifactObservation }),
    ...(record.archiveRef === undefined
      ? {}
      : { archiveRef: boundedId(record.archiveRef, `${context}.archiveRef`) })
  }
}

/** Strictly parses the bounded artifact receipt shared by terminal command results and Sessions. */
export function parseAgentCommandArtifactObservation(
  value: unknown,
  context = 'Agent command artifact observation'
): AgentCommandArtifactObservation {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'status',
      'partial',
      'stopReasons',
      'scanned',
      'returned',
      'omitted',
      'coverage',
      'changes',
      'changesTruncated',
      'changesOmitted',
      'expectedOutputs',
      'warnings'
    ] as const,
    context
  )
  if (record.schemaVersion !== AGENT_COMMAND_ARTIFACT_OBSERVATION_SCHEMA_VERSION) {
    throw invalidProtocolValue(
      context,
      `unsupported schema version ${String(record.schemaVersion)}`
    )
  }
  const status = expectEnum(
    record.status,
    ['complete', 'partial', 'failed'] as const,
    `${context}.status`
  )
  const partial = expectBoolean(record.partial, `${context}.partial`)
  if ((status === 'complete') === partial) {
    throw invalidProtocolValue(context, 'partial must be false only for complete observations')
  }
  const stopReasons = parseBoundedStrings(
    record.stopReasons,
    `${context}.stopReasons`,
    MAX_ARTIFACT_WARNINGS
  )
  const rawChanges = boundedArray(record.changes, `${context}.changes`, MAX_ARTIFACT_CHANGES)
  const changes = rawChanges.map((change, index) =>
    parseArtifactChange(change, `${context}.changes[${index}]`)
  )
  const rawExpectedOutputs = boundedArray(
    record.expectedOutputs,
    `${context}.expectedOutputs`,
    MAX_ARTIFACT_EXPECTED_OUTPUTS
  )
  const expectedOutputs = rawExpectedOutputs.map((output, index) =>
    parseExpectedArtifactOutcome(output, `${context}.expectedOutputs[${index}]`)
  )
  const rawWarnings = boundedArray(record.warnings, `${context}.warnings`, MAX_ARTIFACT_WARNINGS)
  const warnings = rawWarnings.map((warning, index) =>
    parseArtifactWarning(warning, `${context}.warnings[${index}]`)
  )
  const returned = expectSafeInteger(record.returned, `${context}.returned`, 0)
  if (returned !== changes.length) {
    throw invalidProtocolValue(context, 'returned must equal the number of reported changes')
  }
  return {
    schemaVersion: AGENT_COMMAND_ARTIFACT_OBSERVATION_SCHEMA_VERSION,
    status,
    partial,
    stopReasons,
    scanned: expectSafeInteger(record.scanned, `${context}.scanned`, 0),
    returned,
    omitted: expectSafeInteger(record.omitted, `${context}.omitted`, 0),
    coverage: parseArtifactCoverage(record.coverage, `${context}.coverage`),
    changes,
    changesTruncated: expectBoolean(record.changesTruncated, `${context}.changesTruncated`),
    changesOmitted: expectSafeInteger(record.changesOmitted, `${context}.changesOmitted`, 0),
    expectedOutputs,
    warnings
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
  if (rawChunks.length > AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS) {
    throw invalidProtocolValue(
      context,
      `chunks exceeded ${AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS} items`
    )
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

function parseArtifactCoverage(
  value: unknown,
  context: string
): AgentCommandArtifactObservationCoverage {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['workspaceIncluded', 'expectedOutputCount', 'additionalRootCount', 'before', 'after'] as const,
    context
  )
  return {
    workspaceIncluded: expectBoolean(record.workspaceIncluded, `${context}.workspaceIncluded`),
    expectedOutputCount: expectSafeInteger(
      record.expectedOutputCount,
      `${context}.expectedOutputCount`,
      0
    ),
    additionalRootCount: expectSafeInteger(
      record.additionalRootCount,
      `${context}.additionalRootCount`,
      0
    ),
    before: parseArtifactSnapshotCoverage(record.before, `${context}.before`),
    after: parseArtifactSnapshotCoverage(record.after, `${context}.after`)
  }
}

function parseArtifactSnapshotCoverage(
  value: unknown,
  context: string
): AgentCommandArtifactSnapshotCoverage {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'rootsScanned',
      'directoryEntriesScanned',
      'officeFilesSeen',
      'filesHashed',
      'filesUnhashed',
      'bytesHashed',
      'symlinksSkipped',
      'excludedDirectories',
      'durationMs',
      'timeBudgetExceeded',
      'cancelled',
      'truncated'
    ] as const,
    context
  )
  return {
    rootsScanned: expectSafeInteger(record.rootsScanned, `${context}.rootsScanned`, 0),
    directoryEntriesScanned: expectSafeInteger(
      record.directoryEntriesScanned,
      `${context}.directoryEntriesScanned`,
      0
    ),
    officeFilesSeen: expectSafeInteger(record.officeFilesSeen, `${context}.officeFilesSeen`, 0),
    filesHashed: expectSafeInteger(record.filesHashed, `${context}.filesHashed`, 0),
    filesUnhashed: expectSafeInteger(record.filesUnhashed, `${context}.filesUnhashed`, 0),
    bytesHashed: expectSafeInteger(record.bytesHashed, `${context}.bytesHashed`, 0),
    symlinksSkipped: expectSafeInteger(record.symlinksSkipped, `${context}.symlinksSkipped`, 0),
    excludedDirectories: expectSafeInteger(
      record.excludedDirectories,
      `${context}.excludedDirectories`,
      0
    ),
    durationMs: expectSafeInteger(record.durationMs, `${context}.durationMs`, 0),
    timeBudgetExceeded: expectBoolean(record.timeBudgetExceeded, `${context}.timeBudgetExceeded`),
    cancelled: expectBoolean(record.cancelled, `${context}.cancelled`),
    truncated: expectBoolean(record.truncated, `${context}.truncated`)
  }
}

function parseArtifactChange(value: unknown, context: string): AgentCommandArtifactChange {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'kind',
      'artifactKind',
      'path',
      'scope',
      'previousPath',
      'previousScope',
      'before',
      'after'
    ] as const,
    context
  )
  const previousPath = optionalArtifactPath(record.previousPath, `${context}.previousPath`)
  const previousScope =
    record.previousScope === undefined
      ? undefined
      : expectEnum(
          record.previousScope,
          ['workspace', 'external'] as const,
          `${context}.previousScope`
        )
  if ((previousPath === undefined) !== (previousScope === undefined)) {
    throw invalidProtocolValue(context, 'previousPath and previousScope must appear together')
  }
  const before = optionalArtifactMetadata(record.before, `${context}.before`)
  const after = optionalArtifactMetadata(record.after, `${context}.after`)
  return {
    kind: expectEnum(
      record.kind,
      ['created', 'modified', 'replaced', 'deleted', 'renamed'] as const,
      `${context}.kind`
    ),
    artifactKind: expectEnum(
      record.artifactKind,
      ['document', 'spreadsheet', 'presentation'] as const,
      `${context}.artifactKind`
    ),
    path: artifactPath(record.path, `${context}.path`),
    scope: expectEnum(record.scope, ['workspace', 'external'] as const, `${context}.scope`),
    ...(previousPath === undefined ? {} : { previousPath }),
    ...(previousScope === undefined ? {} : { previousScope }),
    ...(before === undefined ? {} : { before }),
    ...(after === undefined ? {} : { after })
  }
}

function parseExpectedArtifactOutcome(
  value: unknown,
  context: string
): AgentCommandExpectedArtifactOutcome {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['requestedPath', 'outcome', 'path', 'scope', 'artifactKind', 'metadata'] as const,
    context
  )
  const path = optionalArtifactPath(record.path, `${context}.path`)
  const scope =
    record.scope === undefined
      ? undefined
      : expectEnum(record.scope, ['workspace', 'external'] as const, `${context}.scope`)
  const artifactKind =
    record.artifactKind === undefined
      ? undefined
      : expectEnum(
          record.artifactKind,
          ['document', 'spreadsheet', 'presentation'] as const,
          `${context}.artifactKind`
        )
  const metadata = optionalArtifactMetadata(record.metadata, `${context}.metadata`)
  return {
    requestedPath: artifactPath(record.requestedPath, `${context}.requestedPath`),
    outcome: expectEnum(
      record.outcome,
      [
        'created',
        'modified',
        'replaced',
        'renamed',
        'unchanged',
        'missing',
        'unobserved',
        'invalid'
      ] as const,
      `${context}.outcome`
    ),
    ...(path === undefined ? {} : { path }),
    ...(scope === undefined ? {} : { scope }),
    ...(artifactKind === undefined ? {} : { artifactKind }),
    ...(metadata === undefined ? {} : { metadata })
  }
}

function optionalArtifactMetadata(
  value: unknown,
  context: string
): AgentCommandArtifactMetadata | undefined {
  return value === undefined ? undefined : parseArtifactMetadata(value, context)
}

function parseArtifactMetadata(value: unknown, context: string): AgentCommandArtifactMetadata {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['sizeBytes', 'sha256', 'validation'] as const, context)
  const sha256 =
    record.sha256 === undefined ? undefined : boundedString(record.sha256, `${context}.sha256`, 64)
  if (sha256 !== undefined && !SHA256_PATTERN.test(sha256)) {
    throw invalidProtocolValue(context, 'expected a lowercase SHA-256 digest')
  }
  return {
    sizeBytes: expectSafeInteger(record.sizeBytes, `${context}.sizeBytes`, 0),
    ...(sha256 === undefined ? {} : { sha256 }),
    validation: parseArtifactValidation(record.validation, `${context}.validation`)
  }
}

function parseArtifactValidation(value: unknown, context: string): AgentCommandArtifactValidation {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['status', 'code', 'message'] as const, context)
  const code = optionalArtifactText(record.code, `${context}.code`)
  const message = optionalArtifactText(record.message, `${context}.message`)
  return {
    status: expectEnum(
      record.status,
      ['valid', 'invalid', 'not_applicable', 'unchecked'] as const,
      `${context}.status`
    ),
    ...(code === undefined ? {} : { code }),
    ...(message === undefined ? {} : { message })
  }
}

function parseArtifactWarning(
  value: unknown,
  context: string
): AgentCommandArtifactObservationWarning {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['phase', 'code', 'path', 'message'] as const, context)
  const path = optionalArtifactPath(record.path, `${context}.path`)
  return {
    phase: expectEnum(record.phase, ['setup', 'before', 'after'] as const, `${context}.phase`),
    code: requiredArtifactText(record.code, `${context}.code`),
    ...(path === undefined ? {} : { path }),
    message: requiredArtifactText(record.message, `${context}.message`)
  }
}

function boundedArray(value: unknown, context: string, maximum: number): unknown[] {
  const values = expectArray(value, context)
  if (values.length > maximum) {
    throw invalidProtocolValue(context, `exceeded ${maximum} items`)
  }
  return values
}

function parseBoundedStrings(value: unknown, context: string, maximum: number): string[] {
  return boundedArray(value, context, maximum).map((entry, index) =>
    requiredArtifactText(entry, `${context}[${index}]`)
  )
}

function artifactPath(value: unknown, context: string): string {
  const path = boundedString(value, context, MAX_ARTIFACT_PATH_BYTES)
  if (!path.trim() || hasAsciiControlCharacter(path)) {
    throw invalidProtocolValue(context, 'expected a non-empty path without control characters')
  }
  return path
}

function optionalArtifactPath(value: unknown, context: string): string | undefined {
  return value === undefined ? undefined : artifactPath(value, context)
}

function requiredArtifactText(value: unknown, context: string): string {
  const text = boundedString(value, context, MAX_ARTIFACT_TEXT_BYTES)
  if (!text.trim() || hasAsciiControlCharacter(text)) {
    throw invalidProtocolValue(context, 'expected non-empty text without control characters')
  }
  return text
}

function optionalArtifactText(value: unknown, context: string): string | undefined {
  return value === undefined ? undefined : requiredArtifactText(value, context)
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

function parsePublishedOutputs(value: unknown, context: string): AgentCommandPublishedOutput[] {
  const values = expectArray(value, context)
  if (values.length > MAX_PUBLISHED_OUTPUTS) {
    throw invalidProtocolValue(context, `outputs exceeded ${MAX_PUBLISHED_OUTPUTS} items`)
  }
  return values.map((output, index) => parsePublishedOutput(output, `${context}[${index}]`))
}

function parsePublishedOutput(value: unknown, context: string): AgentCommandPublishedOutput {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['name', 'kind', 'readPath', 'mimeType', 'sizeBytes', 'sha256', 'width', 'height'] as const,
    context
  )
  const name = boundedString(record.name, `${context}.name`, MAX_PUBLISHED_OUTPUT_NAME_BYTES)
  if (!name.trim() || hasAsciiControlCharacter(name)) {
    throw invalidProtocolValue(context, 'name must be non-empty and contain no control characters')
  }
  const kind = expectEnum(record.kind, ['image', 'document'] as const, `${context}.kind`)
  const sha256 = boundedString(record.sha256, `${context}.sha256`, 64)
  if (!SHA256_PATTERN.test(sha256)) {
    throw invalidProtocolValue(context, 'expected a lowercase SHA-256 digest')
  }
  const readPath = boundedString(record.readPath, `${context}.readPath`, 128)
  const expectedReadPath =
    kind === 'image' ? `image-artifact://sha256/${sha256}` : `artifact://sha256/${sha256}`
  if (readPath !== expectedReadPath) {
    throw invalidProtocolValue(context, 'readPath does not match the published content identity')
  }
  const mimeType = boundedString(record.mimeType, `${context}.mimeType`, 128)
  const sizeBytes = boundedInteger(
    record.sizeBytes,
    `${context}.sizeBytes`,
    1,
    MAX_PUBLISHED_OUTPUT_BYTES
  )
  if (kind === 'document') {
    if (
      mimeType !== 'application/pdf' ||
      record.width !== undefined ||
      record.height !== undefined
    ) {
      throw invalidProtocolValue(context, 'document output must be a PDF without image dimensions')
    }
    return { name, kind, readPath, mimeType, sizeBytes, sha256 }
  }

  if (!['image/png', 'image/jpeg', 'image/webp'].includes(mimeType)) {
    throw invalidProtocolValue(context, 'image output has an unsupported MIME type')
  }
  const width = expectSafeInteger(record.width, `${context}.width`, 1)
  const height = expectSafeInteger(record.height, `${context}.height`, 1)
  return { name, kind, readPath, mimeType, sizeBytes, sha256, width, height }
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
