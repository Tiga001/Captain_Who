import type {
  AgentActionExecutionOutput,
  AgentActionIdRequest,
  AgentApproveActionRequest,
  AgentCancelRunRequest,
  AgentCancelRunResponse,
  AgentCommandSessionGetInput,
  AgentCommandSessionGetOutput,
  AgentCommandSessionListInput,
  AgentCommandSessionListOutput,
  AgentConversationLocator,
  AgentConversationLocatorRequest,
  AgentConversationTurnInput,
  AgentConversationTurnOutput,
  AgentConversationTurnRewriteInput,
  AgentContextWindowSnapshotInput,
  AgentContextWindowSnapshotOutput,
  AgentDetail,
  AgentDetailRequest,
  AgentEvent,
  AgentFileChangeContentPage,
  AgentFileChangeDiffInput,
  AgentFileChangeDiffPage,
  AgentFileChangeHistoryDiffInput,
  AgentFileChangeHistoryDiffPage,
  AgentFileChangeReadInput,
  AgentObserverConversation,
  AgentObserverConversationRequest,
  AgentObserverEventEnvelope,
  AgentManualContextCompactionStartInput,
  AgentManualContextCompactionStatusInput,
  AgentManualContextCompactionCancelInput,
  AgentManualContextCompactionOperation,
  AgentManualContextCompactionStatusOutput,
  AgentManualContextCompactionNotification,
  AgentProviderTransitionNotification,
  AgentProviderTransitionOperation,
  AgentProviderTransitionPreflightInput,
  AgentProviderTransitionPreflightOutput,
  AgentProviderTransitionStartInput,
  AgentProviderTransitionStatusInput,
  AgentProviderTransitionStatusOutput,
  AgentRejectActionRequest,
  AgentSteerRunInput,
  AgentSteerRunOutput,
  AgentTemplate,
  AgentTemplateCreateRequest,
  AgentTemplateDeleteRequest,
  AgentTemplateList,
  AgentTemplateListRequest,
  AgentTemplateProjectAssignmentRequest,
  AgentTemplateSetEnabledRequest,
  AgentTemplateUpdateRequest,
  AgentTreeLookup,
  AgentTreeRequest,
  AgentUsageClearInput,
  AgentUsageClearOutput,
  AgentUsageSummaryInput,
  AgentUsageSummaryOutput,
  CollaborationApprovalDecisionRequest,
  CollaborationApprovalDecisionResult,
  CollaborationApprovalList,
  CollaborationApprovalListRequest,
  CollaborationEventEnvelope,
  CollaborationEventsPage,
  CollaborationEventsRequest,
  CollaborationResyncEnvelope,
  PendingAgentActionSnapshot
} from '@mycopilot/protocol'
import {
  AGENT_PROMPT_PREFERENCES_CHANGED_METHOD,
  parseAgentPromptPreferencesChanged,
  type AgentPromptPreferencesChanged,
  AGENT_COLLABORATION_GET_SETTINGS_METHOD,
  AGENT_COLLABORATION_UPDATE_SETTINGS_METHOD,
  AGENT_COLLABORATION_SETTINGS_CHANGED_METHOD,
  parseAgentCollaborationSettings,
  parseAgentCollaborationSettingsGetInput,
  parseAgentCollaborationSettingsUpdate,
  type AgentCollaborationSettings,
  type AgentCollaborationSettingsGetInput,
  type AgentCollaborationSettingsUpdate,
  AGENT_APPROVE_ACTION_METHOD,
  AGENT_CANCEL_ACTION_METHOD,
  AGENT_CANCEL_RUN_METHOD,
  AGENT_CLEAR_USAGE_RECORDS_METHOD,
  AGENT_COLLABORATION_APPROVALS_DECIDE_METHOD,
  AGENT_COLLABORATION_APPROVALS_LIST_METHOD,
  AGENT_COLLABORATION_EVENT_NOTIFICATION_METHOD,
  AGENT_COLLABORATION_GET_AGENT_METHOD,
  AGENT_COLLABORATION_GET_TREE_METHOD,
  AGENT_COLLABORATION_LIST_EVENTS_METHOD,
  AGENT_COLLABORATION_LOAD_OBSERVER_CONVERSATION_METHOD,
  AGENT_COLLABORATION_LOCATE_CONVERSATION_METHOD,
  AGENT_COLLABORATION_OBSERVER_EVENT_NOTIFICATION_METHOD,
  AGENT_COLLABORATION_RESYNC_NOTIFICATION_METHOD,
  AGENT_COLLABORATION_TEMPLATES_CREATE_METHOD,
  AGENT_COLLABORATION_TEMPLATES_DELETE_METHOD,
  AGENT_COLLABORATION_TEMPLATES_LIST_METHOD,
  AGENT_COLLABORATION_TEMPLATES_SET_ENABLED_METHOD,
  AGENT_COLLABORATION_TEMPLATES_SET_PROJECT_ASSIGNMENT_METHOD,
  AGENT_COLLABORATION_TEMPLATES_UPDATE_METHOD,
  AGENT_COMMAND_SESSIONS_GET_METHOD,
  AGENT_COMMAND_SESSIONS_LIST_METHOD,
  AGENT_EVENT_NOTIFICATION_METHOD,
  AGENT_GET_CONTEXT_WINDOW_SNAPSHOT_METHOD,
  AGENT_GET_FILE_CHANGE_DIFF_METHOD,
  AGENT_GET_FILE_CHANGE_HISTORY_DIFF_METHOD,
  AGENT_GET_PROVIDER_TRANSITION_STATUS_METHOD,
  AGENT_GET_USAGE_SUMMARY_METHOD,
  AGENT_LIST_PENDING_ACTIONS_METHOD,
  AGENT_START_MANUAL_CONTEXT_COMPACTION_METHOD,
  AGENT_GET_MANUAL_CONTEXT_COMPACTION_STATUS_METHOD,
  AGENT_CANCEL_MANUAL_CONTEXT_COMPACTION_METHOD,
  AGENT_MANUAL_CONTEXT_COMPACTION_NOTIFICATION_METHOD,
  parseAgentManualContextCompactionStartInput,
  parseAgentManualContextCompactionStatusInput,
  parseAgentManualContextCompactionCancelInput,
  parseAgentManualContextCompactionOperation,
  parseAgentManualContextCompactionStatusOutput,
  parseAgentManualContextCompactionNotification,
  AGENT_PREFLIGHT_PROVIDER_TRANSITION_METHOD,
  AGENT_PROVIDER_TRANSITION_NOTIFICATION_METHOD,
  AGENT_READ_FILE_CHANGE_METHOD,
  AGENT_REJECT_ACTION_METHOD,
  AGENT_START_CONVERSATION_TURN_METHOD,
  AGENT_START_PROVIDER_TRANSITION_METHOD,
  AGENT_STEER_RUN_METHOD,
  parseAgentActionExecutionOutputForHost,
  parseAgentCommandSessionGetInput,
  parseAgentCommandSessionGetOutput,
  parseAgentCommandSessionListInput,
  parseAgentCommandSessionListOutput,
  parseAgentConversationLocator,
  parseAgentConversationLocatorRequest,
  parseAgentDetail,
  parseAgentDetailRequest,
  parseAgentEventForHost,
  parseAgentFileChangeContentPageForHost,
  parseAgentFileChangeDiffPageForHost,
  parseAgentFileChangeHistoryDiffPageForHost,
  parseAgentObserverConversation,
  parseAgentObserverConversationRequest,
  parseAgentObserverEventEnvelope,
  parseAgentProviderTransitionNotification,
  parseAgentProviderTransitionOperation,
  parseAgentProviderTransitionPreflightInput,
  parseAgentProviderTransitionPreflightOutput,
  parseAgentProviderTransitionStartInput,
  parseAgentProviderTransitionStatusInput,
  parseAgentProviderTransitionStatusOutput,
  parseAgentTemplate,
  parseAgentTemplateCreateRequest,
  parseAgentTemplateDeleteRequest,
  parseAgentTemplateList,
  parseAgentTemplateListRequest,
  parseAgentTemplateProjectAssignmentRequest,
  parseAgentTemplateSetEnabledRequest,
  parseAgentTemplateUpdateRequest,
  parseAgentTreeLookup,
  parseAgentTreeRequest,
  parseCollaborationApprovalDecisionRequest,
  parseCollaborationApprovalDecisionResult,
  parseCollaborationApprovalList,
  parseCollaborationApprovalListRequest,
  parseCollaborationEventEnvelope,
  parseCollaborationEventsPage,
  parseCollaborationEventsRequest,
  parseCollaborationResyncEnvelope,
  parsePendingAgentActionSnapshotsForHost
} from '@mycopilot/protocol'

