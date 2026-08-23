import {
  AUTOMATION_PERMISSION_MODE_VERSION,
  AUTOMATION_SCHEMA_VERSION,
  parseAutomationErrorData,
  type AutomationAttentionAcknowledgeOutput,
  type AutomationAttentionSummaryOutput,
  type AutomationCreateInput,
  type AutomationDeleteOutput,
  type AutomationEvent,
  type AutomationListOutput,
  type AutomationResync,
  type AutomationRun,
  type AutomationRunsListOutput,
  type AutomationTask
} from '@mycopilot/protocol'
import { HostInvocationError, unwrapHostInvocation } from '@mycopilot/host-api'
import { hostClient } from '../../host/hostClient'
import type {
  AutomationAttentionQuery,
  AutomationDraft,
  AutomationErrorDetails,
  AutomationListQuery,
  AutomationMutationTarget,
  AutomationRunsQuery,
  AutomationUpdateDraft
} from './automationTypes'

const DEFAULT_PAGE_SIZE = 50
const MAX_PAGE_SIZE = 100

export function hasAutomationHostApi(): boolean {
  const api = (
    hostClient as unknown as {
      automations?: Partial<typeof hostClient.automations>
    }
  ).automations
  return Boolean(
    api?.list &&
    api.get &&
    api.create &&
    api.update &&
    api.setEnabled &&
    api.runNow &&
    api.delete &&
    api.listRuns &&
    api.attentionSummary &&
    api.acknowledgeAttention &&
    api.onEvent &&
    api.onResync
  )
}

export class AutomationClientError extends Error implements AutomationErrorDetails {
  readonly code: AutomationErrorDetails['code']
  readonly automationId: string | null
  readonly currentRevision: number | null
  readonly field: string | null
  readonly retryable: boolean

  constructor(details: AutomationErrorDetails, options?: ErrorOptions) {
    super(details.message, options)
    this.name = 'AutomationClientError'
    this.code = details.code
    this.automationId = details.automationId
    this.currentRevision = details.currentRevision
    this.field = details.field
    this.retryable = details.retryable
  }
}

export function getAutomationErrorDetails(error: unknown): AutomationErrorDetails {
  if (error instanceof AutomationClientError) return error
  if (error instanceof HostInvocationError) {
    try {
      const parsed = parseAutomationErrorData(error.data)
      return {
        code: parsed.code,
        message: parsed.message,
        automationId: parsed.automationId,
        currentRevision: parsed.currentRevision,
        field: parsed.field,
        retryable: parsed.retryable
      }
    } catch {
      return {
        code: 'transport',
        message: error.message,
        automationId: null,
        currentRevision: null,
        field: null,
        retryable: false
      }
    }
  }
  return {
    code: 'transport',
    message: error instanceof Error ? error.message : String(error),
    automationId: null,
    currentRevision: null,
    field: null,
    retryable: true
  }
}

function asAutomationClientError(error: unknown): AutomationClientError {
  return error instanceof AutomationClientError
    ? error
    : new AutomationClientError(getAutomationErrorDetails(error), {
        cause: error instanceof Error ? error : undefined
      })
}

async function invoke<T>(
  operation: () => Promise<Parameters<typeof unwrapHostInvocation<T>>[0]>
): Promise<T> {
  try {
    return unwrapHostInvocation(await operation())
  } catch (error) {
    throw asAutomationClientError(error)
  }
}

function pageSize(limit: number | undefined): number {
  if (limit === undefined) return DEFAULT_PAGE_SIZE
  return Math.max(1, Math.min(MAX_PAGE_SIZE, Math.trunc(limit)))
}

export function createAutomationRequestId(): string {
  const randomUuid = globalThis.crypto?.randomUUID?.bind(globalThis.crypto)
  if (randomUuid) return randomUuid()
  // The fallback is only for older test/webview runtimes. Core still owns business IDs.
  return `renderer-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`
}

export async function listAutomations(
  query: AutomationListQuery = {}
): Promise<AutomationListOutput> {
  return invoke(() =>
    hostClient.automations.list({
      schemaVersion: AUTOMATION_SCHEMA_VERSION,
      ...(query.status ? { status: query.status } : {}),
      ...(query.query?.trim() ? { query: query.query.trim() } : {}),
      ...(query.cursor ? { cursor: query.cursor } : {}),
      limit: pageSize(query.limit)
    })
  )
}

