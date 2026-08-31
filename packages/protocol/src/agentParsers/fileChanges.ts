import type {
  AgentFileChangePreview,
  AgentFileChangeContentPage,
  AgentFileChangeDiffPage,
  AgentFileChangeHistoryDiffPage,
  AgentFileChangeProposal,
  AgentFileChangeResult,
  AgentFileChangeSnapshot
} from '../agent'
import {
  expectBoolean,
  expectEnum,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  invalidProtocolValue
} from '../skills/validation'
import { parseAgentApprovalStatus } from './actionPayloads'
import {
  MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES,
  MAX_RENDERER_SAFE_AGENT_EVENT_BYTES,
  expectBoundedNonEmptyString,
  expectBoundedString,
  expectFileChangeSchemaVersion,
  expectOpaqueRunId,
  expectSafeCode
} from './shared'
export function parseAgentFileChangePreview(
  value: unknown,
  context: string
): AgentFileChangePreview {
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    [
      'schemaVersion',
      'previewId',
      'streamId',
      'attempt',
      'toolCallIndex',
      'toolCallId',
      'transactionId',
      'filePath',
      'additions',
      'deletions',
      'lineCount',
      'byteCount',
      'generatedBytes',
      'contentOffsetBytes',
      'contentDelta',
      'updatedAt'
    ] as const,
    context
  )
  const attempt = expectSafeInteger(item.attempt, `${context}.attempt`, 1)
  const generatedBytes = expectSafeInteger(item.generatedBytes, `${context}.generatedBytes`, 0)
  const contentOffsetBytes = expectSafeInteger(
    item.contentOffsetBytes,
    `${context}.contentOffsetBytes`,
    0
  )
  const contentDelta = expectBoundedString(
    item.contentDelta,
    `${context}.contentDelta`,
    MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES
  )
  if (contentOffsetBytes + new TextEncoder().encode(contentDelta).byteLength > generatedBytes) {
    throw invalidProtocolValue(context, 'content byte cursor exceeds generatedBytes')
  }
  return {
    schemaVersion: expectFileChangeSchemaVersion(item.schemaVersion, `${context}.schemaVersion`),
    previewId: expectOpaqueRunId(item.previewId, `${context}.previewId`),
    streamId: expectOpaqueRunId(item.streamId, `${context}.streamId`),
    attempt,
    toolCallIndex: expectSafeInteger(item.toolCallIndex, `${context}.toolCallIndex`, 0),
    toolCallId:
      item.toolCallId === null
        ? null
        : expectBoundedNonEmptyString(item.toolCallId, `${context}.toolCallId`, 2048),
    transactionId: expectOpaqueRunId(item.transactionId, `${context}.transactionId`),
    filePath: expectBoundedNonEmptyString(item.filePath, `${context}.filePath`, 16 * 1024),
    additions: expectSafeInteger(item.additions, `${context}.additions`, 0),
    deletions: expectSafeInteger(item.deletions, `${context}.deletions`, 0),
    lineCount: expectSafeInteger(item.lineCount, `${context}.lineCount`, 0),
    byteCount: expectSafeInteger(item.byteCount, `${context}.byteCount`, 0),
    generatedBytes,
    contentOffsetBytes,
    contentDelta,
    updatedAt: expectSafeInteger(item.updatedAt, `${context}.updatedAt`, 0)
  }
}

