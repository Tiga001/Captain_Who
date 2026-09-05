import {
  HostInvocationError,
  unwrapHostInvocation,
  type HumanInteractionHostApi
} from '@mycopilot/host-api'
import {
  parseHumanInteractionListOutput,
  parseHumanInteractionRequestSnapshot,
  type HumanInteractionAnswer,
  type HumanInteractionIgnoreInput,
  type HumanInteractionRequestSnapshot,
  type HumanInteractionSubmitInput
} from '@mycopilot/protocol'
import {
  EMPTY_HUMAN_INTERACTION_DRAFT,
  humanInteractionAnswers,
  mergeHumanInteractionRequest,
  type HumanInteractionDraft
} from './humanInteractionState'

type MutationKind = 'submit' | 'ignore'
interface Attempt {
  kind: MutationKind
  input: HumanInteractionSubmitInput | HumanInteractionIgnoreInput
  uncertain: boolean
}
export interface HumanInteractionOperation {
  pendingAction: MutationKind | null
  isSubmitting: boolean
  isDraftLocked: boolean
  error: string | null
}
export interface HumanInteractionControllerSnapshot {
  readonly requests: Readonly<Record<string, HumanInteractionRequestSnapshot>>
  readonly drafts: Readonly<Record<string, HumanInteractionDraft>>
  readonly operations: Readonly<Record<string, HumanInteractionOperation>>
  readonly selected: Readonly<Record<string, string | null>>
  readonly minimized: Readonly<Record<string, boolean>>
  readonly loads: Readonly<
    Record<string, { status: 'loading' | 'ready' | 'error'; error: string | null }>
  >
}
const idleOperation: HumanInteractionOperation = {
  pendingAction: null,
  isSubmitting: false,
  isDraftLocked: false,
  error: null
}

