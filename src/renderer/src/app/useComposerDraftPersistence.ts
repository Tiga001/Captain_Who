import { useCallback, useEffect, useState } from 'react'
import type { ChatComposerDraft } from '../features/chat/chatTypes'
import { saveComposerDraft, saveComposerDraftMessage } from '../features/storage/storageClient'
import { hostClient } from '../host/hostClient'
import { ComposerDraftPersistenceQueue } from './composerDraftPersistence'

export function useComposerDraftPersistence() {
  const [queue] = useState(
    () =>
      new ComposerDraftPersistenceQueue(
        {
          saveDraft: saveComposerDraft,
          saveMessage: saveComposerDraftMessage
        },
        (error) => console.error('Failed to save Composer draft to SQLite', error)
      )
  )

  useEffect(() => {
    const flush = () => void queue.flushAll()
    const flushWhenHidden = () => {
      if (document.visibilityState === 'hidden') flush()
    }

    window.addEventListener('beforeunload', flush)
    window.addEventListener('pagehide', flush)
    window.addEventListener('blur', flush)
    document.addEventListener('focusout', flush)
    document.addEventListener('visibilitychange', flushWhenHidden)
    return () => {
      window.removeEventListener('beforeunload', flush)
      window.removeEventListener('pagehide', flush)
      window.removeEventListener('blur', flush)
      document.removeEventListener('focusout', flush)
      document.removeEventListener('visibilitychange', flushWhenHidden)
      flush()
    }
  }, [queue])

  useEffect(() => hostClient.app.onFlushBeforeQuit?.(() => queue.sealAndFlushAll()), [queue])

  const scheduleMessageSave = useCallback(
    (scopeId: string, draft: ChatComposerDraft) => queue.scheduleMessage(scopeId, draft),
    [queue]
  )
  const persistDraftNow = useCallback(
    (scopeId: string, draft: ChatComposerDraft) => queue.persistNow(scopeId, draft),
    [queue]
  )
  const flushDraft = useCallback((scopeId: string) => queue.flushScope(scopeId), [queue])
  const discardDraft = useCallback((scopeId: string) => queue.discardScope(scopeId), [queue])
  const resumeDraft = useCallback((scopeId: string) => queue.resumeScope(scopeId), [queue])

  return { discardDraft, flushDraft, persistDraftNow, resumeDraft, scheduleMessageSave }
}
