import type {
  AutomationAttentionAcknowledgeInput,
  AutomationAttentionAcknowledgeOutput,
  AutomationAttentionSummaryInput,
  AutomationAttentionSummaryOutput,
  AutomationCreateInput,
  AutomationDeleteInput,
  AutomationDeleteOutput,
  AutomationEvent,
  AutomationGetInput,
  AutomationListInput,
  AutomationListOutput,
  AutomationResync,
  AutomationRun,
  AutomationRunNowInput,
  AutomationRunsListInput,
  AutomationRunsListOutput,
  AutomationSetEnabledInput,
  AutomationTask,
  AutomationUpdateInput,
  CorePingRequest,
  CorePingResponse,
  CoreShutdownResponse,
  NotificationBatchAcknowledgeInput,
  NotificationBatchAcknowledgeOutput,
  NotificationBatchReleaseInput,
  NotificationBatchReleaseOutput,
  NotificationBatchSuppressInput,
  NotificationBatchSuppressOutput,
  NotificationBatchesClaimInput,
  NotificationBatchesClaimOutput,
  NotificationBatchValidateInput,
  NotificationBatchValidateOutput,
  NotificationEvent,
  NotificationListInput,
  NotificationListOutput,
  NotificationMarkSeenInput,
  NotificationMarkSeenOutput,
  NotificationResync,
  NotificationSettingsGetInput,
  NotificationSettingsGetOutput,
  NotificationSettingsUpdateInput,
  NotificationSettingsUpdateOutput,
  NotificationSummaryInput,
  NotificationSummaryOutput
} from '@mycopilot/protocol'
import {
  AUTOMATION_ATTENTION_ACKNOWLEDGE_METHOD,
  AUTOMATION_ATTENTION_SUMMARY_METHOD,
  AUTOMATION_CREATE_METHOD,
  AUTOMATION_DELETE_METHOD,
  AUTOMATION_ERROR_CODE,
  AUTOMATION_EVENT_NOTIFICATION_METHOD,
  AUTOMATION_GET_METHOD,
  AUTOMATION_LIST_METHOD,
  AUTOMATION_RESYNC_NOTIFICATION_METHOD,
  AUTOMATION_RUN_NOW_METHOD,
  AUTOMATION_RUNS_LIST_METHOD,
  AUTOMATION_SET_ENABLED_METHOD,
  AUTOMATION_UPDATE_METHOD,
  NOTIFICATION_BATCH_ACKNOWLEDGE_METHOD,
  NOTIFICATION_BATCH_RELEASE_METHOD,
  NOTIFICATION_BATCH_SUPPRESS_METHOD,
  NOTIFICATION_BATCH_VALIDATE_METHOD,
  NOTIFICATION_BATCHES_CLAIM_METHOD,
  NOTIFICATION_EVENT_NOTIFICATION_METHOD,
  NOTIFICATION_LIST_METHOD,
  NOTIFICATION_MARK_SEEN_METHOD,
  NOTIFICATION_RESYNC_NOTIFICATION_METHOD,
  NOTIFICATION_SETTINGS_GET_METHOD,
  NOTIFICATION_SETTINGS_UPDATE_METHOD,
  NOTIFICATION_SUMMARY_METHOD,
  parseAutomationAttentionAcknowledgeInput,
  parseAutomationAttentionAcknowledgeOutput,
  parseAutomationAttentionSummaryInput,
  parseAutomationAttentionSummaryOutput,
  parseAutomationCreateInput,
  parseAutomationDeleteInput,
  parseAutomationDeleteOutput,
  parseAutomationErrorData,
  parseAutomationEvent,
  parseAutomationGetInput,
  parseAutomationListInput,
  parseAutomationListOutput,
  parseAutomationResync,
  parseAutomationRun,
  parseAutomationRunNowInput,
  parseAutomationRunsListInput,
  parseAutomationRunsListOutput,
  parseAutomationSetEnabledInput,
  parseAutomationTask,
  parseAutomationUpdateInput,
  parseNotificationBatchAcknowledgeInput,
  parseNotificationBatchAcknowledgeOutput,
  parseNotificationBatchReleaseInput,
  parseNotificationBatchReleaseOutput,
  parseNotificationBatchSuppressInput,
  parseNotificationBatchSuppressOutput,
  parseNotificationBatchesClaimInput,
  parseNotificationBatchesClaimOutput,
  parseNotificationBatchValidateInput,
  parseNotificationBatchValidateOutput,
  parseNotificationEvent,
  parseNotificationListInput,
  parseNotificationListOutput,
  parseNotificationMarkSeenInput,
  parseNotificationMarkSeenOutput,
  parseNotificationResync,
  parseNotificationSettingsGetInput,
  parseNotificationSettingsGetOutput,
  parseNotificationSettingsUpdateInput,
  parseNotificationSettingsUpdateOutput,
  parseNotificationSummaryInput,
  parseNotificationSummaryOutput
} from '@mycopilot/protocol'

