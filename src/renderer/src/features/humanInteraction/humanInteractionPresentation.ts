import {
  parseHumanInteractionResponseDisplay,
  type HumanInteractionRequestSnapshot,
  type HumanInteractionResponseDisplay
} from '@mycopilot/protocol'
import type {
  ChatAgentTimelineItem,
  ChatConversation,
  ChatGuidanceTimelineItem,
  ChatMessage
} from '../chat/chatTypes'
import { humanInteractionResponseDisplay } from './humanInteractionState'

export function readHumanInteractionDisplay(
  value: unknown
): HumanInteractionResponseDisplay | null {
  try {
    return parseHumanInteractionResponseDisplay(
      typeof value === 'string' ? JSON.parse(value) : value
    )
  } catch {
    return null
  }
}

export function humanInteractionDisplayText(display: HumanInteractionResponseDisplay): string {
  return display.answers.map((answer) => `${answer.question}\n${answer.answer}`).join('\n\n')
}

export function readHumanInteractionGuidanceDisplay(
  item: Pick<ChatGuidanceTimelineItem, 'clientMessageId' | 'guidanceId' | 'content'>
): HumanInteractionResponseDisplay | null {
  // The Host generates this client identity; ordinary Composer steering generates its own UUID.
  // A fork assigns a new guidanceId but preserves the frozen source client identity.
  if (!item.guidanceId || !item.clientMessageId.startsWith('human-answer-')) return null
  return readHumanInteractionDisplay(item.content)
}

export function humanInteractionUserDisplay(
  message: ChatMessage,
  requests: readonly HumanInteractionRequestSnapshot[]
) {
  if (message.role !== 'user') return null
  const display = readHumanInteractionDisplay(message.content)
  if (!display) return null
  const bound = requests.some(
    (request) =>
      request.status === 'submitted' &&
      request.response?.responseId === display.responseId &&
      request.requestId === display.requestId &&
      request.delivery?.userMessageId === message.id
  )
  // Forks retain frozen display material and explicit snapshot provenance, never live operations.
  return bound || message.inputOrigin?.kind === 'historical_snapshot' ? display : null
}

function displayItem(
  display: HumanInteractionResponseDisplay,
  createdAt: number,
  id: string
): ChatGuidanceTimelineItem {
  return {
    id,
    type: 'user_guidance',
    clientMessageId: `human-answer-display:${display.responseId}`,
    guidanceId: `display:${display.responseId}`,
    content: JSON.stringify(display),
    attachments: [],
    status: 'applied',
    createdAt
  }
}

/** Rendering-only projection. These copies must never be persisted or passed to send/steer APIs.
 * Sync answers remain ToolResults in the authoritative history. Delivery retries share responseId.
 */
export function projectHumanInteractionConversation(
  conversation: ChatConversation,
  requests: readonly HumanInteractionRequestSnapshot[]
): ChatConversation {
  const locations = new Map<string, string>()
  const offer = (value: unknown, location: string) => {
    const display = readHumanInteractionDisplay(value)
    if (display && !locations.has(display.responseId)) {
      locations.set(display.responseId, location)
    }
  }
  // A durable User row wins over an older guidance route which was rejected during retargeting.
  for (const message of conversation.messages)
    if (humanInteractionUserDisplay(message, requests)) offer(message.content, message.id)
  for (const status of ['applied', 'queued', 'submitting', 'rejected']) {
    for (const message of conversation.messages) {
      for (const item of message.agentRun?.timeline ?? []) {
        if (
          item.type === 'user_guidance' &&
          item.status === status &&
          readHumanInteractionGuidanceDisplay(item)
        )
          offer(item.content, `${message.id}:${item.id}`)
      }
    }
  }
  for (const message of conversation.messages) {
    for (const result of message.agentRun?.toolResults ?? []) {
      if (result.tool === 'request_user_input' && result.ok)
        offer(result.result, `${message.id}:call:${result.callId}`)
    }
  }
  const fallback = new Map<string, ChatGuidanceTimelineItem[]>()
  for (const request of [...requests].sort(
    (a, b) => (a.response?.createdAt ?? 0) - (b.response?.createdAt ?? 0) || a.sequence - b.sequence
  )) {
    const display = humanInteractionResponseDisplay(request)
    if (!display || locations.has(display.responseId)) continue
    const anchor =
      conversation.messages.find(
        (message) =>
          request.delivery?.targetRunId && message.agentRun?.runId === request.delivery.targetRunId
      ) ?? conversation.messages.find((message) => message.id === request.assistantMessageId)
    if (!anchor?.agentRun) continue
    const location = `${anchor.id}:response:${display.responseId}`
    offer(display, location)
    fallback.set(anchor.id, [
      ...(fallback.get(anchor.id) ?? []),
      displayItem(display, request.response!.createdAt, location)
    ])
  }
  const messages: ChatMessage[] = []
  for (const message of conversation.messages) {
    if (message.role === 'user') {
      const display = humanInteractionUserDisplay(message, requests)
      // Ordinary content is never deduplicated merely because it happens to contain valid JSON.
      if (!display) messages.push(message)
      else messages.push({ ...message, humanInteractionDisplay: display })
      continue
    }
    const run = message.agentRun
    if (!run) {
      messages.push(message)
      continue
    }
    const emittedCalls = new Set<string>()
    const timeline = run.timeline.flatMap<ChatAgentTimelineItem>((item) => {
      if (item.type === 'user_guidance') {
        const display = readHumanInteractionGuidanceDisplay(item)
        if (display)
          return locations.get(display.responseId) === `${message.id}:${item.id}`
            ? [item.status === 'applied' ? item : { ...item, status: 'applied' as const }]
            : []
      }
      if (item.type === 'tool_call') {
        const call = run.toolCalls.find((call) => call.id === item.callId)
        if (call?.tool === 'request_user_input' || call?.tool === 'request_user_input_async') {
          emittedCalls.add(item.callId)
          const display = readHumanInteractionDisplay(
            run.toolResults.find((result) => result.callId === item.callId)?.result
          )
          return display &&
            locations.get(display.responseId) === `${message.id}:call:${item.callId}`
            ? [
                {
                  ...displayItem(display, message.createdAt, item.id),
                  traceSequence: item.traceSequence
                }
              ]
            : []
        }
      }
      return [item]
    })
    // Restored checkpoints may contain a result before its timeline item has arrived.
    for (const result of run.toolResults) {
      if (emittedCalls.has(result.callId)) continue
      const display = readHumanInteractionDisplay(result.result)
      if (display && locations.get(display.responseId) === `${message.id}:call:${result.callId}`)
        timeline.push(displayItem(display, message.createdAt, `response:${display.responseId}`))
    }
    timeline.push(...(fallback.get(message.id) ?? []))
    messages.push(
      timeline.length === run.timeline.length &&
        timeline.every((item, index) => item === run.timeline[index])
        ? message
        : { ...message, agentRun: { ...run, timeline } }
    )
  }
  return { ...conversation, messages }
}
