import {
  parseHumanInteractionResponseDisplay,
  type HumanInteractionRequestSnapshot,
  type HumanInteractionResponseDisplay
} from '@mycopilot/protocol'
import type {
  ChatAgentTimelineItem,
  ChatAgentRunView,
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
      request.delivery?.userMessageId === message.id &&
      JSON.stringify(readHumanInteractionDisplay(humanInteractionResponseDisplay(request))) ===
        JSON.stringify(display)
  )
  // A generic historical origin also belongs to ordinary User JSON. Only Host-verified output
  // metadata proves an idle answer, including after a fork or deletion of its source conversation.
  const proven = readHumanInteractionDisplay(message.humanInteractionDisplay)
  return bound || (proven && JSON.stringify(proven) === JSON.stringify(display)) ? display : null
}

export function humanInteractionRequestsForMessage(
  message: ChatMessage,
  requests: readonly HumanInteractionRequestSnapshot[]
): HumanInteractionRequestSnapshot[] {
  const callTools = indexCallTools(message.agentRun?.toolCalls ?? [])
  return requests.filter(
    (request) =>
      request.status === 'open' &&
      request.assistantMessageId === message.id &&
      request.runId === message.agentRun?.runId &&
      matchesRequestTool(callTools, request)
  )
}

/** Independent request notifications can precede their owning message/trace snapshot. */
export function getUnanchoredHumanInteractionRequests(
  conversation: ChatConversation,
  requests: readonly HumanInteractionRequestSnapshot[]
) {
  const byMessage = new Map<string, HumanInteractionRequestSnapshot[]>()
  for (const request of requests) {
    if (request.conversationId !== conversation.id || request.status !== 'open') continue
    append(byMessage, request.assistantMessageId, request)
  }
  const anchoredIds = new Set<string>()
  for (const message of conversation.messages) {
    const candidates = byMessage.get(message.id)
    if (!candidates) continue
    for (const request of humanInteractionRequestsForMessage(message, candidates))
      anchoredIds.add(request.requestId)
  }
  return requests.filter(
    (request) =>
      request.conversationId === conversation.id &&
      request.status === 'open' &&
      !anchoredIds.has(request.requestId)
  )
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

type CallTools = Map<string, Set<string>>
function indexCallTools(calls: ChatAgentRunView['toolCalls']): CallTools {
  const tools = new Map<string, Set<string>>()
  for (const call of calls) {
    let names = tools.get(call.id)
    if (!names) tools.set(call.id, (names = new Set()))
    names.add(call.tool)
  }
  return tools
}
function matchesRequestTool(tools: CallTools, request: HumanInteractionRequestSnapshot) {
  const names = tools.get(request.toolCallId)
  const expected = request.mode === 'sync' ? 'request_user_input' : 'request_user_input_async'
  return !names || (names.size === 1 && names.has(expected))
}
function append<T>(map: Map<string, T[]>, key: string, value: T) {
  const items = map.get(key)
  if (items) items.push(value)
  else map.set(key, [value])
}
function isInteractionTool(tool: string) {
  return tool === 'request_user_input' || tool === 'request_user_input_async'
}
type Display = HumanInteractionResponseDisplay
interface RunMaterial {
  calls: ChatAgentRunView['toolCalls']
  results: ChatAgentRunView['toolResults']
  timeline: ChatAgentRunView['timeline']
  lengths: readonly number[]
  callsById: Map<string, ChatAgentRunView['toolCalls'][number]>
  resultsById: Map<string, ChatAgentRunView['toolResults'][number]>
  callTools: CallTools
  timelineCallIds: Set<string>
  guidance: Map<ChatGuidanceTimelineItem, Display>
  syncResults: { callId: string; display: Display }[]
  hasInteraction: boolean
}
function analyzeRun(run: ChatAgentRunView): RunMaterial {
  const callsById: RunMaterial['callsById'] = new Map()
  let hasInteraction = false
  for (const call of run.toolCalls) {
    // Array.find used the first duplicate, while request validation rejects any wrong duplicate.
    if (!callsById.has(call.id)) callsById.set(call.id, call)
    if (isInteractionTool(call.tool)) hasInteraction = true
  }
  const resultsById: RunMaterial['resultsById'] = new Map()
  const syncResults: RunMaterial['syncResults'] = []
  for (const result of run.toolResults) {
    if (!resultsById.has(result.callId)) resultsById.set(result.callId, result)
    if (result.tool !== 'request_user_input' || !result.ok) continue
    const display = readHumanInteractionDisplay(result.result)
    if (display) syncResults.push({ callId: result.callId, display })
  }
  const guidance: RunMaterial['guidance'] = new Map()
  const timelineCallIds = new Set<string>()
  for (const item of run.timeline) {
    if (item.type === 'tool_call') timelineCallIds.add(item.callId)
    if (item.type !== 'user_guidance') continue
    const display = readHumanInteractionGuidanceDisplay(item)
    if (display) guidance.set(item, display)
  }
  return {
    calls: run.toolCalls,
    results: run.toolResults,
    timeline: run.timeline,
    lengths: [run.toolCalls.length, run.toolResults.length, run.timeline.length],
    callsById,
    resultsById,
    callTools: indexCallTools(run.toolCalls),
    timelineCallIds,
    guidance,
    syncResults,
    hasInteraction: hasInteraction || guidance.size > 0 || syncResults.length > 0
  }
}
interface RequestVersion {
  request: HumanInteractionRequestSnapshot
  revision: number
  deliveryRevision: number | undefined
  status: HumanInteractionRequestSnapshot['status']
  deliveryStatus: string | undefined
  targetRunId: string | null | undefined
  userMessageId: string | null | undefined
  response: HumanInteractionRequestSnapshot['response']
}
function requestVersion(request: HumanInteractionRequestSnapshot): RequestVersion {
  return {
    request,
    revision: request.revision,
    deliveryRevision: request.delivery?.revision,
    status: request.status,
    deliveryStatus: request.delivery?.status,
    targetRunId: request.delivery?.targetRunId,
    userMessageId: request.delivery?.userMessageId,
    response: request.response
  }
}
function sameVersion(a: RequestVersion, b: RequestVersion) {
  return (
    a.request === b.request &&
    a.revision === b.revision &&
    a.deliveryRevision === b.deliveryRevision &&
    a.status === b.status &&
    a.deliveryStatus === b.deliveryStatus &&
    a.targetRunId === b.targetRunId &&
    a.userMessageId === b.userMessageId &&
    a.response === b.response
  )
}
interface RequestIndex {
  versions: RequestVersion[]
  revision: number
  byUserMessage: Map<string, { request: HumanInteractionRequestSnapshot; display: Display }[]>
  openByMessage: Map<string, HumanInteractionRequestSnapshot[]>
  responses: { request: HumanInteractionRequestSnapshot; display: Display }[]
}
function indexRequests(versions: RequestVersion[], revision: number): RequestIndex {
  const index: RequestIndex = {
    versions,
    revision,
    byUserMessage: new Map(),
    openByMessage: new Map(),
    responses: []
  }
  for (const { request } of versions) {
    if (request.status === 'open') append(index.openByMessage, request.assistantMessageId, request)
    const display = humanInteractionResponseDisplay(request)
    if (!display) continue
    const entry = { request, display }
    index.responses.push(entry)
    if (request.delivery?.userMessageId)
      append(index.byUserMessage, request.delivery.userMessageId, entry)
  }
  index.responses.sort(
    (a, b) =>
      a.request.response!.createdAt - b.request.response!.createdAt ||
      a.request.sequence - b.request.sequence
  )
  return index
}
interface UserMaterial {
  content: string
  proof: ChatMessage['humanInteractionDisplay']
  display: Display | null
  proven: boolean
  requestRevision?: number
  verifiedDisplay?: Display | null
}
interface ProjectedMessage {
  material: RunMaterial | UserMaterial
  requestRevision: number
  locationRevision: number
  run: ChatMessage['agentRun']
  createdAt: number
  content: string
  runId: string | null | undefined
  value: ChatMessage
}

/** Own one selector per conversation surface. Weak keys release removed/reloaded messages and runs.
 * Renderer snapshots are immutable; array replacement and appended legacy arrays also invalidate.
 * Request and delivery revisions are copied separately so a receipt arriving before/after its
 * message (or updating the same request object) cannot reuse a stale answer projection.
 */
export function createHumanInteractionConversationSelector() {
  let conversationId: string | undefined
  let runs = new WeakMap<ChatAgentRunView, RunMaterial>()
  let users = new WeakMap<ChatMessage, UserMaterial>()
  let projected = new WeakMap<ChatMessage, ProjectedMessage>()
  let requestIndex = indexRequests([], 0)
  let previousLocations = new Map<string, string>()
  let locationRevision = 0

  return (conversation: ChatConversation, requests: readonly HumanInteractionRequestSnapshot[]) => {
    if (conversationId !== conversation.id) {
      conversationId = conversation.id
      runs = new WeakMap()
      users = new WeakMap()
      projected = new WeakMap()
      requestIndex = indexRequests([], 0)
      previousLocations = new Map()
      locationRevision = 0
    }
    const versions = requests
      .filter((request) => request.conversationId === conversation.id)
      .map(requestVersion)
    if (
      versions.length !== requestIndex.versions.length ||
      versions.some((version, i) => !sameVersion(version, requestIndex.versions[i]))
    ) {
      requestIndex = indexRequests(versions, requestIndex.revision + 1)
    }
    const materials = new Map<ChatMessage, RunMaterial>()
    const userDisplays = new Map<ChatMessage, Display>()
    const messagesById = new Map<string, ChatMessage>()
    const messagesByRun = new Map<string, ChatMessage>()
    const locations = new Map<string, string>()
    const offer = (display: Display, location: string) => {
      if (!locations.has(display.responseId)) locations.set(display.responseId, location)
    }
    for (const message of conversation.messages) {
      if (!messagesById.has(message.id)) messagesById.set(message.id, message)
      const run = message.agentRun
      if (run) {
        if (run.runId && !messagesByRun.has(run.runId)) messagesByRun.set(run.runId, message)
        let material = runs.get(run)
        if (
          !material ||
          material.calls !== run.toolCalls ||
          material.results !== run.toolResults ||
          material.timeline !== run.timeline ||
          material.lengths[0] !== run.toolCalls.length ||
          material.lengths[1] !== run.toolResults.length ||
          material.lengths[2] !== run.timeline.length
        ) {
          material = analyzeRun(run)
          runs.set(run, material)
        }
        materials.set(message, material)
      }
      if (message.role !== 'user') continue
      let material = users.get(message)
      if (
        !material ||
        material.content !== message.content ||
        material.proof !== message.humanInteractionDisplay
      ) {
        const display = readHumanInteractionDisplay(message.content)
        const proof = readHumanInteractionDisplay(message.humanInteractionDisplay)
        material = {
          content: message.content,
          proof: message.humanInteractionDisplay,
          display,
          proven: !!display && !!proof && JSON.stringify(proof) === JSON.stringify(display)
        }
        users.set(message, material)
      }
      if (material.requestRevision !== requestIndex.revision) {
        const display = material.display
        const bound =
          display &&
          requestIndex.byUserMessage
            .get(message.id)
            ?.some(
              (entry) =>
                entry.request.requestId === display.requestId &&
                entry.display.responseId === display.responseId &&
                JSON.stringify(readHumanInteractionDisplay(entry.display)) ===
                  JSON.stringify(display)
            )
        material.verifiedDisplay = material.proven || bound ? display : null
        material.requestRevision = requestIndex.revision
      }
      const display = material.verifiedDisplay
      if (display) {
        userDisplays.set(message, display)
        offer(display, message.id)
      }
    }
    // Priority is durable User > applied/queued/submitting/rejected guidance > sync result.
    for (const status of ['applied', 'queued', 'submitting', 'rejected']) {
      for (const [message, material] of materials)
        for (const [item, display] of material.guidance)
          if (item.status === status) offer(display, `${message.id}:${item.id}`)
    }
    for (const [message, material] of materials)
      for (const result of material.syncResults)
        offer(result.display, `${message.id}:call:${result.callId}`)

    const fallback = new Map<string, ChatGuidanceTimelineItem[]>()
    const syncCallFallbacks = new Map<string, ChatGuidanceTimelineItem>()
    const syncFallbackMessages = new Set<string>()
    for (const { request, display } of requestIndex.responses) {
      if (locations.has(display.responseId)) continue
      const anchor =
        (request.delivery?.targetRunId
          ? messagesByRun.get(request.delivery.targetRunId)
          : undefined) ?? messagesById.get(request.assistantMessageId)
      if (!anchor?.agentRun) continue
      const material = materials.get(anchor)!
      if (
        request.mode === 'sync' &&
        anchor.id === request.assistantMessageId &&
        anchor.agentRun.runId === request.runId &&
        material.timelineCallIds.has(request.toolCallId) &&
        matchesRequestTool(material.callTools, request)
      ) {
        const location = `${anchor.id}:call:${request.toolCallId}`
        offer(display, location)
        syncCallFallbacks.set(location, displayItem(display, request.response!.createdAt, location))
        syncFallbackMessages.add(anchor.id)
      } else {
        const location = `${anchor.id}:response:${display.responseId}`
        offer(display, location)
        append(fallback, anchor.id, displayItem(display, request.response!.createdAt, location))
      }
    }
    if (
      locations.size !== previousLocations.size ||
      [...locations].some(([id, location]) => previousLocations.get(id) !== location)
    ) {
      previousLocations = locations
      locationRevision++
    }
    const callLocations = new Set(locations.values())
    const messages = conversation.messages.map((message): ChatMessage => {
      const run = message.agentRun
      const material = message.role === 'user' ? users.get(message) : materials.get(message)
      if (!material) return message
      const cached = projected.get(message)
      if (
        cached?.material === material &&
        cached.requestRevision === requestIndex.revision &&
        cached.locationRevision === locationRevision &&
        cached.run === run &&
        cached.createdAt === message.createdAt &&
        cached.content === message.content &&
        cached.runId === run?.runId
      )
        return cached.value
      const remember = (value: ChatMessage) => {
        projected.set(message, {
          material,
          requestRevision: requestIndex.revision,
          locationRevision,
          run,
          createdAt: message.createdAt,
          content: message.content,
          runId: run?.runId,
          value
        })
        return value
      }
      if (message.role === 'user') {
        const display = userDisplays.get(message)
        return remember(
          !display || message.humanInteractionDisplay === display
            ? message
            : { ...message, humanInteractionDisplay: display }
        )
      }
      if (!run) return message
      const runMaterial = material as RunMaterial
      const openRequests = (requestIndex.openByMessage.get(message.id) ?? []).filter(
        (request) =>
          request.runId === run.runId && matchesRequestTool(runMaterial.callTools, request)
      )
      // An ordinary tool-heavy run takes this fast path even without a live request table.
      if (
        !runMaterial.hasInteraction &&
        openRequests.length === 0 &&
        !fallback.has(message.id) &&
        !syncFallbackMessages.has(message.id)
      )
        return remember(message)
      const emittedCalls = new Set<string>()
      const openRequestsByCall = new Map(
        openRequests.map((request) => [request.toolCallId, request])
      )
      const timeline = run.timeline.flatMap<ChatAgentTimelineItem>((item) => {
        if (item.type === 'user_guidance') {
          const display = runMaterial.guidance.get(item)
          if (display)
            return locations.get(display.responseId) === `${message.id}:${item.id}`
              ? [item.status === 'applied' ? item : { ...item, status: 'applied' as const }]
              : []
        }
        if (item.type === 'tool_call') {
          const call = runMaterial.callsById.get(item.callId)
          const syncFallback = syncCallFallbacks.get(`${message.id}:call:${item.callId}`)
          if (
            (call && isInteractionTool(call.tool)) ||
            openRequestsByCall.has(item.callId) ||
            syncFallback
          ) {
            if (emittedCalls.has(item.callId)) return []
            emittedCalls.add(item.callId)
            const display =
              readHumanInteractionDisplay(runMaterial.resultsById.get(item.callId)?.result) ??
              readHumanInteractionDisplay(syncFallback?.content)
            return display &&
              locations.get(display.responseId) === `${message.id}:call:${item.callId}`
              ? [
                  {
                    ...displayItem(display, syncFallback?.createdAt ?? message.createdAt, item.id),
                    traceSequence: item.traceSequence
                  }
                ]
              : !display && openRequestsByCall.has(item.callId)
                ? [item]
                : []
          }
        }
        return [item]
      })
      const toolCalls = [...run.toolCalls]
      const callIds = new Set(runMaterial.callsById.keys())
      for (const request of openRequests.sort((a, b) => a.sequence - b.sequence)) {
        if (readHumanInteractionDisplay(runMaterial.resultsById.get(request.toolCallId)?.result))
          continue
        if (!callIds.has(request.toolCallId)) {
          toolCalls.push({
            id: request.toolCallId,
            tool: request.mode === 'sync' ? 'request_user_input' : 'request_user_input_async',
            args: {},
            approvalStatus: 'not_required',
            reason: null
          })
          callIds.add(request.toolCallId)
        }
        if (emittedCalls.has(request.toolCallId)) continue
        timeline.push({
          id: `human-request:${request.requestId}`,
          type: 'tool_call',
          callId: request.toolCallId
        })
        emittedCalls.add(request.toolCallId)
      }
      for (const result of run.toolResults) {
        if (
          emittedCalls.has(result.callId) ||
          !callLocations.has(`${message.id}:call:${result.callId}`)
        )
          continue
        const display = readHumanInteractionDisplay(result.result)
        if (display && locations.get(display.responseId) === `${message.id}:call:${result.callId}`)
          timeline.push(displayItem(display, message.createdAt, `response:${display.responseId}`))
      }
      timeline.push(...(fallback.get(message.id) ?? []))
      return remember(
        timeline.length === run.timeline.length &&
          toolCalls.length === run.toolCalls.length &&
          timeline.every((item, index) => item === run.timeline[index])
          ? message
          : { ...message, agentRun: { ...run, timeline, toolCalls } }
      )
    })
    return messages.every((message, index) => message === conversation.messages[index])
      ? conversation
      : { ...conversation, messages }
  }
}

/** Uncached entry point, also useful when projecting a one-off immutable history snapshot.
 * Rendering-only copies must never be persisted or passed to send/steer APIs.
 */
export function projectHumanInteractionConversation(
  conversation: ChatConversation,
  requests: readonly HumanInteractionRequestSnapshot[]
): ChatConversation {
  return createHumanInteractionConversationSelector()(conversation, requests)
}
