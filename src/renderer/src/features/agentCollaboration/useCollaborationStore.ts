import { useEffect, useMemo, useSyncExternalStore } from 'react'
import { CollaborationStore } from './collaborationStore'

export function useCollaborationStore(rootConversationId: string) {
  const store = useMemo(() => new CollaborationStore(rootConversationId), [rootConversationId])
  const snapshot = useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot)

  useEffect(() => {
    store.start()
    return () => store.destroy()
  }, [store])

  return snapshot
}
