import type { AgentProposedAction } from '@mycopilot/protocol'
import type {
  ChatAgentRunView,
  ChatAgentTimelineItem,
  ChatMcpToolInvocationView
} from '../chat/chatTypes'
import {
  isFileChangeSnapshot,
  isFileChangeProposal,
  isPersistableApproval,
  isSkillInstallationRequest,
  isToolCall,
  isToolResult
} from './persistedAgentRunApprovalValidators'
import {
  parseCommandSessions,
  projectDurableCommandSessions
} from './persistedAgentRunCommandValidators'
import { parseStoredMcpInvocation } from './persistedAgentRunMcpValidators'
import { parseTimelineItem } from './persistedAgentRunTimelineValidators'
import { projectBuiltinCapabilityToolResult } from '../agentRun/builtinCapabilityResultProjection'
import {
  MAX_STORED_RUN_ITEMS,
  hasExactKeys,
  hasOwn,
  hasUniqueStrings,
  isAgentInterruption,
  isBoundedString,
  isOptionalBoundedString,
  isOptionalSafeInteger,
  isRecord,
  isRecordArray,
  isSafeInteger
} from './persistedAgentRunValidation'

export const STORED_AGENT_RUN_CORRUPTION_ERROR = 'Stored Agent run is malformed'

const RUN_STATUSES = new Set<ChatAgentRunView['status']>([
  'starting',
  'idle',
  'queued',
  'running',
  'waiting_for_approval',
  'waiting_for_user_input',
  'completed',
  'failed',
  'cancelled'
])
const TERMINAL_RUN_STATUSES = new Set<ChatAgentRunView['status']>([
  'completed',
  'failed',
  'cancelled'
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
  'fileChangeProposals',
  'fileChanges',
  'commandSessions',
  'mcpInvocations',
  'collaborationTimelineActivities',
  'collaborationFinalResponseBoundary',
  'messageStreamCheckpoints',
  'timeline',
  'state',
  'interruption',
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
  'fileChangeProposals',
  'fileChanges',
  'mcpInvocations',
  'messageStreamCheckpoints',
  'timeline'
] as const

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

function isCollaborationTimelineActivity(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(value, [
      'activityId',
      'agentId',
      'occurredAt',
      'ownerAgentId',
      'ownerConversationId',
      'taskMessageId',
      'anchorMessageId',
      'traceBoundarySequence',
      'runId',
      'semantic',
      'sequence',
      'taskNameSnapshot',
      'turnId'
    ]) &&
    isBoundedString(value.activityId, 2048) &&
    isBoundedString(value.agentId, 256) &&
    isSafeInteger(value.occurredAt) &&
    isBoundedString(value.ownerAgentId, 256) &&
    isBoundedString(value.ownerConversationId, 2048) &&
    (value.semantic === 'updated'
      ? value.taskMessageId === null
      : isBoundedString(value.taskMessageId, 2048)) &&
    isBoundedString(value.anchorMessageId, 2048) &&
    isSafeInteger(value.traceBoundarySequence) &&
    (value.runId === null || isBoundedString(value.runId, 2048)) &&
    ['started', 'updated', 'waiting_approval', 'completed', 'failed', 'interrupted'].includes(
      value.semantic as string
    ) &&
    isSafeInteger(value.sequence) &&
    isBoundedString(value.taskNameSnapshot, 256) &&
    (value.turnId === null || isBoundedString(value.turnId, 2048))
  )
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
  // Legacy parent-owned projections cannot be attributed to a dispatcher. Drop only those
  // projections; the surrounding final answer, tools and current activities remain readable.
  const storedActivities = value.collaborationTimelineActivities
  const collaborationTimelineActivities = Array.isArray(storedActivities)
    ? storedActivities.filter(
        (activity: unknown) =>
          !(
            isRecord(activity) &&
            (hasOwn(activity, 'parentAgentId') || hasOwn(activity, 'parentConversationId')) &&
            !hasOwn(activity, 'ownerAgentId') &&
            !hasOwn(activity, 'ownerConversationId')
          )
      )
    : storedActivities
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
    !isRecordArray(value.fileChangeProposals, isFileChangeProposal) ||
    !isRecordArray(value.fileChanges, isFileChangeSnapshot) ||
    !Array.isArray(value.mcpInvocations) ||
    value.mcpInvocations.length > MAX_STORED_RUN_ITEMS ||
    (hasOwn(value, 'collaborationFinalResponseBoundary') &&
      !isSafeInteger(value.collaborationFinalResponseBoundary)) ||
    (hasOwn(value, 'collaborationTimelineActivities') &&
      (!Array.isArray(storedActivities) ||
        storedActivities.length > MAX_STORED_RUN_ITEMS ||
        !isRecordArray(collaborationTimelineActivities, isCollaborationTimelineActivity))) ||
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
        hasExactKeys(checkpoint, ['previousContent']) &&
        isBoundedString(checkpoint.previousContent, 4 * 1024 * 1024, true)
    )
  ) {
    return undefined
  }
  if (hasOwn(value, 'state') && !isState(value.state)) return undefined
  if (hasOwn(value, 'interruption') && !isAgentInterruption(value.interruption)) return undefined
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
    ...(Array.isArray(collaborationTimelineActivities)
      ? {
          collaborationTimelineActivities:
            collaborationTimelineActivities as ChatAgentRunView['collaborationTimelineActivities']
        }
      : {}),
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
  // Protected capability/risk approvals are Host-owned, just like MCP approvals. Synthetic
  // activation calls exist only to anchor the live approval UI and must not survive a Renderer
  // reload independently of the authoritative pending-action store. Browser-risk approvals never
  // own a second Tool call; the original browser call remains the durable activity anchor.
  const builtinCapabilityCallIds = new Set(
    run.approvals.flatMap((action) =>
      action.type === 'builtin_capability_activation' ? [action.approval.callId] : []
    )
  )
  const builtinCapabilityToolCallIds = new Set(
    run.timeline.flatMap((item) =>
      item.type === 'tool_call' && item.identity?.type === 'builtin_capability' ? [item.callId] : []
    )
  )
  const commandSessions = projectDurableCommandSessions(run.commandSessions, runCommandCallIds)
  const persistedRun: Record<string, unknown> = {
    ...run,
    toolCalls: run.toolCalls
      .filter((call) => !builtinCapabilityCallIds.has(call.id))
      .map((call) =>
        builtinCapabilityToolCallIds.has(call.id) ? { ...call, args: {}, reason: null } : call
      ),
    toolResults: run.toolResults
      .filter((result) => !builtinCapabilityCallIds.has(result.callId))
      .map((result) =>
        builtinCapabilityToolCallIds.has(result.callId)
          ? projectBuiltinCapabilityToolResult(result)
          : result
      ),
    webSearchActivities: run.webSearchActivities ?? [],
    readActivities: run.readActivities ?? [],
    approvals: run.approvals.filter(
      (
        action
      ): action is Exclude<
        AgentProposedAction,
        {
          type:
            | 'mcp_tool_call'
            | 'builtin_capability_activation'
            | 'builtin_mcp_tool_approval'
            | 'browser_risk_approval'
            | 'file_change'
        }
      > =>
        action.type !== 'mcp_tool_call' &&
        action.type !== 'builtin_capability_activation' &&
        action.type !== 'builtin_mcp_tool_approval' &&
        action.type !== 'browser_risk_approval' &&
        action.type !== 'file_change'
    ),
    // FileChange proposals may contain inline Diff text. Pending approvals are Host-owned and
    // rehydrated from the canonical pending-action store; ordinary chat persistence stores none.
    fileChangeProposals: [],
    fileChanges: run.fileChanges ?? [],
    mcpInvocations: run.mcpInvocations ?? [],
    messageStreamCheckpoints: run.messageStreamCheckpoints ?? {},
    timeline: run.timeline.filter(
      (item) => item.type !== 'tool_call' || !builtinCapabilityCallIds.has(item.callId)
    ),
    ...(commandSessions ? { commandSessions } : {})
  }
  delete persistedRun.fileChangePreviews
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
