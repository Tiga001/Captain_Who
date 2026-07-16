import { describe, expect, it } from 'vitest'
import { GitReviewSyntaxHighlightManager } from '../syntaxHighlighting/GitReviewSyntaxHighlightManager'
import {
  createPlainGitReviewHighlightResult,
  type GitReviewHighlightInput,
  type GitReviewHighlightWorkerMessage,
  type GitReviewHighlightWorkerResponse
} from '../syntaxHighlighting/gitReviewSyntaxHighlightTypes'

const INPUT: GitReviewHighlightInput = {
  cacheKey: 'file.ts:revision-1',
  code: 'const value = 1',
  language: 'typescript'
}

describe('GitReviewSyntaxHighlightManager', () => {
  it('deduplicates in-flight work and serves a valid result from its LRU', async () => {
    const worker = new FakeWorker()
    const manager = createManager(worker)

    const first = manager.highlight(INPUT)
    const second = manager.highlight({ ...INPUT })
    const request = worker.highlightRequests()[0]
    expect(request).toBeDefined()
    expect(worker.highlightRequests()).toHaveLength(1)
    if (!request) throw new Error('Expected a highlight request')
    worker.respond(resultResponse(request, INPUT))

    await expect(first).resolves.toMatchObject({ cacheKey: INPUT.cacheKey })
    await expect(second).resolves.toMatchObject({ cacheKey: INPUT.cacheKey })
    await expect(manager.highlight(INPUT)).resolves.toMatchObject({ cacheKey: INPUT.cacheKey })
    expect(worker.highlightRequests()).toHaveLength(1)
    manager.release()
  })

  it('cancels shared worker work only after the final consumer aborts', async () => {
    const worker = new FakeWorker()
    const manager = createManager(worker)
    const firstController = new AbortController()
    const secondController = new AbortController()
    const first = manager.highlight(INPUT, { signal: firstController.signal })
    const second = manager.highlight(INPUT, { signal: secondController.signal })

    firstController.abort()
    await expect(first).rejects.toMatchObject({ name: 'AbortError' })
    expect(worker.messages.some((message) => message.type === 'cancel')).toBe(false)

    secondController.abort()
    await expect(second).rejects.toMatchObject({ name: 'AbortError' })
    expect(worker.messages.filter((message) => message.type === 'cancel')).toHaveLength(1)
    manager.release()
  })

  it('ignores a cancelled request result after a newer request has reused the same identity', async () => {
    const worker = new FakeWorker()
    const manager = createManager(worker)
    const controller = new AbortController()
    const stalePromise = manager.highlight(INPUT, { signal: controller.signal })
    const staleRequest = worker.highlightRequests()[0]
    if (!staleRequest) throw new Error('Expected the stale request')
    controller.abort()
    await expect(stalePromise).rejects.toMatchObject({ name: 'AbortError' })

    let freshResolved = false
    const freshPromise = manager.highlight(INPUT).then((result) => {
      freshResolved = true
      return result
    })
    const freshRequest = worker.highlightRequests()[1]
    if (!freshRequest) throw new Error('Expected the fresh request')

    worker.respond(resultResponse(staleRequest, INPUT))
    await Promise.resolve()
    expect(freshResolved).toBe(false)

    worker.respond(resultResponse(freshRequest, INPUT))
    await expect(freshPromise).resolves.toMatchObject({ cacheKey: INPUT.cacheKey })
    manager.release()
  })

  it('fails closed to plain text for worker errors and malformed token styles', async () => {
    const worker = new FakeWorker()
    const manager = createManager(worker)
    const errored = manager.highlight(INPUT)
    const errorRequest = worker.highlightRequests()[0]
    if (!errorRequest) throw new Error('Expected an error request')
    worker.respond({
      generation: errorRequest.generation,
      requestId: errorRequest.requestId,
      type: 'error'
    })
    await expect(errored).resolves.toMatchObject({ mode: 'plain', reason: 'worker-error' })

    const malformedInput = { ...INPUT, cacheKey: 'malformed' }
    const malformed = manager.highlight(malformedInput)
    const malformedRequest = worker.highlightRequests()[1]
    if (!malformedRequest) throw new Error('Expected a malformed request')
    const unsafe = createPlainGitReviewHighlightResult(malformedInput, 'plain-language')
    const firstToken = unsafe.lines[0]?.tokens[0]
    const unsafeResult = {
      ...unsafe,
      lines: [
        {
          line: 0,
          tokens: firstToken ? [{ ...firstToken, color: 'red', html: '<b>unsafe</b>' }] : []
        }
      ]
    }
    worker.respond({
      generation: malformedRequest.generation,
      requestId: malformedRequest.requestId,
      result: unsafeResult,
      type: 'result'
    } as unknown as GitReviewHighlightWorkerResponse)
    await expect(malformed).resolves.toMatchObject({ mode: 'plain', reason: 'worker-error' })
    manager.release()
  })

  it('evicts the least recently used result under its entry bound', async () => {
    const worker = new FakeWorker()
    const manager = new GitReviewSyntaxHighlightManager({
      maxCacheCharacters: 10_000,
      maxCacheEntries: 1,
      workerFactory: () => worker as unknown as Worker
    })
    const firstInput = { ...INPUT, cacheKey: 'first' }
    const secondInput = { ...INPUT, cacheKey: 'second' }

    await completeNext(manager, worker, firstInput)
    await completeNext(manager, worker, secondInput)
    const repeated = manager.highlight(firstInput)
    const repeatedRequest = worker.highlightRequests()[2]
    expect(repeatedRequest).toBeDefined()
    if (!repeatedRequest) throw new Error('Expected the evicted request to run again')
    worker.respond(resultResponse(repeatedRequest, firstInput))
    await repeated
    manager.release()
  })

  it('releases the worker and resolves pending consumers with plain text', async () => {
    const worker = new FakeWorker()
    const manager = createManager(worker)
    const pending = manager.highlight(INPUT)

    manager.release()

    await expect(pending).resolves.toMatchObject({ mode: 'plain', reason: 'worker-error' })
    expect(worker.terminated).toBe(true)
    expect(worker.messages.at(-1)).toMatchObject({ type: 'dispose' })
  })
})

