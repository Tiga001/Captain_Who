import { useCallback, useRef, useState, type SetStateAction } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { page } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import type { ChatConversation } from '../../features/chat/chatTypes'
import { useConversationNavigation } from '../useConversationNavigation'

const storage = vi.hoisted(() => ({
  save: vi.fn(),
  metas: vi.fn(),
  load: vi.fn(),
  fork: vi.fn()
}))
vi.mock('../../features/storage/storageClient', () => ({
  saveConversationMeta: storage.save,
  loadConversationMetas: storage.metas,
  loadConversation: storage.load,
  forkConversation: storage.fork
}))

const chat = (id: string): ChatConversation => ({
  id,
  title: id,
  modelId: 'model-a',
  projectId: 'project-a',
  createdAt: 1,
  updatedAt: 1,
  unreadAt: 12,
  archivedAt: null,
  messages: []
})
const showToast = vi.fn()
const archived = vi.fn()
const waitForSaves = vi.fn<() => Promise<void>>()
const messages = {
  activeCommandSession: 'activeCommandSession',
  archiveFailed: 'archiveFailed',
  archiveWorkflowActive: 'archiveWorkflowActive',
  continueInNewTaskFailed: 'continueInNewTaskFailed',
  continueInNewTaskBusy: 'continueInNewTaskBusy',
  originArchived: 'originArchived',
  originMissing: 'originMissing',
  originOpenFailed: 'originOpenFailed'
}

function Harness({ memberships = {} }: { memberships?: Readonly<Record<string, unknown>> }) {
  const [conversations, setConversations] = useState(() => [chat('chat-a'), chat('chat-b')])
  const conversationsRef = useRef(conversations)
  const [activeId, setActiveId] = useState<string | null>('chat-a')
  const activeIdRef = useRef(activeId)
  const scrollRef = useRef(new Map<string, number>())
  const [completed, setCompleted] = useState(false)
  const updateConversations = useCallback((action: SetStateAction<ChatConversation[]>) => {
    const next = typeof action === 'function' ? action(conversationsRef.current) : action
    conversationsRef.current = next
    setConversations(next)
  }, [])
  const onArchived = useCallback((conversation: ChatConversation) => {
    archived(conversation)
    activeIdRef.current = null
    setActiveId(null)
  }, [])
  const navigation = useConversationNavigation({
    activeConversationIdRef: activeIdRef,
    conversationScrollPositionsRef: scrollRef,
    conversationsRef,
    drafts: {},
    hydrateConversation: async () => null,
    workflowMemberships: memberships,
    messages,
    onActiveConversationArchived: onArchived,
    persistDraftNow: async () => {},
    setActiveConversationId: setActiveId,
    setActiveConversationInitialScrollTop: () => {},
    setConversationScrollToBottomSignal: () => {},
    setConversationsWithRef: updateConversations,
    setDraftsWithRef: () => {},
    setScrollTargetMessageId: () => {},
    setSettingsOpen: () => {},
    showToast,
    waitForConversationSaves: waitForSaves
  })
  return (
    <>
      <button
        onClick={async () => {
          await navigation.archiveConversation('chat-a')
          setCompleted(true)
        }}
      >
        归档当前对话
      </button>
      <button
        onClick={async () => {
          await navigation.archiveConversations(() => true)
          setCompleted(true)
        }}
      >
        归档所有对话
      </button>
      <output data-testid="navigation-state">
        {JSON.stringify({ conversations, activeId, completed })}
      </output>
    </>
  )
}

function state() {
  return JSON.parse(page.getByTestId('navigation-state').element().textContent ?? '{}') as {
    conversations: ChatConversation[]
    activeId: string | null
    completed: boolean
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  waitForSaves.mockResolvedValue(undefined)
  let saved = [chat('chat-a'), chat('chat-b')]
  storage.save.mockImplementation(async (candidate: ChatConversation) => {
    saved = saved.map((item) =>
      item.id === candidate.id
        ? { ...candidate, archivedAt: candidate.pendingArchivedAt, pendingArchivedAt: undefined }
        : item
    )
  })
  storage.metas.mockImplementation(async () => saved)
})

