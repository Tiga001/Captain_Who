import {
  assessGitReviewHighlightBudget,
  createPlainGitReviewHighlightResult,
  isValidGitReviewHighlightResult,
  normalizeGitReviewHighlightBudget,
  normalizeGitReviewLanguage,
  type GitReviewHighlightBudget,
  type GitReviewHighlightInput,
  type GitReviewHighlightResult,
  type GitReviewHighlightWorkerMessage,
  type GitReviewHighlightWorkerResponse
} from './gitReviewSyntaxHighlightTypes'

export interface GitReviewSyntaxHighlightManagerOptions {
  budget?: Partial<GitReviewHighlightBudget>
  maxCacheCharacters?: number
  maxCacheEntries?: number
  /** Test seam; production always uses the module worker below. */
  workerFactory?: () => Worker
}

export interface GitReviewHighlightCallOptions {
  signal?: AbortSignal
}

interface HighlightConsumer {
  abortListener?: () => void
  reject: (error: Error) => void
  resolve: (result: GitReviewHighlightResult) => void
  signal?: AbortSignal
}

interface PendingHighlight {
  consumers: Map<symbol, HighlightConsumer>
  generation: number
  identity: string
  input: GitReviewHighlightInput
  requestId: number
}

const DEFAULT_MAX_CACHE_ENTRIES = 64
const DEFAULT_MAX_CACHE_CHARACTERS = 2_000_000

/**
 * Owns one lazy module worker, request coalescing and the renderer-side safety boundary.
 * Worker failures are intentionally data failures: callers receive plain tokens and keep painting.
 */
export class GitReviewSyntaxHighlightManager {
  private readonly budget: GitReviewHighlightBudget
  private readonly cache: WeightedLru<GitReviewHighlightResult>
  private readonly workerFactory: () => Worker
  private readonly inFlightByIdentity = new Map<string, PendingHighlight>()
  private readonly pendingByRequestId = new Map<number, PendingHighlight>()
  private disposed = false
  private generation = 0
  private nextRequestId = 1
  private worker?: Worker

  constructor(options: GitReviewSyntaxHighlightManagerOptions = {}) {
    this.budget = normalizeGitReviewHighlightBudget(options.budget)
    this.cache = new WeightedLru(
      positiveInteger(options.maxCacheEntries, DEFAULT_MAX_CACHE_ENTRIES),
      positiveInteger(options.maxCacheCharacters, DEFAULT_MAX_CACHE_CHARACTERS)
    )
    this.workerFactory =
      options.workerFactory ??
      (() =>
        new Worker(new URL('./gitReviewSyntaxHighlight.worker.ts', import.meta.url), {
          name: 'git-review-syntax-highlighter',
          type: 'module'
        }))
  }

  highlight(
    input: GitReviewHighlightInput,
    options: GitReviewHighlightCallOptions = {}
  ): Promise<GitReviewHighlightResult> {
    if (options.signal?.aborted) return Promise.reject(createAbortError())

    const normalizedInput: GitReviewHighlightInput = {
      ...input,
      language: normalizeGitReviewLanguage(input.language)
    }
    if (this.disposed) {
      return Promise.resolve(createPlainGitReviewHighlightResult(normalizedInput, 'worker-error'))
    }

    if (normalizedInput.code.length === 0) {
      return Promise.resolve(createPlainGitReviewHighlightResult(normalizedInput, 'empty'))
    }

    const assessment = assessGitReviewHighlightBudget(normalizedInput.code, this.budget)
    if (assessment.wholeDocumentFallback) {
      return Promise.resolve(
        createPlainGitReviewHighlightResult(normalizedInput, assessment.wholeDocumentFallback)
      )
    }

    const identity = createRequestIdentity(normalizedInput)
    const cached = this.cache.get(identity)
    if (cached) return Promise.resolve(cached)

    const existing = this.inFlightByIdentity.get(identity)
    if (existing) return this.subscribe(existing, options.signal)

    let worker: Worker
    try {
      worker = this.ensureWorker()
    } catch {
      return Promise.resolve(createPlainGitReviewHighlightResult(normalizedInput, 'worker-error'))
    }

    const pending: PendingHighlight = {
      consumers: new Map(),
      generation: this.generation,
      identity,
      input: normalizedInput,
      requestId: this.nextRequestId
    }
    this.nextRequestId += 1
    this.inFlightByIdentity.set(identity, pending)
    this.pendingByRequestId.set(pending.requestId, pending)

    const resultPromise = this.subscribe(pending, options.signal)
    const message: GitReviewHighlightWorkerMessage = {
      generation: pending.generation,
      input: { ...normalizedInput, budget: this.budget },
      requestId: pending.requestId,
      type: 'highlight'
    }
    try {
      worker.postMessage(message)
    } catch {
      this.complete(
        pending,
        createPlainGitReviewHighlightResult(normalizedInput, 'worker-error'),
        false
      )
      this.failWorker(worker, pending.generation)
    }
    return resultPromise
  }