import { CoreServerStorageApi } from './coreServerStorageApi'

const AGENT_REWRITE_CONVERSATION_TURN_METHOD = 'agent.rewriteConversationTurn'

function validateProviderTransitionResponseIdentity(
  request: { conversationId: string; targetModelId: string },
  response: { conversationId: string; targetModelId: string }
): void {
  if (
    response.conversationId !== request.conversationId ||
    response.targetModelId !== request.targetModelId
  ) {
    throw new Error('Invalid Provider transition response identity')
  }
}

function assertAgentActionExecutionIdentity(
  request: AgentActionIdRequest,
  response: AgentActionExecutionOutput
): AgentActionExecutionOutput {
  if (response.actionId !== request.actionId || response.agentOutput.runId !== request.runId) {
    throw new Error('Invalid Agent action execution identity')
  }
  return response
}

const AGENT_OBSERVER_EVENT_TYPES = {
  started: true,
  tool_set_changed: true,
  state: true,
  message_delta: true,
  message_stream_started: true,
  message_stream_reset: true,
  message_stream_committed: true,
  llm_retry: true,
  tool_input_progress: true,
  file_change_preview_updated: true,
  file_change_preview_cleared: true,
  message: true,
  guidance_queued: true,
  guidance_applied: true,
  guidance_rejected: true,
  tool_call: true,
  tool_result: true,
  mcp_tool_invocation_state_changed: true,
  todo_updated: true,
  skill_activated: true,
  file_change_updated: true,
  context_window_updated: true,
  context_compaction_started: true,
  context_compaction_finished: true,
  approval_required: true,
  file_change_proposed: true,
  command_started: true,
  command_output: true,
  command_exited: true,
  command_interrupted: true,
  error: true,
  done: true
} as const satisfies Readonly<Record<AgentEvent['type'], true>>

