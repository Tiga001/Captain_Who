// Renderer chat presentation adapter for activated Skills, Skill-owned resources, Office tools,
// and Office artifacts. This module derives UI-only views from persisted agentRun data and never
// treats display metadata as authorization or execution evidence.

import type {
  ActivatedSkillSummary,
  AgentCommandArtifactKind,
  AgentCommandArtifactScope,
  AgentManagedDocumentArtifact,
  AgentToolCall,
  AgentToolResult,
  OfficeDocumentKind,
  OfficeOperation
} from '@mycopilot/protocol'
import type { ChatAgentRunView } from './chatTypes'
import { parseManagedCommandOutputs } from './managedCommandOutputs'

export type AgentActivityStatus =
  'waiting' | 'running' | 'completed' | 'failed' | 'cancelled' | 'rejected' | 'conflict'

export type SkillResourceActivityKind = 'reference' | 'template' | 'capability'

export interface ParsedSkillResourceUri {
  resourcePath: string
  skill: ActivatedSkillSummary
}

export interface SkillResourceActivityItem {
  call: AgentToolCall
  detail: string
  kind: SkillResourceActivityKind
  resourceKey: string
  resourceName: string
  result?: AgentToolResult
  skill?: ActivatedSkillSummary
  status: AgentActivityStatus
}

export interface SkillScriptActivityView {
  call: AgentToolCall
  detail?: string
  result?: AgentToolResult
  skill?: ActivatedSkillSummary
  status: AgentActivityStatus
}

export type OfficeActivityMode = 'create' | 'edit' | 'view' | 'query' | 'validate' | 'export'

/** Renderer-only operation for the typed Host status probe, which is not an OfficeExecution
 * operation. It is presentation metadata only and never feeds execution. */
export type OfficeActivityOperation = OfficeOperation | 'status'

export type OfficeActivityCategory = 'read' | 'create' | 'edit' | 'export'

export interface OfficeActivityView {
  call: AgentToolCall
  category: OfficeActivityCategory
  detail?: string
  documentKind: OfficeDocumentKind
  error?: string
  mode: OfficeActivityMode
  operation: OfficeActivityOperation
  reason?: string
  result?: AgentToolResult
  status: AgentActivityStatus
}

export interface OfficeActivityGroupIdentity {
  category: OfficeActivityCategory
  documentKind: OfficeDocumentKind
  fileIdentity: string
  key: string
  mode: OfficeActivityMode
}

export interface OfficeArtifactEntry {
  artifactKind: AgentCommandArtifactKind
  changeKind: 'created' | 'modified' | 'replaced' | 'renamed'
  displayName?: string
  id: string
  managedReadPath?: string
  managedArtifact?: AgentManagedDocumentArtifact
  path: string
  scope: AgentCommandArtifactScope
}

const OFFICE_TOOLS = new Set<AgentToolCall['tool']>([
  'office_document',
  'office_spreadsheet',
  'office_presentation'
])

const HIDDEN_SKILL_TOOLS = new Set<AgentToolCall['tool']>([
  'skills_activate',
  'skills_list_resources',
  'skills_preflight_script'
])

function asRecord(value: unknown): Record<string, unknown> | undefined {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined
}

function stringValue(record: Record<string, unknown> | undefined, key: string): string | undefined {
  const value = record?.[key]
  return typeof value === 'string' && value.trim() ? value.trim() : undefined
}

function booleanValue(
  record: Record<string, unknown> | undefined,
  key: string
): boolean | undefined {
  const value = record?.[key]
  return typeof value === 'boolean' ? value : undefined
}

function numberValue(record: Record<string, unknown> | undefined, key: string): number | undefined {
  const value = record?.[key]
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined
}

function resultRecords(result: AgentToolResult | undefined): Record<string, unknown>[] {
  const direct = asRecord(result?.result)
  const execution = asRecord(direct?.execution)
  return [direct, execution].filter(
    (record): record is Record<string, unknown> => record !== undefined
  )
}

function firstString(records: Record<string, unknown>[], key: string): string | undefined {
  for (const record of records) {
    const value = stringValue(record, key)
    if (value) return value
  }
  return undefined
}

