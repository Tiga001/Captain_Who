import { useCallback, useRef, useState } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { WorkflowInstance, WorkflowResponse } from '@mycopilot/protocol'
import type { ChatConversation, ChatComposerDraft } from '../../features/chat/chatTypes'
import { createComposerDraft } from '../chatMessageFactory'
import { createWorkflow, createWorkflowNode } from '../../features/workflows/workflowAuthoring'
import { useWorkflowWorkspace, type WorkflowDraftPreferences } from '../useWorkflowWorkspace'

const mocks = vi.hoisted(() => ({
  request: vi.fn(),
  metas: vi.fn(),
  drafts: vi.fn(),
  flush: vi.fn(),
  wait: vi.fn(),
  persist: vi.fn()
}))
vi.mock('../../features/workflows/workflowClient', () => ({ requestWorkflows: mocks.request }))
vi.mock('../../features/storage/storageClient', () => ({
  loadConversationMetas: mocks.metas,
  loadComposerDrafts: mocks.drafts
}))

const instance: WorkflowInstance = {
  id: 'workflow-a',
  templateId: 'template-a',
  templateRevision: 1,
  name: 'Cross-project review',
  color: '#2478d4',
  revision: 1,
  updatedAt: 1,
  needsReview: false,
  running: false,
  bindings: [
    { nodeId: 'agent-a', conversationId: 'existing' },
    { nodeId: 'agent-b', conversationId: 'created' }
  ]
}
const chat = (id: string, projectId: string | null): ChatConversation => ({
  id,
  projectId,
  modelId: 'existing-model',
  title: id,
  createdAt: 1,
  updatedAt: 1,
  messages: [
    { id: 'active-reply', role: 'assistant', content: 'working', createdAt: 1, status: 'pending' }
  ],
  messagesLoaded: true
})
const response: WorkflowResponse = {
  records: [
    {
      definition: {
        ...createWorkflow(),
        id: 'template-a',
        nodes: [
          {
            ...createWorkflowNode('Existing role', 0, 0, 'node-model'),
            id: 'agent-a',
            permissionMode: 'custom'
          },
          {
            ...createWorkflowNode('Created role', 0, 0, 'template-model'),
            id: 'agent-b',
            permissionMode: 'full'
          }
        ]
      },
      issues: [],
      enabled: true,
      revision: 1,
      updatedAt: 1
    }
  ],
  issues: [],
  instances: [instance],
  affectedConversationIds: ['existing', 'created']
}

function Harness() {
  const [conversations, setConversations] = useState([
    chat('existing', 'project-a'),
    chat('unrelated', 'project-b')
  ])
  const [syncError, setSyncError] = useState(false)
  const [drafts, setDrafts] = useState<Record<string, ChatComposerDraft>>({
    existing: {
      ...createComposerDraft(),
      permissionMode: 'default' as const,
      message: 'unsent text',
      modelId: 'existing-model',
      queuedMessages: [
        {
          id: 'queued',
          clientMessageId: 'queued',
          content: 'queued before binding',
          attachments: [],
          modelId: 'queued-model',
          permissionMode: 'default',
          projectId: 'project-a',
          skills: [],
          status: 'pending',
          createdAt: 1
        }
      ]
    },
    unrelated: { ...createComposerDraft(), permissionMode: 'full' as const, message: 'keep me' }
  })
  const draftsRef = useRef(drafts)
  const applyDraftPreferences = useCallback(
    async (id: string, preferences: WorkflowDraftPreferences, fallback?: ChatComposerDraft) => {
      const existing = draftsRef.current[id] ?? fallback
      if (!existing) return false
      const next = { ...existing, ...preferences }
      draftsRef.current = { ...draftsRef.current, [id]: next }
      setDrafts(draftsRef.current)
      await mocks.persist(id, next)
      return true
    },
    []
  )
  const workflow = useWorkflowWorkspace({
    flushDraft: mocks.flush,
    waitForConversationSaves: mocks.wait,
    setConversations,
    applyDraftPreferences
  })
  return (
    <>
      <button
        onClick={async () => {
          await workflow.beforeCommit(['existing', 'existing'])
          try {
            await workflow.committed(response)
          } catch {
            setSyncError(true)
          }
        }}
      >
        commit
      </button>
      <button onClick={() => void workflow.committed({ records: [], issues: [], instances: [] })}>
        remove workflow
      </button>
      <button
        onClick={async () => {
          await workflow.retrySynchronization()
          setSyncError(false)
        }}
      >
        retry sync
      </button>
      <button
        onClick={() => {
          draftsRef.current = {
            ...draftsRef.current,
            existing: {
              ...draftsRef.current.existing,
              modelId: 'user-model',
              permissionMode: 'full'
            }
          }
          setDrafts(draftsRef.current)
        }}
      >
        change configuration
      </button>
      <button
        onClick={() => {
          const current = draftsRef.current.existing
          draftsRef.current = {
            ...draftsRef.current,
            existing: {
              ...current,
              message: 'new unsent text',
              queuedMessages: [
                ...current.queuedMessages,
                {
                  id: 'late',
                  clientMessageId: 'late',
                  content: 'queued during refresh',
                  attachments: [],
                  modelId: current.modelId,
                  permissionMode: current.permissionMode,
                  projectId: null,
                  skills: [],
                  status: 'pending',
                  createdAt: 2
                }
              ]
            }
          }
          setDrafts(draftsRef.current)
        }}
      >
        live draft update
      </button>
      {syncError && <span>sync failed</span>}
      <output data-testid="state">
        {JSON.stringify({ conversations, drafts, memberships: workflow.memberships })}
      </output>
    </>
  )
}

