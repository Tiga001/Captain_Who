import type {
  AgentMcpDispatchCertainty,
  AgentMcpServerScope,
  AgentMcpToolInvocationOutcome,
  AgentMcpToolInvocationState,
  AgentProposedAction
} from '@mycopilot/protocol'
import { parseAgentCommandArtifactObservation } from '@mycopilot/protocol'
import type {
  ChatAgentRunView,
  ChatAgentTimelineItem,
  ChatCommandSessionView,
  ChatMcpToolInvocationView
} from '../chat/chatTypes'
import { parseManagedCommandOutputs } from '../chat/managedCommandOutputs'

const MAX_STORED_RUN_ITEMS = 10_000
const MAX_OFFICE_RENDER_PAGES = 128
const MAX_OFFICE_PAGE_NUMBER = 10_000
const MAX_OFFICE_SCREENSHOT_DIMENSION = 16_384
const MAX_OFFICE_GRID_COLUMNS = 32
const MAX_U32 = 0xffff_ffff
export const STORED_AGENT_RUN_CORRUPTION_ERROR = 'Stored Agent run is malformed'
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/iu

const RUN_STATUSES = new Set<ChatAgentRunView['status']>([
  'starting',
  'idle',
  'queued',
  'running',
  'waiting_for_approval',
  'completed',
  'failed',
  'cancelled'
])
const TERMINAL_RUN_STATUSES = new Set<ChatAgentRunView['status']>([
  'completed',
  'failed',
  'cancelled'
])
const MCP_STATES = new Set<AgentMcpToolInvocationState>([
  'pending_approval',
  'approved',
  'dispatching',
  'running',
  'completed',
  'failed',
  'cancelled',
  'rejected',
  'expired',
  'payload_unavailable',
  'policy_denied',
  'outcome_unknown'
])
const MCP_OUTCOMES = new Set<AgentMcpToolInvocationOutcome>([
  'succeeded',
  'tool_error',
  'output_too_large',
  'transport_error',
  'timed_out',
  'cancelled',
  'rejected',
  'expired',
  'payload_unavailable',
  'policy_denied',
  'outcome_unknown'
])
const MCP_DISPATCH_CERTAINTIES = new Set<AgentMcpDispatchCertainty>([
  'definitely_not_dispatched',
  'possibly_dispatched',
  'response_received'
])
const TERMINAL_COMMAND_STATUSES = new Set<ChatCommandSessionView['status']>([
  'exited',
  'interrupted',
  'timed_out',
  'failed',
  'outcome_unknown'
])

const STORED_RUN_KEYS = [
  'runId',
  'status',
  'startedAt',
  'firstResponseAt',
  'lastResponseAt',
  'completedAt',
  'toolDefinitions',
  'toolSetRevision',
  'todo',
  'toolCalls',
  'toolResults',
  'webSearchActivities',
  'readActivities',
  'approvals',
  'skillInstallations',
  'diffs',
  'fileDrafts',
  'commandSessions',
  'mcpInvocations',
  'messageStreamCheckpoints',
  'timeline',
  'state',
  'error',
  'usage',
  'finishReason',
  'activatedSkills',
  'skillActivationRevision',
  'explicitSkillSelections'
] as const
const REQUIRED_STORED_RUN_KEYS = [
  'runId',
  'status',
  'startedAt',
  'toolDefinitions',
  'toolCalls',
  'toolResults',
  'webSearchActivities',
  'readActivities',
  'approvals',
  'diffs',
  'fileDrafts',
  'mcpInvocations',
  'messageStreamCheckpoints',
  'timeline'
] as const

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function hasOwn(record: Record<string, unknown>, key: string): boolean {
  return Object.prototype.hasOwnProperty.call(record, key)
}

function hasExactKeys(
  record: Record<string, unknown>,
  required: readonly string[],
  optional: readonly string[] = []
): boolean {
  const allowed = new Set([...required, ...optional])
  return (
    required.every((key) => hasOwn(record, key)) &&
    Object.keys(record).every((key) => allowed.has(key))
  )
}

function isBoundedString(value: unknown, maximum = 4096, allowEmpty = false): value is string {
  return (
    typeof value === 'string' &&
    (allowEmpty || value.length > 0) &&
    Array.from(value).length <= maximum
  )
}

function isOptionalBoundedString(
  record: Record<string, unknown>,
  key: string,
  maximum = 4096,
  allowEmpty = false
): boolean {
  return !hasOwn(record, key) || isBoundedString(record[key], maximum, allowEmpty)
}

function isSafeInteger(value: unknown, minimum = 0): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= minimum
}

function isOptionalSafeInteger(record: Record<string, unknown>, key: string, minimum = 0): boolean {
  return !hasOwn(record, key) || isSafeInteger(record[key], minimum)
}

function isRecordArray(
  value: unknown,
  validate: (record: Record<string, unknown>) => boolean
): boolean {
  return (
    Array.isArray(value) &&
    value.length <= MAX_STORED_RUN_ITEMS &&
    value.every((item) => isRecord(item) && validate(item))
  )
}

function isStringArray(value: unknown, maximumLength = 4096): value is string[] {
  return (
    Array.isArray(value) &&
    value.length <= MAX_STORED_RUN_ITEMS &&
    value.every((entry) => isBoundedString(entry, maximumLength, true))
  )
}

function isNullableBoundedString(
  value: unknown,
  maximum = 4096,
  allowEmpty = false
): value is string | null {
  return value === null || isBoundedString(value, maximum, allowEmpty)
}

function isNullableSafeInteger(value: unknown, minimum = 0): value is number | null {
  return value === null || isSafeInteger(value, minimum)
}

function hasUniqueStrings(values: readonly string[]): boolean {
  return new Set(values).size === values.length
}

function isToolDefinition(record: Record<string, unknown>): boolean {
  return (
    hasExactKeys(record, [
      'name',
      'description',
      'inputSchema',
      'safety',
      'requiresWorkspace',
      'requiresApproval',
      'approvalMode'
    ]) &&
    isBoundedString(record.name, 1024) &&
    isBoundedString(record.description, 128 * 1024, true) &&
    (record.safety === 'read_only' ||
      record.safety === 'requires_approval' ||
      record.safety === 'destructive') &&
    typeof record.requiresWorkspace === 'boolean' &&
    typeof record.requiresApproval === 'boolean' &&
    (record.approvalMode === 'never' ||
      record.approvalMode === 'always' ||
      record.approvalMode === 'dynamic')
  )
}

function isApprovalStatus(value: unknown): boolean {
  return (
    value === 'not_required' || value === 'required' || value === 'approved' || value === 'rejected'
  )
}

