import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import type {
  GitReviewHighlightWorkerMessage,
  GitReviewHighlightWorkerResponse
} from '../syntaxHighlighting/gitReviewSyntaxHighlightTypes'

const listeners: Array<(event: MessageEvent<GitReviewHighlightWorkerMessage>) => void> = []
const responses: GitReviewHighlightWorkerResponse[] = []
const originalSelfDescriptor = Object.getOwnPropertyDescriptor(globalThis, 'self')
let closed = false

const scope = {
  addEventListener(
    type: 'message',
    listener: (event: MessageEvent<GitReviewHighlightWorkerMessage>) => void
  ): void {
    if (type === 'message') listeners.push(listener)
  },
  close(): void {
    closed = true
  },
  postMessage(response: GitReviewHighlightWorkerResponse): void {
    responses.push(response)
  }
}

beforeAll(async () => {
  Object.defineProperty(globalThis, 'self', {
    configurable: true,
    value: scope
  })
  await import('../syntaxHighlighting/gitReviewSyntaxHighlight.worker')
})

afterAll(async () => {
  dispatch({ generation: 1, type: 'dispose' })
  await flushMicrotasks()
  if (originalSelfDescriptor) {
    Object.defineProperty(globalThis, 'self', originalSelfDescriptor)
  } else {
    Reflect.deleteProperty(globalThis, 'self')
  }
})

describe('Git review syntax worker cancellation lifecycle', () => {
  it('ignores a late cancellation after a request has completed', async () => {
    const request = createRequest()
    dispatch(request)
    await flushMicrotasks()
    expect(responses).toHaveLength(1)

    dispatch({ generation: request.generation, requestId: request.requestId, type: 'cancel' })
    // Reusing an id is a test probe: a leaked late-cancellation key would swallow this request.
    dispatch(request)
    await flushMicrotasks()

    expect(responses).toHaveLength(2)
    expect(responses[1]).toMatchObject({
      generation: request.generation,
      requestId: request.requestId,
      type: 'result'
    })
    expect(closed).toBe(false)
  })
})

function createRequest(): Extract<GitReviewHighlightWorkerMessage, { type: 'highlight' }> {
  return {
    generation: 1,
    input: {
      budget: {
        maxContentCharacters: 1_000,
        maxLines: 100,
        tokenizeMaxLineLength: 100,
        tokenizeTimeLimitMs: 100
      },
      cacheKey: 'late-cancel',
      code: 'plain text',
      language: 'text'
    },
    requestId: 1,
    type: 'highlight'
  }
}

function dispatch(message: GitReviewHighlightWorkerMessage): void {
  const listener = listeners[0]
  if (!listener) throw new Error('Worker message listener was not registered')
  listener({ data: message } as MessageEvent<GitReviewHighlightWorkerMessage>)
}

async function flushMicrotasks(): Promise<void> {
  for (let index = 0; index < 8; index += 1) await Promise.resolve()
}