export function parseAgentFileChangeSnapshot(
  value: unknown,
  context: string
): AgentFileChangeSnapshot {
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    [
      'schemaVersion',
      'transactionId',
      'conversationId',
      'projectId',
      'filePath',
      'operation',
      'updateStrategy',
      'status',
      'baseRevision',
      'additions',
      'deletions',
      'lineCount',
      'byteCount',
      'mutationCount',
      'nextMutationIndex',
      'statsFinal',
      'summary',
      'createdAt',
      'updatedAt'
    ] as const,
    context
  )
  const operation = expectEnum(
    item.operation,
    ['create', 'update', 'delete'] as const,
    `${context}.operation`
  )
  const updateStrategy =
    item.updateStrategy === null
      ? null
      : expectEnum(item.updateStrategy, ['modify', 'rewrite'] as const, `${context}.updateStrategy`)
  if ((operation === 'update') !== (updateStrategy !== null)) {
    throw invalidProtocolValue(context, 'operation/updateStrategy combination is invalid')
  }
  const baseRevision =
    item.baseRevision === null
      ? null
      : expectBoundedNonEmptyString(item.baseRevision, `${context}.baseRevision`, 2048)
  if ((operation === 'create') !== (baseRevision === null)) {
    throw invalidProtocolValue(context, 'operation/baseRevision combination is invalid')
  }
  const mutationCount = expectSafeInteger(item.mutationCount, `${context}.mutationCount`, 0)
  const nextMutationIndex = expectSafeInteger(
    item.nextMutationIndex,
    `${context}.nextMutationIndex`,
    0
  )
  if (nextMutationIndex !== mutationCount) {
    throw invalidProtocolValue(context, 'mutation cursor is inconsistent')
  }
  const createdAt = expectSafeInteger(item.createdAt, `${context}.createdAt`, 0)
  const updatedAt = expectSafeInteger(item.updatedAt, `${context}.updatedAt`, 0)
  if (updatedAt < createdAt) {
    throw invalidProtocolValue(context, 'updatedAt precedes createdAt')
  }
  const status = expectEnum(
    item.status,
    [
      'drafting',
      'ready',
      'waiting_approval',
      'applying',
      'applied',
      'already_applied',
      'rejected',
      'conflict',
      'failed',
      'outcome_unknown',
      'aborted',
      'expired'
    ] as const,
    `${context}.status`
  )
  const statsFinal = expectBoolean(item.statsFinal, `${context}.statsFinal`)
  if ((status === 'drafting' || status === 'ready') === statsFinal) {
    throw invalidProtocolValue(context, 'status/statsFinal combination is invalid')
  }
  const lineCount = expectSafeInteger(item.lineCount, `${context}.lineCount`, 0)
  const byteCount = expectSafeInteger(item.byteCount, `${context}.byteCount`, 0)
  if (operation === 'delete' && (lineCount !== 0 || byteCount !== 0)) {
    throw invalidProtocolValue(context, 'delete target statistics must be zero')
  }
  return {
    schemaVersion: expectFileChangeSchemaVersion(item.schemaVersion, `${context}.schemaVersion`),
    transactionId: expectOpaqueRunId(item.transactionId, `${context}.transactionId`),
    conversationId: expectOpaqueRunId(item.conversationId, `${context}.conversationId`),
    projectId:
      item.projectId === null ? null : expectOpaqueRunId(item.projectId, `${context}.projectId`),
    filePath: expectBoundedNonEmptyString(item.filePath, `${context}.filePath`, 16 * 1024),
    operation,
    updateStrategy,
    status,
    baseRevision,
    additions: expectSafeInteger(item.additions, `${context}.additions`, 0),
    deletions: expectSafeInteger(item.deletions, `${context}.deletions`, 0),
    lineCount,
    byteCount,
    mutationCount,
    nextMutationIndex,
    statsFinal,
    summary:
      item.summary === null
        ? null
        : expectBoundedString(item.summary, `${context}.summary`, 16 * 1024),
    createdAt,
    updatedAt
  }
}