  release(): void {
    if (this.disposed) return
    this.disposed = true
    this.cache.clear()

    const worker = this.worker
    this.worker = undefined
    if (worker) {
      try {
        const message: GitReviewHighlightWorkerMessage = {
          generation: this.generation,
          type: 'dispose'
        }
        worker.postMessage(message)
      } catch {
        // The worker is already unavailable; termination below is still safe.
      }
      detachWorker(worker)
      worker.terminate()
    }

    for (const pending of [...this.pendingByRequestId.values()]) {
      this.complete(
        pending,
        createPlainGitReviewHighlightResult(pending.input, 'worker-error'),
        false
      )
    }
  }

  private subscribe(
    pending: PendingHighlight,
    signal?: AbortSignal
  ): Promise<GitReviewHighlightResult> {
    if (signal?.aborted) return Promise.reject(createAbortError())

    return new Promise((resolve, reject) => {
      const id = Symbol('git-review-highlight-consumer')
      const consumer: HighlightConsumer = { reject, resolve, signal }
      pending.consumers.set(id, consumer)

      if (signal) {
        consumer.abortListener = () => {
          if (!pending.consumers.delete(id)) return
          detachAbortListener(consumer)
          reject(createAbortError())
          if (pending.consumers.size === 0) this.cancel(pending)
        }
        signal.addEventListener('abort', consumer.abortListener, { once: true })
        // Covers an abort that raced with listener registration.
        if (signal.aborted) consumer.abortListener()
      }
    })
  }

  private ensureWorker(): Worker {
    if (this.disposed) throw new Error('Syntax highlighter has been released')
    if (this.worker) return this.worker

    const worker = this.workerFactory()
    const generation = this.generation + 1
    this.generation = generation
    worker.onmessage = (event: MessageEvent<GitReviewHighlightWorkerResponse>) => {
      this.handleWorkerMessage(worker, generation, event.data)
    }
    worker.onerror = (event) => {
      event.preventDefault()
      this.failWorker(worker, generation)
    }
    worker.onmessageerror = () => {
      this.failWorker(worker, generation)
    }
    this.worker = worker
    return worker
  }

  private handleWorkerMessage(
    worker: Worker,
    generation: number,
    response: GitReviewHighlightWorkerResponse
  ): void {
    if (
      worker !== this.worker ||
      generation !== this.generation ||
      !isWorkerResponseEnvelope(response) ||
      response.generation !== generation
    ) {
      return
    }

    const pending = this.pendingByRequestId.get(response.requestId)
    if (!pending || pending.generation !== generation) return

    if (
      response.type === 'error' ||
      !isValidGitReviewHighlightResult(response.result, pending.input)
    ) {
      this.complete(
        pending,
        createPlainGitReviewHighlightResult(pending.input, 'worker-error'),
        false
      )
      return
    }

    this.complete(pending, response.result, true)
  }

  private complete(
    pending: PendingHighlight,
    result: GitReviewHighlightResult,
    cacheResult: boolean
  ): void {
    if (this.pendingByRequestId.get(pending.requestId) !== pending) return
    this.pendingByRequestId.delete(pending.requestId)
    if (this.inFlightByIdentity.get(pending.identity) === pending) {
      this.inFlightByIdentity.delete(pending.identity)
    }
    if (cacheResult)
      this.cache.set(pending.identity, result, Math.max(1, pending.input.code.length))

    const consumers = [...pending.consumers.values()]
    pending.consumers.clear()
    for (const consumer of consumers) {
      detachAbortListener(consumer)
      consumer.resolve(result)
    }
  }

