import { useEffect, useMemo, useSyncExternalStore } from 'react'
import { unwrapHostInvocation, type HumanInteractionHostApi } from '@mycopilot/host-api'
import {
  parseHumanInteractionListOutput,
  parseHumanInteractionRequestSnapshot,
  type HumanInteractionRequestSnapshot
} from '@mycopilot/protocol'
import { hostClient } from '../../host/hostClient'
import { mergeHumanInteractionRequest } from '../humanInteraction/humanInteractionState'
import type { ChatConversation } from './chatTypes'

export type ConversationAttentionById = Readonly<
  Record<string, { waitingApproval: boolean; waitingAnswer: boolean; unread: boolean }>
>

const EMPTY_QUESTIONS: Readonly<Record<string, boolean>> = {}
const emptySubscribe = () => () => {}
const emptySnapshot = () => EMPTY_QUESTIONS

/** A lightweight all-conversation projection; no message bodies or answer drafts are loaded. */
class ConversationQuestions {
  private ids = new Set<string>()
  private readonly requests = new Map<string, HumanInteractionRequestSnapshot>()
  private readonly touched = new Map<string, number>()
  private readonly deleted = new Set<string>()
  private readonly known = new Set<string>()
  private readonly queue = new Set<string>()
  private readonly inFlight = new Set<string>()
  private readonly listeners = new Set<() => void>()
  private snapshot = EMPTY_QUESTIONS
  private serial = 0
  private connected = false

  constructor(private readonly api: HumanInteractionHostApi) {}

  getSnapshot = () => this.snapshot
  subscribe = (listener: () => void) => {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  private publish() {
    const next: Record<string, boolean> = {}
    for (const id of this.known) if (this.ids.has(id)) next[id] = false
    for (const request of this.requests.values())
      if (this.ids.has(request.conversationId) && request.status === 'open')
        next[request.conversationId] = true
    if (JSON.stringify(next) === JSON.stringify(this.snapshot)) return
    this.snapshot = next
    for (const listener of this.listeners) listener()
  }

  private merge(request: HumanInteractionRequestSnapshot) {
    if (this.deleted.has(request.requestId)) return
    const previous = this.requests.get(request.requestId)
    const next = mergeHumanInteractionRequest(previous, request)
    if (previous === next) return
    this.requests.set(next.requestId, next)
    this.touched.set(next.requestId, ++this.serial)
  }

  watch(ids: string[]) {
    const previous = this.ids
    this.ids = new Set(ids)
    for (const id of ids) if (!previous.has(id)) this.queue.add(id)
    this.publish()
    this.pump()
  }

  private refresh = () => {
    for (const id of this.ids) this.queue.add(id)
    this.pump()
  }

  private pump() {
    if (!this.connected) return
    for (const id of this.queue) {
      if (this.inFlight.size >= 4) break
      if (this.inFlight.has(id)) continue
      this.queue.delete(id)
      if (!this.ids.has(id)) continue
      this.inFlight.add(id)
      void this.load(id).finally(() => {
        this.inFlight.delete(id)
        this.pump()
      })
    }
  }

  private async load(conversationId: string) {
    const started = this.serial
    const scanned = new Map<string, HumanInteractionRequestSnapshot>()
    const cursors = new Set<string>()
    let cursor: string | null = null
    try {
      do {
        const page = parseHumanInteractionListOutput(
          unwrapHostInvocation(await this.api.listRequests({ conversationId, cursor, limit: 100 }))
        )
        if (!this.connected || !this.ids.has(conversationId)) return
        for (const request of page.items) {
          if (request.conversationId !== conversationId) throw new Error('Question owner mismatch')
          scanned.set(request.requestId, request)
        }
        cursor = page.nextCursor
        if (cursor && cursors.has(cursor)) throw new Error('Question cursor repeated')
        if (cursor) cursors.add(cursor)
      } while (cursor)
      for (const request of scanned.values()) this.merge(request)
      // A stale scan must not remove questions received after its first page was requested.
      for (const request of this.requests.values()) {
        if (
          request.conversationId === conversationId &&
          !scanned.has(request.requestId) &&
          (this.touched.get(request.requestId) ?? 0) <= started
        ) {
          this.requests.delete(request.requestId)
          this.deleted.add(request.requestId)
        }
      }
      this.known.add(conversationId)
      this.publish()
    } catch {
      // Preserve the last known attention state until a successful recovery scan.
    }
  }

  connect() {
    this.connected = true
    const unsubscribe = this.api.onRequestChanged((value) => {
      try {
        const request = parseHumanInteractionRequestSnapshot(value)
        this.merge(request)
        this.known.add(request.conversationId)
        this.publish()
      } catch {
        // Ignore invalid notifications; focus/resync restores authoritative data.
      }
    })
    const unsubscribeResync = this.api.onResync?.(this.refresh)
    const refreshVisible = () => {
      if (document.visibilityState !== 'hidden') this.refresh()
    }
    window.addEventListener('focus', refreshVisible)
    window.addEventListener('online', refreshVisible)
    document.addEventListener('visibilitychange', refreshVisible)
    this.pump()
    return () => {
      this.connected = false
      unsubscribe()
      unsubscribeResync?.()
      window.removeEventListener('focus', refreshVisible)
      window.removeEventListener('online', refreshVisible)
      document.removeEventListener('visibilitychange', refreshVisible)
    }
  }
}

export function useConversationAttention(
  conversations: readonly ChatConversation[]
): ConversationAttentionById {
  const api = hostClient.humanInteraction
  const questions = useMemo(
    () =>
      typeof api?.listRequests === 'function' && typeof api?.onRequestChanged === 'function'
        ? new ConversationQuestions(api)
        : null,
    [api]
  )
  const waitingAnswers = useSyncExternalStore(
    questions?.subscribe ?? emptySubscribe,
    questions?.getSnapshot ?? emptySnapshot
  )
  const ids = JSON.stringify(
    conversations.filter((item) => !item.archivedAt).map((item) => item.id)
  )
  useEffect(() => questions?.connect(), [questions])
  useEffect(() => questions?.watch(JSON.parse(ids)), [questions, ids])
  const attentionSnapshot = JSON.stringify(
    Object.fromEntries(
      conversations.map((conversation) => [
        conversation.id,
        {
          waitingApproval: conversation.messages.some(
            (message) =>
              message.role === 'assistant' && message.agentRun?.status === 'waiting_for_approval'
          ),
          waitingAnswer:
            waitingAnswers[conversation.id] ??
            conversation.messages.some(
              (message) =>
                message.role === 'assistant' &&
                message.agentRun?.status === 'waiting_for_user_input'
            ),
          unread: Boolean(conversation.unreadAt)
        }
      ])
    )
  )
  // Token-only updates keep the projection reference stable for memoized sidebar/graph views.
  return useMemo(
    () => JSON.parse(attentionSnapshot) as ConversationAttentionById,
    [attentionSnapshot]
  )
}