function isToolCall(record: Record<string, unknown>): boolean {
  return (
    hasExactKeys(record, ['id', 'tool', 'args', 'approvalStatus', 'reason']) &&
    isBoundedString(record.id, 1024) &&
    isBoundedString(record.tool, 1024) &&
    isApprovalStatus(record.approvalStatus) &&
    (record.reason === null || isBoundedString(record.reason, 4096, true))
  )
}

function isToolResult(record: Record<string, unknown>): boolean {
  return (
    hasExactKeys(record, ['callId', 'tool', 'ok'], ['result', 'error']) &&
    isBoundedString(record.callId, 1024) &&
    isBoundedString(record.tool, 1024) &&
    typeof record.ok === 'boolean' &&
    isOptionalBoundedString(record, 'error', 128 * 1024, true)
  )
}

function isDiff(record: Record<string, unknown>): boolean {
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

function isFileInputRef(value: unknown): boolean {
  if (!isRecord(value) || typeof value.type !== 'string') return false
  if (
    value.type === 'attachment' &&
    hasExactKeys(value, ['type', 'readPath']) &&
    isBoundedString(value.readPath, 16 * 1024)
  ) {
    return true
  }
  if (
    (value.type === 'workspace' || value.type === 'external') &&
    hasExactKeys(value, ['type', 'path']) &&
    isBoundedString(value.path, 16 * 1024)
  ) {
    return true
  }
  if (
    value.type === 'generated_artifact' &&
    hasExactKeys(value, ['type', 'uri', 'path']) &&
    isBoundedString(value.uri, 16 * 1024) &&
    isBoundedString(value.path, 16 * 1024)
  ) {
    return true
  }
  return (
    value.type === 'skill_resource' &&
    hasExactKeys(value, ['type', 'uri']) &&
    isBoundedString(value.uri, 16 * 1024)
  )
}

function isFileInputSpec(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(value, ['mountPath', 'source']) &&
    isBoundedString(value.mountPath, 16 * 1024) &&
    isFileInputRef(value.source)
  )
}

function isFileInputBinding(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(value, ['schemaVersion', 'mountPath', 'source', 'sizeBytes', 'sha256']) &&
    value.schemaVersion === 1 &&
    isBoundedString(value.mountPath, 16 * 1024) &&
    isFileInputRef(value.source) &&
    isSafeInteger(value.sizeBytes) &&
    typeof value.sha256 === 'string' &&
    /^[0-9a-f]{64}$/u.test(value.sha256)
  )
}

function isOfficePropertyMap(value: unknown): boolean {
  if (!isRecord(value)) return false
  return Object.values(value).every(
    (property) =>
      typeof property === 'string' ||
      typeof property === 'number' ||
      typeof property === 'boolean' ||
      (isRecord(property) &&
        hasExactKeys(property, ['resourcePath']) &&
        isBoundedString(property.resourcePath, 16 * 1024))
  )
}

function isOfficePosition(value: unknown): boolean {
  return (
    isRecord(value) &&
    ((value.type === 'index' &&
      hasExactKeys(value, ['type', 'index']) &&
      isSafeInteger(value.index)) ||
      ((value.type === 'after' || value.type === 'before') &&
        hasExactKeys(value, ['type', 'target']) &&
        isBoundedString(value.target, 16 * 1024)))
  )
}

function isOfficeParameters(value: unknown): boolean {
  if (!isRecord(value) || typeof value.type !== 'string') return false
  switch (value.type) {
    case 'help':
      return (
        hasExactKeys(value, ['type'], ['verb', 'element']) &&
        (!hasOwn(value, 'verb') ||
          [
            'status',
            'help',
            'create',
            'view',
            'get',
            'query',
            'validate',
            'set',
            'add',
            'remove',
            'move',
            'swap'
          ].includes(value.verb as string)) &&
        isOptionalBoundedString(value, 'element', 1024)
      )
    case 'create':
      return (
        hasExactKeys(value, ['type'], ['locale', 'minimal', 'overwrite']) &&
        isOptionalBoundedString(value, 'locale', 1024) &&
        (!hasOwn(value, 'minimal') || typeof value.minimal === 'boolean') &&
        (!hasOwn(value, 'overwrite') || typeof value.overwrite === 'boolean')
      )
    case 'view': {
      if (
        !hasExactKeys(
          value,
          ['type', 'mode'],
          [
            'start',
            'end',
            'maxLines',
            'issueType',
            'limit',
            'columns',
            'pages',
            'range',
            'viewport',
            'grid',
            'renderMode',
            'pageCount'
          ]
        ) ||
        ![
          'text',
          'annotated',
          'outline',
          'stats',
          'issues',
          'html',
          'svg',
          'screenshot',
          'forms'
        ].includes(value.mode as string) ||
        !isOptionalSafeInteger(value, 'start') ||
        !isOptionalSafeInteger(value, 'end') ||
        !isOptionalSafeInteger(value, 'maxLines') ||
        !isOptionalBoundedString(value, 'issueType', 1024) ||
        !isOptionalSafeInteger(value, 'limit') ||
        (hasOwn(value, 'columns') && !isStringArray(value.columns, 1024)) ||
        !isOptionalBoundedString(value, 'range', 16 * 1024) ||
        (hasOwn(value, 'renderMode') &&
          value.renderMode !== 'auto' &&
          value.renderMode !== 'html') ||
        (hasOwn(value, 'pageCount') && typeof value.pageCount !== 'boolean')
      ) {
        return false
      }
      if (
        hasOwn(value, 'pages') &&
        !isRecordArray(value.pages, (page) =>
          Boolean(
            hasExactKeys(page, ['start'], ['end']) &&
            isSafeInteger(page.start) &&
            isOptionalSafeInteger(page, 'end')
          )
        )
      ) {
        return false
      }
      if (
        hasOwn(value, 'viewport') &&
        (!isRecord(value.viewport) ||
          !hasExactKeys(value.viewport, ['width', 'height']) ||
          !isSafeInteger(value.viewport.width, 1) ||
          !isSafeInteger(value.viewport.height, 1))
      ) {
        return false
      }
      if (hasOwn(value, 'grid')) {
        if (!isRecord(value.grid)) return false
        if (!(
          (value.grid.mode === 'auto' && hasExactKeys(value.grid, ['mode'])) ||
          (value.grid.mode === 'columns' &&
            hasExactKeys(value.grid, ['mode', 'columns']) &&
            isSafeInteger(value.grid.columns, 1))
        )) {
          return false
        }
      }
      return true
    }
    case 'get':
      return (
        hasExactKeys(value, ['type'], ['target', 'depth']) &&
        isOptionalBoundedString(value, 'target', 16 * 1024) &&
        isOptionalSafeInteger(value, 'depth')
      )
    case 'query':
      return (
        hasExactKeys(value, ['type', 'selector'], ['contains', 'compact', 'fields']) &&
        isBoundedString(value.selector, 16 * 1024) &&
        isOptionalBoundedString(value, 'contains', 16 * 1024, true) &&
        (!hasOwn(value, 'compact') || typeof value.compact === 'boolean') &&
        (!hasOwn(value, 'fields') || isStringArray(value.fields, 1024))
      )
    case 'validate':
      return hasExactKeys(value, ['type'])
    case 'set':
      return (
        hasExactKeys(value, ['type', 'target'], ['properties', 'replacement', 'force']) &&
        isBoundedString(value.target, 16 * 1024) &&
        (!hasOwn(value, 'properties') || isOfficePropertyMap(value.properties)) &&
        (!hasOwn(value, 'replacement') ||
          (isRecord(value.replacement) &&
            hasExactKeys(value.replacement, ['find', 'replace']) &&
            isBoundedString(value.replacement.find, 128 * 1024, true) &&
            isBoundedString(value.replacement.replace, 128 * 1024, true))) &&
        (!hasOwn(value, 'force') || typeof value.force === 'boolean')
      )
    case 'add':
      return (
        hasExactKeys(
          value,
          ['type', 'parent', 'elementType'],
          ['copyFrom', 'position', 'properties', 'force']
        ) &&
        isBoundedString(value.parent, 16 * 1024) &&
        isBoundedString(value.elementType, 1024) &&
        isOptionalBoundedString(value, 'copyFrom', 16 * 1024) &&
        (!hasOwn(value, 'position') || isOfficePosition(value.position)) &&
        (!hasOwn(value, 'properties') || isOfficePropertyMap(value.properties)) &&
        (!hasOwn(value, 'force') || typeof value.force === 'boolean')
      )
    case 'remove':
      return (
        hasExactKeys(value, ['type', 'target'], ['shift', 'properties']) &&
        isBoundedString(value.target, 16 * 1024) &&
        (!hasOwn(value, 'shift') || value.shift === 'left' || value.shift === 'up') &&
        (!hasOwn(value, 'properties') || isOfficePropertyMap(value.properties))
      )
    case 'move':
      return (
        hasExactKeys(value, ['type', 'target'], ['newParent', 'position', 'properties']) &&
        isBoundedString(value.target, 16 * 1024) &&
        isOptionalBoundedString(value, 'newParent', 16 * 1024) &&
        (!hasOwn(value, 'position') || isOfficePosition(value.position)) &&
        (!hasOwn(value, 'properties') || isOfficePropertyMap(value.properties))
      )
    case 'swap':
      return (
        hasExactKeys(value, ['type', 'firstTarget', 'secondTarget']) &&
        isBoundedString(value.firstTarget, 16 * 1024) &&
        isBoundedString(value.secondTarget, 16 * 1024)
      )
    default:
      return false
  }
}