type AgentObserverEventLogType = AgentEvent['type'] | 'unknown'

interface AgentObserverWarning {
  category: 'validation_failed' | 'handler_failed'
  eventType: AgentObserverEventLogType
  count: number
}

function readAgentObserverEventLogType(value: unknown): AgentObserverEventLogType {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return 'unknown'
  const event = 'event' in value ? value.event : undefined
  if (typeof event !== 'object' || event === null || Array.isArray(event)) return 'unknown'
  const eventType = 'type' in event ? event.type : undefined
  if (
    typeof eventType !== 'string' ||
    !Object.prototype.hasOwnProperty.call(AGENT_OBSERVER_EVENT_TYPES, eventType)
  ) {
    return 'unknown'
  }
  return eventType as AgentEvent['type']
}

function shouldLogAgentObserverWarning(count: number): boolean {
  return count === 1 || count % 100 === 0
}

/** Agent/collaboration request facade; it does not start or stop the JSON-RPC process. */
export class CoreServerAgentApi extends CoreServerStorageApi {
  private readonly agentObserverWarningCounts = new Map<string, number>()

  private warnAgentObserverEvent(
    category: AgentObserverWarning['category'],
    eventType: AgentObserverEventLogType
  ): void {
    const key = `${category}:${eventType}`
    const count = (this.agentObserverWarningCounts.get(key) ?? 0) + 1
    this.agentObserverWarningCounts.set(key, count)
    if (!shouldLogAgentObserverWarning(count)) return
    const warning: AgentObserverWarning = { category, eventType, count }
    console.warn(
      category === 'validation_failed'
        ? 'Ignored invalid Agent observer event'
        : 'Agent observer handler failed',
      warning
    )
  }