export async function getAutomation(automationId: string): Promise<AutomationTask> {
  return invoke(() =>
    hostClient.automations.get({ schemaVersion: AUTOMATION_SCHEMA_VERSION, automationId })
  )
}

export async function createAutomation(
  draft: AutomationDraft,
  requestId = createAutomationRequestId()
): Promise<AutomationTask> {
  const input: AutomationCreateInput = {
    schemaVersion: AUTOMATION_SCHEMA_VERSION,
    requestId,
    status: draft.status,
    title: draft.title,
    prompt: draft.prompt,
    destination: draft.destination,
    permissionMode: draft.permissionMode,
    permissionModeVersion: AUTOMATION_PERMISSION_MODE_VERSION,
    schedule: draft.schedule,
    notificationPolicy: draft.notificationPolicy
  }
  return invoke(() => hostClient.automations.create(input))
}

export async function updateAutomation(
  target: AutomationMutationTarget,
  draft: AutomationUpdateDraft
): Promise<AutomationTask> {
  return invoke(() =>
    hostClient.automations.update({
      schemaVersion: AUTOMATION_SCHEMA_VERSION,
      automationId: target.automationId,
      expectedRevision: target.revision,
      title: draft.title,
      prompt: draft.prompt,
      destination: draft.destination,
      permissionMode: draft.permissionMode,
      permissionModeVersion: AUTOMATION_PERMISSION_MODE_VERSION,
      schedule: draft.schedule,
      notificationPolicy: draft.notificationPolicy
    })
  )
}

export async function setAutomationEnabled(
  target: AutomationMutationTarget,
  enabled: boolean
): Promise<AutomationTask> {
  return invoke(() =>
    hostClient.automations.setEnabled({
      schemaVersion: AUTOMATION_SCHEMA_VERSION,
      automationId: target.automationId,
      expectedRevision: target.revision,
      enabled
    })
  )
}

export async function runAutomationNow(
  automationId: string,
  requestId = createAutomationRequestId()
): Promise<AutomationRun> {
  return invoke(() =>
    hostClient.automations.runNow({
      schemaVersion: AUTOMATION_SCHEMA_VERSION,
      automationId,
      requestId
    })
  )
}

export async function deleteAutomation(
  target: AutomationMutationTarget
): Promise<AutomationDeleteOutput> {
  return invoke(() =>
    hostClient.automations.delete({
      schemaVersion: AUTOMATION_SCHEMA_VERSION,
      automationId: target.automationId,
      expectedRevision: target.revision
    })
  )
}

export async function listAutomationRuns(
  automationId: string,
  query: AutomationRunsQuery = {}
): Promise<AutomationRunsListOutput> {
  return invoke(() =>
    hostClient.automations.listRuns({
      schemaVersion: AUTOMATION_SCHEMA_VERSION,
      automationId,
      ...(query.cursor ? { cursor: query.cursor } : {}),
      limit: pageSize(query.limit)
    })
  )
}

export async function getAutomationAttention(
  query: AutomationAttentionQuery = {}
): Promise<AutomationAttentionSummaryOutput> {
  return invoke(() =>
    hostClient.automations.attentionSummary({
      schemaVersion: AUTOMATION_SCHEMA_VERSION,
      ...(query.cursor ? { cursor: query.cursor } : {}),
      limit: pageSize(query.limit)
    })
  )
}

export async function acknowledgeAutomationAttention(
  attentionId: string
): Promise<AutomationAttentionAcknowledgeOutput> {
  return invoke(() =>
    hostClient.automations.acknowledgeAttention({
      schemaVersion: AUTOMATION_SCHEMA_VERSION,
      attentionId
    })
  )
}

export function onAutomationEvent(handler: (event: AutomationEvent) => void): () => void {
  return hostClient.automations.onEvent(handler)
}

export function onAutomationResync(handler: (event: AutomationResync) => void): () => void {
  return hostClient.automations.onResync(handler)
}
