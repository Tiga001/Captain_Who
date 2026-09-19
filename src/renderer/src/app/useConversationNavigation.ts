import { useCallback, useRef, useState, type Dispatch, type SetStateAction } from 'react'
import { HostInvocationError } from '@mycopilot/host-api'
import { parseStorageForkConversationErrorData } from '@mycopilot/protocol'
import type { StorageConversationForkPoint } from '@mycopilot/protocol'
import type {
  ChatComposerDraft,
  ChatConversation,
  ChatConversationContinuationOrigin
} from '../features/chat/chatTypes'
import {
  forkConversation,
  loadConversation,
  loadConversationMetas,
  saveConversationMeta
} from '../features/storage/storageClient'
import { createComposerDraft, createForkComposerDraft, createId } from './chatMessageFactory'

type MutableRef<T> = { current: T }
const ARCHIVE_CONVERSATION_MAX_ATTEMPTS = 3

class ConversationArchiveError extends Error {
  constructor(
    message: string,
    readonly safeToRollback: boolean
  ) {
    super(message)
  }
}

interface ConversationNavigationMessages {
  activeCommandSession: string
  archiveFailed: string
  continueInNewTaskFailed: string
  continueInNewTaskBusy: string
  originArchived: string
  originMissing: string
  originOpenFailed: string
}

interface UseConversationNavigationOptions {
  activeConversationIdRef: MutableRef<string | null>
  conversationScrollPositionsRef: MutableRef<Map<string, number>>
  conversationsRef: MutableRef<ChatConversation[]>
  drafts: Record<string, ChatComposerDraft>
  hydrateConversation: (conversationId: string) => Promise<ChatConversation | null>
  messages: ConversationNavigationMessages
  onActiveConversationArchived: (conversation: ChatConversation) => void
  persistDraftNow: (scopeId: string, draft: ChatComposerDraft) => Promise<void>
  setActiveConversationId: Dispatch<SetStateAction<string | null>>
  setActiveConversationInitialScrollTop: Dispatch<SetStateAction<number | null>>
  setConversationScrollToBottomSignal: Dispatch<SetStateAction<number>>
  setConversationsWithRef: Dispatch<SetStateAction<ChatConversation[]>>
  setDraftsWithRef: Dispatch<SetStateAction<Record<string, ChatComposerDraft>>>
  setScrollTargetMessageId: Dispatch<SetStateAction<string | null>>
  setSettingsOpen: Dispatch<SetStateAction<boolean>>
  showToast: (message: string) => void
  waitForConversationSaves: (conversationId: string) => Promise<void>
}

