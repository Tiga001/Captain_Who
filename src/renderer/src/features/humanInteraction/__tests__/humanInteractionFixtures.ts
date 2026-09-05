import type { HumanInteractionHostApi, HostInvocationResult } from '@mycopilot/host-api'
import type {
  HumanInteractionListInput,
  HumanInteractionListOutput,
  HumanInteractionRequestSnapshot,
  HumanInteractionSubmitInput
} from '@mycopilot/protocol'
import { vi } from 'vitest'
export function question(
  id = 'question',
  sequence = 1,
  conversationId = 'chat'
): HumanInteractionRequestSnapshot {
  return {
    schemaVersion: 1,
    requestId: id,
    sequence,
    conversationId,
    runId: `run-${conversationId}`,
    assistantMessageId: `assistant-${conversationId}`,
    toolCallId: `call-${id}`,
    mode: 'async',
    status: 'open',
    revision: 0,
    policyRevision: 0,
    questions: [
      { id: `${id}-one`, title: 'Choose', options: [{ id: `${id}-option`, label: 'Blue' }] },
      { id: `${id}-two`, title: 'Details', options: null }
    ],
    response: null,
    delivery: null,
    createdAt: sequence,
    updatedAt: sequence
  }
}
export function submitted(
  request: HumanInteractionRequestSnapshot,
  input?: HumanInteractionSubmitInput
): HumanInteractionRequestSnapshot {
  const responseId = `response-${request.requestId}`
  return {
    ...request,
    status: 'submitted',
    revision: 1,
    updatedAt: 20,
    response: {
      responseId,
      requestId: request.requestId,
      submissionId: input?.submissionId ?? 'submit',
      kind: 'submitted',
      answers:
        input?.answers ?? request.questions.map((q) => ({ kind: 'skipped', questionId: q.id })),
      createdAt: 20
    },
    delivery: {
      responseId,
      status: 'pending',
      revision: 0,
      targetRunId: null,
      userMessageId: null,
      errorCode: null
    }
  }
}
export function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (error: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}
export function fakeHost(initial: HumanInteractionRequestSnapshot[] = []) {
  const database = new Map(initial.map((request) => [request.requestId, request]))
  const requestListeners = new Set<(request: HumanInteractionRequestSnapshot) => void>()
  const resyncListeners = new Set<() => void>()
  const api = {
    getSettings: vi.fn(async () => ({
      ok: true as const,
      value: { enabled: true, revision: 0, updatedAt: 0 }
    })),
    updateSettings: vi.fn(),
    listRequests: vi.fn(
      async ({
        conversationId
      }: HumanInteractionListInput): Promise<HostInvocationResult<HumanInteractionListOutput>> => ({
        ok: true as const,
        value: {
          items: [...database.values()]
            .filter((request) => request.conversationId === conversationId)
            .sort((a, b) => b.sequence - a.sequence),
          nextCursor: null as string | null
        }
      })
    ),
    submit: vi.fn(
      async (
        input: HumanInteractionSubmitInput
      ): Promise<HostInvocationResult<HumanInteractionRequestSnapshot>> => {
        const result = submitted(database.get(input.requestId)!, input)
        database.set(result.requestId, result)
        return { ok: true, value: result }
      }
    ),
    ignore: vi.fn(async (input): Promise<HostInvocationResult<HumanInteractionRequestSnapshot>> => {
      const request = database.get(input.requestId)!
      const result: HumanInteractionRequestSnapshot = {
        ...request,
        status: 'ignored',
        revision: 1,
        updatedAt: 20,
        response: {
          responseId: `response-${request.requestId}`,
          requestId: request.requestId,
          submissionId: input.submissionId,
          kind: 'ignored',
          answers: [],
          createdAt: 20
        },
        delivery: null
      }
      database.set(result.requestId, result)
      return { ok: true, value: result }
    }),
    onSettingsChanged: vi.fn(() => () => {}),
    onRequestChanged: (handler) => {
      requestListeners.add(handler)
      return () => {
        requestListeners.delete(handler)
      }
    },
    onResync: (handler) => {
      resyncListeners.add(handler)
      return () => {
        resyncListeners.delete(handler)
      }
    }
  } satisfies HumanInteractionHostApi
  return {
    api,
    database,
    requestListeners,
    resyncListeners,
    notify(request: HumanInteractionRequestSnapshot) {
      database.set(request.requestId, request)
      for (const handler of requestListeners) handler(request)
    }
  }
}