function isOfficeRequest(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(value, [
      'documentKind',
      'operation',
      'documentPath',
      'outputPath',
      'destinationPath',
      'inputs',
      'timeoutMs',
      'parameters'
    ]) &&
    ['document', 'spreadsheet', 'presentation'].includes(value.documentKind as string) &&
    [
      'help',
      'create',
      'view',
      'get',
      'query',
      'validate',
      'set',
      'add',
      'remove',
      'move',
      'swap'
    ].includes(value.operation as string) &&
    isNullableBoundedString(value.documentPath, 16 * 1024) &&
    isNullableBoundedString(value.outputPath, 16 * 1024) &&
    isNullableBoundedString(value.destinationPath, 16 * 1024) &&
    Array.isArray(value.inputs) &&
    value.inputs.length <= MAX_STORED_RUN_ITEMS &&
    value.inputs.every(isFileInputSpec) &&
    isNullableSafeInteger(value.timeoutMs) &&
    isOfficeParameters(value.parameters) &&
    isRecord(value.parameters) &&
    value.parameters.type === value.operation
  )
}

function isOfficePathSlot(value: unknown): boolean {
  return (
    isRecord(value) &&
    (((value.type === 'document' || value.type === 'output' || value.type === 'destination') &&
      hasExactKeys(value, ['type'])) ||
      (value.type === 'resource' &&
        hasExactKeys(value, ['type', 'index']) &&
        isSafeInteger(value.index)))
  )
}

function isOfficePathIdentity(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(value, ['revision', 'device', 'inode']) &&
    isBoundedString(value.revision, 1024) &&
    isNullableSafeInteger(value.device) &&
    isNullableSafeInteger(value.inode)
  )
}

function isOfficeFrozenPath(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(value, [
      'slot',
      'logicalPath',
      'purpose',
      'scope',
      'normalizedPath',
      'state',
      'objectIdentity',
      'parentIdentity',
      'contentRevision',
      'size',
      'writeDisposition'
    ]) &&
    isOfficePathSlot(value.slot) &&
    isBoundedString(value.logicalPath, 16 * 1024) &&
    ['readSource', 'writeTarget', 'inPlaceTarget'].includes(value.purpose as string) &&
    ['workspace', 'external', 'attachment'].includes(value.scope as string) &&
    isBoundedString(value.normalizedPath, 16 * 1024) &&
    (value.state === 'missing' || value.state === 'present') &&
    (value.objectIdentity === null || isOfficePathIdentity(value.objectIdentity)) &&
    isOfficePathIdentity(value.parentIdentity) &&
    isNullableBoundedString(value.contentRevision, 1024) &&
    isNullableSafeInteger(value.size) &&
    (value.writeDisposition === null ||
      value.writeDisposition === 'createNew' ||
      value.writeDisposition === 'replaceExisting')
  )
}

function isOfficePrepared(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(value, [
      'schemaVersion',
      'providerId',
      'engineRevision',
      'workspaceRevision',
      'access',
      'request',
      'argv',
      'resolvedRenderPlan',
      'paths',
      'inputBindings'
    ]) &&
    value.schemaVersion === 6 &&
    isBoundedString(value.providerId, 1024) &&
    isBoundedString(value.engineRevision, 4096) &&
    isNullableBoundedString(value.workspaceRevision, 4096) &&
    (value.access === 'readOnly' || value.access === 'fileWrite') &&
    isOfficeRequest(value.request) &&
    isStringArray(value.argv, 64 * 1024) &&
    (value.resolvedRenderPlan === null ||
      isOfficePresentationRenderPlan(value.resolvedRenderPlan)) &&
    (value.resolvedRenderPlan !== null) === requiresPresentationRenderPlan(value.request) &&
    Array.isArray(value.paths) &&
    value.paths.length <= MAX_STORED_RUN_ITEMS &&
    value.paths.every(isOfficeFrozenPath) &&
    Array.isArray(value.inputBindings) &&
    value.inputBindings.length <= MAX_STORED_RUN_ITEMS &&
    value.inputBindings.every(isFileInputBinding)
  )
}

function requiresPresentationRenderPlan(value: unknown): boolean {
  if (!isRecord(value) || !isRecord(value.parameters)) return false
  return (
    value.documentKind === 'presentation' &&
    value.operation === 'view' &&
    value.parameters.type === 'view' &&
    value.parameters.mode === 'screenshot'
  )
}