describe('workflow workspace synchronization', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.request.mockResolvedValue({ records: [], issues: [], instances: [] })
    mocks.flush.mockResolvedValue(undefined)
    mocks.wait.mockResolvedValue(undefined)
    mocks.persist.mockResolvedValue(undefined)
    mocks.metas.mockResolvedValue([
      chat('existing', 'project-a'),
      { ...chat('created', null), modelId: 'template-model' }
    ])
    mocks.drafts.mockResolvedValue({
      existing: { ...createComposerDraft(), permissionMode: 'custom', message: 'unsent text' },
      created: { ...createComposerDraft(), permissionMode: 'full' },
      unrelated: { ...createComposerDraft(), permissionMode: 'default', message: 'stale value' }
    })
  })

  it('flushes existing saves, applies next-turn binding preferences, and keeps unrelated conversations and drafts', async () => {
    const screen = await render(<Harness />)
    await screen.getByRole('button', { name: 'commit', exact: true }).click()
    const read = () => JSON.parse(screen.getByTestId('state').element().textContent!)
    await expect.poll(() => read().conversations).toHaveLength(3)
    expect(mocks.flush.mock.calls).toEqual([['existing']])
    expect(mocks.wait.mock.calls).toEqual([['existing']])
    expect(read().drafts.existing.permissionMode).toBe('custom')
    expect(read().drafts.existing.modelId).toBe('node-model')
    expect(
      read().conversations.find((item: ChatConversation) => item.id === 'existing').modelId
    ).toBe('existing-model')
    expect(
      read().conversations.find((item: ChatConversation) => item.id === 'existing').messages[0]
        .status
    ).toBe('pending')
    expect(read().drafts.existing.queuedMessages[0].modelId).toBe('queued-model')
    expect(read().drafts.existing.queuedMessages[0].permissionMode).toBe('default')
    expect(read().drafts.existing.message).toBe('unsent text')
    expect(read().drafts.unrelated.permissionMode).toBe('full')
    expect(read().drafts.unrelated.message).toBe('keep me')
    expect(read().memberships.existing).toEqual(read().memberships.created)
    expect(
      read().conversations.find((item: ChatConversation) => item.id === 'existing').projectId
    ).toBe('project-a')
    await screen.getByRole('button', { name: 'remove workflow' }).click()
    await expect.poll(() => read().memberships).toEqual({})
    expect(read().conversations).toHaveLength(3)
    expect(mocks.metas).toHaveBeenCalledOnce()
  })

  it('retains affected conversations after a failed refresh and retries without another save', async () => {
    mocks.drafts.mockRejectedValueOnce(new Error('temporary read failure'))
    const screen = await render(<Harness />)
    await screen.getByRole('button', { name: 'commit', exact: true }).click()
    await expect.element(screen.getByText('sync failed')).toBeVisible()
    const read = () => JSON.parse(screen.getByTestId('state').element().textContent!)
    expect(read().drafts.existing.permissionMode).toBe('custom')
    await screen.getByRole('button', { name: 'change configuration' }).click()
    await screen.getByRole('button', { name: 'retry sync' }).click()
    await expect.poll(() => read().conversations).toHaveLength(3)
    expect(read().drafts.existing.permissionMode).toBe('full')
    expect(read().drafts.existing.modelId).toBe('user-model')
    expect(mocks.request).toHaveBeenCalledTimes(1)
    expect(mocks.metas).toHaveBeenCalledTimes(2)
    expect(mocks.persist.mock.calls.filter(([id]) => id === 'existing')).toHaveLength(1)
  })

  it('keeps live queue/text updates while storage is refreshing and initializes config only once', async () => {
    let resolveDrafts!: (value: Record<string, ChatComposerDraft>) => void
    mocks.drafts.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveDrafts = resolve
        })
    )
    const screen = await render(<Harness />)
    await screen.getByRole('button', { name: 'commit', exact: true }).click()
    await expect.poll(() => mocks.drafts.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'live draft update' }).click()
    resolveDrafts({
      existing: { ...createComposerDraft(), modelId: 'stale-model' },
      created: { ...createComposerDraft(), modelId: 'template-model' }
    })
    const read = () => JSON.parse(screen.getByTestId('state').element().textContent!)
    await expect.poll(() => read().drafts.created?.modelId).toBe('template-model')
    expect(read().drafts.existing.message).toBe('new unsent text')
    expect(read().drafts.existing.modelId).toBe('node-model')
    expect(
      read().drafts.existing.queuedMessages.map((item: { modelId: string }) => item.modelId)
    ).toEqual(['queued-model', 'node-model'])
    expect(mocks.persist.mock.calls.filter(([id]) => id === 'existing')).toHaveLength(1)
  })

  it('reloads persisted membership after template or binding changes', async () => {
    const screen = await render(<Harness />)
    await expect.poll(() => mocks.request.mock.calls.length).toBe(1)
    mocks.request.mockResolvedValue({ records: [], issues: [], instances: [instance] })
    window.dispatchEvent(new Event('captain:workflows-changed'))
    await expect
      .poll(
        () =>
          JSON.parse(screen.getByTestId('state').element().textContent!).memberships.existing?.id
      )
      .toBe(instance.id)
  })
})
