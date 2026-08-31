import type { StorageComposerDraftMessageUpdate } from '@mycopilot/protocol'
import type { ChatComposerDraft } from '../features/chat/chatTypes'

export const COMPOSER_DRAFT_MESSAGE_DEBOUNCE_MS = 300

interface ComposerDraftPersistenceAdapter {
  saveDraft: (scopeId: string, draft: ChatComposerDraft) => Promise<unknown>
  saveMessage: (input: StorageComposerDraftMessageUpdate) => Promise<boolean>
}

interface PendingMessageSave {
  draft: ChatComposerDraft
  timer: ReturnType<typeof setTimeout>
}

/**
 * Serializes saves per draft scope while coalescing rapidly changing text. Full saves remain
 * immediate and supersede any pending text-only snapshot, so an older timer can never restore
 * attachments, Skills, or queue state that the user already changed.
 */
export class ComposerDraftPersistenceQueue {
  private readonly chains = new Map<string, Promise<void>>()
  private readonly discardedScopes = new Set<string>()
  private readonly pendingMessages = new Map<string, PendingMessageSave>()
  private sealedForQuit = false

  constructor(
    private readonly adapter: ComposerDraftPersistenceAdapter,
    private readonly onError: (error: unknown) => void,
    private readonly debounceMs = COMPOSER_DRAFT_MESSAGE_DEBOUNCE_MS
  ) {}

  scheduleMessage(scopeId: string, draft: ChatComposerDraft): void {
    if (this.discardedScopes.has(scopeId)) return
    if (this.sealedForQuit) {
      this.cancelPendingMessage(scopeId)
      void this.enqueueMessage(scopeId, draft)
      return
    }

    const pending = this.pendingMessages.get(scopeId)
    if (pending) clearTimeout(pending.timer)

    const timer = setTimeout(() => {
      void this.flushScope(scopeId)
    }, this.debounceMs)
    this.pendingMessages.set(scopeId, { draft, timer })
  }

  persistNow(scopeId: string, draft: ChatComposerDraft): Promise<void> {
    if (this.discardedScopes.has(scopeId)) return Promise.resolve()
    this.cancelPendingMessage(scopeId)
    return this.enqueue(scopeId, () => this.adapter.saveDraft(scopeId, draft))
  }

  flushScope(scopeId: string): Promise<void> {
    const pending = this.pendingMessages.get(scopeId)
    if (!pending) return this.chains.get(scopeId) ?? Promise.resolve()

    clearTimeout(pending.timer)
    this.pendingMessages.delete(scopeId)
    return this.enqueueMessage(scopeId, pending.draft)
  }

  async flushAll(): Promise<void> {
    do {
      for (const scopeId of [...this.pendingMessages.keys()]) void this.flushScope(scopeId)
      await Promise.all([...this.chains.values()])
    } while (this.pendingMessages.size > 0 || this.chains.size > 0)
  }

  sealAndFlushAll(): Promise<void> {
    this.sealedForQuit = true
    return this.flushAll()
  }

  async discardScope(scopeId: string): Promise<void> {
    this.discardedScopes.add(scopeId)
    this.cancelPendingMessage(scopeId)
    do {
      await (this.chains.get(scopeId) ?? Promise.resolve())
    } while (this.chains.has(scopeId))
  }

  resumeScope(scopeId: string): void {
    this.discardedScopes.delete(scopeId)
  }

  private cancelPendingMessage(scopeId: string): void {
    const pending = this.pendingMessages.get(scopeId)
    if (!pending) return
    clearTimeout(pending.timer)
    this.pendingMessages.delete(scopeId)
  }

  private enqueueMessage(scopeId: string, draft: ChatComposerDraft): Promise<void> {
    return this.enqueue(scopeId, async () => {
      const found = await this.adapter.saveMessage({
        scopeId,
        message: draft.message,
        updatedAt: draft.updatedAt
      })
      if (!found) await this.adapter.saveDraft(scopeId, draft)
    })
  }

  private enqueue(scopeId: string, operation: () => Promise<unknown>): Promise<void> {
    const previous = this.chains.get(scopeId) ?? Promise.resolve()
    const tracked = previous
      .catch(() => undefined)
      .then(operation)
      .then(() => undefined)
      .catch((error) => {
        this.onError(error)
      })
      .finally(() => {
        if (this.chains.get(scopeId) === tracked) this.chains.delete(scopeId)
      })
    this.chains.set(scopeId, tracked)
    return tracked
  }
}