import { CoreServerHumanInteractionApi } from './coreServerHumanInteractionApi'
import { CoreJsonRpcClient } from './jsonRpcClient'

const CORE_PING_METHOD = 'core.ping'
const CORE_SHUTDOWN_METHOD = 'core.shutdown'

function rethrowValidatedAutomationError(error: unknown): never {
  if (
    typeof error !== 'object' ||
    error === null ||
    Array.isArray(error) ||
    !('code' in error) ||
    error.code !== AUTOMATION_ERROR_CODE
  ) {
    throw error
  }

  const data = parseAutomationErrorData('data' in error ? error.data : undefined)
  throw Object.assign(new Error(data.message), {
    name: 'AutomationError',
    code: AUTOMATION_ERROR_CODE,
    data
  })
}

export interface CoreServerOptions {
  /**
   * The Electron Host-owned application data root. Production construction must inject the
   * value frozen from app.getPath('userData'); the optional fallback keeps isolated unit-test
   * construction and non-entrypoint consumers source-compatible.
   */
  appDataRoot?: string
}

/** Sole owner of the Core JSON-RPC process lifecycle; request families live in stateless bases. */
export class CoreServer extends CoreServerHumanInteractionApi {
  private readonly automationResyncHandlers = new Set<(event: AutomationResync) => void>()
  private automationResyncSubscription: (() => void) | null = null
  private latestAutomationResync: AutomationResync | null = null
  private readonly notificationResyncHandlers = new Set<(event: NotificationResync) => void>()
  private notificationResyncSubscription: (() => void) | null = null
  private latestNotificationResync: NotificationResync | null = null

  constructor(options: CoreServerOptions = {}) {
    super(
      options.appDataRoot === undefined
        ? new CoreJsonRpcClient()
        : new CoreJsonRpcClient({ appDataRoot: options.appDataRoot })
    )
  }

  start(): void {
    this.ensureAutomationResyncSubscription()
    this.ensureNotificationResyncSubscription()
    this.rpc.start()
  }

  stop(): void {
    this.rpc.stop()
    this.latestAutomationResync = null
    this.latestNotificationResync = null
  }

  async shutdown(): Promise<void> {
    // Fence lazy process restart before inspecting the current child. Notification delivery and
    // other late Host producers may still have an in-flight promise, but none can start a fresh
    // scheduler after the shutdown sequence has begun.
    this.rpc.beginShutdown()
    if (!this.rpc.isRunning()) return

    // Core first gives the managed Playwright bridge up to two seconds to settle, then shuts down
    // the remaining Agent/MCP services in parallel under their own two-second bounds. Keep the
    // Host watchdog larger than that composed budget so it does not kill Core in the middle of
    // external MCP or Agent cleanup. This remains a hard upper bound for application exit.
    const hostShutdownTimeoutMs = 6_000
    let timeoutId: ReturnType<typeof setTimeout> | null = null
    const timeout = new Promise<void>((resolve) => {
      timeoutId = setTimeout(resolve, hostShutdownTimeoutMs)
      timeoutId.unref()
    })
    const shutdown = this.rpc
      .requestDuringShutdown<CoreShutdownResponse>(CORE_SHUTDOWN_METHOD)
      .then((response) => {
        if (response.timedOut) {
          console.warn('core-server shutdown timed out while waiting for active agent runs')
        }
      })
      .catch((error) => {
        console.warn('Failed to request core-server shutdown', error)
      })

    await Promise.race([shutdown, timeout])
    if (timeoutId) clearTimeout(timeoutId)
    this.rpc.stop()
  }

  ping(input?: CorePingRequest): Promise<CorePingResponse> {
    return this.rpc.request<CorePingResponse, CorePingRequest>(CORE_PING_METHOD, input ?? {})
  }

  listAutomations(input: AutomationListInput): Promise<AutomationListOutput> {
    const request = parseAutomationListInput(input)
    return this.rpc
      .request<unknown, AutomationListInput>(AUTOMATION_LIST_METHOD, request)
      .then(parseAutomationListOutput)
      .catch(rethrowValidatedAutomationError)
  }

