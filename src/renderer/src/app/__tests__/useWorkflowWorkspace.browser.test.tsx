import { useCallback, useLayoutEffect, useRef, useState } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type {
  WorkflowDefinition,
  WorkflowInstance,
  WorkflowResponse,
  WorkflowRuntimeSnapshot
} from '@mycopilot/protocol'
import { parseWorkflowRuntimeSnapshot } from '@mycopilot/protocol'
import type { ChatConversation, ChatComposerDraft } from '../../features/chat/chatTypes'
import { createComposerDraft } from '../chatMessageFactory'
import { createWorkflow, createWorkflowNode } from '../../features/workflows/workflowAuthoring'
import { useWorkflowWorkspace, type WorkflowDraftPreferences } from '../useWorkflowWorkspace'
import { mergeStoredWorkflowPreferences } from '../workflowDraftPreferenceSync'
import { ComposerDraftPersistenceQueue } from '../composerDraftPersistence'

const mocks = vi.hoisted(() => ({
  request: vi.fn(),
  metas: vi.fn(),
  drafts: vi.fn(),
  flush: vi.fn(),
  wait: vi.fn(),
  persist: vi.fn(),
  runtimeListener: null as ((snapshot: WorkflowRuntimeSnapshot) => void) | null
}))
vi.mock('../../host/hostClient', () => ({
  hostClient: {
    agent: {
      onWorkflowRuntimeChanged: (listener: (snapshot: WorkflowRuntimeSnapshot) => void) => {
        mocks.runtimeListener = listener
        return () => {
          mocks.runtimeListener = null
        }
      }
    }
  }
}))
vi.mock('../../features/workflows/workflowClient', () => ({ requestWorkflows: mocks.request }))
vi.mock('../../features/storage/storageClient', () => ({
  loadConversationMetas: mocks.metas,
  loadComposerDrafts: mocks.drafts
}))

const definition: WorkflowDefinition = {
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
}
const instance: WorkflowInstance = {
  id: 'workflow-a',
  templateId: 'template-a',
  templateRevision: 1,
  definition,
  name: 'Cross-project review',
  color: '#2478d4',
  revision: 1,
  updatedAt: 1,
  needsReview: false,
  enabled: true,
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
      definition,
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

function Harness({ persistLiveChanges = false }: { persistLiveChanges?: boolean }) {
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
  const conversationsRef = useRef(conversations)
  useLayoutEffect(() => {
    conversationsRef.current = conversations
  }, [conversations])
  const hydrateDraft = useCallback((id: string, storedDraft: ChatComposerDraft) => {
    if (draftsRef.current[id]) return
    draftsRef.current = { ...draftsRef.current, [id]: storedDraft }
    setDrafts(draftsRef.current)
  }, [])
  const readDraftPreferences = useCallback(
    () =>
      Object.fromEntries(
        Object.entries(draftsRef.current).map(([id, draft]) => [
          id,
          { modelId: draft.modelId, permissionMode: draft.permissionMode }
        ])
      ),
    []
  )
  const applyStoredDraftPreferences = useCallback(
    async (
      id: string,
      stored: Partial<WorkflowDraftPreferences>,
      expected: Partial<WorkflowDraftPreferences>,
      forcePersist = false
    ) => {
      const current = draftsRef.current[id]
      if (!current) return
      const next = mergeStoredWorkflowPreferences(current, stored, expected)
      if (next === current && !forcePersist) return
      draftsRef.current = { ...draftsRef.current, [id]: next }
      setDrafts(draftsRef.current)
      await mocks.persist(id, next)
    },
    []
  )
  const workflow = useWorkflowWorkspace({
    readDraftPreferences,
    applyStoredDraftPreferences,
    conversationsRef,
    hydrateDraft,
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
        remove organization
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
          if (persistLiveChanges) void mocks.persist('existing', draftsRef.current.existing)
        }}
      >
        live draft update
      </button>
      {syncError && <span>sync failed</span>}
      <output data-testid="state">
        {JSON.stringify({
          conversations,
          drafts,
          memberships: workflow.memberships,
          neighbors: workflow.neighborNodes,
          allMemberships: workflow.allMemberships
        })}
      </output>
    </>
  )
}

