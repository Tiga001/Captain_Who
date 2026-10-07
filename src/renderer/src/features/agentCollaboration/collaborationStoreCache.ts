import { CollaborationStore } from './collaborationStore'

const MAX_CACHED_ROOTS = 4
const IDLE_ROOT_TTL_MS = 5 * 60_000

/** Owned by one AppShell/auth scope, never shared between renderer windows or accounts. */
export class CollaborationStoreCache {
  private readonly entries = new Map<string, { store: CollaborationStore; touchedAt: number }>()
  private active: CollaborationStore | null = null

  constructor(
    private readonly createStore = (rootConversationId: string) =>
      new CollaborationStore(rootConversationId),
    private readonly now = Date.now
  ) {}

  get(rootConversationId: string): CollaborationStore {
    const now = this.now()
    for (const [root, entry] of this.entries) {
      if (entry.store !== this.active && now - entry.touchedAt >= IDLE_ROOT_TTL_MS) {
        entry.store.destroy()
        this.entries.delete(root)
      }
    }
    const existing = this.entries.get(rootConversationId)
    const store = existing?.store ?? this.createStore(rootConversationId)
    this.entries.delete(rootConversationId)
    this.entries.set(rootConversationId, { store, touchedAt: now })
    while (this.entries.size > MAX_CACHED_ROOTS) {
      const oldest = [...this.entries].find(([, entry]) => entry.store !== this.active)
      if (!oldest) break
      oldest[1].store.destroy()
      this.entries.delete(oldest[0])
    }
    return store
  }

  activate(store: CollaborationStore): void {
    if (this.active && this.active !== store) this.release(this.active)
    this.active = store
    store.start()
  }

  release(store: CollaborationStore): void {
    store.pause()
    const entry = this.entries.get(store.rootConversationId)
    if (entry?.store === store) entry.touchedAt = this.now()
    if (this.active === store) this.active = null
  }

  /** A global recovery invalidates idle cursors too, without waking hidden roots. */
  invalidateRecovery(): void {
    for (const { store } of this.entries.values()) store.invalidateRecoveryCheckpoint()
  }
}