export class HumanInteractionController {
  private state: HumanInteractionControllerSnapshot = {
    requests: {},
    drafts: {},
    operations: {},
    selected: {},
    minimized: {},
    loads: {}
  }
  private readonly listeners = new Set<() => void>()
  private readonly deleted = new Set<string>()
  private readonly touched = new Map<string, number>()
  private serial = 0
  private readonly generations = new Map<string, number>()
  private readonly attempts = new Map<string, Attempt>()
  private readonly submissionIds = new Map<string, Map<string, string>>()
  private readonly inFlight = new Map<string, Promise<void>>()
  private references = 0
  private cleanup: (() => void) | null = null
  constructor(
    readonly api: HumanInteractionHostApi,
    private readonly createId: () => string = () => crypto.randomUUID()
  ) {}
  getSnapshot = (): HumanInteractionControllerSnapshot => this.state
  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }
  private publish(patch: Partial<HumanInteractionControllerSnapshot>): void {
    this.state = { ...this.state, ...patch }
    for (const listener of this.listeners) listener()
  }
  connect(): () => void {
    this.references += 1
    if (this.references === 1) {
      const requests = this.api.onRequestChanged((request) => this.merge(request))
      const resync = this.api.onResync?.(() => {
        for (const conversationId of Object.keys(this.state.loads))
          void this.refresh(conversationId)
      })
      this.cleanup = () => {
        requests()
        resync?.()
      }
    }
    return () => {
      this.references -= 1
      if (!this.references) {
        this.cleanup?.()
        this.cleanup = null
      }
    }
  }
  merge(value: HumanInteractionRequestSnapshot): void {
    let incoming: HumanInteractionRequestSnapshot
    try {
      incoming = parseHumanInteractionRequestSnapshot(value)
    } catch {
      return
    }
    if (this.deleted.has(incoming.requestId)) return
    const previous = this.state.requests[incoming.requestId]
    const next = mergeHumanInteractionRequest(previous, incoming)
    if (previous === next) return
    this.touched.set(next.requestId, ++this.serial)
    const patch: {
      -readonly [
        Key in keyof HumanInteractionControllerSnapshot
      ]?: HumanInteractionControllerSnapshot[Key]
    } = { requests: { ...this.state.requests, [next.requestId]: next } }
    if (!previous && next.mode === 'async' && next.status === 'open') {
      const newestSequence = Object.values(this.state.requests).reduce(
        (latest, request) =>
          request.conversationId === next.conversationId &&
          request.mode === 'async' &&
          request.status === 'open'
            ? Math.max(latest, request.sequence)
            : latest,
        -1
      )
      if (next.sequence > newestSequence)
        patch.selected = { ...this.state.selected, [next.conversationId]: next.requestId }
    }
    if (next.status !== 'open') {
      this.attempts.delete(next.requestId)
      this.submissionIds.delete(next.requestId)
      patch.operations = { ...this.state.operations, [next.requestId]: idleOperation }
    }
    this.publish(patch)
  }
  async refresh(conversationId: string): Promise<boolean> {
    const generation = (this.generations.get(conversationId) ?? 0) + 1
    this.generations.set(conversationId, generation)
    const started = this.serial
    this.publish({
      loads: { ...this.state.loads, [conversationId]: { status: 'loading', error: null } }
    })
    const seen = new Set<string>(),
      cursors = new Set<string>()
    let cursor: string | null = null
    try {
      do {
        const page = parseHumanInteractionListOutput(
          unwrapHostInvocation(await this.api.listRequests({ conversationId, cursor, limit: 100 }))
        )
        if (this.generations.get(conversationId) !== generation) return false
        for (const request of page.items) {
          if (request.conversationId !== conversationId)
            throw new Error('Question list ownership mismatch')
          seen.add(request.requestId)
          this.merge(request)
        }
        cursor = page.nextCursor
        if (cursor && cursors.has(cursor)) throw new Error('Question list cursor repeated')
        if (cursor) cursors.add(cursor)
      } while (cursor)
      // Only a complete, successful scan can remove missing facts. Notifications received during
      // the scan are newer than its first page and cannot be erased by that page's cursor window.
      const requests = { ...this.state.requests }
      for (const request of Object.values(requests)) {
        if (
          request.conversationId === conversationId &&
          !seen.has(request.requestId) &&
          (this.touched.get(request.requestId) ?? 0) <= started
        ) {
          this.deleted.add(request.requestId)
          delete requests[request.requestId]
        }
      }
      this.publish({
        requests,
        loads: { ...this.state.loads, [conversationId]: { status: 'ready', error: null } }
      })
      return true
    } catch {
      if (this.generations.get(conversationId) !== generation) return false
      this.publish({
        loads: { ...this.state.loads, [conversationId]: { status: 'error', error: 'load_failed' } }
      })
      return false
    }
  }
  setPage(requestId: string, pageIndex: number): void {
    const request = this.state.requests[requestId]
    if (!request || request.status !== 'open' || !Number.isInteger(pageIndex)) return
    const draft = this.state.drafts[requestId] ?? EMPTY_HUMAN_INTERACTION_DRAFT
    this.publish({
      drafts: {
        ...this.state.drafts,
        [requestId]: {
          ...draft,
          pageIndex: Math.min(Math.max(0, pageIndex), request.questions.length - 1)
        }
      }
    })
  }
  setAnswer(requestId: string, answer: HumanInteractionAnswer): void {
    const request = this.state.requests[requestId]
    if (!request || request.status !== 'open' || this.state.operations[requestId]?.isDraftLocked)
      return
    const question = request.questions.find((question) => question.id === answer.questionId)
    if (
      !question ||
      (answer.kind === 'option' &&
        !question.options?.some((option) => option.id === answer.optionId))
    )
      return
    const draft = this.state.drafts[requestId] ?? EMPTY_HUMAN_INTERACTION_DRAFT
    this.publish({
      drafts: {
        ...this.state.drafts,
        [requestId]: { ...draft, answers: { ...draft.answers, [answer.questionId]: { ...answer } } }
      },
      operations: { ...this.state.operations, [requestId]: idleOperation }
    })
  }
  open(requestId: string): void {
    const request = this.state.requests[requestId]
    if (!request || request.status !== 'open' || request.mode !== 'async') return
    this.publish({
      selected: { ...this.state.selected, [request.conversationId]: requestId },
      minimized: { ...this.state.minimized, [requestId]: false }
    })
  }
  minimize(requestId: string): void {
    if (this.state.requests[requestId]?.mode !== 'async') return
    this.publish({ minimized: { ...this.state.minimized, [requestId]: true } })
  }
  submit(requestId: string): Promise<void> {
    return this.mutate(requestId, 'submit')
  }
  ignore(requestId: string): Promise<void> {
    return this.mutate(requestId, 'ignore')
  }
  private mutate(requestId: string, kind: MutationKind): Promise<void> {
    const existing = this.inFlight.get(requestId)
    if (existing) return existing
    const request = this.state.requests[requestId]
    if (!request || request.status !== 'open' || (kind === 'ignore' && request.mode !== 'async'))
      return Promise.resolve()
    let attempt = this.attempts.get(requestId)
    if (attempt && attempt.kind !== kind) return Promise.resolve()
    if (!attempt) {
      const base = {
        conversationId: request.conversationId,
        requestId,
        expectedRevision: request.revision,
        submissionId: ''
      }
      const answers =
        kind === 'submit'
          ? humanInteractionAnswers(
              request,
              this.state.drafts[requestId] ?? EMPTY_HUMAN_INTERACTION_DRAFT
            )
          : null
      if (kind === 'submit' && !answers) return Promise.resolve()
      const fingerprint = JSON.stringify({ kind, revision: request.revision, answers })
      const identities = this.submissionIds.get(requestId) ?? new Map<string, string>()
      const identity = identities.get(fingerprint) ?? this.createId()
      identities.set(fingerprint, identity)
      this.submissionIds.set(requestId, identities)
      base.submissionId = identity
      attempt = { kind, input: answers ? { ...base, answers } : base, uncertain: false }
      this.attempts.set(requestId, attempt)
    }
    const frozen = attempt
    this.publish({
      operations: {
        ...this.state.operations,
        [requestId]: { pendingAction: kind, isSubmitting: true, isDraftLocked: true, error: null }
      }
    })
    const promise = Promise.resolve().then(async () => {
      try {
        const result =
          frozen.kind === 'submit'
            ? await this.api.submit(frozen.input as HumanInteractionSubmitInput)
            : await this.api.ignore(frozen.input)
        const snapshot = parseHumanInteractionRequestSnapshot(unwrapHostInvocation(result))
        if (
          snapshot.requestId !== requestId ||
          snapshot.conversationId !== request.conversationId ||
          snapshot.status === 'open' ||
          mergeHumanInteractionRequest(request, snapshot).status === 'open'
        )
          throw new Error('Invalid settlement receipt')
        this.merge(snapshot)
      } catch (error) {
        if (this.state.requests[requestId]?.status !== 'open' || this.deleted.has(requestId)) return
        const data =
          error instanceof HostInvocationError
            ? (error.data as { type?: string; code?: string } | null)
            : null
        const definitive =
          data?.type === 'human_interaction_error' &&
          ['invalid_input', 'conflict', 'not_found'].includes(data.code ?? '')
        frozen.uncertain = !definitive
        if (definitive) this.attempts.delete(requestId)
        this.publish({
          operations: {
            ...this.state.operations,
            [requestId]: {
              pendingAction: definitive ? null : kind,
              isSubmitting: false,
              isDraftLocked: !definitive,
              error: definitive ? 'state_changed' : 'outcome_unknown'
            }
          }
        })
        await this.refresh(request.conversationId)
      } finally {
        this.inFlight.delete(requestId)
        const operation = this.state.operations[requestId]
        if (operation?.isSubmitting)
          this.publish({
            operations: {
              ...this.state.operations,
              [requestId]: { ...operation, isSubmitting: false }
            }
          })
      }
    })
    this.inFlight.set(requestId, promise)
    return promise
  }
}
