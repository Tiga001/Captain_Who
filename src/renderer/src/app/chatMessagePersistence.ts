import type { ChatMessage } from '../features/chat/chatTypes'
import type { PendingMessageSave } from './AppShellSupport'

export const CHAT_MESSAGE_CHECKPOINT_INTERVAL_MS = 300

type Timer = ReturnType<typeof setTimeout>

interface ChatMessagePersistenceAdapter {
  save(payload: PendingMessageSave): Promise<void>
}

function messageSaveKey(conversationId: string, messageId: string): string {
  return `${conversationId}:${messageId}`
}

/**
 * Serializes full message-state writes per message and rate-limits streaming checkpoints.
 *
 * Checkpoint timers are fixed-window rather than trailing debounce timers: a continuous stream
 * therefore remains crash-recoverable without writing on every Renderer paint. Immediate saves
 * cancel an older checkpoint timer and share the same single-flight drain, so a terminal snapshot
 * is always written after any older in-flight checkpoint.
 */
export class ChatMessagePersistenceQueue {
  private readonly checkpointDue = new Set<string>()
  private readonly checkpointTimers = new Map<string, Timer>()
  private readonly drains = new Map<string, Promise<void>>()
  private sealedForQuit = false

  constructor(
    private readonly adapter: ChatMessagePersistenceAdapter,
    private readonly pendingSaves: Map<string, PendingMessageSave>,
    private readonly onError: (error: unknown) => void,
    private readonly checkpointIntervalMs = CHAT_MESSAGE_CHECKPOINT_INTERVAL_MS
  ) {}

  scheduleCheckpoint(conversationId: string, message: ChatMessage): void {
    const key = messageSaveKey(conversationId, message.id)
    this.pendingSaves.set(key, { conversationId, message })

    if (this.sealedForQuit) {
      this.cancelCheckpointTimer(key)
      this.checkpointDue.add(key)
      this.ensureDrain(key)
      return
    }
    if (this.checkpointTimers.has(key) || this.checkpointDue.has(key)) return

    const timer = setTimeout(() => {
      this.checkpointTimers.delete(key)
      this.checkpointDue.add(key)
      this.ensureDrain(key)
    }, this.checkpointIntervalMs)
    this.checkpointTimers.set(key, timer)
  }

  persistNow(conversationId: string, message: ChatMessage): void {
    const key = messageSaveKey(conversationId, message.id)
    this.pendingSaves.set(key, { conversationId, message })
    this.cancelCheckpointTimer(key)
    this.checkpointDue.add(key)
    this.ensureDrain(key)
  }

  flushMessage(conversationId: string, messageId: string): Promise<void> {
    return this.flushKeys([messageSaveKey(conversationId, messageId)])
  }

  flushConversation(conversationId: string): Promise<void> {
    return this.flushMatchingKeys((key, payload) =>
      payload ? payload.conversationId === conversationId : key.startsWith(`${conversationId}:`)
    )
  }

  flushAll(): Promise<void> {
    return this.flushMatchingKeys(() => true)
  }

  sealAndFlushAll(): Promise<void> {
    this.sealedForQuit = true
    return this.flushAll()
  }

  private async flushMatchingKeys(
    matches: (key: string, payload: PendingMessageSave | undefined) => boolean
  ): Promise<void> {
    while (true) {
      const keys = this.allKeys().filter((key) => matches(key, this.pendingSaves.get(key)))
      if (keys.length === 0) return
      await this.flushKeys(keys)
    }
  }

  private async flushKeys(keys: readonly string[]): Promise<void> {
    const uniqueKeys = [...new Set(keys)]
    while (true) {
      for (const key of uniqueKeys) {
        this.cancelCheckpointTimer(key)
        if (this.pendingSaves.has(key)) this.checkpointDue.add(key)
        this.ensureDrain(key)
      }

      const activeDrains = uniqueKeys
        .map((key) => this.drains.get(key))
        .filter((drain): drain is Promise<void> => Boolean(drain))
      if (activeDrains.length > 0) await Promise.all(activeDrains)

      // Let a drain's finalizer observe work queued in the same microtask turn before deciding
      // whether this boundary is fully durable.
      await Promise.resolve()
      if (
        uniqueKeys.every(
          (key) =>
            !this.pendingSaves.has(key) &&
            !this.checkpointDue.has(key) &&
            !this.checkpointTimers.has(key) &&
            !this.drains.has(key)
        )
      ) {
        return
      }
    }
  }

  private allKeys(): string[] {
    return [
      ...new Set([
        ...this.pendingSaves.keys(),
        ...this.checkpointDue,
        ...this.checkpointTimers.keys(),
        ...this.drains.keys()
      ])
    ]
  }

  private cancelCheckpointTimer(key: string): void {
    const timer = this.checkpointTimers.get(key)
    if (timer === undefined) return
    clearTimeout(timer)
    this.checkpointTimers.delete(key)
  }

  private ensureDrain(key: string): void {
    if (this.drains.has(key) || !this.checkpointDue.has(key)) return

    const drain = (async () => {
      while (this.checkpointDue.has(key)) {
        this.checkpointDue.delete(key)
        const payload = this.pendingSaves.get(key)
        if (!payload) continue
        this.pendingSaves.delete(key)

        try {
          await this.adapter.save(payload)
        } catch (error) {
          this.onError(error)
        }
      }
    })()

    const tracked = drain.finally(() => {
      if (this.drains.get(key) === tracked) this.drains.delete(key)
      // An immediate save can be accepted after the async drain has returned but before this
      // finalizer runs. Re-checking here prevents that microtask race from stranding the snapshot.
      if (this.checkpointDue.has(key)) this.ensureDrain(key)
    })
    this.drains.set(key, tracked)
  }
}