function firstBoolean(records: Record<string, unknown>[], key: string): boolean | undefined {
  for (const record of records) {
    const value = booleanValue(record, key)
    if (value !== undefined) return value
  }
  return undefined
}

function firstNumber(records: Record<string, unknown>[], key: string): number | undefined {
  for (const record of records) {
    const value = numberValue(record, key)
    if (value !== undefined) return value
  }
  return undefined
}

export function isHiddenSkillTool(tool: AgentToolCall['tool']): boolean {
  return HIDDEN_SKILL_TOOLS.has(tool)
}

export function isOfficeTool(tool: AgentToolCall['tool']): boolean {
  return OFFICE_TOOLS.has(tool)
}

export function getActivatedSkills(run: ChatAgentRunView): ActivatedSkillSummary[] {
  const byId = new Map<string, ActivatedSkillSummary>()
  run.activatedSkills?.forEach((skill) => {
    if (!byId.has(skill.id)) byId.set(skill.id, skill)
  })
  return [...byId.values()]
}

/**
 * Parses only the documented Skill package URI fields needed for presentation. The adapter also
 * verifies the decoded id against this run's activatedSkills before associating a display name.
 */
export function parseSkillResourceUri(
  uri: string | undefined,
  activatedSkills: ActivatedSkillSummary[]
): ParsedSkillResourceUri | undefined {
  if (!uri?.startsWith('skill://package/')) return undefined
  const remainder = uri.slice('skill://package/'.length)
  const segments = remainder.split('/')
  if (segments.length < 2 || !segments[0] || !segments[1]) return undefined

  try {
    const skillId = decodeURIComponent(segments[0])
    const skill = activatedSkills.find((candidate) => candidate.id === skillId)
    if (!skill) return undefined
    const resourcePath = segments
      .slice(2)
      .map((segment) => decodeURIComponent(segment))
      .join('/')
    return { resourcePath, skill }
  } catch {
    return undefined
  }
}

export function getToolActivityStatus(
  call: AgentToolCall,
  result: AgentToolResult | undefined,
  settledStatus?: 'completed' | 'failed' | 'cancelled',
  options: { requireResultForSuccess?: boolean } = {}
): AgentActivityStatus {
  if (!result) {
    if (call.approvalStatus === 'rejected') return 'rejected'
    if (settledStatus === 'cancelled') return 'cancelled'
    if (settledStatus === 'failed') return 'failed'
    if (settledStatus === 'completed') {
      return options.requireResultForSuccess ? 'failed' : 'completed'
    }
    return call.approvalStatus === 'required' ? 'waiting' : 'running'
  }

  const records = resultRecords(result)
  const structuredStatus = firstString(records, 'status')?.toLowerCase()

  // Some authoritative rejection/conflict results intentionally keep ok=true so the model can
  // continue. Status therefore takes precedence over the transport-level ok flag.
  if (structuredStatus === 'rejected') return 'rejected'
  if (structuredStatus === 'conflict') return 'conflict'
  if (firstBoolean(records, 'cancelled') === true) return 'cancelled'
  if (firstBoolean(records, 'timedOut') === true) return 'failed'
  if (!result.ok) return 'failed'
  if (structuredStatus === 'failed') return 'failed'
  return 'completed'
}

function getResourceUri(call: AgentToolCall, result?: AgentToolResult): string | undefined {
  const args = asRecord(call.args)
  const resultRecord = asRecord(result?.result)
  return (
    stringValue(resultRecord, 'uri') ??
    stringValue(resultRecord, 'sourceUri') ??
    stringValue(args, 'uri') ??
    stringValue(args, 'sourceUri')
  )
}

function normalizeResourcePath(path: string): string {
  return path.replace(/\\/g, '/').replace(/^\/+/, '')
}

function getResourceName(path: string): string {
  const normalized = normalizeResourcePath(path).replace(/\/+$/, '')
  return normalized.split('/').filter(Boolean).at(-1) ?? ''
}

