import { useEffect, useMemo, useSyncExternalStore } from 'react'
import { CollaborationStore } from './collaborationStore'
import type { CollaborationStoreSnapshot } from './collaborationStore'

export function useCollaborationStore(rootConversationId: string) {
  const store = useMemo(() => new CollaborationStore(rootConversationId), [rootConversationId])
  const snapshot = useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot)

  useEffect(() => {
    store.start()
    return () => store.destroy()
  }, [store])

  return snapshot
}

const EMPTY_SUBSCRIBE = () => () => undefined
const EMPTY_GET_SNAPSHOT = () => null

/** App-shell owner for the optional active root. It never opens a Host subscription for drafts. */
export function useOptionalCollaborationStore(
  rootConversationId: string | null
): CollaborationStoreSnapshot | null {
  const store = useMemo(
    () => (rootConversationId ? new CollaborationStore(rootConversationId) : null),
    [rootConversationId]
  )
  const snapshot = useSyncExternalStore(
    store?.subscribe ?? EMPTY_SUBSCRIBE,
    store?.getSnapshot ?? EMPTY_GET_SNAPSHOT,
    store?.getSnapshot ?? EMPTY_GET_SNAPSHOT
  )

  useEffect(() => {
    store?.start()
    return () => store?.destroy()
  }, [store])

  return snapshot
}
