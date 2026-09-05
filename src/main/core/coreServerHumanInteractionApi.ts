import {
  HUMAN_INTERACTION_GET_SETTINGS_METHOD,
  HUMAN_INTERACTION_UPDATE_SETTINGS_METHOD,
  HUMAN_INTERACTION_LIST_REQUESTS_METHOD,
  HUMAN_INTERACTION_SUBMIT_METHOD,
  HUMAN_INTERACTION_IGNORE_METHOD,
  HUMAN_INTERACTION_SETTINGS_CHANGED_METHOD,
  HUMAN_INTERACTION_REQUEST_CHANGED_METHOD,
  parseHumanInteractionSettingsGetInput,
  parseHumanInteractionSettings,
  parseHumanInteractionSettingsUpdate,
  parseHumanInteractionListInput,
  parseHumanInteractionListOutput,
  parseHumanInteractionSubmitInput,
  parseHumanInteractionIgnoreInput,
  parseHumanInteractionRequestSnapshot,
  type HumanInteractionSettingsGetInput,
  type HumanInteractionSettings,
  type HumanInteractionSettingsUpdate,
  type HumanInteractionListInput,
  type HumanInteractionListOutput,
  type HumanInteractionSubmitInput,
  type HumanInteractionIgnoreInput,
  type HumanInteractionRequestSnapshot
} from '@mycopilot/protocol'
import { CoreServerManagementApi } from './coreServerManagementApi'

/** Stateless transport adapter. Core owns settings, request admission, responses and delivery. */
export class CoreServerHumanInteractionApi extends CoreServerManagementApi {
  async getHumanInteractionSettings(
    input: HumanInteractionSettingsGetInput
  ): Promise<HumanInteractionSettings> {
    const request = parseHumanInteractionSettingsGetInput(input)
    return parseHumanInteractionSettings(
      await this.rpc.request<unknown>(HUMAN_INTERACTION_GET_SETTINGS_METHOD, request)
    )
  }

  async updateHumanInteractionSettings(
    input: HumanInteractionSettingsUpdate
  ): Promise<HumanInteractionSettings> {
    const request = parseHumanInteractionSettingsUpdate(input)
    const result = parseHumanInteractionSettings(
      await this.rpc.request<unknown>(HUMAN_INTERACTION_UPDATE_SETTINGS_METHOD, request)
    )
    if (result.enabled !== request.enabled || result.revision < request.expectedRevision) {
      throw new Error('Invalid human interaction settings update result')
    }
    return result
  }

  async listHumanInteractionRequests(
    input: HumanInteractionListInput
  ): Promise<HumanInteractionListOutput> {
    const request = parseHumanInteractionListInput(input)
    const result = parseHumanInteractionListOutput(
      await this.rpc.request<unknown>(HUMAN_INTERACTION_LIST_REQUESTS_METHOD, request)
    )
    if (
      result.items.length > request.limit ||
      result.items.some((item) => item.conversationId !== request.conversationId)
    ) {
      throw new Error('Invalid human interaction request list ownership')
    }
    return result
  }

  async submitHumanInteractionRequest(
    input: HumanInteractionSubmitInput
  ): Promise<HumanInteractionRequestSnapshot> {
    const request = parseHumanInteractionSubmitInput(input)
    const result = parseHumanInteractionRequestSnapshot(
      await this.rpc.request<unknown>(HUMAN_INTERACTION_SUBMIT_METHOD, request)
    )
    validateResponseIdentity(result, request, 'submitted')
    return result
  }

  async ignoreHumanInteractionRequest(
    input: HumanInteractionIgnoreInput
  ): Promise<HumanInteractionRequestSnapshot> {
    const request = parseHumanInteractionIgnoreInput(input)
    const result = parseHumanInteractionRequestSnapshot(
      await this.rpc.request<unknown>(HUMAN_INTERACTION_IGNORE_METHOD, request)
    )
    validateResponseIdentity(result, request, 'ignored')
    return result
  }

  onHumanInteractionSettingsChanged(
    handler: (settings: HumanInteractionSettings) => void
  ): () => void {
    return this.rpc.onNotification(HUMAN_INTERACTION_SETTINGS_CHANGED_METHOD, (value) => {
      let settings: HumanInteractionSettings
      try {
        settings = parseHumanInteractionSettings(value)
      } catch {
        console.warn('Ignored invalid human interaction settings notification')
        return
      }
      handler(settings)
    })
  }

  onHumanInteractionRequestChanged(
    handler: (request: HumanInteractionRequestSnapshot) => void
  ): () => void {
    return this.rpc.onNotification(HUMAN_INTERACTION_REQUEST_CHANGED_METHOD, (value) => {
      let request: HumanInteractionRequestSnapshot
      try {
        request = parseHumanInteractionRequestSnapshot(value)
      } catch {
        console.warn('Ignored invalid human interaction request notification')
        return
      }
      handler(request)
    })
  }
}

function validateResponseIdentity(
  result: HumanInteractionRequestSnapshot,
  input: HumanInteractionIgnoreInput,
  status: 'submitted' | 'ignored'
): void {
  if (
    result.conversationId !== input.conversationId ||
    result.requestId !== input.requestId ||
    result.status !== status ||
    result.response?.submissionId !== input.submissionId ||
    result.revision < input.expectedRevision
  ) {
    throw new Error('Invalid human interaction response identity')
  }
}