  async startManualContextCompaction(
    input: AgentManualContextCompactionStartInput
  ): Promise<AgentManualContextCompactionOperation> {
    const request = parseAgentManualContextCompactionStartInput(input)
    try {
      const output = parseAgentManualContextCompactionOperation(
        await this.rpc.request<unknown, AgentManualContextCompactionStartInput>(
          AGENT_START_MANUAL_CONTEXT_COMPACTION_METHOD,
          request
        )
      )
      const operations = [output]
      if (
        operations.some(
          (operation) =>
            operation.conversationId !== request.conversationId ||
            ('operationId' in request &&
              request.operationId !== undefined &&
              operation.operationId !== request.operationId) ||
            ('requestId' in request && operation.requestId !== request.requestId)
        )
      )
        throw new Error('Invalid operation identity')
      return output
    } catch {
      throw new Error('Unable to complete this compaction request. Please try again.')
    }
  }
  async getManualContextCompactionStatus(
    input: AgentManualContextCompactionStatusInput
  ): Promise<AgentManualContextCompactionStatusOutput> {
    const request = parseAgentManualContextCompactionStatusInput(input)
    try {
      const output = parseAgentManualContextCompactionStatusOutput(
        await this.rpc.request<unknown, AgentManualContextCompactionStatusInput>(
          AGENT_GET_MANUAL_CONTEXT_COMPACTION_STATUS_METHOD,
          request
        )
      )
      const operations = output.operations
      if (
        operations.some(
          (operation) =>
            operation.conversationId !== request.conversationId ||
            ('operationId' in request &&
              request.operationId !== undefined &&
              operation.operationId !== request.operationId) ||
            ('requestId' in request && operation.requestId !== request.requestId)
        )
      )
        throw new Error('Invalid operation identity')
      return output
    } catch {
      throw new Error('Unable to restore compaction status. Please try again.')
    }
  }
  async cancelManualContextCompaction(
    input: AgentManualContextCompactionCancelInput
  ): Promise<AgentManualContextCompactionOperation> {
    const request = parseAgentManualContextCompactionCancelInput(input)
    try {
      const output = parseAgentManualContextCompactionOperation(
        await this.rpc.request<unknown, AgentManualContextCompactionCancelInput>(
          AGENT_CANCEL_MANUAL_CONTEXT_COMPACTION_METHOD,
          request
        )
      )
      const operations = [output]
      if (
        operations.some(
          (operation) =>
            operation.conversationId !== request.conversationId ||
            ('operationId' in request &&
              request.operationId !== undefined &&
              operation.operationId !== request.operationId) ||
            ('requestId' in request && operation.requestId !== request.requestId)
        )
      )
        throw new Error('Invalid operation identity')
      return output
    } catch {
      throw new Error('Unable to complete this compaction request. Please try again.')
    }
  }
  onManualContextCompaction(
    handler: (event: AgentManualContextCompactionNotification) => void
  ): () => void {
    return this.rpc.onNotification(
      AGENT_MANUAL_CONTEXT_COMPACTION_NOTIFICATION_METHOD,
      (params) => {
        try {
          handler(parseAgentManualContextCompactionNotification(params))
        } catch {
          console.warn('Ignored invalid manual compaction notification')
        }
      }
    )
  }

  preflightProviderTransition(
    input: AgentProviderTransitionPreflightInput
  ): Promise<AgentProviderTransitionPreflightOutput> {
    const request = parseAgentProviderTransitionPreflightInput(input)
    return this.rpc
      .request<unknown, AgentProviderTransitionPreflightInput>(
        AGENT_PREFLIGHT_PROVIDER_TRANSITION_METHOD,
        request
      )
      .then((value) => {
        const output = parseAgentProviderTransitionPreflightOutput(value)
        validateProviderTransitionResponseIdentity(request, output)
        return output
      })
      .catch(() => {
        throw new Error('Unable to check this model switch. Please try again.')
      })
  }

  startProviderTransition(
    input: AgentProviderTransitionStartInput
  ): Promise<AgentProviderTransitionOperation> {
    const request = parseAgentProviderTransitionStartInput(input)
    return this.rpc
      .request<unknown, AgentProviderTransitionStartInput>(
        AGENT_START_PROVIDER_TRANSITION_METHOD,
        request
      )
      .then((value) => {
        const operation = parseAgentProviderTransitionOperation(value)
        validateProviderTransitionResponseIdentity(request, operation)
        return operation
      })
      .catch((error: unknown) => {
        const safeError = new Error('The model-switch check expired. Please try again.')
        // Preserve only the JSON-RPC rejection marker. Private diagnostics stay in Core,
        // while transport failures without a response code remain uncertain to the caller.
        if (error instanceof Error && 'code' in error && typeof error.code === 'number') {
          Object.assign(safeError, { code: error.code })
        }
        throw safeError
      })
  }

  getProviderTransitionStatus(
    input: AgentProviderTransitionStatusInput
  ): Promise<AgentProviderTransitionStatusOutput> {
    const request = parseAgentProviderTransitionStatusInput(input)
    return this.rpc
      .request<unknown, AgentProviderTransitionStatusInput>(
        AGENT_GET_PROVIDER_TRANSITION_STATUS_METHOD,
        request
      )
      .then((value) => {
        const output = parseAgentProviderTransitionStatusOutput(value)
        if (
          output.operations.some(
            (operation) =>
              operation.conversationId !== request.conversationId ||
              (request.operationId !== undefined && operation.operationId !== request.operationId)
          )
        ) {
          throw new Error('Invalid Provider transition status response identity')
        }
        return output
      })
      .catch(() => {
        throw new Error('Unable to restore the model-switch status. Please try again.')
      })
  }