function getResourceKind(
  path: string,
  sourcePrefix: string | undefined
): SkillResourceActivityKind {
  const normalizedPath = normalizeResourcePath(path).toLowerCase()
  const normalizedPrefix = normalizeResourcePath(sourcePrefix ?? '').toLowerCase()
  if (
    normalizedPath === 'templates' ||
    normalizedPath.startsWith('templates/') ||
    normalizedPrefix === 'templates' ||
    normalizedPrefix.startsWith('templates/')
  ) {
    return 'template'
  }
  if (normalizedPath === 'references' || normalizedPath.startsWith('references/')) {
    return 'reference'
  }
  return 'capability'
}

export function getSkillResourceActivityItem(
  run: ChatAgentRunView,
  call: AgentToolCall,
  settledStatus?: 'completed' | 'failed' | 'cancelled'
): SkillResourceActivityItem | undefined {
  if (call.tool !== 'skills_read_resource' && call.tool !== 'skills_materialize_resource') {
    return undefined
  }

  const result = run.toolResults.find((candidate) => candidate.callId === call.id)
  const args = asRecord(call.args)
  const uri = getResourceUri(call, result)
  const parsed = parseSkillResourceUri(uri, getActivatedSkills(run))
  const sourcePrefix = stringValue(args, 'sourcePrefix')
  const resourcePath = parsed?.resourcePath || sourcePrefix || ''
  const kind =
    call.tool === 'skills_materialize_resource'
      ? getResourceKind(resourcePath, sourcePrefix)
      : getResourceKind(resourcePath, undefined)
  const resourceName = getResourceName(sourcePrefix || resourcePath)
  const detail = call.reason?.trim() || resourceName

  return {
    call,
    detail,
    kind,
    resourceKey:
      call.tool === 'skills_materialize_resource' && sourcePrefix
        ? `${uri ?? parsed?.skill.id ?? 'unknown'}#${normalizeResourcePath(sourcePrefix)}`
        : uri || `${parsed?.skill.id ?? 'unknown'}:${resourcePath || call.id}`,
    resourceName,
    result,
    skill: parsed?.skill,
    status: getToolActivityStatus(call, result, settledStatus, { requireResultForSuccess: true })
  }
}

export function getSkillScriptActivityView(
  run: ChatAgentRunView,
  call: AgentToolCall,
  settledStatus?: 'completed' | 'failed' | 'cancelled'
): SkillScriptActivityView | undefined {
  if (call.tool !== 'skills_run_script') return undefined
  const result = run.toolResults.find((candidate) => candidate.callId === call.id)
  const args = asRecord(call.args)
  const resultRecord = asRecord(result?.result)
  const skillId =
    stringValue(resultRecord, 'skillId') ??
    parseSkillResourceUri(stringValue(args, 'scriptUri'), getActivatedSkills(run))?.skill.id
  const skill = getActivatedSkills(run).find((candidate) => candidate.id === skillId)
  return {
    call,
    detail: call.reason?.trim() || undefined,
    result,
    skill,
    status: getToolActivityStatus(call, result, settledStatus, { requireResultForSuccess: true })
  }
}

function officeDocumentKindForTool(tool: AgentToolCall['tool']): OfficeDocumentKind | undefined {
  if (tool === 'office_document') return 'document'
  if (tool === 'office_spreadsheet') return 'spreadsheet'
  if (tool === 'office_presentation') return 'presentation'
  return undefined
}

/**
 * Current Office calls keep the strict operation payload under `request`. Falling back to the
 * root preserves already-persisted flat typed calls, but this view helper never interprets legacy
 * provider arguments.
 */
function officeRequestArgs(value: unknown): Record<string, unknown> | undefined {
  const args = asRecord(value)
  return asRecord(args?.request) ?? args
}

function officeOperation(value: unknown): OfficeActivityOperation {
  const operations: OfficeActivityOperation[] = [
    'help',
    'status',
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
  ]
  return typeof value === 'string' && operations.includes(value as OfficeActivityOperation)
    ? (value as OfficeActivityOperation)
    : 'get'
}