function preferencesChanged(kinds: string[], sequence = 10): WorkflowRuntimeSnapshot {
  return {
    instanceId: instance.id,
    sequence: sequence + kinds.length,
    inputs: [],
    events: [
      ...kinds.map((kind, index) => ({
        instanceId: instance.id,
        sequence: sequence + index,
        kind,
        sourceNodeId: 'agent-b',
        targetNodeId: 'agent-a',
        inputId: null,
        messageId: null,
        createdAt: 3
      })),
      {
        instanceId: instance.id,
        sequence: sequence + kinds.length,
        kind: 'members_changed',
        sourceNodeId: 'agent-b',
        targetNodeId: null,
        inputId: null,
        messageId: null,
        createdAt: 3
      }
    ]
  }
}
function mockExistingMember() {
  mocks.request.mockResolvedValue({
    records: [],
    issues: [],
    instances: [{ ...instance, bindings: instance.bindings.slice(0, 1) }]
  })
}
function freshPreferences(
  organizationRevision: number,
  preferences: Partial<WorkflowDraftPreferences>
): WorkflowRuntimeSnapshot {
  return {
    ...preferencesChanged(
      [
        ...(Object.hasOwn(preferences, 'modelId') ? ['member_model_changed'] : []),
        ...(Object.hasOwn(preferences, 'permissionMode') ? ['member_permissions_changed'] : [])
      ],
      organizationRevision * 10
    ),
    preferenceUpdates: [
      {
        nodeId: 'agent-a',
        conversationId: 'existing',
        organizationRevision,
        ...preferences
      }
    ]
  }
}