class FakeWorker {
  messages: GitReviewHighlightWorkerMessage[] = []
  onerror: Worker['onerror'] = null
  onmessage: Worker['onmessage'] = null
  onmessageerror: Worker['onmessageerror'] = null
  terminated = false

  postMessage(message: GitReviewHighlightWorkerMessage): void {
    this.messages.push(message)
  }

  terminate(): void {
    this.terminated = true
  }

  highlightRequests(): Array<Extract<GitReviewHighlightWorkerMessage, { type: 'highlight' }>> {
    return this.messages.filter(
      (message): message is Extract<GitReviewHighlightWorkerMessage, { type: 'highlight' }> =>
        message.type === 'highlight'
    )
  }

  respond(response: GitReviewHighlightWorkerResponse): void {
    this.onmessage?.call(
      this as unknown as Worker,
      { data: response } as MessageEvent<GitReviewHighlightWorkerResponse>
    )
  }
}

function createManager(worker: FakeWorker): GitReviewSyntaxHighlightManager {
  return new GitReviewSyntaxHighlightManager({
    workerFactory: () => worker as unknown as Worker
  })
}

function resultResponse(
  request: Extract<GitReviewHighlightWorkerMessage, { type: 'highlight' }>,
  input: GitReviewHighlightInput
): GitReviewHighlightWorkerResponse {
  return {
    generation: request.generation,
    requestId: request.requestId,
    result: createPlainGitReviewHighlightResult(input, 'plain-language'),
    type: 'result'
  }
}

async function completeNext(
  manager: GitReviewSyntaxHighlightManager,
  worker: FakeWorker,
  input: GitReviewHighlightInput
): Promise<void> {
  const result = manager.highlight(input)
  const request = worker.highlightRequests().at(-1)
  if (!request) throw new Error('Expected a request')
  worker.respond(resultResponse(request, input))
  await result
}