function officeMode(
  operation: OfficeActivityOperation,
  outputPath: string | undefined
): OfficeActivityMode {
  if (operation === 'create') return 'create'
  if (['set', 'add', 'remove', 'move', 'swap'].includes(operation)) return 'edit'
  if (operation === 'validate') return 'validate'
  if (operation === 'query') return 'query'
  if (operation === 'view' && outputPath) return 'export'
  return 'view'
}

function officeActivityCategory(mode: OfficeActivityMode): OfficeActivityCategory {
  if (mode === 'view' || mode === 'query' || mode === 'validate') return 'read'
  return mode
}

export function getOfficeActivityGroupIdentity(
  call: AgentToolCall
): OfficeActivityGroupIdentity | undefined {
  const documentKind = officeDocumentKindForTool(call.tool)
  if (!documentKind) return undefined
  const args = officeRequestArgs(call.args)
  const operation = officeOperation(args?.operation)
  const mode = officeMode(operation, stringValue(args, 'outputPath'))
  const category = officeActivityCategory(mode)
  const sourcePath =
    stringValue(args, 'filePath') ?? stringValue(args, 'documentPath') ?? stringValue(args, 'path')
  const destinationPath = stringValue(args, 'destinationPath')
  const outputPath = stringValue(args, 'outputPath')
  const logicalPath =
    category === 'edit'
      ? (destinationPath ?? sourcePath)
      : category === 'export'
        ? (outputPath ?? sourcePath)
        : sourcePath
  // Consecutive information reads are one user-visible check even when OfficeCLI consults
  // multiple files or starts with pathless help/status probes. File-writing activities retain a
  // normalized target identity so unrelated create/edit/export work never shares a card.
  const fileIdentity =
    category === 'read'
      ? 'read-check'
      : logicalPath
        ? normalizedArtifactPath(logicalPath)
        : `call:${call.id}`

  return {
    category,
    documentKind,
    fileIdentity,
    key: `${documentKind}:${category}:${fileIdentity}`,
    mode
  }
}

export function getOfficeActivityView(
  run: ChatAgentRunView,
  call: AgentToolCall,
  settledStatus?: 'completed' | 'failed' | 'cancelled'
): OfficeActivityView | undefined {
  const documentKind = officeDocumentKindForTool(call.tool)
  if (!documentKind) return undefined
  const result = run.toolResults.find((candidate) => candidate.callId === call.id)
  const args = officeRequestArgs(call.args)
  const operation = officeOperation(args?.operation)
  const mode = officeMode(operation, stringValue(args, 'outputPath'))
  const status = getToolActivityStatus(call, result, settledStatus, {
    requireResultForSuccess: true
  })
  const records = resultRecords(result)
  const resultError =
    result?.error ?? firstString(records, 'error') ?? firstString(records, 'message')
  const reason = call.reason?.trim() || undefined

  return {
    call,
    category: officeActivityCategory(mode),
    detail: reason,
    documentKind,
    error: resultError,
    mode,
    operation,
    reason,
    result,
    status
  }
}