  onProviderTransition(handler: (event: AgentProviderTransitionNotification) => void): () => void {
    return this.rpc.onNotification(AGENT_PROVIDER_TRANSITION_NOTIFICATION_METHOD, (params) => {
      try {
        handler(parseAgentProviderTransitionNotification(params))
      } catch {
        // Do not echo rejected transition payloads: they may contain future private fields.
        console.warn('Ignored invalid Provider transition notification')
      }
    })
  }

  startConversationTurn(input: AgentConversationTurnInput): Promise<AgentConversationTurnOutput> {
    return this.rpc.request<AgentConversationTurnOutput, AgentConversationTurnInput>(
      AGENT_START_CONVERSATION_TURN_METHOD,
      input
    )
  }

  rewriteConversationTurn(
    input: AgentConversationTurnRewriteInput
  ): Promise<AgentConversationTurnOutput> {
    return this.rpc.request<AgentConversationTurnOutput, AgentConversationTurnRewriteInput>(
      AGENT_REWRITE_CONVERSATION_TURN_METHOD,
      input
    )
  }

  getContextWindowSnapshot(
    input: AgentContextWindowSnapshotInput
  ): Promise<AgentContextWindowSnapshotOutput> {
    return this.rpc.request<AgentContextWindowSnapshotOutput, AgentContextWindowSnapshotInput>(
      AGENT_GET_CONTEXT_WINDOW_SNAPSHOT_METHOD,
      input
    )
  }

  listCommandSessions(input: AgentCommandSessionListInput): Promise<AgentCommandSessionListOutput> {
    const request = parseAgentCommandSessionListInput(input)
    return this.rpc
      .request<unknown, AgentCommandSessionListInput>(AGENT_COMMAND_SESSIONS_LIST_METHOD, request)
      .then(parseAgentCommandSessionListOutput)
  }

  getCommandSession(input: AgentCommandSessionGetInput): Promise<AgentCommandSessionGetOutput> {
    const request = parseAgentCommandSessionGetInput(input)
    return this.rpc
      .request<unknown, AgentCommandSessionGetInput>(AGENT_COMMAND_SESSIONS_GET_METHOD, request)
      .then(parseAgentCommandSessionGetOutput)
  }

  cancelRun(input: AgentCancelRunRequest): Promise<AgentCancelRunResponse> {
    return this.rpc.request<AgentCancelRunResponse, AgentCancelRunRequest>(
      AGENT_CANCEL_RUN_METHOD,
      input
    )
  }

  steerRun(input: AgentSteerRunInput): Promise<AgentSteerRunOutput> {
    return this.rpc.request<AgentSteerRunOutput, AgentSteerRunInput>(AGENT_STEER_RUN_METHOD, input)
  }

  listPendingActions(): Promise<PendingAgentActionSnapshot[]> {
    return this.rpc
      .request<unknown>(AGENT_LIST_PENDING_ACTIONS_METHOD)
      .then(parsePendingAgentActionSnapshotsForHost)
  }

  approveAction(input: AgentApproveActionRequest): Promise<AgentActionExecutionOutput> {
    return this.rpc
      .request<unknown, AgentApproveActionRequest>(AGENT_APPROVE_ACTION_METHOD, input)
      .then(parseAgentActionExecutionOutputForHost)
      .then((output) => assertAgentActionExecutionIdentity(input, output))
  }

  rejectAction(input: AgentRejectActionRequest): Promise<AgentActionExecutionOutput> {
    return this.rpc
      .request<unknown, AgentRejectActionRequest>(AGENT_REJECT_ACTION_METHOD, input)
      .then(parseAgentActionExecutionOutputForHost)
      .then((output) => assertAgentActionExecutionIdentity(input, output))
  }

  cancelAction(input: AgentActionIdRequest): Promise<boolean> {
    return this.rpc.request<boolean, AgentActionIdRequest>(AGENT_CANCEL_ACTION_METHOD, input)
  }

  getUsageSummary(input: AgentUsageSummaryInput): Promise<AgentUsageSummaryOutput> {
    return this.rpc.request<AgentUsageSummaryOutput, AgentUsageSummaryInput>(
      AGENT_GET_USAGE_SUMMARY_METHOD,
      input
    )
  }

