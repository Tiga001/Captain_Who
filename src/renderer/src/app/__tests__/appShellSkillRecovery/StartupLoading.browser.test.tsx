import type { SkillSelection } from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatComposerDraft, ChatConversation } from '../../../features/chat/chatTypes'
import { testState } from './state'
import { deferred } from './eventFixtures'
import { storedConversation } from './conversationFixtures'
import {
  AppShell,
  createComposerDraft,
  defaultUiPreferences,
  renderSelectedConversation
} from './setup'

describe('conversation startup loading', () => {
  it('preserves a newer window preference when the startup snapshot returns late', async () => {
    const saved = { ...defaultUiPreferences(), predictNextUserMessage: true }
    const pending = deferred<typeof saved>()
    testState.loadUiPreferences.mockReturnValueOnce(pending.promise)
    const screen = await renderSelectedConversation()
    const active = screen.getByTestId('active-conversation-id')
    const changed = testState.onPredictionPreferenceChanged.mock.lastCall?.[0]
    changed(false)
    pending.resolve(saved)
    await new Promise((resolve) => window.setTimeout(resolve, 0))
    await expect.element(active).toHaveAttribute('data-prediction-enabled', 'false')
    changed(true)
    await expect.element(active).toHaveAttribute('data-prediction-enabled', 'true')
    testState.rightOpen = true
    testState.rightMaximized = true
    await screen.rerender(<AppShell />)
    await expect.element(active).toHaveAttribute('data-prediction-enabled', 'false')
  })

  it('waits for restored drafts before enabling catch-up when opening an idle saved chat', async () => {
    const pending = deferred<Record<string, ChatComposerDraft>>()
    testState.loadComposerDrafts.mockReturnValueOnce(pending.promise)
    testState.loadUiPreferences.mockResolvedValueOnce({
      ...defaultUiPreferences(),
      predictNextUserMessage: true
    })
    const screen = await renderSelectedConversation()
    const active = screen.getByTestId('active-conversation-id')
    await expect.element(active).toHaveAttribute('data-prediction-enabled', 'false')
    pending.resolve({})
    await expect.element(active).toHaveAttribute('data-prediction-enabled', 'true')
  })

  it('keeps the prompt stable when the global new-conversation action is repeated on the root page', async () => {
    const screen = await render(<AppShell />)
    const promptIndex = screen.getByTestId('new-conversation-prompt-index')
    await expect.element(promptIndex).toBeVisible()
    const initialPromptIndex = promptIndex.element().textContent

    await screen.getByRole('button', { name: 'new-conversation', exact: true }).click()

    await expect.element(promptIndex).toHaveTextContent(initialPromptIndex ?? '')
    await expect.element(screen.getByTestId('new-conversation-project')).toHaveTextContent('none')
  })

  it('selects a different prompt when returning from an existing conversation', async () => {
    const screen = await render(<AppShell />)
    const promptIndex = screen.getByTestId('new-conversation-prompt-index')
    await expect.element(promptIndex).toBeVisible()
    const initialPromptIndex = promptIndex.element().textContent

    await screen.getByRole('button', { name: 'select-conversation-a', exact: true }).click()
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-a')
    await screen.getByRole('button', { name: 'new-conversation', exact: true }).click()

    await expect.element(screen.getByTestId('new-conversation-prompt-index')).toBeVisible()
    expect(screen.getByTestId('new-conversation-prompt-index').element().textContent).not.toBe(
      initialPromptIndex
    )
  })

  it('starts on a new conversation and hydrates a stored conversation only after selection', async () => {
    const active = storedConversation()
    const archived = {
      ...storedConversation(),
      id: 'conversation-archived',
      title: 'Archived',
      messages: [],
      messagesLoaded: false,
      archivedAt: 10
    }
    const activeDetail = deferred<ChatConversation>()
    testState.loadConversationMetas.mockResolvedValueOnce([
      { ...active, messages: [], messagesLoaded: false },
      archived
    ])
    testState.loadConversation.mockReturnValueOnce(activeDetail.promise)

    const screen = await render(<AppShell />)

    await expect.poll(() => testState.loadConversationMetas.mock.calls.length).toBe(1)
    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()
    expect(testState.loadConversation).not.toHaveBeenCalled()

    await screen.getByRole('button', { name: 'select-conversation-a', exact: true }).click()
    await expect.poll(() => testState.loadConversation.mock.calls).toEqual([['conversation-a']])
    await expect.element(screen.getByText('chat.loadingConversation')).toBeVisible()
    expect(testState.loadConversation).not.toHaveBeenCalledWith('conversation-archived')

    activeDetail.resolve(active)
    await expect.element(screen.getByRole('button', { name: 'submit-with-skill' })).toBeVisible()
    expect(testState.loadConversation).not.toHaveBeenCalledWith('conversation-archived')
  })

  it('commits preferences and drafts without waiting for the conversation catalog', async () => {
    const conversationMetas = deferred<ChatConversation[]>()
    testState.loadConversationMetas.mockReturnValueOnce(conversationMetas.promise)
    testState.loadUiPreferences.mockResolvedValueOnce({
      ...defaultUiPreferences(),
      translucentSidebar: true,
      showContextWindowUsage: false
    })
    testState.loadComposerDrafts.mockResolvedValueOnce({
      'new-conversation': createComposerDraft({ message: 'independent draft' })
    })

    const screen = await render(<AppShell />)

    await expect.element(screen.getByTestId('sidebar-translucent')).toHaveTextContent('true')
    await expect
      .element(screen.getByTestId('new-conversation-draft'))
      .toHaveTextContent('independent draft')

    conversationMetas.resolve([])
  })

  it('restores the complete new-conversation draft after visiting an existing conversation', async () => {
    const attachment = {
      id: 'new-task-attachment',
      kind: 'file' as const,
      name: 'requirements.txt',
      mimeType: 'text/plain',
      sizeBytes: 12,
      encoding: 'managed' as const,
      data: 'managed-test-cmVxdWlyZW1lbnRz'
    }
    const bundledSkill: SkillSelection = {
      id: 'bundled:application:documents',
      revision: 'skill-package-sha256-v2:documents'
    }
    const newConversationDraft = createComposerDraft({
      attachments: [attachment],
      message: 'unfinished new task',
      modelId: 'model-2',
      permissionMode: 'custom',
      projectId: 'project-a',
      skills: [bundledSkill]
    })
    testState.loadComposerDrafts.mockResolvedValueOnce({
      'conversation-a': createComposerDraft({ modelId: 'model-1', projectId: 'project-a' }),
      'new-conversation': newConversationDraft
    })

    const screen = await render(<AppShell />)
    await expect
      .element(screen.getByTestId('new-conversation-draft'))
      .toHaveTextContent(newConversationDraft.message)

    await screen.getByRole('button', { name: 'select-conversation-a', exact: true }).click()
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-a')
    await screen.getByRole('button', { name: 'new-conversation', exact: true }).click()

    expect(
      JSON.parse(screen.getByTestId('new-conversation-payload').element().textContent ?? '{}')
    ).toEqual({
      attachments: newConversationDraft.attachments,
      message: newConversationDraft.message,
      modelId: newConversationDraft.modelId,
      permissionMode: newConversationDraft.permissionMode,
      projectId: newConversationDraft.projectId,
      skills: newConversationDraft.skills
    })
  })

  it('changes only the workspace when opening a project-specific new conversation', async () => {
    const bundledSkill: SkillSelection = {
      id: 'bundled:application:spreadsheets',
      revision: 'skill-package-sha256-v2:spreadsheets'
    }
    const workspaceSkill: SkillSelection = {
      id: 'workspace:old-project:local-skill',
      revision: 'skill-package-sha256-v2:local-skill'
    }
    const newConversationDraft = createComposerDraft({
      attachments: [
        {
          id: 'new-task-image',
          kind: 'image',
          name: 'reference.png',
          mimeType: 'image/png',
          sizeBytes: 8,
          encoding: 'managed',
          data: 'managed-test-aW1hZ2U='
        }
      ],
      message: 'continue this new task',
      modelId: 'model-2',
      permissionMode: 'full',
      projectId: null,
      skills: [bundledSkill, workspaceSkill]
    })
    testState.loadComposerDrafts.mockResolvedValueOnce({
      'conversation-a': createComposerDraft({ modelId: 'model-1', projectId: 'project-a' }),
      'new-conversation': newConversationDraft
    })

    const screen = await render(<AppShell />)
    await screen.getByRole('button', { name: 'new-conversation-project-a', exact: true }).click()

    expect(
      JSON.parse(screen.getByTestId('new-conversation-payload').element().textContent ?? '{}')
    ).toEqual({
      attachments: newConversationDraft.attachments,
      message: newConversationDraft.message,
      modelId: newConversationDraft.modelId,
      permissionMode: newConversationDraft.permissionMode,
      projectId: 'project-a',
      skills: [bundledSkill]
    })
    await expect.poll(() => testState.saveComposerDraft.mock.calls.length).toBe(1)
    expect(testState.saveComposerDraft).toHaveBeenLastCalledWith(
      'new-conversation',
      expect.objectContaining({
        attachments: newConversationDraft.attachments,
        message: newConversationDraft.message,
        modelId: newConversationDraft.modelId,
        projectId: 'project-a',
        skills: [bundledSkill]
      })
    )
  })
})
