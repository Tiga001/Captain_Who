import {
  MAX_STORED_RUN_ITEMS,
  hasExactKeys,
  hasOwn,
  isApprovalStatus,
  isBoundedString,
  isNullableBoundedString,
  isNullableSafeInteger,
  isOptionalBoundedString,
  isRecord,
  isRecordArray,
  isSafeInteger,
  isStringArray
} from './persistedAgentRunValidation'
import { isOfficeOperation } from './persistedAgentRunOfficeValidators'

export function isToolCall(record: Record<string, unknown>): boolean {
  return (
    hasExactKeys(record, ['id', 'tool', 'args', 'approvalStatus', 'reason']) &&
    isBoundedString(record.id, 1024) &&
    isBoundedString(record.tool, 1024) &&
    isApprovalStatus(record.approvalStatus) &&
    (record.reason === null || isBoundedString(record.reason, 4096, true))
  )
}

export function isToolResult(record: Record<string, unknown>): boolean {
  return (
    hasExactKeys(record, ['callId', 'tool', 'ok'], ['result', 'error']) &&
    isBoundedString(record.callId, 1024) &&
    isBoundedString(record.tool, 1024) &&
    typeof record.ok === 'boolean' &&
    isOptionalBoundedString(record, 'error', 128 * 1024, true)
  )
}

export function isDiff(record: Record<string, unknown>): boolean {
  return (
    hasExactKeys(record, [
      'id',
      'operation',
      'filePath',
      'patch',
      'baseRevision',
      'summary',
      'approvalStatus'
    ]) &&
    isBoundedString(record.id, 1024) &&
    (record.operation === 'create' ||
      record.operation === 'update' ||
      record.operation === 'delete') &&
    isBoundedString(record.filePath, 16 * 1024) &&
    isBoundedString(record.patch, 4 * 1024 * 1024, true) &&
    (record.baseRevision === null || isBoundedString(record.baseRevision, 1024)) &&
    (record.summary === null || isBoundedString(record.summary, 16 * 1024, true)) &&
    isApprovalStatus(record.approvalStatus)
  )
}

function isFileWriteProposal(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(value, [
      'id',
      'draftId',
      'mode',
      'filePath',
      'baseRevision',
      'summary',
      'additions',
      'deletions',
      'lineCount',
      'byteCount',
      'approvalStatus'
    ]) &&
    isBoundedString(value.id, 1024) &&
    isBoundedString(value.draftId, 1024) &&
    ['create', 'rewrite', 'modify', 'append', 'upsert'].includes(value.mode as string) &&
    isBoundedString(value.filePath, 16 * 1024) &&
    isNullableBoundedString(value.baseRevision, 1024) &&
    isNullableBoundedString(value.summary, 16 * 1024, true) &&
    isSafeInteger(value.additions) &&
    isSafeInteger(value.deletions) &&
    isSafeInteger(value.lineCount) &&
    isSafeInteger(value.byteCount) &&
    isApprovalStatus(value.approvalStatus)
  )
}

function isCommandObservationRequest(value: unknown): boolean {
  return (
    value === null ||
    (isRecord(value) &&
      hasExactKeys(value, ['kinds'], ['expectedOutputs', 'additionalRoots']) &&
      Array.isArray(value.kinds) &&
      value.kinds.length <= MAX_STORED_RUN_ITEMS &&
      value.kinds.every((kind) => kind === 'office') &&
      (!hasOwn(value, 'expectedOutputs') || isStringArray(value.expectedOutputs, 16 * 1024)) &&
      (!hasOwn(value, 'additionalRoots') || isStringArray(value.additionalRoots, 16 * 1024)))
  )
}

function isCommandAction(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(value, [
      'id',
      'command',
      'cwd',
      'timeoutMs',
      'approvalStatus',
      'riskLevel',
      'reason',
      'observe'
    ]) &&
    isBoundedString(value.id, 1024) &&
    isBoundedString(value.command, 4 * 1024 * 1024, true) &&
    isNullableBoundedString(value.cwd, 16 * 1024, true) &&
    isNullableSafeInteger(value.timeoutMs) &&
    isApprovalStatus(value.approvalStatus) &&
    (value.riskLevel === null ||
      ['read_only', 'writes_workspace', 'network', 'destructive', 'unknown'].includes(
        value.riskLevel as string
      )) &&
    isNullableBoundedString(value.reason, 16 * 1024, true) &&
    isCommandObservationRequest(value.observe)
  )
}