export function useConversationNavigation({
  activeConversationIdRef,
  conversationScrollPositionsRef,
  conversationsRef,
  drafts,
  hydrateConversation,
  messages,
  onActiveConversationArchived,
  persistDraftNow,
  setActiveConversationId,
  setActiveConversationInitialScrollTop,
  setConversationScrollToBottomSignal,
  setConversationsWithRef,
  setDraftsWithRef,
  setScrollTargetMessageId,
  setSettingsOpen,
  showToast,
  waitForConversationSaves
}: UseConversationNavigationOptions) {
  const archiveRequestsInFlightRef = useRef(new Set<string>())
  const forkRequestsInFlightRef = useRef(new Set<string>())
  const [forkingConversationIds, setForkingConversationIds] = useState<ReadonlySet<string>>(
    () => new Set()
  )
  const forkRequestIdentitiesRef = useRef(new Map<string, { point: string; requestId: string }>())
  const selectConversation = useCallback(
    (conversationId: string, messageId?: string | null, loadedConversation?: ChatConversation) => {
      activeConversationIdRef.current = conversationId
      const selectedConversation =
        loadedConversation ??
        conversationsRef.current.find((conversation) => conversation.id === conversationId)
      const shouldRestoreRememberedPosition = !messageId && !selectedConversation?.unreadAt

      setScrollTargetMessageId(messageId ?? null)
      setActiveConversationInitialScrollTop(
        shouldRestoreRememberedPosition
          ? (conversationScrollPositionsRef.current.get(conversationId) ?? null)
          : null
      )

      let conversationToSave: ChatConversation | null = null
      let loadedConversationFound = false
      const nextConversations = conversationsRef.current.map((conversation) => {
        if (conversation.id !== conversationId) return conversation
        loadedConversationFound = true
        const selected = loadedConversation ?? conversation
        if (!selected.unreadAt) return selected

        conversationToSave = {
          ...selected,
          unreadAt: null
        }
        return conversationToSave
      })
      if (loadedConversation && !loadedConversationFound) {
        const selected = loadedConversation.unreadAt
          ? { ...loadedConversation, unreadAt: null }
          : loadedConversation
        if (loadedConversation.unreadAt) conversationToSave = selected
        nextConversations.unshift(selected)
      }

      if (loadedConversation || conversationToSave) {
        setConversationsWithRef(nextConversations)
      }
      if (conversationToSave) {
        void saveConversationMeta(conversationToSave)
      }

      setActiveConversationId(conversationId)
      if (!loadedConversation) {
        void hydrateConversation(conversationId)
      }
    },
    [
      activeConversationIdRef,
      conversationScrollPositionsRef,
      conversationsRef,
      hydrateConversation,
      setActiveConversationId,
      setActiveConversationInitialScrollTop,
      setConversationsWithRef,
      setScrollTargetMessageId
    ]
  )

  const continueInNewTask = useCallback(
    async (sourceConversationId: string, forkPoint: StorageConversationForkPoint) => {
      // Shared by timeline and Composer actions, including clicks during pending persistence.
      if (forkRequestsInFlightRef.current.has(sourceConversationId)) return
      forkRequestsInFlightRef.current.add(sourceConversationId)
      setForkingConversationIds(new Set(forkRequestsInFlightRef.current))
      try {
        await waitForConversationSaves(sourceConversationId)
        const source = conversationsRef.current.find(({ id }) => id === sourceConversationId)
        const point = JSON.stringify({
          forkPoint,
          // `latest` is relative to the source revision, not a stable boundary identity.
          ...(forkPoint.kind === 'latest'
            ? { updatedAt: source?.updatedAt, head: source?.messages.at(-1)?.id }
            : {})
        })
        const previousRequest = forkRequestIdentitiesRef.current.get(sourceConversationId)
        const requestId =
          previousRequest?.point === point
            ? previousRequest.requestId
            : createId('conversation-fork-request')
        // A lost response may still have committed. Retrying the same point reuses its identity
        // until success, so an uncertain transport failure cannot create a duplicate branch.
        forkRequestIdentitiesRef.current.set(sourceConversationId, { point, requestId })
        const newConversation = await forkConversation({
          requestId,
          sourceConversationId,
          forkPoint
        })
        forkRequestIdentitiesRef.current.delete(sourceConversationId)
        const sourceDraft =
          drafts[sourceConversationId] ??
          createComposerDraft({
            modelId: newConversation.modelId ?? undefined,
            projectId: newConversation.projectId
          })
        const newDraft = createForkComposerDraft(sourceDraft, newConversation)

        setConversationsWithRef((currentConversations) => [
          newConversation,
          ...currentConversations.filter((conversation) => conversation.id !== newConversation.id)
        ])
        setDraftsWithRef((currentDrafts) => ({
          ...currentDrafts,
          [newConversation.id]: newDraft
        }))
        void persistDraftNow(newConversation.id, newDraft)
        conversationScrollPositionsRef.current.delete(newConversation.id)
        activeConversationIdRef.current = newConversation.id
        setScrollTargetMessageId(null)
        setActiveConversationInitialScrollTop(null)
        setConversationScrollToBottomSignal((signal) => signal + 1)
        setActiveConversationId(newConversation.id)
        setSettingsOpen(false)
      } catch (error) {
        console.error('Failed to continue conversation in a new task', error)
        showToast(
          resolveConversationForkErrorMessage(
            error,
            messages.activeCommandSession,
            messages.continueInNewTaskBusy,
            messages.continueInNewTaskFailed
          )
        )
      } finally {
        forkRequestsInFlightRef.current.delete(sourceConversationId)
        setForkingConversationIds(new Set(forkRequestsInFlightRef.current))
      }
    },
    [
      activeConversationIdRef,
      conversationScrollPositionsRef,
      conversationsRef,
      drafts,
      messages.continueInNewTaskFailed,
      messages.continueInNewTaskBusy,
      messages.activeCommandSession,
      persistDraftNow,
      setActiveConversationId,
      setActiveConversationInitialScrollTop,
      setConversationScrollToBottomSignal,
      setConversationsWithRef,
      setDraftsWithRef,
      setScrollTargetMessageId,
      setSettingsOpen,
      showToast,
      waitForConversationSaves
    ]
  )

  const openContinuationOrigin = useCallback(
    async (origin: ChatConversationContinuationOrigin) => {
      try {
        const sourceConversation = await loadConversation(origin.sourceConversationId)
        if (!sourceConversation) {
          showToast(messages.originMissing)
          return
        }
        if (sourceConversation.archivedAt !== null && sourceConversation.archivedAt !== undefined) {
          showToast(messages.originArchived)
          return
        }
        selectConversation(origin.sourceConversationId, origin.sourceMessageId, sourceConversation)
      } catch (error) {
        console.error('Failed to open continuation origin', error)
        showToast(messages.originOpenFailed)
      }
    },
    [
      messages.originArchived,
      messages.originMissing,
      messages.originOpenFailed,
      selectConversation,
      showToast
    ]
  )

  const rememberConversationScrollPosition = useCallback(
    (conversationId: string, scrollTop: number) => {
      conversationScrollPositionsRef.current.set(conversationId, scrollTop)
    },
    [conversationScrollPositionsRef]
  )

  const patchConversation = useCallback(
    (conversationId: string, patch: Partial<ChatConversation>) => {
      let nextConversation: ChatConversation | null = null
      const nextConversations = conversationsRef.current.map((conversation) => {
        if (conversation.id !== conversationId) return conversation

        nextConversation = {
          ...conversation,
          ...patch,
          updatedAt: Math.max(
            Date.now(),
            conversation.updatedAt + 1,
            patch.updatedAt ?? Number.NEGATIVE_INFINITY
          )
        }
        return nextConversation
      })

      if (nextConversation) {
        setConversationsWithRef(nextConversations)
        void saveConversationMeta(nextConversation)
      }
    },
    [conversationsRef, setConversationsWithRef]
  )

  const archiveConversations = useCallback(
    async (predicate: (conversation: ChatConversation) => boolean) => {
      const originalsById = new Map(
        conversationsRef.current
          .filter(
            (conversation) =>
              !conversation.archivedAt &&
              !archiveRequestsInFlightRef.current.has(conversation.id) &&
              predicate(conversation)
          )
          .map(
            (conversation) =>
              [conversation.id, { ...conversation, pendingArchivedAt: undefined }] as const
          )
      )
      if (originalsById.size === 0) return
      for (const conversationId of originalsById.keys()) {
        archiveRequestsInFlightRef.current.add(conversationId)
      }

      // Fence the local metadata synchronously before the first await, but keep the active page
      // selected. Pre-existing saves carry an older updatedAt; later Run/rename saves spread this
      // archive token, so neither side can silently undo a confirmed archive.
      const intentsById = new Map<string, ChatConversation>()
      setConversationsWithRef((currentConversations) =>
        currentConversations.map((conversation) => {
          if (!originalsById.has(conversation.id)) return conversation
          const archiveToken =
            conversation.pendingArchivedAt ?? Math.max(Date.now(), conversation.updatedAt + 1)
          const intent = {
            ...conversation,
            pendingArchivedAt: archiveToken,
            unreadAt: null,
            updatedAt:
              conversation.pendingArchivedAt === undefined
                ? archiveToken
                : Math.max(Date.now(), conversation.updatedAt + 1)
          }
          intentsById.set(conversation.id, intent)
          return intent
        })
      )
      const candidates = [...intentsById.values()]

      const archiveCandidate = async (
        initialConversation: ChatConversation
      ): Promise<ChatConversation> => {
        const archiveToken = initialConversation.pendingArchivedAt
        if (archiveToken === undefined) {
          throw new Error('Conversation archive intent is missing')
        }
        let baseConversation = initialConversation
        let lastAttemptReadUnarchived = false
        let lastError: unknown = null
        for (let attempt = 0; attempt < ARCHIVE_CONVERSATION_MAX_ATTEMPTS; attempt += 1) {
          lastAttemptReadUnarchived = false
          await waitForConversationSaves(initialConversation.id)
          const latestLocal = conversationsRef.current.find(
            (conversation) => conversation.id === initialConversation.id
          )
          if (latestLocal && latestLocal.updatedAt > baseConversation.updatedAt) {
            baseConversation = latestLocal
          }
          const candidate = {
            ...baseConversation,
            pendingArchivedAt: archiveToken,
            unreadAt: null,
            updatedAt:
              attempt === 0
                ? Math.max(baseConversation.updatedAt, initialConversation.updatedAt)
                : Math.max(Date.now(), baseConversation.updatedAt + 1)
          }
          if (candidate.updatedAt > (latestLocal?.updatedAt ?? Number.NEGATIVE_INFINITY)) {
            setConversationsWithRef((currentConversations) =>
              currentConversations.map((conversation) =>
                conversation.id === candidate.id && conversation.pendingArchivedAt === archiveToken
                  ? { ...conversation, updatedAt: candidate.updatedAt }
                  : conversation
              )
            )
          }
          try {
            await saveConversationMeta(candidate)
          } catch (error) {
            // A rejected IPC response does not prove the SQLite commit failed. Read back the
            // exact row before deciding whether this attempt needs a retry.
            lastError = error
          }
          // Run/tool events can enqueue newer metadata while the archive write is in flight.
          // Drain those writes, read the DB authority back, and retry from that version if the
          // guarded upsert rejected our stale candidate.
          try {
            await waitForConversationSaves(initialConversation.id)
            const stored = (await loadConversationMetas()).find(
              (conversation) => conversation.id === initialConversation.id
            )
            if (!stored) {
              throw new Error('Conversation disappeared while being archived')
            }
            if (stored.archivedAt === archiveToken) return stored
            lastAttemptReadUnarchived = true
            baseConversation = stored
          } catch (error) {
            lastError = error
          }
        }
        throw new ConversationArchiveError(
          lastError instanceof Error
            ? lastError.message
            : 'Conversation archive did not win the metadata version race',
          lastAttemptReadUnarchived
        )
      }
      const outcomes: PromiseSettledResult<ChatConversation>[] = []
      // Bulk project/archive-all actions stay memory bounded and never hydrate message history.
      for (const candidate of candidates) {
        try {
          outcomes.push({ status: 'fulfilled', value: await archiveCandidate(candidate) })
        } catch (reason) {
          outcomes.push({ status: 'rejected', reason })
        }
      }
      const archivedById = new Map<string, ChatConversation>()
      const failedIds = new Set<string>()
      let failed = false
      for (const [index, outcome] of outcomes.entries()) {
        const candidate = candidates[index]
        if (!candidate) continue
        if (outcome.status === 'fulfilled') {
          archivedById.set(candidate.id, outcome.value)
        } else {
          failed = true
          if (outcome.reason instanceof ConversationArchiveError && outcome.reason.safeToRollback) {
            failedIds.add(candidate.id)
          }
          console.error('Failed to archive conversation', outcome.reason)
        }
      }

      if (archivedById.size > 0) {
        setConversationsWithRef((currentConversations) =>
          currentConversations.map((conversation) =>
            mergeConversationArchiveResult(
              conversation,
              archivedById.get(conversation.id),
              intentsById.get(conversation.id)?.pendingArchivedAt
            )
          )
        )

        const activeConversationId = activeConversationIdRef.current
        const archivedActiveConversation = activeConversationId
          ? archivedById.get(activeConversationId)
          : undefined
        const activeConversation = activeConversationId
          ? conversationsRef.current.find(
              (conversation) => conversation.id === activeConversationId
            )
          : undefined
        if (
          activeConversation &&
          archivedActiveConversation &&
          activeConversation.archivedAt === archivedActiveConversation.archivedAt &&
          activeConversation.pendingArchivedAt === undefined
        ) {
          onActiveConversationArchived(activeConversation)
        }
      }
      if (failedIds.size > 0) {
        setConversationsWithRef((currentConversations) =>
          currentConversations.map((conversation) => {
            if (!failedIds.has(conversation.id)) return conversation
            const intent = intentsById.get(conversation.id)
            const original = originalsById.get(conversation.id)
            if (
              !intent ||
              !original ||
              conversation.pendingArchivedAt !== intent.pendingArchivedAt
            ) {
              return conversation
            }
            return {
              ...conversation,
              archivedAt: original.archivedAt,
              pendingArchivedAt: undefined,
              unreadAt: conversation.unreadAt === null ? original.unreadAt : conversation.unreadAt
            }
          })
        )
      }
      if (failed) showToast(messages.archiveFailed)
      for (const conversationId of originalsById.keys()) {
        archiveRequestsInFlightRef.current.delete(conversationId)
      }
    },
    [
      activeConversationIdRef,
      conversationsRef,
      messages.archiveFailed,
      onActiveConversationArchived,
      setConversationsWithRef,
      showToast,
      waitForConversationSaves
    ]
  )

  const archiveConversation = useCallback(
    async (conversationId: string) => {
      await archiveConversations((conversation) => conversation.id === conversationId)
    },
    [archiveConversations]
  )

  return {
    archiveConversation,
    archiveConversations,
    continueInNewTask,
    forkingConversationIds,
    openContinuationOrigin,
    patchConversation,
    rememberConversationScrollPosition,
    selectConversation
  }
}

function mergeConversationArchiveResult(
  conversation: ChatConversation,
  stored: ChatConversation | undefined,
  expectedArchiveToken: number | null | undefined
): ChatConversation {
  if (!stored || conversation.pendingArchivedAt !== expectedArchiveToken) return conversation
  return {
    ...conversation,
    updatedAt: Math.max(conversation.updatedAt, stored.updatedAt),
    archivedAt: stored.archivedAt,
    pendingArchivedAt: undefined,
    unreadAt: null
  }
}

function resolveConversationForkErrorMessage(
  error: unknown,
  activeCommandSessionMessage: string,
  busyMessage: string,
  fallbackMessage: string
): string {
  if (!(error instanceof HostInvocationError)) return fallbackMessage
  if (error.code === -32001) return busyMessage

  try {
    const data = parseStorageForkConversationErrorData(error.data)
    if (data.code === 'active_command_session') return activeCommandSessionMessage
  } catch {
    // Unknown or malformed recovery data must never expose the underlying Host/Core message.
  }

  return fallbackMessage
}
