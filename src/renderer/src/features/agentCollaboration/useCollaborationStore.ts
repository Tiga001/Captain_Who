import { useEffect, useMemo, useSyncExternalStore } from 'react'
import type { CollaborationStoreSnapshot } from './collaborationStore'
import { CollaborationStoreCache } from './collaborationStoreCache'
import { useAccountAuth } from '../auth/AccountAuthContext'
import { hostCollaborationDataSource } from './collaborationClient'

const EMPTY_SUBSCRIBE = () => () => undefined
const EMPTY_GET_SNAPSHOT = () => null

/** App-shell owner: drafts have no root reads/subscriptions; one resync listener covers the cache. */
export function useOptionalCollaborationStore(
  rootConversationId: string | null
): CollaborationStoreSnapshot | null {
  const auth = useAccountAuth()
  // The startup/login overlay keeps AppShell mounted. Do not let its in-memory root history
  // survive an account/status transition, even if the selected conversation is unchanged.
  const authScope = auth ? `${auth.state.status}:${auth.state.profile?.userId ?? ''}` : 'local-host'
  // eslint-disable-next-line react-hooks/exhaustive-deps -- Account changes intentionally replace the cache owner.
  const cache = useMemo(() => new CollaborationStoreCache(), [authScope])
  const store = useMemo(
    () => (rootConversationId ? cache.get(rootConversationId) : null),
    [cache, rootConversationId]
  )
  const snapshot = useSyncExternalStore(
    store?.subscribe ?? EMPTY_SUBSCRIBE,
    store?.getSnapshot ?? EMPTY_GET_SNAPSHOT,
    store?.getSnapshot ?? EMPTY_GET_SNAPSHOT
  )

  useEffect(
    () => hostCollaborationDataSource.subscribeResync(() => cache.invalidateRecovery()),
    [cache]
  )

  useEffect(() => {
    if (!store) return
    cache.activate(store)
    return () => cache.release(store)
  }, [cache, store])

  return snapshot
}