function isSkillMaterialization(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(value, [
      'id',
      'sourceUri',
      'sourcePrefix',
      'destination',
      'approvalStatus',
      'reason'
    ]) &&
    isBoundedString(value.id, 1024) &&
    isBoundedString(value.sourceUri, 16 * 1024) &&
    isNullableBoundedString(value.sourcePrefix, 16 * 1024, true) &&
    isBoundedString(value.destination, 16 * 1024) &&
    isApprovalStatus(value.approvalStatus) &&
    isNullableBoundedString(value.reason, 16 * 1024, true)
  )
}

function isSkillScriptRequirements(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(value, [], ['pythonDistributions', 'commands']) &&
    (!hasOwn(value, 'pythonDistributions') || isStringArray(value.pythonDistributions, 1024)) &&
    (!hasOwn(value, 'commands') || isStringArray(value.commands, 1024))
  )
}

function isSkillScriptPreflight(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(
      value,
      ['status', 'interpreter', 'runtimeFingerprint'],
      ['interpreterVersion', 'dependencies', 'errorCode', 'message']
    ) &&
    ['ready', 'missing_dependencies', 'unsupported', 'conflict'].includes(value.status as string) &&
    value.interpreter === 'python3' &&
    isBoundedString(value.runtimeFingerprint, 4096) &&
    isOptionalBoundedString(value, 'interpreterVersion', 1024) &&
    (!hasOwn(value, 'dependencies') ||
      isRecordArray(value.dependencies, (dependency) =>
        Boolean(
          hasExactKeys(dependency, ['kind', 'name', 'status'], ['version']) &&
          (dependency.kind === 'python_distribution' || dependency.kind === 'command') &&
          isBoundedString(dependency.name, 1024) &&
          (dependency.status === 'available' || dependency.status === 'missing') &&
          isOptionalBoundedString(dependency, 'version', 1024)
        )
      )) &&
    isOptionalBoundedString(value, 'errorCode', 1024) &&
    isOptionalBoundedString(value, 'message', 128 * 1024, true)
  )
}

function isSkillScript(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(value, [
      'id',
      'scriptUri',
      'skillId',
      'skillRevision',
      'resourcePath',
      'resourceDigest',
      'interpreter',
      'args',
      'requirements',
      'preflight',
      'timeoutMs',
      'approvalStatus',
      'reason'
    ]) &&
    isBoundedString(value.id, 1024) &&
    isBoundedString(value.scriptUri, 16 * 1024) &&
    isBoundedString(value.skillId, 1024) &&
    isBoundedString(value.skillRevision, 1024) &&
    isBoundedString(value.resourcePath, 16 * 1024) &&
    isBoundedString(value.resourceDigest, 1024) &&
    value.interpreter === 'python3' &&
    isStringArray(value.args, 64 * 1024) &&
    isSkillScriptRequirements(value.requirements) &&
    isSkillScriptPreflight(value.preflight) &&
    isNullableSafeInteger(value.timeoutMs) &&
    isApprovalStatus(value.approvalStatus) &&
    isNullableBoundedString(value.reason, 16 * 1024, true)
  )
}