  clearUsageRecords(input: AgentUsageClearInput): Promise<AgentUsageClearOutput> {
    return this.rpc.request<AgentUsageClearOutput, AgentUsageClearInput>(
      AGENT_CLEAR_USAGE_RECORDS_METHOD,
      input
    )
  }

  readFileChange(input: AgentFileChangeReadInput): Promise<AgentFileChangeContentPage> {
    return this.rpc
      .request<unknown, AgentFileChangeReadInput>(AGENT_READ_FILE_CHANGE_METHOD, input)
      .then((value) => {
        const page = parseAgentFileChangeContentPageForHost(value)
        if (page.fileChange.transactionId !== input.transactionId) {
          throw new Error('Invalid FileChange content page identity')
        }
        return page
      })
  }

  getFileChangeDiff(input: AgentFileChangeDiffInput): Promise<AgentFileChangeDiffPage> {
    return this.rpc
      .request<unknown, AgentFileChangeDiffInput>(AGENT_GET_FILE_CHANGE_DIFF_METHOD, input)
      .then((value) => {
        const page = parseAgentFileChangeDiffPageForHost(value)
        if (page.transactionId !== input.transactionId) {
          throw new Error('Invalid FileChange Diff page identity')
        }
        return page
      })
  }

  getFileChangeHistoryDiff(
    input: AgentFileChangeHistoryDiffInput
  ): Promise<AgentFileChangeHistoryDiffPage> {
    return this.rpc
      .request<unknown, AgentFileChangeHistoryDiffInput>(
        AGENT_GET_FILE_CHANGE_HISTORY_DIFF_METHOD,
        input
      )
      .then((value) => {
        const page = parseAgentFileChangeHistoryDiffPageForHost(value)
        if (
          page.conversationId !== input.conversationId ||
          page.assistantMessageId !== input.assistantMessageId ||
          page.runId !== input.runId ||
          page.toolCallId !== input.toolCallId
        ) {
          throw new Error('Invalid FileChange history Diff page identity')
        }
        return page
      })
  }

  onAgentEvent(handler: (event: AgentEvent) => void): () => void {
    return this.rpc.onNotification(AGENT_EVENT_NOTIFICATION_METHOD, (params) => {
      try {
        handler(parseAgentEventForHost(params))
      } catch {
        // MCP event rejection must not echo the rejected payload or a parser diagnostic.
        console.warn('Ignored invalid Agent event')
      }
    })
  }

  getCollaborationSettings(
    input: AgentCollaborationSettingsGetInput
  ): Promise<AgentCollaborationSettings> {
    return this.rpc
      .request<unknown, AgentCollaborationSettingsGetInput>(
        AGENT_COLLABORATION_GET_SETTINGS_METHOD,
        parseAgentCollaborationSettingsGetInput(input)
      )
      .then(parseAgentCollaborationSettings)
  }

  updateCollaborationSettings(
    input: AgentCollaborationSettingsUpdate
  ): Promise<AgentCollaborationSettings> {
    return this.rpc
      .request<unknown, AgentCollaborationSettingsUpdate>(
        AGENT_COLLABORATION_UPDATE_SETTINGS_METHOD,
        parseAgentCollaborationSettingsUpdate(input)
      )
      .then(parseAgentCollaborationSettings)
  }

  onPromptPreferencesChanged(handler: (event: AgentPromptPreferencesChanged) => void): () => void {
    return this.rpc.onNotification(AGENT_PROMPT_PREFERENCES_CHANGED_METHOD, (params) => {
      let event: AgentPromptPreferencesChanged
      try {
        event = parseAgentPromptPreferencesChanged(params)
      } catch {
        console.warn('Ignored invalid prompt preferences notification')
        return
      }
      handler(event)
    })
  }

  onCollaborationSettingsChanged(
    handler: (settings: AgentCollaborationSettings) => void
  ): () => void {
    return this.rpc.onNotification(AGENT_COLLABORATION_SETTINGS_CHANGED_METHOD, (params) => {
      let settings: AgentCollaborationSettings
      try {
        settings = parseAgentCollaborationSettings(params)
      } catch {
        console.warn('Ignored invalid collaboration settings')
        return
      }
      handler(settings)
    })
  }

  getCollaborationTree(input: AgentTreeRequest): Promise<AgentTreeLookup> {
    const request = parseAgentTreeRequest(input)
    return this.rpc
      .request<unknown, AgentTreeRequest>(AGENT_COLLABORATION_GET_TREE_METHOD, request)
      .then((value) => {
        const lookup = parseAgentTreeLookup(value)
        const tree = lookup.tree
        if (!tree) return lookup
        if (
          tree.rootConversationId !== request.rootConversationId ||
          tree.agents.some(
            (agent) =>
              agent.rootConversationId !== tree.rootConversationId ||
              agent.rootAgentId !== tree.rootAgentId ||
              agent.projectId !== tree.projectId
          )
        ) {
          throw new Error('Invalid collaboration tree response identity')
        }
        return lookup
      })
  }