  getAutomation(input: AutomationGetInput): Promise<AutomationTask> {
    const request = parseAutomationGetInput(input)
    return this.rpc
      .request<unknown, AutomationGetInput>(AUTOMATION_GET_METHOD, request)
      .then((value) => {
        const output = parseAutomationTask(value)
        if (output.automationId !== request.automationId)
          throw new Error('Invalid Automation task response identity')
        return output
      })
      .catch(rethrowValidatedAutomationError)
  }

  createAutomation(input: AutomationCreateInput): Promise<AutomationTask> {
    const request = parseAutomationCreateInput(input)
    return this.rpc
      .request<unknown, AutomationCreateInput>(AUTOMATION_CREATE_METHOD, request)
      .then(parseAutomationTask)
      .catch(rethrowValidatedAutomationError)
  }

  updateAutomation(input: AutomationUpdateInput): Promise<AutomationTask> {
    const request = parseAutomationUpdateInput(input)
    return this.rpc
      .request<unknown, AutomationUpdateInput>(AUTOMATION_UPDATE_METHOD, request)
      .then((value) => {
        const output = parseAutomationTask(value)
        if (output.automationId !== request.automationId)
          throw new Error('Invalid Automation task response identity')
        return output
      })
      .catch(rethrowValidatedAutomationError)
  }

  setAutomationEnabled(input: AutomationSetEnabledInput): Promise<AutomationTask> {
    const request = parseAutomationSetEnabledInput(input)
    return this.rpc
      .request<unknown, AutomationSetEnabledInput>(AUTOMATION_SET_ENABLED_METHOD, request)
      .then((value) => {
        const output = parseAutomationTask(value)
        if (output.automationId !== request.automationId)
          throw new Error('Invalid Automation task response identity')
        return output
      })
      .catch(rethrowValidatedAutomationError)
  }

  runAutomationNow(input: AutomationRunNowInput): Promise<AutomationRun> {
    const request = parseAutomationRunNowInput(input)
    return this.rpc
      .request<unknown, AutomationRunNowInput>(AUTOMATION_RUN_NOW_METHOD, request)
      .then((value) => {
        const output = parseAutomationRun(value)
        if (output.automationId !== request.automationId) {
          throw new Error('Invalid Automation run response identity')
        }
        return output
      })
      .catch(rethrowValidatedAutomationError)
  }

  deleteAutomation(input: AutomationDeleteInput): Promise<AutomationDeleteOutput> {
    const request = parseAutomationDeleteInput(input)
    return this.rpc
      .request<unknown, AutomationDeleteInput>(AUTOMATION_DELETE_METHOD, request)
      .then((value) => {
        const output = parseAutomationDeleteOutput(value)
        if (output.automationId !== request.automationId)
          throw new Error('Invalid Automation delete response identity')
        return output
      })
      .catch(rethrowValidatedAutomationError)
  }

  listAutomationRuns(input: AutomationRunsListInput): Promise<AutomationRunsListOutput> {
    const request = parseAutomationRunsListInput(input)
    return this.rpc
      .request<unknown, AutomationRunsListInput>(AUTOMATION_RUNS_LIST_METHOD, request)
      .then((value) => {
        const output = parseAutomationRunsListOutput(value)
        if (output.automationId !== request.automationId)
          throw new Error('Invalid Automation runs response identity')
        return output
      })
      .catch(rethrowValidatedAutomationError)
  }

  getAutomationAttentionSummary(
    input: AutomationAttentionSummaryInput
  ): Promise<AutomationAttentionSummaryOutput> {
    const request = parseAutomationAttentionSummaryInput(input)
    return this.rpc
      .request<unknown, AutomationAttentionSummaryInput>(
        AUTOMATION_ATTENTION_SUMMARY_METHOD,
        request
      )
      .then(parseAutomationAttentionSummaryOutput)
      .catch(rethrowValidatedAutomationError)
  }

  acknowledgeAutomationAttention(
    input: AutomationAttentionAcknowledgeInput
  ): Promise<AutomationAttentionAcknowledgeOutput> {
    const request = parseAutomationAttentionAcknowledgeInput(input)
    return this.rpc
      .request<unknown, AutomationAttentionAcknowledgeInput>(
        AUTOMATION_ATTENTION_ACKNOWLEDGE_METHOD,
        request
      )
      .then((value) => {
        const output = parseAutomationAttentionAcknowledgeOutput(value)
        if (output.attention.attentionId !== request.attentionId)
          throw new Error('Invalid Automation attention response identity')
        return output
      })
      .catch(rethrowValidatedAutomationError)
  }