export function isSkillInstallationRequest(value: unknown): boolean {
  if (
    !isRecord(value) ||
    !hasExactKeys(value, [
      'schemaVersion',
      'id',
      'installRef',
      'preview',
      'approvalStatus',
      'expiresAt'
    ]) ||
    value.schemaVersion !== 1 ||
    !isBoundedString(value.id, 1024) ||
    !isBoundedString(value.installRef, 4096) ||
    !isApprovalStatus(value.approvalStatus) ||
    !isSafeInteger(value.expiresAt) ||
    !isRecord(value.preview)
  ) {
    return false
  }
  const preview = value.preview
  return (
    hasExactKeys(preview, [
      'name',
      'description',
      'sourceSummary',
      'resolvedRevision',
      'fileCount',
      'totalBytes',
      'resourceSummary',
      'containsScripts',
      'warnings',
      'compatibility',
      'operation',
      'impact'
    ]) &&
    isBoundedString(preview.name, 1024) &&
    isBoundedString(preview.description, 128 * 1024, true) &&
    isBoundedString(preview.resolvedRevision, 4096) &&
    isSafeInteger(preview.fileCount) &&
    isSafeInteger(preview.totalBytes) &&
    isRecord(preview.resourceSummary) &&
    hasExactKeys(preview.resourceSummary, ['total', 'references', 'assets', 'scripts', 'bytes']) &&
    Object.values(preview.resourceSummary).every((entry) => isSafeInteger(entry)) &&
    typeof preview.containsScripts === 'boolean' &&
    isRecordArray(preview.warnings, (warning) =>
      Boolean(
        hasExactKeys(warning, ['code', 'message', 'requiresAcknowledgement']) &&
        isBoundedString(warning.code, 1024) &&
        isBoundedString(warning.message, 128 * 1024, true) &&
        typeof warning.requiresAcknowledgement === 'boolean'
      )
    ) &&
    isBoundedString(preview.compatibility, 1024) &&
    isBoundedString(preview.operation, 1024) &&
    isBoundedString(preview.impact, 1024)
  )
}

export function isPersistableApproval(record: Record<string, unknown>): boolean {
  switch (record.type) {
    case 'tool_call':
      return (
        hasExactKeys(record, ['type', 'call']) && isRecord(record.call) && isToolCall(record.call)
      )
    case 'diff':
      return hasExactKeys(record, ['type', 'diff']) && isRecord(record.diff) && isDiff(record.diff)
    case 'file_write':
      return hasExactKeys(record, ['type', 'fileWrite']) && isFileWriteProposal(record.fileWrite)
    case 'command':
      return hasExactKeys(record, ['type', 'command']) && isCommandAction(record.command)
    case 'skill_materialization':
      return (
        hasExactKeys(record, ['type', 'materialization']) &&
        isSkillMaterialization(record.materialization)
      )
    case 'skill_script':
      return hasExactKeys(record, ['type', 'script']) && isSkillScript(record.script)
    case 'office_operation':
      return (
        hasExactKeys(record, ['type', 'officeOperation']) &&
        isOfficeOperation(record.officeOperation)
      )
    case 'skill_installation':
      return (
        hasExactKeys(record, ['type', 'installation']) &&
        isSkillInstallationRequest(record.installation)
      )
    default:
      // MCP approvals are deliberately process-only. The current persisted projection carries
      // only their bounded lifecycle view under mcpInvocations.
      return false
  }
}

export function isFileDraft(record: Record<string, unknown>): boolean {
  return (
    hasExactKeys(
      record,
      [
        'draftId',
        'conversationId',
        'filePath',
        'mode',
        'status',
        'additions',
        'deletions',
        'lineCount',
        'byteCount',
        'chunkCount',
        'nextChunkIndex',
        'statsFinal',
        'createdAt',
        'updatedAt'
      ],
      ['projectId', 'baseRevision', 'summary']
    ) &&
    isBoundedString(record.draftId, 1024) &&
    isBoundedString(record.conversationId, 1024, true) &&
    isBoundedString(record.filePath, 16 * 1024) &&
    ['create', 'rewrite', 'modify', 'append', 'upsert'].includes(record.mode as string) &&
    [
      'drafting',
      'ready',
      'waiting_approval',
      'applying',
      'applied',
      'rejected',
      'conflict',
      'failed',
      'aborted',
      'expired'
    ].includes(record.status as string) &&
    isSafeInteger(record.additions) &&
    isSafeInteger(record.deletions) &&
    isSafeInteger(record.lineCount) &&
    isSafeInteger(record.byteCount) &&
    isSafeInteger(record.chunkCount) &&
    isSafeInteger(record.nextChunkIndex) &&
    typeof record.statsFinal === 'boolean' &&
    isSafeInteger(record.createdAt) &&
    isSafeInteger(record.updatedAt) &&
    isOptionalBoundedString(record, 'projectId', 1024) &&
    isOptionalBoundedString(record, 'baseRevision', 1024) &&
    isOptionalBoundedString(record, 'summary', 16 * 1024, true)
  )
}