  getCollaborationAgent(input: AgentDetailRequest): Promise<AgentDetail> {
    const request = parseAgentDetailRequest(input)
    return this.rpc
      .request<unknown, AgentDetailRequest>(AGENT_COLLABORATION_GET_AGENT_METHOD, request)
      .then((value) => {
        const detail = parseAgentDetail(value)
        if (
          detail.summary.agentId !== request.agentId ||
          detail.summary.rootConversationId !== request.rootConversationId
        ) {
          throw new Error('Invalid collaboration Agent response identity')
        }
        return detail
      })
  }

  locateCollaborationConversation(
    input: AgentConversationLocatorRequest
  ): Promise<AgentConversationLocator> {
    const request = parseAgentConversationLocatorRequest(input)
    return this.rpc
      .request<unknown, AgentConversationLocatorRequest>(
        AGENT_COLLABORATION_LOCATE_CONVERSATION_METHOD,
        request
      )
      .then((value) => {
        const locator = parseAgentConversationLocator(value)
        if (locator.agentId !== request.agentId) {
          throw new Error('Invalid collaboration locator response identity')
        }
        return locator
      })
  }

  loadCollaborationObserverConversation(
    input: AgentObserverConversationRequest
  ): Promise<AgentObserverConversation | null> {
    const request = parseAgentObserverConversationRequest(input)
    return this.rpc
      .request<unknown, AgentObserverConversationRequest>(
        AGENT_COLLABORATION_LOAD_OBSERVER_CONVERSATION_METHOD,
        request
      )
      .then((value) => {
        const response = parseAgentObserverConversation(value)
        if (
          response &&
          (response.rootConversationId !== request.rootConversationId ||
            response.conversationId !== request.conversationId)
        ) {
          throw new Error('Invalid observer Conversation response identity')
        }
        return response
      })
  }

  listCollaborationEvents(input: CollaborationEventsRequest): Promise<CollaborationEventsPage> {
    const request = parseCollaborationEventsRequest(input)
    return this.rpc
      .request<unknown, CollaborationEventsRequest>(AGENT_COLLABORATION_LIST_EVENTS_METHOD, request)
      .then((value) => {
        const page = parseCollaborationEventsPage(value)
        if (
          page.rootConversationId !== request.rootConversationId ||
          page.events.some(
            (event) =>
              event.rootConversationId !== page.rootConversationId ||
              event.rootAgentId !== page.rootAgentId
          )
        ) {
          throw new Error('Invalid collaboration event page identity')
        }
        return page
      })
  }

  listAgentTemplates(input: AgentTemplateListRequest): Promise<AgentTemplateList> {
    const request = parseAgentTemplateListRequest(input)
    return this.rpc
      .request<unknown, AgentTemplateListRequest>(
        AGENT_COLLABORATION_TEMPLATES_LIST_METHOD,
        request
      )
      .then((value) => {
        return parseAgentTemplateList(value)
      })
  }

  createAgentTemplate(input: AgentTemplateCreateRequest): Promise<AgentTemplate> {
    const request = parseAgentTemplateCreateRequest(input)
    return this.rpc
      .request<unknown, AgentTemplateCreateRequest>(
        AGENT_COLLABORATION_TEMPLATES_CREATE_METHOD,
        request
      )
      .then((value) => {
        const output = parseAgentTemplate(value)
        if (output.templateId !== request.templateId) {
          throw new Error('Invalid Agent template response identity')
        }
        return output
      })
  }

  updateAgentTemplate(input: AgentTemplateUpdateRequest): Promise<AgentTemplate> {
    const request = parseAgentTemplateUpdateRequest(input)
    return this.rpc
      .request<unknown, AgentTemplateUpdateRequest>(
        AGENT_COLLABORATION_TEMPLATES_UPDATE_METHOD,
        request
      )
      .then((value) => {
        const output = parseAgentTemplate(value)
        if (output.templateId !== request.templateId) {
          throw new Error('Invalid Agent template response identity')
        }
        return output
      })
  }

