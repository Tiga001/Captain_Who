import {
  normalizeGitReviewHighlightBudget,
  type GitReviewHighlightWorkerMessage,
  type GitReviewHighlightWorkerResponse
} from './gitReviewSyntaxHighlightTypes'
import {
  disposeGitReviewSyntaxHighlighter,
  highlightGitReviewCodeInWorker
} from './gitReviewSyntaxHighlightWorkerCore'

interface GitReviewHighlightWorkerScope {
  addEventListener(
    type: 'message',
    listener: (event: MessageEvent<GitReviewHighlightWorkerMessage>) => void
  ): void
  close(): void
  postMessage(message: GitReviewHighlightWorkerResponse): void
}

const workerScope = self as unknown as GitReviewHighlightWorkerScope
const cancelledRequests = new Set<string>()
let disposed = false
let queue: Promise<void> = Promise.resolve()

workerScope.addEventListener('message', (event) => {
  const message = event.data

  if (message.type === 'cancel') {
    cancelledRequests.add(requestKey(message.generation, message.requestId))
    return
  }

  if (message.type === 'dispose') {
    disposed = true
    void queue.finally(async () => {
      await disposeGitReviewSyntaxHighlighter()
      workerScope.close()
    })
    return
  }

  // A single queue keeps Shiki's mutable grammar registry deterministic while cancellation
  // messages remain immediately observable during asynchronous grammar loading.
  queue = queue.then(
    () => processHighlightRequest(message),
    () => processHighlightRequest(message)
  )
})

async function processHighlightRequest(
  message: Extract<GitReviewHighlightWorkerMessage, { type: 'highlight' }>
): Promise<void> {
  const key = requestKey(message.generation, message.requestId)
  if (disposed || cancelledRequests.delete(key)) return

  try {
    const result = await highlightGitReviewCodeInWorker({
      ...message.input,
      budget: normalizeGitReviewHighlightBudget(message.input.budget)
    })
    if (disposed || cancelledRequests.delete(key)) return
    workerScope.postMessage({
      generation: message.generation,
      requestId: message.requestId,
      result,
      type: 'result'
    })
  } catch {
    if (disposed || cancelledRequests.delete(key)) return
    workerScope.postMessage({
      generation: message.generation,
      requestId: message.requestId,
      type: 'error'
    })
  }
}

function requestKey(generation: number, requestId: number): string {
  return `${generation}:${requestId}`
}