afterEach(() => vi.restoreAllMocks())

describe('conversation workflow archive protection', () => {
  it('blocks a single enabled workflow member before writing or changing navigation', async () => {
    await render(<Harness memberships={{ 'chat-a': { instanceId: 'enabled-workflow' } }} />)
    await page.getByRole('button', { name: '归档当前对话', exact: true }).click()
    await expect.poll(() => state().completed).toBe(true)
    expect(storage.save).not.toHaveBeenCalled()
    expect(waitForSaves).not.toHaveBeenCalled()
    expect(archived).not.toHaveBeenCalled()
    expect(state().activeId).toBe('chat-a')
    expect(state().conversations).toEqual([chat('chat-a'), chat('chat-b')])
    expect(showToast).toHaveBeenCalledExactlyOnceWith('archiveWorkflowActive')
  })

  it('blocks the whole bulk selection if any selected conversation is an enabled member', async () => {
    await render(<Harness memberships={{ 'chat-b': { instanceId: 'enabled-workflow' } }} />)
    await page.getByRole('button', { name: '归档所有对话', exact: true }).click()
    await expect.poll(() => state().completed).toBe(true)
    expect(storage.save).not.toHaveBeenCalled()
    expect(storage.metas).not.toHaveBeenCalled()
    expect(archived).not.toHaveBeenCalled()
    expect(state().activeId).toBe('chat-a')
    expect(state().conversations.every((item) => !item.archivedAt && !item.pendingArchivedAt)).toBe(
      true
    )
    expect(showToast).toHaveBeenCalledExactlyOnceWith('archiveWorkflowActive')
  })

  it('archives normally when disabled workflows produce no active memberships', async () => {
    await render(<Harness memberships={{}} />)
    await page.getByRole('button', { name: '归档当前对话', exact: true }).click()
    await expect.poll(() => state().completed).toBe(true)
    expect(storage.save).toHaveBeenCalledTimes(1)
    expect(storage.metas).toHaveBeenCalledTimes(1)
    expect(archived).toHaveBeenCalledTimes(1)
    expect(state().activeId).toBeNull()
    expect(state().conversations[0].archivedAt).toBeGreaterThan(0)
    expect(state().conversations[0].pendingArchivedAt).toBeUndefined()
    expect(state().conversations[1]).toEqual(chat('chat-b'))
    expect(showToast).not.toHaveBeenCalled()
  })

  it('rolls back a stale-membership archive intent after one authoritative workflow rejection', async () => {
    let rejectWrite!: (reason: Error) => void
    storage.save.mockImplementationOnce(
      () =>
        new Promise<void>((_resolve, reject) => {
          rejectWrite = reject
        })
    )
    vi.spyOn(console, 'error').mockImplementation(() => {})
    await render(<Harness memberships={{}} />)
    await page.getByRole('button', { name: '归档当前对话', exact: true }).click()
    await expect.poll(() => storage.save.mock.calls.length).toBe(1)
    expect(state().conversations[0].pendingArchivedAt).toBeGreaterThan(0)
    expect(state().activeId).toBe('chat-a')
    rejectWrite(new Error('workflow_active_archive_blocked'))
    await expect.poll(() => state().completed).toBe(true)
    expect(storage.save).toHaveBeenCalledTimes(1)
    expect(storage.metas).not.toHaveBeenCalled()
    expect(archived).not.toHaveBeenCalled()
    expect(state().activeId).toBe('chat-a')
    expect(state().conversations[0].pendingArchivedAt).toBeUndefined()
    expect(state().conversations[0].archivedAt).toBeNull()
    expect(state().conversations[0].unreadAt).toBe(12)
    expect(showToast).toHaveBeenCalledExactlyOnceWith('archiveWorkflowActive')
  })
})