  setAgentTemplateEnabled(input: AgentTemplateSetEnabledRequest): Promise<AgentTemplate> {
    const request = parseAgentTemplateSetEnabledRequest(input)
    return this.rpc
      .request<unknown, AgentTemplateSetEnabledRequest>(
        AGENT_COLLABORATION_TEMPLATES_SET_ENABLED_METHOD,
        request
      )
      .then((value) => {
        const output = parseAgentTemplate(value)
        if (output.templateId !== request.templateId) {
          throw new Error('Invalid Agent template response identity')
        }
        return output
      })
  }

  setAgentTemplateProjectAssignment(
    input: AgentTemplateProjectAssignmentRequest
  ): Promise<AgentTemplate> {
    const request = parseAgentTemplateProjectAssignmentRequest(input)
    return this.rpc
      .request<unknown, AgentTemplateProjectAssignmentRequest>(
        AGENT_COLLABORATION_TEMPLATES_SET_PROJECT_ASSIGNMENT_METHOD,
        request
      )
      .then((value) => {
        const output = parseAgentTemplate(value)
        if (output.templateId !== request.templateId) {
          throw new Error('Invalid Agent template response identity')
        }
        return output
      })
  }

  deleteAgentTemplate(input: AgentTemplateDeleteRequest): Promise<AgentTemplate> {
    const request = parseAgentTemplateDeleteRequest(input)
    return this.rpc
      .request<unknown, AgentTemplateDeleteRequest>(
        AGENT_COLLABORATION_TEMPLATES_DELETE_METHOD,
        request
      )
      .then((value) => {
        const output = parseAgentTemplate(value)
        if (output.templateId !== request.templateId) {
          throw new Error('Invalid Agent template response identity')
        }
        return output
      })
  }

  listCollaborationApprovals(
    input: CollaborationApprovalListRequest
  ): Promise<CollaborationApprovalList> {
    const request = parseCollaborationApprovalListRequest(input)
    return this.rpc
      .request<unknown, CollaborationApprovalListRequest>(
        AGENT_COLLABORATION_APPROVALS_LIST_METHOD,
        request
      )
      .then((value) => {
        const output = parseCollaborationApprovalList(value)
        if (
          output.approvals.some(
            (approval) => approval.rootConversationId !== request.rootConversationId
          )
        ) {
          throw new Error('Invalid collaboration Approval list identity')
        }
        return output
      })
  }

  decideCollaborationApproval(
    input: CollaborationApprovalDecisionRequest
  ): Promise<CollaborationApprovalDecisionResult> {
    const request = parseCollaborationApprovalDecisionRequest(input)
    return this.rpc
      .request<unknown, CollaborationApprovalDecisionRequest>(
        AGENT_COLLABORATION_APPROVALS_DECIDE_METHOD,
        request
      )
      .then((value) => {
        const output = parseCollaborationApprovalDecisionResult(value)
        if (output.approvalId !== request.approvalId) {
          throw new Error('Invalid collaboration Approval response identity')
        }
        return output
      })
  }

  onCollaborationEvent(handler: (event: CollaborationEventEnvelope) => void): () => void {
    return this.rpc.onNotification(AGENT_COLLABORATION_EVENT_NOTIFICATION_METHOD, (params) => {
      try {
        handler(parseCollaborationEventEnvelope(params))
      } catch {
        console.warn('Ignored invalid collaboration event')
      }
    })
  }

  onCollaborationObserverEvent(handler: (event: AgentObserverEventEnvelope) => void): () => void {
    return this.rpc.onNotification(
      AGENT_COLLABORATION_OBSERVER_EVENT_NOTIFICATION_METHOD,
      (params) => {
        let event: AgentObserverEventEnvelope
        try {
          event = parseAgentObserverEventEnvelope(params)
        } catch {
          this.warnAgentObserverEvent('validation_failed', readAgentObserverEventLogType(params))
          return
        }
        try {
          handler(event)
        } catch {
          // Preserve the notification boundary's fail-closed behavior without misclassifying a
          // renderer/subscriber failure as an invalid Core event or echoing the event payload.
          this.warnAgentObserverEvent('handler_failed', event.event.type)
        }
      }
    )
  }

  onCollaborationResync(handler: (event: CollaborationResyncEnvelope) => void): () => void {
    return this.rpc.onNotification(AGENT_COLLABORATION_RESYNC_NOTIFICATION_METHOD, (params) => {
      try {
        handler(parseCollaborationResyncEnvelope(params))
      } catch {
        console.warn('Ignored invalid collaboration resync')
      }
    })
  }
}