function isOfficePresentationRenderPlan(value: unknown): boolean {
  if (
    !isRecord(value) ||
    !hasExactKeys(value, [
      'requestedPages',
      'slideWidthEmu',
      'slideHeightEmu',
      'viewport',
      'grid'
    ]) ||
    !Array.isArray(value.requestedPages) ||
    value.requestedPages.length === 0 ||
    value.requestedPages.length > MAX_OFFICE_RENDER_PAGES ||
    !value.requestedPages.every(
      (page, index, pages) =>
        isSafeInteger(page) &&
        page > 0 &&
        page <= MAX_OFFICE_PAGE_NUMBER &&
        (index === 0 || (pages[index - 1] as number) < page)
    ) ||
    !isSafeInteger(value.slideWidthEmu) ||
    value.slideWidthEmu <= 0 ||
    value.slideWidthEmu > MAX_U32 ||
    !isSafeInteger(value.slideHeightEmu) ||
    value.slideHeightEmu <= 0 ||
    value.slideHeightEmu > MAX_U32 ||
    !isRecord(value.viewport) ||
    !hasExactKeys(value.viewport, ['width', 'height']) ||
    !isSafeInteger(value.viewport.width) ||
    value.viewport.width <= 0 ||
    value.viewport.width > MAX_OFFICE_SCREENSHOT_DIMENSION ||
    !isSafeInteger(value.viewport.height) ||
    value.viewport.height <= 0 ||
    value.viewport.height > MAX_OFFICE_SCREENSHOT_DIMENSION
  ) {
    return false
  }
  return (
    value.grid === null ||
    (isRecord(value.grid) &&
      hasExactKeys(value.grid, ['mode', 'columns']) &&
      value.grid.mode === 'columns' &&
      isSafeInteger(value.grid.columns) &&
      value.grid.columns > 0 &&
      value.grid.columns <= MAX_OFFICE_GRID_COLUMNS)
  )
}

function isOfficeOperation(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(value, [
      'schemaVersion',
      'id',
      'semanticArgs',
      'prepared',
      'approvalStatus',
      'reason'
    ]) &&
    value.schemaVersion === 6 &&
    isBoundedString(value.id, 1024) &&
    isRecord(value.semanticArgs) &&
    isOfficePrepared(value.prepared) &&
    isApprovalStatus(value.approvalStatus) &&
    isBoundedString(value.reason, 16 * 1024, true)
  )
}