export function parseAgentFileChangeProposal(
  value: unknown,
  context: string
): AgentFileChangeProposal {
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    [
      'schemaVersion',
      'id',
      'transactionId',
      'operation',
      'updateStrategy',
      'filePath',
      'inlineDiff',
      'baseRevision',
      'summary',
      'additions',
      'deletions',
      'lineCount',
      'byteCount',
      'approvalStatus'
    ] as const,
    context
  )
  const operation = expectEnum(
    item.operation,
    ['create', 'update', 'delete'] as const,
    `${context}.operation`
  )
  const updateStrategy =
    item.updateStrategy === null
      ? null
      : expectEnum(item.updateStrategy, ['modify', 'rewrite'] as const, `${context}.updateStrategy`)
  if (operation !== 'update' && updateStrategy !== null) {
    throw invalidProtocolValue(context, 'only update may carry updateStrategy')
  }
  const baseRevision =
    item.baseRevision === null
      ? null
      : expectBoundedNonEmptyString(item.baseRevision, `${context}.baseRevision`, 2048)
  if ((operation === 'create') !== (baseRevision === null)) {
    throw invalidProtocolValue(context, 'operation/baseRevision combination is invalid')
  }
  const inlineDiff =
    item.inlineDiff === null
      ? null
      : parseAuthoritativeInlineDiff(item.inlineDiff, `${context}.inlineDiff`)
  if (operation === 'delete' && inlineDiff === null) {
    throw invalidProtocolValue(context, 'delete requires its complete authoritative inline diff')
  }
  if (
    operation === 'update' &&
    ((inlineDiff === null && updateStrategy === null) ||
      (inlineDiff !== null && updateStrategy !== null))
  ) {
    throw invalidProtocolValue(
      context,
      'staged update requires updateStrategy while direct update forbids it'
    )
  }
  const lineCount = expectSafeInteger(item.lineCount, `${context}.lineCount`, 0)
  const byteCount = expectSafeInteger(item.byteCount, `${context}.byteCount`, 0)
  if (operation === 'delete' && (lineCount !== 0 || byteCount !== 0)) {
    throw invalidProtocolValue(context, 'delete target statistics must be zero')
  }
  const approvalStatus = parseAgentApprovalStatus(item.approvalStatus, `${context}.approvalStatus`)
  if (approvalStatus !== 'required' && approvalStatus !== 'approved') {
    throw invalidProtocolValue(context, 'FileChange approvalStatus is invalid')
  }
  return {
    schemaVersion: expectFileChangeSchemaVersion(item.schemaVersion, `${context}.schemaVersion`),
    id: expectOpaqueRunId(item.id, `${context}.id`),
    transactionId: expectOpaqueRunId(item.transactionId, `${context}.transactionId`),
    operation,
    updateStrategy,
    filePath: expectBoundedNonEmptyString(item.filePath, `${context}.filePath`, 16 * 1024),
    inlineDiff,
    baseRevision,
    summary:
      item.summary === null
        ? null
        : expectBoundedString(item.summary, `${context}.summary`, 16 * 1024),
    additions: expectSafeInteger(item.additions, `${context}.additions`, 0),
    deletions: expectSafeInteger(item.deletions, `${context}.deletions`, 0),
    lineCount,
    byteCount,
    approvalStatus
  }
}

export function parseAuthoritativeInlineDiff(value: unknown, context: string) {
  const item = expectRecord(value, context)
  expectOnlyKeys(item, ['patch', 'truncated'] as const, context)
  const truncated = expectBoolean(item.truncated, `${context}.truncated`)
  if (truncated) {
    throw invalidProtocolValue(context, 'authoritative inline diff cannot be truncated')
  }
  return {
    patch: expectBoundedString(
      item.patch,
      `${context}.patch`,
      MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES
    ),
    truncated
  }
}

export function parseAgentFileChangeResult(value: unknown, context: string): AgentFileChangeResult {
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    [
      'schemaVersion',
      'status',
      'outcome',
      'transactionId',
      'operation',
      'updateStrategy',
      'filePath',
      'additions',
      'deletions',
      'lineCount',
      'byteCount',
      'revision',
      'errorCode',
      'error',
      'message'
    ] as const,
    context
  )
  const status = expectEnum(
    item.status,
    [
      'applied',
      'already_applied',
      'failed',
      'conflict',
      'rejected',
      'outcome_unknown',
      'aborted',
      'expired'
    ] as const,
    `${context}.status`
  )
  const outcome = expectEnum(
    item.outcome,
    ['definitely_not_executed', 'applied', 'outcome_unknown'] as const,
    `${context}.outcome`
  )
  const operation = expectEnum(
    item.operation,
    ['create', 'update', 'delete'] as const,
    `${context}.operation`
  )
  const updateStrategy =
    item.updateStrategy === null
      ? null
      : expectEnum(item.updateStrategy, ['modify', 'rewrite'] as const, `${context}.updateStrategy`)
  const revision =
    item.revision === null
      ? null
      : expectBoundedNonEmptyString(item.revision, `${context}.revision`, 2048)
  const errorCode =
    item.errorCode === null ? null : expectSafeCode(item.errorCode, `${context}.errorCode`)
  const error =
    item.error === null ? null : expectBoundedString(item.error, `${context}.error`, 16 * 1024)
  const message =
    item.message === null
      ? null
      : expectBoundedString(item.message, `${context}.message`, 16 * 1024)
  const hasSafeDiagnostic =
    errorCode !== null ||
    (error !== null && error.trim().length > 0) ||
    (message !== null && message.trim().length > 0)
  const succeeded = status === 'applied' || status === 'already_applied'
  const unknown = status === 'outcome_unknown'
  const successRevisionValid = operation === 'delete' ? revision === null : revision !== null
  const lineCount = expectSafeInteger(item.lineCount, `${context}.lineCount`, 0)
  const byteCount = expectSafeInteger(item.byteCount, `${context}.byteCount`, 0)
  if (
    (operation !== 'update' && updateStrategy !== null) ||
    (operation === 'delete' && (lineCount !== 0 || byteCount !== 0)) ||
    (succeeded &&
      (outcome !== 'applied' || !successRevisionValid || errorCode !== null || error !== null)) ||
    (unknown && (outcome !== 'outcome_unknown' || revision !== null || errorCode === null)) ||
    (!succeeded && !unknown && (outcome !== 'definitely_not_executed' || revision !== null)) ||
    (!succeeded && !hasSafeDiagnostic)
  ) {
    throw invalidProtocolValue(context, 'FileChange result fields form an illegal terminal state')
  }
  return {
    schemaVersion: expectFileChangeSchemaVersion(item.schemaVersion, `${context}.schemaVersion`),
    status,
    outcome,
    transactionId: expectOpaqueRunId(item.transactionId, `${context}.transactionId`),
    operation,
    updateStrategy,
    filePath: expectBoundedNonEmptyString(item.filePath, `${context}.filePath`, 16 * 1024),
    additions: expectSafeInteger(item.additions, `${context}.additions`, 0),
    deletions: expectSafeInteger(item.deletions, `${context}.deletions`, 0),
    lineCount,
    byteCount,
    revision,
    errorCode,
    error,
    message
  }
}