function normalizedArtifactPath(path: string): string {
  const normalized = path.replace(/\\/g, '/').replace(/^\.\//, '')
  return normalized.replace(/\/{2,}/g, '/')
}

function artifactKindForPath(path: string): AgentCommandArtifactKind | undefined {
  const extension = path.split('.').at(-1)?.toLowerCase()
  if (extension === 'docx') return 'document'
  if (extension === 'xlsx' || extension === 'xlsm' || extension === 'csv') return 'spreadsheet'
  if (extension === 'pptx') return 'presentation'
  return undefined
}

function validArtifactMetadata(
  metadata: Record<string, unknown> | undefined,
  path: string
): boolean {
  const validation = asRecord(metadata?.validation)
  const status = stringValue(validation, 'status')
  if (status === 'valid') return true
  return status === 'not_applicable' && path.toLowerCase().endsWith('.csv')
}

function isSuccessfulCommandResult(result: AgentToolResult): boolean {
  if (!result.ok) return false
  const records = resultRecords(result)
  const status = firstString(records, 'status')?.toLowerCase()
  if (status === 'rejected' || status === 'conflict' || status === 'failed') return false
  if (firstBoolean(records, 'cancelled') === true || firstBoolean(records, 'timedOut') === true) {
    return false
  }
  const exitCode = firstNumber(records, 'exitCode')
  return exitCode === undefined || exitCode === 0
}

function getArtifactObservation(result: AgentToolResult): Record<string, unknown> | undefined {
  const payload = asRecord(result.result)
  return (
    asRecord(payload?.artifactObservation) ??
    asRecord(asRecord(payload?.execution)?.artifactObservation)
  )
}

function setArtifactEntry(
  entries: Map<string, OfficeArtifactEntry>,
  entry: Omit<OfficeArtifactEntry, 'id'>
): void {
  const key = `${entry.scope}:${normalizedArtifactPath(entry.path)}`
  entries.set(key, { ...entry, id: key })
}

function artifactScopeForPath(path: string, explicitScope?: string): AgentCommandArtifactScope {
  if (explicitScope === 'workspace' || explicitScope === 'external') return explicitScope
  return isAbsoluteLocalPath(path) || hasUnresolvedPathAlias(path) ? 'external' : 'workspace'
}

function collectObservedArtifacts(
  entries: Map<string, OfficeArtifactEntry>,
  result: AgentToolResult
): void {
  if (!isSuccessfulCommandResult(result)) return
  collectArtifactObservation(entries, getArtifactObservation(result))
}

function collectArtifactObservation(
  entries: Map<string, OfficeArtifactEntry>,
  observation: Record<string, unknown> | undefined
): void {
  const observationStatus = stringValue(observation, 'status')
  if (!observation || observationStatus === 'failed') return

  const changes = Array.isArray(observation.changes) ? observation.changes : []
  changes.forEach((value) => {
    const change = asRecord(value)
    const kind = stringValue(change, 'kind')
    const path = stringValue(change, 'path')
    const scope = stringValue(change, 'scope')
    if (!path || (scope !== 'workspace' && scope !== 'external')) return
    if (kind === 'deleted') {
      entries.delete(`${scope}:${normalizedArtifactPath(path)}`)
      return
    }
    if (!['created', 'modified', 'replaced', 'renamed'].includes(kind ?? '')) return
    const artifactKind = stringValue(change, 'artifactKind') ?? artifactKindForPath(path)
    if (!['document', 'spreadsheet', 'presentation'].includes(artifactKind ?? '')) return
    if (!validArtifactMetadata(asRecord(change?.after), path)) return
    if (kind === 'renamed') {
      const previousPath = stringValue(change, 'previousPath')
      const previousScope = stringValue(change, 'previousScope')
      if (previousPath && (previousScope === 'workspace' || previousScope === 'external')) {
        entries.delete(`${previousScope}:${normalizedArtifactPath(previousPath)}`)
      }
    }
    setArtifactEntry(entries, {
      artifactKind: artifactKind as AgentCommandArtifactKind,
      changeKind: kind as OfficeArtifactEntry['changeKind'],
      path: normalizedArtifactPath(path),
      scope
    })
  })

  const expectedOutputs = Array.isArray(observation.expectedOutputs)
    ? observation.expectedOutputs
    : []
  expectedOutputs.forEach((value) => {
    const expected = asRecord(value)
    const outcome = stringValue(expected, 'outcome')
    const path = stringValue(expected, 'path')
    const explicitScope = stringValue(expected, 'scope')
    if (
      !path ||
      !['created', 'modified', 'replaced', 'renamed'].includes(outcome ?? '') ||
      !validArtifactMetadata(asRecord(expected?.metadata), path)
    ) {
      return
    }
    const scope = artifactScopeForPath(path, explicitScope)
    const artifactKind = stringValue(expected, 'artifactKind') ?? artifactKindForPath(path)
    if (!['document', 'spreadsheet', 'presentation'].includes(artifactKind ?? '')) return
    setArtifactEntry(entries, {
      artifactKind: artifactKind as AgentCommandArtifactKind,
      changeKind: outcome as OfficeArtifactEntry['changeKind'],
      path: normalizedArtifactPath(path),
      scope
    })
  })
}

function collectNativeOfficeArtifact(
  entries: Map<string, OfficeArtifactEntry>,
  call: AgentToolCall,
  result: AgentToolResult
): void {
  if (!result.ok || getToolActivityStatus(call, result) !== 'completed') return
  const documentKind = officeDocumentKindForTool(call.tool)
  if (!documentKind) return
  const args = officeRequestArgs(call.args)
  const operation = officeOperation(args?.operation)
  if (!['create', 'set', 'add', 'remove', 'move', 'swap'].includes(operation)) return
  const path =
    operation === 'create'
      ? (stringValue(args, 'filePath') ??
        stringValue(args, 'documentPath') ??
        stringValue(args, 'path'))
      : (stringValue(args, 'destinationPath') ??
        stringValue(args, 'filePath') ??
        stringValue(args, 'documentPath') ??
        stringValue(args, 'path'))
  if (!path) return
  const normalizedPath = normalizedArtifactPath(path)
  const scope = artifactScopeForPath(normalizedPath)
  setArtifactEntry(entries, {
    artifactKind: documentKind,
    changeKind: operation === 'create' ? 'created' : 'modified',
    path: normalizedPath,
    scope
  })
}

function collectManagedCommandDocuments(
  entries: Map<string, OfficeArtifactEntry>,
  call: AgentToolCall,
  result: AgentToolResult
): void {
  if ((call.tool !== 'run_command' && call.tool !== 'command_session') || !result.ok) return
  const payload = asRecord(result.result)
  const execution = asRecord(payload?.execution)
  const outputs = Array.isArray(execution?.outputs)
    ? execution.outputs
    : Array.isArray(payload?.outputs)
      ? payload.outputs
      : []
  collectManagedCommandDocumentsFromOutputs(entries, outputs)
}

function collectManagedCommandDocumentsFromOutputs(
  entries: Map<string, OfficeArtifactEntry>,
  value: unknown
): void {
  const outputs = parseManagedCommandOutputs(value) ?? []
  outputs.forEach((output) => {
    if (output.kind !== 'document') return
    const { name, readPath } = output
    const id = `managed:${readPath}`
    entries.set(id, {
      artifactKind: 'document',
      changeKind: 'created',
      displayName: name,
      id,
      managedReadPath: readPath,
      managedArtifact: {
        artifactId: `sha256:${output.sha256}`,
        uri: readPath,
        kind: 'document',
        format: 'pdf',
        mimeType: 'application/pdf',
        sizeBytes: output.sizeBytes,
        sha256: output.sha256
      },
      path: readPath,
      scope: 'external'
    })
  })
}

export function getOfficeArtifactEntries(run: ChatAgentRunView): OfficeArtifactEntry[] {
  const entries = new Map<string, OfficeArtifactEntry>()

  run.toolCalls.forEach((call) => {
    const result = run.toolResults.find((candidate) => candidate.callId === call.id)
    if (result) {
      if (call.tool === 'run_command') collectObservedArtifacts(entries, result)
      if (isOfficeTool(call.tool)) collectNativeOfficeArtifact(entries, call, result)
      collectManagedCommandDocuments(entries, call, result)
    }
    if (call.tool === 'run_command') {
      const session = run.commandSessions?.[call.id]
      if (session?.status === 'exited' && session.exitCode === 0) {
        collectArtifactObservation(entries, asRecord(session.artifactObservation))
      }
      collectManagedCommandDocumentsFromOutputs(entries, run.commandSessions?.[call.id]?.outputs)
    }
  })

  return [...entries.values()]
}

export function getOfficeArtifactFileName(path: string): string {
  return normalizedArtifactPath(path).split('/').filter(Boolean).at(-1) ?? path
}

export function hasUnresolvedPathAlias(path: string): boolean {
  return /^@[A-Za-z][A-Za-z0-9_-]*(?:\/|$)/.test(path.trim())
}

export function isAbsoluteLocalPath(path: string): boolean {
  return path.startsWith('/') || /^[A-Za-z]:[\\/]/.test(path)
}
