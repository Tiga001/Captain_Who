import type { ChatConversation } from '../chat/chatTypes'
import { useMemo } from 'react'
import { useAccountAuth } from '../auth/AccountAuthContext'

let nextIdentity = 0

/** Presentation cache only. A fresh authorized page is required before reusing an entry. */
export class ObserverConversationCache {
  readonly identity = ++nextIdentity
  private readonly entries = new Map<string, ChatConversation>()

  get(key: string): ChatConversation | undefined {
    const value = this.entries.get(key)
    if (value) {
      this.entries.delete(key)
      this.entries.set(key, value)
    }
    return value
  }

  set(key: string, conversation: ChatConversation): void {
    // Do not retain an unbounded history after leaving a child. Keep a small recent window;
    // older pages remain available from storage and are revalidated in the background.
    const messages = conversation.messages.slice(-200)
    let bytes = 0
    let start = messages.length
    for (let index = messages.length - 1; index >= 0; index--) {
      bytes += JSON.stringify(messages[index]).length * 2
      if (bytes > 4 * 1024 * 1024) break
      start = index
    }
    this.entries.delete(key)
    if (start < messages.length)
      this.entries.set(key, { ...conversation, messages: messages.slice(start) })
    while (this.entries.size > 4) this.entries.delete(this.entries.keys().next().value!)
  }
}

/** Owned by AppShell, whose login overlay may leave it mounted across account changes. */
export function useObserverConversationCache(): ObserverConversationCache {
  const auth = useAccountAuth()
  const scope = auth ? `${auth.state.status}:${auth.state.profile?.userId ?? ''}` : 'local-host'
  // Account transitions own this cache's lifetime, even though its constructor needs no identity.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  return useMemo(() => new ObserverConversationCache(), [scope])
}