/** Strict Renderer-safe parser for a terminal FileChange result projection. */
export function parseAgentFileChangeResultForHost(value: unknown): AgentFileChangeResult {
  return parseAgentFileChangeResult(value, 'Agent FileChange result')
}

export function parseAgentFileChangeContentPageForHost(value: unknown): AgentFileChangeContentPage {
  const context = 'Agent FileChange content page'
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    ['fileChange', 'content', 'offset', 'nextOffset', 'truncated'] as const,
    context
  )
  const pagination = parseFileChangePageCursor(item, context)
  return {
    fileChange: parseAgentFileChangeSnapshot(item.fileChange, `${context}.fileChange`),
    content: expectBoundedString(
      item.content,
      `${context}.content`,
      MAX_RENDERER_SAFE_AGENT_EVENT_BYTES
    ),
    ...pagination
  }
}

export function parseAgentFileChangeDiffPageForHost(value: unknown): AgentFileChangeDiffPage {
  const context = 'Agent FileChange Diff page'
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    ['transactionId', 'patch', 'offset', 'nextOffset', 'truncated'] as const,
    context
  )
  const pagination = parseFileChangePageCursor(item, context)
  return {
    transactionId: expectOpaqueRunId(item.transactionId, `${context}.transactionId`),
    patch: expectBoundedString(item.patch, `${context}.patch`, MAX_RENDERER_SAFE_AGENT_EVENT_BYTES),
    ...pagination
  }
}

export function parseAgentFileChangeHistoryDiffPageForHost(
  value: unknown
): AgentFileChangeHistoryDiffPage {
  const context = 'Agent FileChange history Diff page'
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    [
      'conversationId',
      'assistantMessageId',
      'runId',
      'toolCallId',
      'patch',
      'offset',
      'nextOffset',
      'truncated'
    ] as const,
    context
  )
  const pagination = parseFileChangePageCursor(item, context)
  return {
    conversationId: expectOpaqueRunId(item.conversationId, `${context}.conversationId`),
    assistantMessageId: expectOpaqueRunId(item.assistantMessageId, `${context}.assistantMessageId`),
    runId: expectOpaqueRunId(item.runId, `${context}.runId`),
    toolCallId: expectOpaqueRunId(item.toolCallId, `${context}.toolCallId`),
    patch: expectBoundedString(item.patch, `${context}.patch`, MAX_RENDERER_SAFE_AGENT_EVENT_BYTES),
    ...pagination
  }
}

export function parseFileChangePageCursor(
  item: Record<string, unknown>,
  context: string
): Pick<AgentFileChangeDiffPage, 'offset' | 'nextOffset' | 'truncated'> {
  const offset = expectSafeInteger(item.offset, `${context}.offset`, 0)
  const nextOffset =
    item.nextOffset === null ? null : expectSafeInteger(item.nextOffset, `${context}.nextOffset`, 0)
  const truncated = expectBoolean(item.truncated, `${context}.truncated`)
  if (truncated !== (nextOffset !== null) || (nextOffset !== null && nextOffset <= offset)) {
    throw invalidProtocolValue(context, 'pagination fields form an invalid page chain')
  }
  return { offset, nextOffset, truncated }
}
