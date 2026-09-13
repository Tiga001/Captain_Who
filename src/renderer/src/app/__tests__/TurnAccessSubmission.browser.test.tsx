import { useRef, useState } from 'react'
import { beforeEach, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { AccountProfile, LicenseState } from '@mycopilot/host-api'
import { AccountAuthContext } from '../../features/auth/AccountAuthContext'
import { LicenseContext } from '../../features/license/LicenseContext'
import { useAppShellMessageSubmission } from '../useAppShellMessageSubmission'
import { createComposerDraft } from '../chatMessageFactory'
import type { ChatConversation, ChatComposerDraft } from '../../features/chat/chatTypes'

const mocks = vi.hoisted(() => ({
  request: vi.fn(),
  login: vi.fn(),
  access: vi.fn(),
  denied: vi.fn(),
  save: vi.fn()
}))
vi.mock('../../host/hostClient', () => ({ hostClient: {} }))
vi.mock('../../features/agentRun/useProviderTransition', () => ({
  useProviderTransition: () => ({
    cancelConfirmation: vi.fn(),
    confirm: vi.fn(),
    loadStatus: vi.fn(),
    retry: vi.fn(),
    store: {},
    request: async () => ({ status: 'completed', operation: { modelId: 'model-1' } })
  })
}))

type Options = Parameters<typeof useAppShellMessageSubmission>[0]
const model = {
  id: 'model-1',
  name: 'Test',
  provider: 'openai',
  model: 'test',
  enabled: true
} as unknown as NonNullable<Options['activeDraftSelectedModel']>
const initialConversation: ChatConversation = {
  id: 'chat',
  title: 'Chat',
  projectId: null,
  modelId: 'model-1',
  messages: [],
  messagesLoaded: true,
  createdAt: 1,
  updatedAt: 1,
  pinnedAt: null,
  archivedAt: null,
  unreadAt: null
}
const initialDraft = () =>
  createComposerDraft({
    modelId: 'model-1',
    message: 'Keep my input',
    queuedMessages: [
      {
        id: 'queue-1',
        clientMessageId: 'client-1',
        content: 'Queued input',
        attachments: [],
        modelId: 'model-1',
        permissionMode: 'default',
        projectId: null,
        skills: [],
        status: 'pending',
        createdAt: 1
      }
    ]
  })

function Harness({
  account = 'one',
  status = 'allowed',
  newChat = false
}: {
  account?: string | null
  status?: LicenseState['status']
  newChat?: boolean
}) {
  return (
    <AccountAuthContext.Provider
      value={{
        state: {
          revision: 1,
          status: account ? 'signedIn' : 'signedOut',
          profile: account ? ({ userId: account } as AccountProfile) : null,
          error: null,
          remembered: true
        },
        loginRequested: false,
        canStartTurn: () => Boolean(account),
        requestLogin: mocks.login,
        dismissLogin: vi.fn(),
        logout: vi.fn()
      }}
    >
      <LicenseContext.Provider
        value={{
          state: {
            revision: 1,
            status,
            reason: status === 'allowed' ? 'active' : null,
            expiresAt: null,
            verifiedAt: null,
            cacheValidUntil: null,
            error: null
          },
          canStartTurn: () => Boolean(account) && status === 'allowed',
          requestAccess: mocks.access,
          handleDenied: mocks.denied,
          refresh: vi.fn()
        }}
      >
        <Submission newChat={newChat} />
      </LicenseContext.Provider>
    </AccountAuthContext.Provider>
  )
}

let finishPersistence: (() => void) | null = null
function Submission({ newChat }: { newChat: boolean }) {
  const [conversations, setConversations] = useState<ChatConversation[]>(
    newChat ? [] : [initialConversation]
  )
  const conversationsRef = useRef(conversations)
  const [drafts, setDrafts] = useState<Record<string, ChatComposerDraft>>({ chat: initialDraft() })
  const draftsRef = useRef(drafts)
  const activeRef = useRef<string | null>(newChat ? null : 'chat')
  const [activeId, setActiveId] = useState<string | null>(newChat ? null : 'chat')
  const activeDraft = drafts[activeId ?? 'chat'] ?? initialDraft()
  const [refs] = useState(() => ({
    auto: { current: vi.fn() },
    attempts: { current: new Map() },
    inflight: { current: new Set<string>() },
    seq: { current: 0 },
    pending: { current: new Map() }
  }))
  const updateDraft = (id: string, draft: ChatComposerDraft) => {
    draftsRef.current = { ...draftsRef.current, [id]: draft }
    setDrafts(draftsRef.current)
  }
  const submission = useAppShellMessageSubmission({
    activeConversationIdRef: activeRef,
    activeDraft,
    activeDraftSelectedModel: model,
    autoSubmitQueuedMessageRef: refs.auto,
    conversations,
    conversationsRef,
    drafts,
    draftsRef,
    editRewriteAttemptsRef: refs.attempts,
    editRewriteInFlightRef: refs.inflight,
    editSubmissionSeqRef: refs.seq,
    enabledModels: [model],
    enqueueChatMessagesUpsert: mocks.save,
    enqueueConversationMetaSave: vi.fn(),
    mutateDraft: (id, change) => {
      const next = change(draftsRef.current[id])
      updateDraft(id, next)
      return next
    },
    pendingProviderTransitionSubmissionsRef: refs.pending,
    requestAssistantResponse: mocks.request,
    restoreSubmittedSkills: vi.fn(),
    setActiveConversationId: setActiveId,
    setActiveConversationInitialScrollTop: vi.fn(),
    setConversationScrollToBottomSignal: vi.fn(),
    setConversationsWithRef: (change) => {
      conversationsRef.current =
        typeof change === 'function' ? change(conversationsRef.current) : change
      setConversations(conversationsRef.current)
    },
    setScrollTargetMessageId: vi.fn(),
    showToast: vi.fn(),
    t: (key) => key,
    updateDraft,
    waitForConversationSaves: async () => {
      if (finishPersistence !== null)
        await new Promise<void>((resolve) => {
          finishPersistence = resolve
        })
    },
    waitForMessageUpserts: async () => undefined,
    waitForMessageStateSaves: async () => undefined,
    waitForRunSettlement: async () => undefined
  })
  return (
    <>
      <button
        onClick={() =>
          void submission.submitMessage(activeDraft.message, {
            modelId: 'model-1',
            projectId: null,
            permissionMode: 'default',
            skills: [],
            attachments: []
          })
        }
      >
        send
      </button>
      <button onClick={() => submission.toggleQueueAutoSend('chat')}>toggle queue</button>
      <output data-testid="draft">{activeDraft.message}</output>
      <output data-testid="queued">{drafts.chat.queuedMessages.length}</output>
      <output data-testid="messages">
        {conversations.reduce((count, item) => count + item.messages.length, 0)}
      </output>
      <output data-testid="queue-enabled">
        {String(submission.queueAutoSendConversationIds.has('chat'))}
      </output>
    </>
  )
}

beforeEach(() => {
  vi.clearAllMocks()
  finishPersistence = null
  mocks.request.mockResolvedValue(true)
  mocks.denied.mockReturnValue(true)
})

it.each([false, true])(
  'guides a late Main refusal only for the same continuous account (switch=%s)',
  async (switchAccount) => {
    let reject!: (error: Error) => void
    mocks.request.mockImplementation(
      () =>
        new Promise((_resolve, no) => {
          reject = no
        })
    )
    const screen = await render(<Harness />)
    await screen.getByRole('button', { name: 'send', exact: true }).click()
    await expect.poll(() => mocks.request.mock.calls.length).toBe(1)
    await screen.rerender(switchAccount ? <Harness account="two" /> : <Harness status="denied" />)
    reject(new Error('ACCOUNT_LICENSE_REQUIRED'))
    await expect.element(screen.getByTestId('messages')).toHaveTextContent('0')
    await expect.element(screen.getByTestId('draft')).toHaveTextContent('Keep my input')
    expect(mocks.denied).toHaveBeenCalledTimes(switchAccount ? 0 : 1)
  }
)

it('does not automatically revive an in-flight intent after license recovery', async () => {
  finishPersistence = () => undefined
  const screen = await render(<Harness />)
  await screen.getByRole('button', { name: 'send', exact: true }).click()
  await screen.rerender(<Harness status="denied" />)
  await screen.rerender(<Harness status="allowed" />)
  finishPersistence!()
  await expect.element(screen.getByTestId('draft')).toHaveTextContent('Keep my input')
  expect(mocks.request).not.toHaveBeenCalled()
})

it.each(['ACCOUNT_LOGIN_REQUIRED', 'ACCOUNT_LICENSE_REQUIRED', 'ACCOUNT_LICENSE_UNAVAILABLE'])(
  'preserves input and rolls back only an explicitly unadmitted new chat (%s)',
  async (code) => {
    let reject!: (error: Error) => void
    mocks.request.mockImplementation(
      () =>
        new Promise((_resolve, no) => {
          reject = no
        })
    )
    const screen = await render(<Harness newChat />)
    await screen.getByRole('button', { name: 'send', exact: true }).click()
    await expect.poll(() => mocks.request.mock.calls.length).toBe(1)
    await expect.element(screen.getByTestId('draft')).toHaveTextContent('Keep my input')
    reject(new Error(code))
    await expect.element(screen.getByTestId('messages')).toHaveTextContent('0')
    await expect.element(screen.getByTestId('draft')).toHaveTextContent('Keep my input')
    expect(mocks.denied).toHaveBeenCalledOnce()
    expect(mocks.save).not.toHaveBeenCalled()
  }
)

it.each(['ACCOUNT_LOGIN_REQUIRED', 'ACCOUNT_LICENSE_REQUIRED', 'ACCOUNT_LICENSE_UNAVAILABLE'])(
  'keeps a denied queued message and requires an explicit restart (%s)',
  async (code) => {
    mocks.request.mockRejectedValueOnce(new Error(code))
    const screen = await render(<Harness />)
    await screen.getByRole('button', { name: 'toggle queue' }).click()
    await expect.poll(() => mocks.request.mock.calls.length).toBe(1)
    await expect.element(screen.getByTestId('queue-enabled')).toHaveTextContent('false')
    await expect.element(screen.getByTestId('queued')).toHaveTextContent('1')
    await expect.element(screen.getByTestId('messages')).toHaveTextContent('0')
    expect(mocks.denied).not.toHaveBeenCalled()
    expect(mocks.access).not.toHaveBeenCalled()
    expect(mocks.login).not.toHaveBeenCalled()
    await screen.rerender(<Harness status="denied" />)
    await screen.rerender(<Harness status="allowed" />)
    expect(mocks.request).toHaveBeenCalledOnce()
    await screen.getByRole('button', { name: 'toggle queue' }).click()
    await expect.poll(() => mocks.request.mock.calls.length).toBe(2)
    await expect.element(screen.getByTestId('queued')).toHaveTextContent('0')
  }
)

it.each(['two', null])(
  'does not continue old composer intent after account changes to %s',
  async (account) => {
    finishPersistence = () => undefined
    const screen = await render(<Harness />)
    await screen.getByRole('button', { name: 'send', exact: true }).click()
    await screen.rerender(<Harness account={account} />)
    finishPersistence!()
    await expect.element(screen.getByTestId('draft')).toHaveTextContent('Keep my input')
    expect(mocks.request).not.toHaveBeenCalled()
  }
)

it('invalidates queued intent across an account switch while persistence is pending', async () => {
  finishPersistence = () => undefined
  const screen = await render(<Harness />)
  await screen.getByRole('button', { name: 'toggle queue' }).click()
  await screen.rerender(<Harness account="two" />)
  await expect.element(screen.getByTestId('queue-enabled')).toHaveTextContent('false')
  finishPersistence!()
  await expect.element(screen.getByTestId('queued')).toHaveTextContent('1')
  expect(mocks.request).not.toHaveBeenCalled()
})