function isSkillInstallationRequest(value: unknown): boolean {
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

function isPersistableApproval(record: Record<string, unknown>): boolean {
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

function isFileDraft(record: Record<string, unknown>): boolean {
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

function isWebSearchSource(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(
      value,
      ['id', 'title', 'url', 'displayUrl', 'domain'],
      ['faviconUrl', 'snippet', 'score', 'publishedDate']
    ) &&
    isBoundedString(value.id, 1024) &&
    isBoundedString(value.title, 128 * 1024, true) &&
    isBoundedString(value.url, 16 * 1024) &&
    isBoundedString(value.displayUrl, 16 * 1024, true) &&
    isBoundedString(value.domain, 4096) &&
    isOptionalBoundedString(value, 'faviconUrl', 16 * 1024, true) &&
    isOptionalBoundedString(value, 'snippet', 128 * 1024, true) &&
    (!hasOwn(value, 'score') ||
      (typeof value.score === 'number' && Number.isFinite(value.score))) &&
    isOptionalBoundedString(value, 'publishedDate', 4096, true)
  )
}

function isWebSearchActivity(record: Record<string, unknown>): boolean {
  if (
    !hasExactKeys(
      record,
      ['callId', 'query', 'provider', 'status', 'sources', 'updatedAt'],
      ['kind', 'answer', 'summaryQuality', 'error', 'responseTime', 'truncated']
    ) ||
    !isBoundedString(record.callId, 1024) ||
    !isBoundedString(record.query, 128 * 1024, true) ||
    !isBoundedString(record.provider, 1024) ||
    !['running', 'completed', 'failed', 'cancelled'].includes(record.status as string) ||
    !Array.isArray(record.sources) ||
    record.sources.length > MAX_STORED_RUN_ITEMS ||
    !record.sources.every(isWebSearchSource) ||
    !isSafeInteger(record.updatedAt) ||
    (hasOwn(record, 'kind') && record.kind !== 'search' && record.kind !== 'fetch') ||
    !isOptionalBoundedString(record, 'answer', 4 * 1024 * 1024, true) ||
    (hasOwn(record, 'summaryQuality') &&
      record.summaryQuality !== 'good' &&
      record.summaryQuality !== 'low') ||
    !isOptionalBoundedString(record, 'error', 128 * 1024, true) ||
    (hasOwn(record, 'responseTime') &&
      record.responseTime !== null &&
      typeof record.responseTime !== 'number' &&
      typeof record.responseTime !== 'string') ||
    (hasOwn(record, 'truncated') && typeof record.truncated !== 'boolean')
  ) {
    return false
  }
  return true
}

function isReadActivity(record: Record<string, unknown>): boolean {
  return (
    hasExactKeys(
      record,
      ['callId', 'tool', 'kind', 'status', 'path', 'fileName', 'updatedAt'],
      ['extension', 'mimeType', 'thumbnailDataUrl', 'fullDataUrl', 'error']
    ) &&
    isBoundedString(record.callId, 1024) &&
    isBoundedString(record.tool, 1024) &&
    ['file', 'image', 'word', 'presentation', 'spreadsheet'].includes(record.kind as string) &&
    ['running', 'completed', 'failed', 'cancelled'].includes(record.status as string) &&
    isBoundedString(record.path, 16 * 1024) &&
    isBoundedString(record.fileName, 4096) &&
    isSafeInteger(record.updatedAt) &&
    isOptionalBoundedString(record, 'extension', 1024, true) &&
    isOptionalBoundedString(record, 'mimeType', 1024, true) &&
    isOptionalBoundedString(record, 'thumbnailDataUrl', 32 * 1024 * 1024, true) &&
    isOptionalBoundedString(record, 'fullDataUrl', 32 * 1024 * 1024, true) &&
    isOptionalBoundedString(record, 'error', 128 * 1024, true)
  )
}

function parseMcpScope(value: unknown): AgentMcpServerScope | undefined {
  if (!isRecord(value) || typeof value.type !== 'string') return undefined
  if (
    (value.type === 'builtin' || value.type === 'user' || value.type === 'managed') &&
    hasExactKeys(value, ['type'])
  ) {
    return { type: value.type }
  }
  if (
    value.type === 'project' &&
    hasExactKeys(value, ['type', 'projectId']) &&
    isBoundedString(value.projectId, 1024)
  ) {
    return { type: 'project', projectId: value.projectId }
  }
  if (
    value.type === 'plugin' &&
    hasExactKeys(value, ['type', 'pluginId']) &&
    isBoundedString(value.pluginId, 1024)
  ) {
    return { type: 'plugin', pluginId: value.pluginId }
  }
  return undefined
}

function hasValidMcpLifecycle(invocation: ChatMcpToolInvocationView): boolean {
  const { state, dispatchCertainty, outcome, isError, errorCode, durationMs, outputTruncated } =
    invocation
  switch (state) {
    case 'pending_approval':
    case 'approved':
      return (
        dispatchCertainty === 'definitely_not_dispatched' &&
        outcome === undefined &&
        isError === undefined &&
        errorCode === undefined &&
        durationMs === undefined &&
        !outputTruncated
      )
    case 'dispatching':
    case 'running':
      return (
        dispatchCertainty === 'possibly_dispatched' &&
        outcome === undefined &&
        isError === undefined &&
        errorCode === undefined &&
        durationMs === undefined &&
        !outputTruncated
      )
    case 'completed':
      return (
        dispatchCertainty === 'response_received' &&
        durationMs !== undefined &&
        ((outcome === 'succeeded' && isError === false && errorCode === undefined) ||
          (outcome === 'tool_error' && isError === true && errorCode !== undefined))
      )
    case 'failed':
      return (
        durationMs !== undefined &&
        errorCode !== undefined &&
        isError === true &&
        ((outcome === 'output_too_large' && dispatchCertainty === 'response_received') ||
          (outcome === 'timed_out' &&
            dispatchCertainty === 'definitely_not_dispatched' &&
            !outputTruncated) ||
          (outcome === 'transport_error' &&
            (dispatchCertainty === 'definitely_not_dispatched' ||
              dispatchCertainty === 'response_received') &&
            (!outputTruncated || dispatchCertainty === 'response_received')))
      )
    case 'cancelled':
      return (
        dispatchCertainty === 'definitely_not_dispatched' &&
        outcome === 'cancelled' &&
        isError === undefined &&
        errorCode !== undefined &&
        !outputTruncated
      )
    case 'rejected':
      return (
        dispatchCertainty === 'definitely_not_dispatched' &&
        outcome === 'rejected' &&
        isError === undefined &&
        errorCode !== undefined &&
        !outputTruncated
      )
    case 'expired':
      return (
        dispatchCertainty === 'definitely_not_dispatched' &&
        outcome === 'expired' &&
        isError === undefined &&
        errorCode !== undefined &&
        !outputTruncated
      )
    case 'payload_unavailable':
      return (
        dispatchCertainty === 'definitely_not_dispatched' &&
        outcome === 'payload_unavailable' &&
        isError === true &&
        errorCode !== undefined &&
        !outputTruncated
      )
    case 'policy_denied':
      return (
        dispatchCertainty === 'definitely_not_dispatched' &&
        outcome === 'policy_denied' &&
        isError === undefined &&
        errorCode !== undefined &&
        !outputTruncated
      )
    case 'outcome_unknown':
      return (
        dispatchCertainty === 'possibly_dispatched' &&
        outcome === 'outcome_unknown' &&
        isError === undefined &&
        errorCode !== undefined &&
        !outputTruncated
      )
  }
}

function parseStoredMcpInvocation(value: unknown): ChatMcpToolInvocationView | undefined {
  if (!isRecord(value)) return undefined
  if (
    !hasExactKeys(
      value,
      [
        'actionId',
        'invocationId',
        'callId',
        'serverId',
        'serverDisplayName',
        'rawToolName',
        'modelToolName',
        'external',
        'state',
        'dispatchCertainty',
        'outputTruncated'
      ],
      ['scope', 'displayReason', 'outcome', 'isError', 'errorCode', 'rejectionReason', 'durationMs']
    ) ||
    typeof value.actionId !== 'string' ||
    !UUID_PATTERN.test(value.actionId) ||
    typeof value.invocationId !== 'string' ||
    !UUID_PATTERN.test(value.invocationId) ||
    value.actionId === value.invocationId ||
    !isBoundedString(value.callId, 1024) ||
    typeof value.serverId !== 'string' ||
    !UUID_PATTERN.test(value.serverId) ||
    !isBoundedString(value.serverDisplayName, 1024) ||
    !isBoundedString(value.rawToolName, 1024) ||
    !isBoundedString(value.modelToolName, 1024) ||
    value.external !== true ||
    typeof value.state !== 'string' ||
    !MCP_STATES.has(value.state as AgentMcpToolInvocationState) ||
    typeof value.dispatchCertainty !== 'string' ||
    !MCP_DISPATCH_CERTAINTIES.has(value.dispatchCertainty as AgentMcpDispatchCertainty) ||
    typeof value.outputTruncated !== 'boolean' ||
    !isOptionalBoundedString(value, 'displayReason', 512) ||
    (hasOwn(value, 'outcome') &&
      (typeof value.outcome !== 'string' ||
        !MCP_OUTCOMES.has(value.outcome as AgentMcpToolInvocationOutcome))) ||
    (hasOwn(value, 'isError') && typeof value.isError !== 'boolean') ||
    !isOptionalBoundedString(value, 'errorCode', 1024) ||
    !isOptionalBoundedString(value, 'rejectionReason', 512) ||
    !isOptionalSafeInteger(value, 'durationMs')
  ) {
    return undefined
  }
  const scope = hasOwn(value, 'scope') ? parseMcpScope(value.scope) : undefined
  if (hasOwn(value, 'scope') && !scope) return undefined

  const invocation: ChatMcpToolInvocationView = {
    actionId: value.actionId,
    invocationId: value.invocationId,
    callId: value.callId,
    serverId: value.serverId,
    serverDisplayName: value.serverDisplayName,
    ...(scope ? { scope } : {}),
    rawToolName: value.rawToolName,
    modelToolName: value.modelToolName,
    ...(typeof value.displayReason === 'string' ? { displayReason: value.displayReason } : {}),
    external: true,
    state: value.state as AgentMcpToolInvocationState,
    dispatchCertainty: value.dispatchCertainty as AgentMcpDispatchCertainty,
    ...(typeof value.outcome === 'string'
      ? { outcome: value.outcome as AgentMcpToolInvocationOutcome }
      : {}),
    ...(typeof value.isError === 'boolean' ? { isError: value.isError } : {}),
    ...(typeof value.errorCode === 'string' ? { errorCode: value.errorCode } : {}),
    ...(typeof value.rejectionReason === 'string'
      ? { rejectionReason: value.rejectionReason }
      : {}),
    ...(typeof value.durationMs === 'number' ? { durationMs: value.durationMs } : {}),
    outputTruncated: value.outputTruncated
  }
  return hasValidMcpLifecycle(invocation) ? invocation : undefined
}

function isTimelineAttachment(record: Record<string, unknown>): boolean {
  return (
    hasExactKeys(
      record,
      ['id', 'kind', 'name', 'sizeBytes'],
      ['mimeType', 'encoding', 'data', 'previewData', 'previewMimeType', 'createdAt']
    ) &&
    isBoundedString(record.id, 1024) &&
    (record.kind === 'file' || record.kind === 'image') &&
    isBoundedString(record.name, 4096) &&
    isSafeInteger(record.sizeBytes) &&
    (!hasOwn(record, 'mimeType') ||
      record.mimeType === null ||
      isBoundedString(record.mimeType, 1024, true)) &&
    (!hasOwn(record, 'encoding') || record.encoding === 'utf8' || record.encoding === 'base64') &&
    isOptionalBoundedString(record, 'data', 32 * 1024 * 1024, true) &&
    (!hasOwn(record, 'previewData') ||
      record.previewData === null ||
      isBoundedString(record.previewData, 32 * 1024 * 1024, true)) &&
    (!hasOwn(record, 'previewMimeType') ||
      record.previewMimeType === null ||
      isBoundedString(record.previewMimeType, 1024, true)) &&
    isOptionalSafeInteger(record, 'createdAt')
  )
}

function parseTimelineItem(value: unknown): ChatAgentTimelineItem | undefined {
  if (!isRecord(value) || !isBoundedString(value.id, 1024) || typeof value.type !== 'string') {
    return undefined
  }
  if (
    value.type === 'message' &&
    hasExactKeys(value, ['id', 'type', 'content'], ['streamId', 'traceSequence']) &&
    isBoundedString(value.content, 4 * 1024 * 1024, true) &&
    isOptionalBoundedString(value, 'streamId', 1024) &&
    isOptionalSafeInteger(value, 'traceSequence')
  ) {
    return value as unknown as ChatAgentTimelineItem
  }
  if (
    value.type === 'tool_call' &&
    hasExactKeys(value, ['id', 'type', 'callId'], ['traceSequence']) &&
    isBoundedString(value.callId, 1024) &&
    isOptionalSafeInteger(value, 'traceSequence')
  ) {
    return value as unknown as ChatAgentTimelineItem
  }
  if (
    value.type === 'mcp_tool_call' &&
    hasExactKeys(value, ['id', 'type', 'invocationId'], ['traceSequence']) &&
    isBoundedString(value.invocationId, 1024) &&
    isOptionalSafeInteger(value, 'traceSequence')
  ) {
    return value as unknown as ChatAgentTimelineItem
  }
  if (
    value.type === 'context_compaction' &&
    hasExactKeys(value, ['id', 'type', 'operationId', 'status'], ['traceSequence']) &&
    isBoundedString(value.operationId, 1024) &&
    ['running', 'applied', 'skipped', 'failed', 'cancelled'].includes(value.status as string) &&
    isOptionalSafeInteger(value, 'traceSequence')
  ) {
    return value as unknown as ChatAgentTimelineItem
  }
  if (
    value.type === 'error' &&
    hasExactKeys(value, ['id', 'type', 'message'], ['traceSequence']) &&
    isBoundedString(value.message, 128 * 1024, true) &&
    isOptionalSafeInteger(value, 'traceSequence')
  ) {
    return value as unknown as ChatAgentTimelineItem
  }
  if (
    value.type === 'user_guidance' &&
    hasExactKeys(
      value,
      ['id', 'type', 'clientMessageId', 'content', 'attachments', 'status', 'createdAt'],
      ['guidanceId', 'rejectionCode', 'error', 'recoverable', 'sequence', 'traceSequence']
    ) &&
    isBoundedString(value.clientMessageId, 1024) &&
    isBoundedString(value.content, 4 * 1024 * 1024, true) &&
    isRecordArray(value.attachments, isTimelineAttachment) &&
    ['submitting', 'queued', 'applied', 'rejected'].includes(value.status as string) &&
    isSafeInteger(value.createdAt) &&
    isOptionalBoundedString(value, 'guidanceId', 1024) &&
    isOptionalBoundedString(value, 'rejectionCode', 1024) &&
    isOptionalBoundedString(value, 'error', 128 * 1024, true) &&
    (!hasOwn(value, 'recoverable') || typeof value.recoverable === 'boolean') &&
    isOptionalSafeInteger(value, 'sequence') &&
    isOptionalSafeInteger(value, 'traceSequence')
  ) {
    return value as unknown as ChatAgentTimelineItem
  }
  return undefined
}

function isToolSetRevision(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(value, ['stable', 'dynamic', 'effective']) &&
    isBoundedString(value.stable, 1024) &&
    isBoundedString(value.dynamic, 1024) &&
    isBoundedString(value.effective, 1024)
  )
}

function isTodo(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(value, ['revision', 'items', 'updatedAt']) &&
    isSafeInteger(value.revision) &&
    isRecordArray(value.items, (item) =>
      Boolean(
        hasExactKeys(item, ['id', 'title', 'status', 'createdAt', 'updatedAt'], ['note']) &&
        isBoundedString(item.id, 1024) &&
        isBoundedString(item.title, 64 * 1024, true) &&
        ['pending', 'in_progress', 'completed', 'blocked'].includes(item.status as string) &&
        isSafeInteger(item.createdAt) &&
        isSafeInteger(item.updatedAt) &&
        isOptionalBoundedString(item, 'note', 64 * 1024, true)
      )
    ) &&
    isSafeInteger(value.updatedAt)
  )
}