  /** Host-only durable batch claim; Renderer cannot mutate native delivery state. */
  claimNotificationBatches(
    input: NotificationBatchesClaimInput
  ): Promise<NotificationBatchesClaimOutput> {
    const request = parseNotificationBatchesClaimInput(input)
    return this.rpc
      .request<unknown, NotificationBatchesClaimInput>(NOTIFICATION_BATCHES_CLAIM_METHOD, request)
      .then((value) => {
        const output = parseNotificationBatchesClaimOutput(value)
        if (output.claimToken !== request.claimToken) {
          throw new Error('Invalid notification batch claim response identity')
        }
        if (output.batches.some((batch) => batch.status !== 'claimed')) {
          throw new Error('Invalid notification batch claim response status')
        }
        const batchIds = new Set(output.batches.map((batch) => batch.batchId))
        if (batchIds.size !== output.batches.length) {
          throw new Error('Invalid duplicate notification batch claim response')
        }
        return output
      })
  }

  /** Host-only final authority check immediately before native presentation. */
  validateNotificationBatch(
    input: NotificationBatchValidateInput
  ): Promise<NotificationBatchValidateOutput> {
    const request = parseNotificationBatchValidateInput(input)
    return this.rpc
      .request<unknown, NotificationBatchValidateInput>(NOTIFICATION_BATCH_VALIDATE_METHOD, request)
      .then((value) => {
        const output = parseNotificationBatchValidateOutput(value)
        if (
          output.batchId !== request.batchId ||
          (output.batch !== null && output.batch.batchId !== request.batchId)
        ) {
          throw new Error('Invalid notification batch validation response identity')
        }
        if (output.batch !== null && output.batch.status !== 'claimed') {
          throw new Error('Invalid notification batch validation response status')
        }
        return output
      })
  }

  acknowledgeNotificationBatch(
    input: NotificationBatchAcknowledgeInput
  ): Promise<NotificationBatchAcknowledgeOutput> {
    const request = parseNotificationBatchAcknowledgeInput(input)
    return this.rpc
      .request<unknown, NotificationBatchAcknowledgeInput>(
        NOTIFICATION_BATCH_ACKNOWLEDGE_METHOD,
        request
      )
      .then((value) => {
        const output = parseNotificationBatchAcknowledgeOutput(value)
        if (output.batchId !== request.batchId || output.disposition !== request.disposition) {
          throw new Error('Invalid notification batch acknowledge response identity')
        }
        const statusMatchesDisposition =
          request.disposition === 'delivered'
            ? output.status === 'pending' ||
              output.status === 'displayed' ||
              output.status === 'sealed'
            : output.status === 'suppressed'
        if (!statusMatchesDisposition) {
          throw new Error('Invalid notification batch acknowledge response status')
        }
        return output
      })
  }

  releaseNotificationBatch(
    input: NotificationBatchReleaseInput
  ): Promise<NotificationBatchReleaseOutput> {
    const request = parseNotificationBatchReleaseInput(input)
    return this.rpc
      .request<unknown, NotificationBatchReleaseInput>(NOTIFICATION_BATCH_RELEASE_METHOD, request)
      .then((value) => {
        const output = parseNotificationBatchReleaseOutput(value)
        if (output.batchId !== request.batchId || output.retryAt < request.retryAt) {
          throw new Error('Invalid notification batch release response identity')
        }
        return output
      })
  }

  suppressNotificationBatch(
    input: NotificationBatchSuppressInput
  ): Promise<NotificationBatchSuppressOutput> {
    const request = parseNotificationBatchSuppressInput(input)
    return this.rpc
      .request<unknown, NotificationBatchSuppressInput>(NOTIFICATION_BATCH_SUPPRESS_METHOD, request)
      .then((value) => {
        const output = parseNotificationBatchSuppressOutput(value)
        if (output.batchId !== request.batchId || output.reason !== request.reason) {
          throw new Error('Invalid notification batch suppress response identity')
        }
        return output
      })
  }

  listNotifications(input: NotificationListInput): Promise<NotificationListOutput> {
    return this.rpc
      .request<unknown, NotificationListInput>(
        NOTIFICATION_LIST_METHOD,
        parseNotificationListInput(input)
      )
      .then(parseNotificationListOutput)
  }

  getNotificationSummary(input: NotificationSummaryInput): Promise<NotificationSummaryOutput> {
    return this.rpc
      .request<unknown, NotificationSummaryInput>(
        NOTIFICATION_SUMMARY_METHOD,
        parseNotificationSummaryInput(input)
      )
      .then(parseNotificationSummaryOutput)
  }

