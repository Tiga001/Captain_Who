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

function isInlineDiff(value: unknown): boolean {
  return (
    value === null ||
    (isRecord(value) &&
      hasExactKeys(value, ['patch', 'truncated']) &&
      isBoundedString(value.patch, 4 * 1024 * 1024, true) &&
      value.truncated === false)
  )
}

export function isFileChangeProposal(record: Record<string, unknown>): boolean {
  const operation = record.operation
  const strategy = record.updateStrategy
  const inlineDiffIsNull = record.inlineDiff === null
  return (
    hasExactKeys(record, [
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
    ]) &&
    record.schemaVersion === 1 &&
    isBoundedString(record.id, 1024) &&
    isBoundedString(record.transactionId, 1024) &&
    (operation === 'create' || operation === 'update' || operation === 'delete') &&
    (strategy === null || strategy === 'modify' || strategy === 'rewrite') &&
    ((operation === 'create' && strategy === null) ||
      (operation === 'update' && inlineDiffIsNull === (strategy !== null)) ||
      (operation === 'delete' && strategy === null && !inlineDiffIsNull)) &&
    isBoundedString(record.filePath, 16 * 1024) &&
    isInlineDiff(record.inlineDiff) &&
    (operation !== 'delete' || record.inlineDiff !== null) &&
    (record.baseRevision === null || isBoundedString(record.baseRevision, 1024)) &&
    (operation === 'create') === (record.baseRevision === null) &&
    (record.summary === null || isBoundedString(record.summary, 16 * 1024, true)) &&
    isSafeInteger(record.additions) &&
    isSafeInteger(record.deletions) &&
    isSafeInteger(record.lineCount) &&
    isSafeInteger(record.byteCount) &&
    (operation !== 'delete' || (record.lineCount === 0 && record.byteCount === 0)) &&
    (record.approvalStatus === 'required' || record.approvalStatus === 'approved')
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
    case 'file_change':
      return (
        hasExactKeys(record, ['type', 'fileChange']) &&
        isRecord(record.fileChange) &&
        isFileChangeProposal(record.fileChange)
      )
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
  const operation = record.operation
  const strategy = record.updateStrategy
  return (
    hasExactKeys(record, [
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
    ]) &&
    record.schemaVersion === 1 &&
    isBoundedString(record.transactionId, 1024) &&
    isBoundedString(record.conversationId, 1024) &&
    isNullableBoundedString(record.projectId, 1024) &&
    isBoundedString(record.filePath, 16 * 1024) &&
    (operation === 'create' || operation === 'update' || operation === 'delete') &&
    (strategy === null || strategy === 'modify' || strategy === 'rewrite') &&
    (operation === 'update') === (strategy !== null) &&
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
    ].includes(record.status as string) &&
    (((record.status === 'drafting' || record.status === 'ready') && record.statsFinal === false) ||
      (record.status !== 'drafting' && record.status !== 'ready' && record.statsFinal === true)) &&
    isSafeInteger(record.additions) &&
    isSafeInteger(record.deletions) &&
    isSafeInteger(record.lineCount) &&
    isSafeInteger(record.byteCount) &&
    (operation !== 'delete' || (record.lineCount === 0 && record.byteCount === 0)) &&
    ((operation === 'create' && record.baseRevision === null) ||
      (operation !== 'create' && isBoundedString(record.baseRevision, 1024))) &&
    isSafeInteger(record.mutationCount) &&
    isSafeInteger(record.nextMutationIndex) &&
    record.nextMutationIndex === record.mutationCount &&
    typeof record.statsFinal === 'boolean' &&
    isSafeInteger(record.createdAt) &&
    isSafeInteger(record.updatedAt) &&
    (record.updatedAt as number) >= (record.createdAt as number) &&
    isNullableBoundedString(record.summary, 16 * 1024, true)
  )
}