function isState(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(value, ['status', 'activeRunId', 'lastError', 'updatedAt']) &&
    typeof value.status === 'string' &&
    RUN_STATUSES.has(value.status as ChatAgentRunView['status']) &&
    value.status !== 'starting' &&
    (value.activeRunId === null || isBoundedString(value.activeRunId, 1024)) &&
    (value.lastError === null || isBoundedString(value.lastError, 128 * 1024, true)) &&
    isSafeInteger(value.updatedAt)
  )
}

function isUsage(value: unknown): boolean {
  if (!isRecord(value)) return false
  const keys = [
    'inputTokens',
    'outputTokens',
    'outputThinkingTokens',
    'totalTokens',
    'cachedInputTokens',
    'cacheCreationInputTokens',
    'billableRequestCount'
  ]
  return (
    hasExactKeys(value, [], keys) && Object.values(value).every((entry) => isSafeInteger(entry))
  )
}

function isActivatedSkill(value: unknown): boolean {
  if (!isRecord(value) || !isRecord(value.source)) return false
  const source = value.source
  const validSource =
    (source.kind === 'workspace' || source.kind === 'bundled' || source.kind === 'installed') &&
    hasExactKeys(source, ['kind', 'id']) &&
    isBoundedString(source.id, 1024)
  return (
    hasExactKeys(value, ['id', 'name', 'revision', 'source']) &&
    isBoundedString(value.id, 1024) &&
    isBoundedString(value.name, 1024) &&
    isBoundedString(value.revision, 1024) &&
    validSource
  )
}

