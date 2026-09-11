import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatConversation } from '../../chat/chatTypes'
import { singleFolderProject } from '../../projects/__tests__/projectFixtures'
import { ArchivedConversationsSettingsPage } from '../pages/ArchivedConversationsSettingsPage'
import '../../../styles/global.css'

vi.mock('../../../config/FrontendConfigProvider', async () => {
  const { getTranslation } = await import('../../../config/frontendTranslations')
  return {
    useFrontendConfig: () => ({
      language: 'en-US',
      t: (key: Parameters<typeof getTranslation>[1]) => getTranslation('en-US', key)
    })
  }
})

const projects = [
  singleFolderProject({ id: 'project-a', name: 'Project A', path: '/workspace/a' }),
  singleFolderProject({ id: 'project-b', name: 'Project B', path: '/workspace/b' })
]

function archivedConversation(
  overrides: Pick<ChatConversation, 'id' | 'title'> & Partial<ChatConversation>
): ChatConversation {
  return {
    projectId: 'project-a',
    modelId: null,
    messages: [],
    createdAt: 1,
    updatedAt: 10,
    archivedAt: 10,
    ...overrides
  }
}

const conversations: ChatConversation[] = [
  archivedConversation({
    id: 'beta-latest',
    projectId: 'project-b',
    title: 'Beta latest',
    updatedAt: 300,
    archivedAt: 300
  }),
  archivedConversation({
    id: 'alpha',
    projectId: 'project-a',
    title: 'Alpha chat',
    updatedAt: 200,
    archivedAt: 200
  }),
  archivedConversation({
    id: 'beta-older',
    projectId: 'project-b',
    title: 'Beta older',
    updatedAt: 150,
    archivedAt: 150
  }),
  archivedConversation({
    id: 'loose',
    projectId: null,
    title: 'Loose chat',
    updatedAt: 100,
    archivedAt: 100
  }),
  {
    id: 'open',
    projectId: 'project-a',
    modelId: null,
    title: 'Still open',
    messages: [],
    createdAt: 1,
    updatedAt: 400,
    archivedAt: null
  }
]

function groupSummaries() {
  return [...document.querySelectorAll('.archived-conversations-group')].map((group) => ({
    title: group.querySelector('.archived-conversations-group__title')?.textContent?.trim(),
    count: group.querySelector('.archived-conversations-group__count')?.textContent,
    titles: [...group.querySelectorAll('.archived-conversation-row__main strong')].map(
      (node) => node.textContent
    )
  }))
}

describe('ArchivedConversationsSettingsPage', () => {
  it('groups archived chats by project and keeps restore and delete actions visible', async () => {
    const onUnarchiveConversation = vi.fn()
    const screen = await render(
      <ArchivedConversationsSettingsPage
        conversations={conversations}
        onDeleteArchivedConversations={vi.fn()}
        onDeleteConversation={vi.fn()}
        onUnarchiveConversation={onUnarchiveConversation}
        projects={projects}
      />
    )

    expect(groupSummaries()).toEqual([
      { title: 'Project B', count: '2 chats', titles: ['Beta latest', 'Beta older'] },
      { title: 'Project A', count: '1 chats', titles: ['Alpha chat'] },
      { title: 'No project', count: '1 chats', titles: ['Loose chat'] }
    ])
    expect(document.body.textContent).not.toContain('Still open')

    const firstRow = document.querySelector('.archived-conversation-row')
    if (!(firstRow instanceof HTMLElement)) throw new Error('archived row missing')
    const subtitle = firstRow.querySelector('.archived-conversation-row__main span')
    const actions = firstRow.querySelector('.archived-conversation-row__actions')
    if (!(subtitle instanceof HTMLElement) || !(actions instanceof HTMLElement)) {
      throw new Error('archived row content missing')
    }
    expect(subtitle.textContent).toBe(
      new Intl.DateTimeFormat('en-US', { dateStyle: 'medium', timeStyle: 'short' }).format(300)
    )
    expect(subtitle.textContent).not.toContain('Project B')
    expect(getComputedStyle(firstRow).minHeight).toBe('52px')
    expect(getComputedStyle(actions).opacity).toBe('1')
    await expect.element(screen.getByRole('button', { name: 'Delete chat' }).first()).toBeVisible()
    await expect.element(screen.getByRole('button', { name: 'Unarchive' }).first()).toBeVisible()

    const filter = screen.getByRole('button', {
      name: 'Filter archived chats by project: All projects'
    })
    expect(getComputedStyle(filter.element()).height).toBe('34px')
    expect(
      getComputedStyle(screen.getByRole('button', { name: 'Delete all' }).element()).height
    ).toBe('32px')

    await screen.getByRole('button', { name: 'Unarchive' }).first().click()
    expect(onUnarchiveConversation).toHaveBeenCalledWith('beta-latest')
  })

  it('filters groups by project and scopes delete-all to the visible chats', async () => {
    const onDeleteArchivedConversations = vi.fn()
    const screen = await render(
      <ArchivedConversationsSettingsPage
        conversations={conversations}
        onDeleteArchivedConversations={onDeleteArchivedConversations}
        onDeleteConversation={vi.fn()}
        onUnarchiveConversation={vi.fn()}
        projects={projects}
      />
    )

    await screen
      .getByRole('button', { name: 'Filter archived chats by project: All projects' })
      .click()
    await screen.getByRole('option', { name: 'Project A' }).click()

    expect(groupSummaries()).toEqual([
      { title: 'Project A', count: '1 chats', titles: ['Alpha chat'] }
    ])

    await screen.getByRole('button', { name: 'Delete all' }).click()
    await expect.element(screen.getByRole('dialog')).toBeVisible()
    await expect.element(screen.getByText('Delete archived chats in Project A?')).toBeVisible()
    await screen.getByRole('button', { name: 'Delete', exact: true }).click()
    expect(onDeleteArchivedConversations).toHaveBeenCalledWith(['alpha'])
  })

  it('shows an empty state when nothing is archived', async () => {
    const screen = await render(
      <ArchivedConversationsSettingsPage
        conversations={[conversations[4]!]}
        onDeleteArchivedConversations={vi.fn()}
        onDeleteConversation={vi.fn()}
        onUnarchiveConversation={vi.fn()}
        projects={projects}
      />
    )

    await expect.element(screen.getByText('No archived chats')).toBeVisible()
    await expect.element(screen.getByRole('button', { name: 'Delete all' })).toBeDisabled()
  })
})