  markNotificationSeen(input: NotificationMarkSeenInput): Promise<NotificationMarkSeenOutput> {
    return this.rpc
      .request<unknown, NotificationMarkSeenInput>(
        NOTIFICATION_MARK_SEEN_METHOD,
        parseNotificationMarkSeenInput(input)
      )
      .then(parseNotificationMarkSeenOutput)
  }

  getNotificationSettings(
    input: NotificationSettingsGetInput
  ): Promise<NotificationSettingsGetOutput> {
    return this.rpc
      .request<unknown, NotificationSettingsGetInput>(
        NOTIFICATION_SETTINGS_GET_METHOD,
        parseNotificationSettingsGetInput(input)
      )
      .then(parseNotificationSettingsGetOutput)
  }

  updateNotificationSettings(
    input: NotificationSettingsUpdateInput
  ): Promise<NotificationSettingsUpdateOutput> {
    const request = parseNotificationSettingsUpdateInput(input)
    return this.rpc
      .request<unknown, NotificationSettingsUpdateInput>(
        NOTIFICATION_SETTINGS_UPDATE_METHOD,
        request
      )
      .then((value) => {
        const output = parseNotificationSettingsUpdateOutput(value)
        if (output.settings.revision <= request.expectedRevision) {
          throw new Error('Invalid notification settings revision response')
        }
        return output
      })
  }

  onNotificationEvent(handler: (event: NotificationEvent) => void): () => void {
    return this.rpc.onNotification(NOTIFICATION_EVENT_NOTIFICATION_METHOD, (params) => {
      let event: NotificationEvent
      try {
        event = parseNotificationEvent(params)
      } catch {
        console.warn('Ignored invalid system notification event')
        return
      }
      try {
        handler(event)
      } catch {
        console.warn('System notification event handler failed')
      }
    })
  }

  onNotificationResync(handler: (event: NotificationResync) => void): () => void {
    this.ensureNotificationResyncSubscription()
    this.notificationResyncHandlers.add(handler)
    const latest = this.latestNotificationResync
    if (latest !== null) {
      try {
        handler(latest)
      } catch {
        console.warn('System notification resync handler failed')
      }
    }
    return () => this.notificationResyncHandlers.delete(handler)
  }

  private ensureNotificationResyncSubscription(): void {
    if (this.notificationResyncSubscription !== null) return
    this.notificationResyncSubscription = this.rpc.onNotification(
      NOTIFICATION_RESYNC_NOTIFICATION_METHOD,
      (params) => {
        let event: NotificationResync
        try {
          event = parseNotificationResync(params)
        } catch {
          console.warn('Ignored invalid system notification resync')
          return
        }
        this.latestNotificationResync = event
        for (const handler of [...this.notificationResyncHandlers]) {
          try {
            handler(event)
          } catch {
            console.warn('System notification resync handler failed')
          }
        }
      }
    )
  }

  onAutomationEvent(handler: (event: AutomationEvent) => void): () => void {
    return this.rpc.onNotification(AUTOMATION_EVENT_NOTIFICATION_METHOD, (params) => {
      let event: AutomationEvent
      try {
        event = parseAutomationEvent(params)
      } catch {
        console.warn('Ignored invalid Automation event')
        return
      }
      try {
        handler(event)
      } catch {
        console.warn('Automation event handler failed')
      }
    })
  }

  onAutomationResync(handler: (event: AutomationResync) => void): () => void {
    this.ensureAutomationResyncSubscription()
    this.automationResyncHandlers.add(handler)
    const latest = this.latestAutomationResync
    if (latest !== null) {
      try {
        handler(latest)
      } catch {
        console.warn('Automation resync handler failed')
      }
    }
    return () => this.automationResyncHandlers.delete(handler)
  }

  private ensureAutomationResyncSubscription(): void {
    if (this.automationResyncSubscription !== null) return
    this.automationResyncSubscription = this.rpc.onNotification(
      AUTOMATION_RESYNC_NOTIFICATION_METHOD,
      (params) => {
        let event: AutomationResync
        try {
          event = parseAutomationResync(params)
        } catch {
          console.warn('Ignored invalid Automation resync')
          return
        }
        this.latestAutomationResync = event
        for (const handler of [...this.automationResyncHandlers]) {
          try {
            handler(event)
          } catch {
            console.warn('Automation resync handler failed')
          }
        }
      }
    )
  }
}