function isSkillSelection(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(value, ['id', 'revision']) &&
    isBoundedString(value.id, 1024) &&
    isBoundedString(value.revision, 1024)
  )
}

function isSkillInstallation(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(value, ['action', 'status']) &&
    isSkillInstallationRequest(value.action) &&
    [
      'waiting_for_approval',
      'installing',
      'installed',
      'already_installed',
      'rejected',
      'failed',
      'uncertain'
    ].includes(value.status as string)
  )
}

function parseCommandSessions(
  value: unknown,
  allowedCallIds: ReadonlySet<string>
): Record<string, ChatCommandSessionView> | undefined | null {
  if (value === undefined) return undefined
  if (!isRecord(value) || Object.keys(value).length > MAX_STORED_RUN_ITEMS) return null
  const sessions: Record<string, ChatCommandSessionView> = {}
  for (const [callId, raw] of Object.entries(value)) {
    if (!allowedCallIds.has(callId) || !isRecord(raw)) return null
    if (
      !hasExactKeys(
        raw,
        ['callId', 'status', 'latestSequence', 'outputTruncated'],
        ['startedAt', 'endedAt', 'exitCode', 'outputs', 'artifactObservation']
      ) ||
      raw.callId !== callId ||
      typeof raw.status !== 'string' ||
      !TERMINAL_COMMAND_STATUSES.has(raw.status as ChatCommandSessionView['status']) ||
      !isSafeInteger(raw.latestSequence) ||
      typeof raw.outputTruncated !== 'boolean' ||
      !isOptionalSafeInteger(raw, 'startedAt') ||
      !isOptionalSafeInteger(raw, 'endedAt') ||
      (hasOwn(raw, 'exitCode') && !isSafeInteger(raw.exitCode, Number.MIN_SAFE_INTEGER))
    ) {
      return null
    }
    if (hasOwn(raw, 'outputs')) {
      if (!Array.isArray(raw.outputs)) return null
      const outputKeys = ['name', 'kind', 'readPath', 'mimeType', 'sizeBytes', 'sha256'] as const
      const optionalOutputKeys = ['width', 'height'] as const
      if (
        !raw.outputs.every(
          (output) => isRecord(output) && hasExactKeys(output, outputKeys, optionalOutputKeys)
        )
      ) {
        return null
      }
    }
    const outputs = parseManagedCommandOutputs(raw.outputs)
    if (hasOwn(raw, 'outputs') && outputs === undefined) return null
    let artifactObservation: ChatCommandSessionView['artifactObservation']
    if (hasOwn(raw, 'artifactObservation')) {
      try {
        artifactObservation = parseAgentCommandArtifactObservation(raw.artifactObservation)
      } catch {
        return null
      }
    }
    sessions[callId] = {
      callId,
      status: raw.status as ChatCommandSessionView['status'],
      ...(typeof raw.startedAt === 'number' ? { startedAt: raw.startedAt } : {}),
      ...(typeof raw.endedAt === 'number' ? { endedAt: raw.endedAt } : {}),
      ...(typeof raw.exitCode === 'number' ? { exitCode: raw.exitCode } : {}),
      latestSequence: raw.latestSequence,
      outputTruncated: raw.outputTruncated,
      ...(outputs && outputs.length > 0 ? { outputs } : {}),
      ...(artifactObservation === undefined ? {} : { artifactObservation })
    }
  }
  return sessions
}

function projectDurableCommandSessions(
  value: ChatAgentRunView['commandSessions'],
  allowedCallIds: ReadonlySet<string>
): Record<string, ChatCommandSessionView> | undefined {
  if (!value) return undefined
  const sessions = Object.fromEntries(
    Object.entries(value).flatMap(([callId, session]) => {
      if (!allowedCallIds.has(callId) || !TERMINAL_COMMAND_STATUSES.has(session.status)) return []
      const outputs = parseManagedCommandOutputs(session.outputs)
      return [
        [
          callId,
          {
            callId,
            status: session.status,
            ...(session.startedAt === undefined ? {} : { startedAt: session.startedAt }),
            ...(session.endedAt === undefined ? {} : { endedAt: session.endedAt }),
            ...(session.exitCode === undefined ? {} : { exitCode: session.exitCode }),
            latestSequence: session.latestSequence,
            outputTruncated: session.outputTruncated,
            ...(outputs && outputs.length > 0 ? { outputs } : {}),
            ...(session.artifactObservation === undefined
              ? {}
              : { artifactObservation: session.artifactObservation })
          }
        ]
      ]
    })
  ) as Record<string, ChatCommandSessionView>
  return Object.keys(sessions).length > 0 ? sessions : undefined
}

/**
 * Parses the current Renderer-owned durable Agent-run projection. This is deliberately not the
 * JSON-RPC MCP event parser: the wire event has required nullable fields and diagnostics, while
 * durable chat state uses omitted optional presentation fields and never stores diagnostics.
 */