  private cancel(pending: PendingHighlight): void {
    if (this.pendingByRequestId.get(pending.requestId) !== pending) return
    this.pendingByRequestId.delete(pending.requestId)
    if (this.inFlightByIdentity.get(pending.identity) === pending) {
      this.inFlightByIdentity.delete(pending.identity)
    }

    const worker = this.worker
    if (!worker || pending.generation !== this.generation) return
    try {
      const message: GitReviewHighlightWorkerMessage = {
        generation: pending.generation,
        requestId: pending.requestId,
        type: 'cancel'
      }
      worker.postMessage(message)
    } catch {
      this.failWorker(worker, pending.generation)
    }
  }

  private failWorker(worker: Worker, generation: number): void {
    if (worker !== this.worker || generation !== this.generation) return
    this.worker = undefined
    detachWorker(worker)
    worker.terminate()

    for (const pending of [...this.pendingByRequestId.values()]) {
      if (pending.generation !== generation) continue
      this.complete(
        pending,
        createPlainGitReviewHighlightResult(pending.input, 'worker-error'),
        false
      )
    }
  }
}

class WeightedLru<T> {
  private readonly entries = new Map<string, { value: T; weight: number }>()
  private totalWeight = 0

  constructor(
    private readonly maxEntries: number,
    private readonly maxWeight: number
  ) {}

  get(key: string): T | undefined {
    const entry = this.entries.get(key)
    if (!entry) return undefined
    this.entries.delete(key)
    this.entries.set(key, entry)
    return entry.value
  }

  set(key: string, value: T, weight: number): void {
    if (weight > this.maxWeight) return
    const previous = this.entries.get(key)
    if (previous) {
      this.totalWeight -= previous.weight
      this.entries.delete(key)
    }
    this.entries.set(key, { value, weight })
    this.totalWeight += weight

    while (this.entries.size > this.maxEntries || this.totalWeight > this.maxWeight) {
      const oldestKey = this.entries.keys().next().value
      if (oldestKey === undefined) break
      const oldest = this.entries.get(oldestKey)
      this.entries.delete(oldestKey)
      if (oldest) this.totalWeight -= oldest.weight
    }
  }

  clear(): void {
    this.entries.clear()
    this.totalWeight = 0
  }
}

let sharedManager: GitReviewSyntaxHighlightManager | undefined

export function getGitReviewSyntaxHighlightManager(): GitReviewSyntaxHighlightManager {
  sharedManager ??= new GitReviewSyntaxHighlightManager()
  return sharedManager
}

export function releaseGitReviewSyntaxHighlightManager(): void {
  sharedManager?.release()
  sharedManager = undefined
}

function createRequestIdentity(input: GitReviewHighlightInput): string {
  // Length prefixes make the identity collision-free even when paths contain separator characters.
  return `${input.cacheKey.length}:${input.cacheKey}${input.language.length}:${input.language}${input.code}`
}

function isWorkerResponseEnvelope(value: unknown): value is GitReviewHighlightWorkerResponse {
  if (!value || typeof value !== 'object') return false
  const candidate = value as Partial<GitReviewHighlightWorkerResponse>
  return (
    (candidate.type === 'result' || candidate.type === 'error') &&
    Number.isInteger(candidate.generation) &&
    Number.isInteger(candidate.requestId)
  )
}

function detachWorker(worker: Worker): void {
  worker.onmessage = null
  worker.onerror = null
  worker.onmessageerror = null
}

function detachAbortListener(consumer: HighlightConsumer): void {
  if (consumer.signal && consumer.abortListener) {
    consumer.signal.removeEventListener('abort', consumer.abortListener)
  }
}

function createAbortError(): Error {
  const error = new Error('Syntax highlight request was cancelled')
  error.name = 'AbortError'
  return error
}

function positiveInteger(value: number | undefined, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) && value >= 1
    ? Math.floor(value)
    : fallback
}