describe('organization workspace synchronization', () => {
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
      created: { ...createComposerDraft(), permissionMode: 'full', modelId: 'template-model' },
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
    await screen.getByRole('button', { name: 'remove organization' }).click()
    await expect.poll(() => read().memberships).toEqual({})
    expect(read().allMemberships).toEqual({})
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

  it('shows membership only while enabled and never reapplies binding preferences on a toggle', async () => {
    mocks.request.mockResolvedValue({ records: [], issues: [], instances: [instance] })
    const screen = await render(<Harness />)
    const read = () => JSON.parse(screen.getByTestId('state').element().textContent!)
    await expect.poll(() => read().memberships.existing?.id).toBe(instance.id)
    expect(read().allMemberships.existing).toEqual(read().memberships.existing)
    mocks.request.mockResolvedValue({
      records: [],
      issues: [],
      instances: [{ ...instance, enabled: false }]
    })
    window.dispatchEvent(new Event('captain:workflows-changed'))
    await expect.poll(() => read().memberships).toEqual({})
    expect(read().allMemberships.existing).toEqual({
      id: instance.id,
      name: instance.name,
      color: instance.color
    })
    mocks.request.mockResolvedValue({ records: [], issues: [], instances: [instance] })
    window.dispatchEvent(new Event('captain:workflows-changed'))
    await expect.poll(() => read().memberships.existing?.id).toBe(instance.id)
    expect(mocks.persist).not.toHaveBeenCalled()
    expect(read().drafts.existing.modelId).toBe('existing-model')
    expect(read().drafts.existing.permissionMode).toBe('default')
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
  it.each(['events', 'summary'])(
    'refreshes managed members and initializes new defaults from %s',
    async (mode) => {
      mocks.request.mockResolvedValue({
        records: [],
        issues: [],
        instances: [
          {
            ...instance,
            bindings: instance.bindings.slice(0, 1),
            definition: { ...definition, nodes: definition.nodes.slice(0, 1) }
          }
        ]
      })
      const screen = await render(<Harness />)
      const read = () => JSON.parse(screen.getByTestId('state').element().textContent!)
      await expect.poll(() => read().memberships.existing?.id).toBe(instance.id)
      expect(read().neighbors.existing).toEqual([])
      mocks.request.mockResolvedValue({ records: [], issues: [], instances: [instance] })
      const snapshot: WorkflowRuntimeSnapshot = {
        instanceId: instance.id,
        sequence: 1,
        inputs: [],
        events: [
          {
            instanceId: instance.id,
            sequence: 1,
            kind: 'members_changed',
            sourceNodeId: 'agent-a',
            targetNodeId: null,
            inputId: null,
            messageId: null,
            createdAt: 2
          }
        ]
      }
      if (mode === 'summary') {
        snapshot.sequence = 700
        snapshot.events = []
        snapshot.summary = { pendingByNode: [], conversationChanges: [], structureRevision: 1 }
      }
      mocks.runtimeListener?.(snapshot)
      await expect.poll(() => read().conversations).toHaveLength(3)
      await expect.poll(() => read().drafts.created?.modelId).toBe('template-model')
      expect(read().drafts.existing.modelId).toBe('existing-model')
      expect(read().neighbors.existing).toEqual([
        { nodeId: 'agent-b', name: 'Created role', conversationId: 'created' }
      ])
      const reads = mocks.request.mock.calls.length
      mocks.runtimeListener?.(snapshot)
      expect(mocks.request.mock.calls.length).toBe(reads)
    }
  )
  it('does not reset existing preferences when member invalidation overtakes the initial catalog read', async () => {
    let resolveInitial!: (value: WorkflowResponse) => void
    mocks.request
      .mockImplementationOnce(
        () =>
          new Promise<WorkflowResponse>((resolve) => {
            resolveInitial = resolve
          })
      )
      .mockResolvedValue({ records: [], issues: [], instances: [instance] })
    const screen = await render(<Harness />)
    const read = () => JSON.parse(screen.getByTestId('state').element().textContent!)
    await screen.getByRole('button', { name: 'change configuration' }).click()
    mocks.runtimeListener?.({
      instanceId: instance.id,
      sequence: 2,
      inputs: [],
      events: [
        {
          instanceId: instance.id,
          sequence: 2,
          kind: 'members_changed',
          sourceNodeId: 'agent-a',
          targetNodeId: null,
          inputId: null,
          messageId: null,
          createdAt: 3
        }
      ]
    })
    await expect.poll(() => read().conversations).toHaveLength(3)
    expect(read().drafts.existing.modelId).toBe('user-model')
    expect(read().drafts.existing.permissionMode).toBe('full')
    expect(mocks.persist).not.toHaveBeenCalled()
    resolveInitial({ records: [], issues: [], instances: [] })
    await expect.poll(() => read().memberships.created?.id).toBe(instance.id)
  })

  it('hydrates missing host-created conversations on initial load without changing existing drafts', async () => {
    mocks.request.mockResolvedValue({ records: [], issues: [], instances: [instance] })
    const screen = await render(<Harness />)
    const read = () => JSON.parse(screen.getByTestId('state').element().textContent!)
    await expect.poll(() => read().conversations).toHaveLength(3)
    expect(read().drafts.created.modelId).toBe('template-model')
    expect(read().drafts.existing.modelId).toBe('existing-model')
    expect(mocks.persist).not.toHaveBeenCalled()
  })
  it('syncs only a targeted model field from the latest stored draft and preserves local content', async () => {
    mockExistingMember()
    mocks.drafts.mockResolvedValue({
      existing: {
        ...createComposerDraft(),
        modelId: 'latest-stored-model',
        permissionMode: 'full',
        message: 'stale stored text',
        queuedMessages: []
      }
    })
    const screen = await render(<Harness />)
    const read = () => JSON.parse(screen.getByTestId('state').element().textContent!)
    await expect.poll(() => read().memberships.existing?.id).toBe(instance.id)
    mocks.runtimeListener?.(preferencesChanged(['member_model_changed']))
    await expect.poll(() => read().drafts.existing.modelId).toBe('latest-stored-model')
    expect(read().drafts.existing.permissionMode).toBe('default')
    expect(read().drafts.existing.message).toBe('unsent text')
    expect(read().drafts.existing.queuedMessages[0].modelId).toBe('queued-model')
    expect(mocks.metas).not.toHaveBeenCalled()
    expect(mocks.persist).toHaveBeenCalledTimes(1)
  })

  it('syncs a targeted permission field without resetting a different model selection', async () => {
    mockExistingMember()
    mocks.drafts.mockResolvedValue({
      existing: {
        ...createComposerDraft(),
        modelId: 'ignored-stored-model',
        permissionMode: 'custom'
      }
    })
    const screen = await render(<Harness />)
    const read = () => JSON.parse(screen.getByTestId('state').element().textContent!)
    await expect.poll(() => read().memberships.existing?.id).toBe(instance.id)
    mocks.runtimeListener?.(preferencesChanged(['member_permissions_changed']))
    await expect.poll(() => read().drafts.existing.permissionMode).toBe('custom')
    expect(read().drafts.existing.modelId).toBe('existing-model')
    expect(read().drafts.existing.message).toBe('unsent text')
  })

  it('keeps newer user selections and queue edits while a preference refresh is pending', async () => {
    mockExistingMember()
    let resolveDrafts!: (value: Record<string, ChatComposerDraft>) => void
    mocks.drafts.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveDrafts = resolve
        })
    )
    const screen = await render(<Harness />)
    const read = () => JSON.parse(screen.getByTestId('state').element().textContent!)
    await expect.poll(() => read().memberships.existing?.id).toBe(instance.id)
    mocks.runtimeListener?.(
      preferencesChanged(['member_model_changed', 'member_permissions_changed'])
    )
    await expect.poll(() => mocks.drafts.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'change configuration' }).click()
    await screen.getByRole('button', { name: 'live draft update' }).click()
    resolveDrafts({
      existing: { ...createComposerDraft(), modelId: 'backend-model', permissionMode: 'custom' }
    })
    await expect.poll(() => read().drafts.existing.message).toBe('new unsent text')
    expect(read().drafts.existing.modelId).toBe('user-model')
    expect(read().drafts.existing.permissionMode).toBe('full')
    expect(read().drafts.existing.queuedMessages).toHaveLength(2)
    expect(mocks.persist).not.toHaveBeenCalled()
  })

  it('retries failed preference reads on focus and never replays the old node default', async () => {
    mockExistingMember()
    const logged = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    mocks.drafts.mockRejectedValueOnce(new Error('temporary read failure')).mockResolvedValue({
      existing: {
        ...createComposerDraft(),
        modelId: 'newer-persisted-user-model',
        permissionMode: 'default'
      }
    })
    const screen = await render(<Harness />)
    const read = () => JSON.parse(screen.getByTestId('state').element().textContent!)
    await expect.poll(() => read().memberships.existing?.id).toBe(instance.id)
    mocks.runtimeListener?.(preferencesChanged(['member_model_changed'], 1))
    await expect.poll(() => logged.mock.calls.length).toBe(1)
    expect(read().drafts.existing.modelId).toBe('existing-model')
    window.dispatchEvent(new Event('focus'))
    await expect.poll(() => read().drafts.existing.modelId).toBe('newer-persisted-user-model')
    expect(read().drafts.existing.modelId).not.toBe(
      definition.nodes[0].kind === 'agent' ? definition.nodes[0].modelConfigId : null
    )
    logged.mockRestore()
  })

  it('applies historical preference events against latest stored values during the first catalog read', async () => {
    let resolveInitial!: (value: WorkflowResponse) => void
    mocks.request
      .mockImplementationOnce(
        () =>
          new Promise<WorkflowResponse>((resolve) => {
            resolveInitial = resolve
          })
      )
      .mockResolvedValue({
        records: [],
        issues: [],
        instances: [{ ...instance, bindings: instance.bindings.slice(0, 1) }]
      })
    mocks.drafts.mockResolvedValue({
      existing: {
        ...createComposerDraft(),
        modelId: 'latest-stored-model',
        permissionMode: 'custom'
      }
    })
    const screen = await render(<Harness />)
    const read = () => JSON.parse(screen.getByTestId('state').element().textContent!)
    mocks.runtimeListener?.(
      preferencesChanged(['member_model_changed', 'member_permissions_changed'], 1)
    )
    await expect.poll(() => read().drafts.existing.modelId).toBe('latest-stored-model')
    expect(read().drafts.existing.permissionMode).toBe('custom')
    resolveInitial({ records: [], issues: [], instances: [] })
    await expect.poll(() => read().memberships.existing?.id).toBe(instance.id)
    expect(read().drafts.existing.message).toBe('unsent text')
  })

  it('applies fresh preferences before queue edits and persists after an older full draft save', async () => {
    mockExistingMember()
    let stored = { ...createComposerDraft(), modelId: 'existing-model' }
    let releaseOldSave!: () => void
    const oldSaveBlocked = new Promise<void>((resolve) => {
      releaseOldSave = resolve
    })
    let oldSaveStarted = false
    const persistence = new ComposerDraftPersistenceQueue(
      {
        saveDraft: async (_id, draft) => {
          if (!oldSaveStarted) {
            oldSaveStarted = true
            await oldSaveBlocked
          }
          stored = draft
        },
        saveMessage: vi.fn().mockResolvedValue(true)
      },
      (error) => {
        throw error
      }
    )
    mocks.persist.mockImplementation((id, draft) => persistence.persistNow(id, draft))
    mocks.drafts.mockImplementation(async () => ({ existing: stored }))
    const screen = await render(<Harness persistLiveChanges />)
    const read = () => JSON.parse(screen.getByTestId('state').element().textContent!)
    await expect.poll(() => read().memberships.existing?.id).toBe(instance.id)
    const staleSave = persistence.persistNow('existing', stored)
    await expect.poll(() => oldSaveStarted).toBe(true)

    stored = { ...stored, modelId: 'admin-model' }
    mocks.runtimeListener?.(freshPreferences(2, { modelId: 'admin-model' }))
    await expect.poll(() => read().drafts.existing.modelId).toBe('admin-model')
    await screen.getByRole('button', { name: 'live draft update' }).click()
    expect(read().drafts.existing.queuedMessages[1].modelId).toBe('admin-model')
    expect(read().drafts.existing.permissionMode).toBe('default')
    expect(mocks.drafts).not.toHaveBeenCalled()

    releaseOldSave()
    await staleSave
    await persistence.flushAll()
    await expect.poll(() => mocks.request.mock.calls.length).toBe(2)
    expect(stored.modelId).toBe('admin-model')
    expect(stored.message).toBe('new unsent text')
    expect(stored.queuedMessages).toHaveLength(2)
    expect(read().drafts.existing.modelId).toBe('admin-model')
    expect(mocks.drafts).not.toHaveBeenCalled()
  })

  it('ignores duplicate and older fresh revisions separately for each preference field', async () => {
    mockExistingMember()
    const screen = await render(<Harness />)
    const read = () => JSON.parse(screen.getByTestId('state').element().textContent!)
    await expect.poll(() => read().memberships.existing?.id).toBe(instance.id)
    mocks.runtimeListener?.(
      freshPreferences(4, { modelId: 'latest-model', permissionMode: 'custom' })
    )
    await expect.poll(() => read().drafts.existing.modelId).toBe('latest-model')
    mocks.runtimeListener?.(freshPreferences(3, { modelId: 'older-model', permissionMode: 'full' }))
    expect(mocks.persist).toHaveBeenCalledTimes(1)
    expect(read().drafts.existing.permissionMode).toBe('custom')
    const latest = freshPreferences(5, { permissionMode: 'full' })
    mocks.runtimeListener?.(latest)
    await expect.poll(() => read().drafts.existing.permissionMode).toBe('full')
    mocks.runtimeListener?.(latest)
    expect(mocks.persist).toHaveBeenCalledTimes(2)
    expect(read().drafts.existing.modelId).toBe('latest-model')
    expect(read().drafts.existing.message).toBe('unsent text')
  })

  it('applies coalesced model and permission edits with independent revisions and preserves a later model update', async () => {
    mockExistingMember()
    const screen = await render(<Harness />)
    const read = () => JSON.parse(screen.getByTestId('state').element().textContent!)
    await expect.poll(() => read().memberships.existing?.id).toBe(instance.id)
    const model = freshPreferences(2, { modelId: 'model-revision-2' })
    const permission = freshPreferences(4, { permissionMode: 'full' })
    const combined = parseWorkflowRuntimeSnapshot({
      ...permission,
      preferenceUpdates: [...model.preferenceUpdates!, ...permission.preferenceUpdates!]
    })
    mocks.runtimeListener?.(combined)
    await expect.poll(() => read().drafts.existing.modelId).toBe('model-revision-2')
    expect(read().drafts.existing.permissionMode).toBe('full')
    expect(read().drafts.existing.message).toBe('unsent text')
    expect(read().drafts.existing.queuedMessages[0].modelId).toBe('queued-model')
    expect(mocks.persist).toHaveBeenCalledTimes(2)

    // Revision 4 only advanced permission: a later-arriving model revision 3 must still apply.
    mocks.runtimeListener?.(freshPreferences(3, { modelId: 'model-revision-3' }))
    await expect.poll(() => read().drafts.existing.modelId).toBe('model-revision-3')
    mocks.runtimeListener?.(combined)
    expect(read().drafts.existing.modelId).toBe('model-revision-3')
    expect(read().drafts.existing.permissionMode).toBe('full')
    expect(mocks.persist).toHaveBeenCalledTimes(3)
    expect(mocks.drafts).not.toHaveBeenCalled()
  })

  it('does not reapply fresh defaults when a later receipt replay has no preference payload', async () => {
    mockExistingMember()
    const screen = await render(<Harness />)
    const read = () => JSON.parse(screen.getByTestId('state').element().textContent!)
    await expect.poll(() => read().memberships.existing?.id).toBe(instance.id)
    mocks.runtimeListener?.(freshPreferences(2, { modelId: 'admin-model' }))
    await expect.poll(() => mocks.request.mock.calls.length).toBe(2)
    await screen.getByRole('button', { name: 'change configuration' }).click()
    mocks.drafts.mockResolvedValue({
      existing: { ...createComposerDraft(), modelId: 'user-model', permissionMode: 'full' }
    })
    mocks.runtimeListener?.(preferencesChanged(['member_model_changed'], 30))
    await expect.poll(() => mocks.drafts.mock.calls.length).toBe(1)
    expect(read().drafts.existing.modelId).toBe('user-model')
    expect(read().drafts.existing.permissionMode).toBe('full')
    expect(mocks.persist).toHaveBeenCalledTimes(1)
  })

  it('invalidates an older pending storage read when fresh preferences arrive', async () => {
    mockExistingMember()
    let resolveDrafts!: (value: Record<string, ChatComposerDraft>) => void
    mocks.drafts.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveDrafts = resolve
        })
    )
    const screen = await render(<Harness />)
    const read = () => JSON.parse(screen.getByTestId('state').element().textContent!)
    await expect.poll(() => read().memberships.existing?.id).toBe(instance.id)
    mocks.runtimeListener?.(preferencesChanged(['member_model_changed'], 10))
    await expect.poll(() => mocks.drafts.mock.calls.length).toBe(1)
    mocks.runtimeListener?.(freshPreferences(2, { modelId: 'admin-model' }))
    await expect.poll(() => read().drafts.existing.modelId).toBe('admin-model')
    resolveDrafts({ existing: { ...createComposerDraft(), modelId: 'old-stored-model' } })
    await expect.poll(() => mocks.request.mock.calls.length).toBe(3)
    expect(read().drafts.existing.modelId).toBe('admin-model')
    expect(mocks.persist).toHaveBeenCalledTimes(1)
  })
})