export function parsePersistedAgentRun(value: unknown): ChatAgentRunView | undefined {
  if (!isRecord(value) || !hasExactKeys(value, REQUIRED_STORED_RUN_KEYS, STORED_RUN_KEYS)) {
    return undefined
  }
  if (
    !hasOwn(value, 'runId') ||
    (value.runId !== null && !isBoundedString(value.runId, 1024)) ||
    typeof value.status !== 'string' ||
    !RUN_STATUSES.has(value.status as ChatAgentRunView['status']) ||
    !isSafeInteger(value.startedAt) ||
    !isOptionalSafeInteger(value, 'firstResponseAt') ||
    !isOptionalSafeInteger(value, 'lastResponseAt') ||
    !isOptionalSafeInteger(value, 'completedAt') ||
    !isRecordArray(value.toolDefinitions, isToolDefinition) ||
    !isRecordArray(value.toolCalls, isToolCall) ||
    !isRecordArray(value.toolResults, isToolResult) ||
    !isRecordArray(value.webSearchActivities, isWebSearchActivity) ||
    !isRecordArray(value.readActivities, isReadActivity) ||
    !isRecordArray(value.approvals, isPersistableApproval) ||
    !isRecordArray(value.diffs, isDiff) ||
    !isRecordArray(value.fileDrafts, isFileDraft) ||
    !Array.isArray(value.mcpInvocations) ||
    value.mcpInvocations.length > MAX_STORED_RUN_ITEMS ||
    !Array.isArray(value.timeline) ||
    value.timeline.length > MAX_STORED_RUN_ITEMS ||
    !isRecord(value.messageStreamCheckpoints) ||
    Object.keys(value.messageStreamCheckpoints).length > MAX_STORED_RUN_ITEMS
  ) {
    return undefined
  }
  const status = value.status as ChatAgentRunView['status']
  if (TERMINAL_RUN_STATUSES.has(status) !== hasOwn(value, 'completedAt')) return undefined
  if (hasOwn(value, 'toolSetRevision') && !isToolSetRevision(value.toolSetRevision))
    return undefined
  if (hasOwn(value, 'todo') && !isTodo(value.todo)) return undefined
  if (
    hasOwn(value, 'skillInstallations') &&
    (!Array.isArray(value.skillInstallations) ||
      value.skillInstallations.length > MAX_STORED_RUN_ITEMS ||
      !value.skillInstallations.every(isSkillInstallation))
  ) {
    return undefined
  }
  if (
    !Object.values(value.messageStreamCheckpoints).every(
      (checkpoint) =>
        isRecord(checkpoint) &&
        hasExactKeys(checkpoint, ['baseContentLength', 'baseWasThinking']) &&
        isSafeInteger(checkpoint.baseContentLength) &&
        typeof checkpoint.baseWasThinking === 'boolean'
    )
  ) {
    return undefined
  }
  if (hasOwn(value, 'state') && !isState(value.state)) return undefined
  if (hasOwn(value, 'usage') && !isUsage(value.usage)) return undefined
  if (!isOptionalBoundedString(value, 'error', 128 * 1024, true)) return undefined
  if (!isOptionalBoundedString(value, 'finishReason', 1024, true)) return undefined
  if (!isOptionalBoundedString(value, 'skillActivationRevision', 1024)) return undefined
  if (
    hasOwn(value, 'activatedSkills') &&
    (!Array.isArray(value.activatedSkills) ||
      value.activatedSkills.length > MAX_STORED_RUN_ITEMS ||
      !value.activatedSkills.every(isActivatedSkill))
  ) {
    return undefined
  }
  if (
    hasOwn(value, 'explicitSkillSelections') &&
    (!Array.isArray(value.explicitSkillSelections) ||
      value.explicitSkillSelections.length > MAX_STORED_RUN_ITEMS ||
      !value.explicitSkillSelections.every(isSkillSelection))
  ) {
    return undefined
  }

  const toolCalls = value.toolCalls as unknown as ChatAgentRunView['toolCalls']
  const toolResults = value.toolResults as unknown as ChatAgentRunView['toolResults']
  const toolCallIds = toolCalls.map((call) => call.id)
  if (!hasUniqueStrings(toolCallIds)) return undefined
  const toolResultIds = toolResults.map((result) => result.callId)
  if (!hasUniqueStrings(toolResultIds)) return undefined

  const invocations = value.mcpInvocations.map(parseStoredMcpInvocation)
  if (invocations.some((invocation) => invocation === undefined)) return undefined
  const mcpInvocations = invocations as ChatMcpToolInvocationView[]
  if (
    !hasUniqueStrings(mcpInvocations.map((invocation) => invocation.actionId)) ||
    !hasUniqueStrings(mcpInvocations.map((invocation) => invocation.invocationId)) ||
    !hasUniqueStrings(mcpInvocations.map((invocation) => invocation.callId))
  ) {
    return undefined
  }

  const timeline = value.timeline.map(parseTimelineItem)
  if (timeline.some((item) => item === undefined)) return undefined
  const typedTimeline = timeline as ChatAgentTimelineItem[]
  if (!hasUniqueStrings(typedTimeline.map((item) => item.id))) return undefined

  const mcpCallIds = new Set(mcpInvocations.map((invocation) => invocation.callId))
  const mcpInvocationIds = new Set(mcpInvocations.map((invocation) => invocation.invocationId))
  if (
    toolCallIds.some((callId) => mcpCallIds.has(callId)) ||
    toolResultIds.some((callId) => mcpCallIds.has(callId)) ||
    typedTimeline.some((item) => item.type === 'tool_call' && mcpCallIds.has(item.callId))
  ) {
    return undefined
  }
  const timelineMcpIds = typedTimeline.flatMap((item) =>
    item.type === 'mcp_tool_call' ? [item.invocationId] : []
  )
  if (
    !hasUniqueStrings(timelineMcpIds) ||
    timelineMcpIds.some((invocationId) => !mcpInvocationIds.has(invocationId)) ||
    mcpInvocations.some((invocation) => !timelineMcpIds.includes(invocation.invocationId))
  ) {
    return undefined
  }

  const runCommandCallIds = new Set(
    toolCalls.filter((call) => call.tool === 'run_command').map((call) => call.id)
  )
  const commandSessions = parseCommandSessions(value.commandSessions, runCommandCallIds)
  if (commandSessions === null) return undefined

  return {
    ...(value as unknown as ChatAgentRunView),
    toolCalls,
    toolResults,
    mcpInvocations,
    timeline: typedTimeline,
    ...(commandSessions === undefined ? {} : { commandSessions })
  }
}

export function parsePersistedAgentRunJson(
  value: string | null | undefined
): ChatAgentRunView | undefined {
  if (value === null || value === undefined) return undefined
  let parsed: unknown
  try {
    parsed = JSON.parse(value) as unknown
  } catch {
    throw new Error(STORED_AGENT_RUN_CORRUPTION_ERROR)
  }
  const run = parsePersistedAgentRun(parsed)
  if (!run) throw new Error(STORED_AGENT_RUN_CORRUPTION_ERROR)
  return run
}

/** Produces the one current, safe persisted projection and verifies it before storage. */
export function stringifyPersistedAgentRun(run: ChatAgentRunView | undefined): string | null {
  if (!run) return null
  const runCommandCallIds = new Set(
    run.toolCalls.filter((call) => call.tool === 'run_command').map((call) => call.id)
  )
  const commandSessions = projectDurableCommandSessions(run.commandSessions, runCommandCallIds)
  const persistedRun: Record<string, unknown> = {
    ...run,
    webSearchActivities: run.webSearchActivities ?? [],
    readActivities: run.readActivities ?? [],
    approvals: run.approvals.filter(
      (action): action is Exclude<AgentProposedAction, { type: 'mcp_tool_call' }> =>
        action.type !== 'mcp_tool_call'
    ),
    fileDrafts: run.fileDrafts ?? [],
    mcpInvocations: run.mcpInvocations ?? [],
    messageStreamCheckpoints: run.messageStreamCheckpoints ?? {},
    ...(commandSessions ? { commandSessions } : {})
  }
  delete persistedRun.fileWritePreviews
  delete persistedRun.commandOutputPreviews
  delete persistedRun.llmRetry
  if (!commandSessions) delete persistedRun.commandSessions

  const encoded = JSON.stringify(persistedRun)
  const canonical = JSON.parse(encoded) as unknown
  if (!parsePersistedAgentRun(canonical)) {
    throw new Error('Refusing to persist a malformed current Agent run projection')
  }
  return encoded
}
