import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { defaultUiPreferences } from '../../features/storage/storageClient'
import { LeftSidebar } from '../shell/sidebar/LeftSidebar'
import '../shell/sidebar/LeftSidebar.css'

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'en-US',
    t: (key: string) => (key === 'sidebar.scheduled' ? 'Scheduled' : key)
  })
}))

vi.mock('../../host/hostClient', () => ({ hostClient: {} }))

const noop = () => undefined

function renderSidebar({
  attentionCount = 0,
  onNewProject = vi.fn().mockResolvedValue(null),
  onOpenScheduled = vi.fn(),
  selected = false
}: {
  attentionCount?: number
  onNewProject?: () => Promise<null>
  onOpenScheduled?: () => void
  selected?: boolean
} = {}) {
  return render(
    <div style={{ height: 640, width: 280 }}>
      <LeftSidebar
        activeConversationId={selected ? null : 'conversation-1'}
        conversations={[
          {
            id: 'conversation-1',
            projectId: null,
            modelId: null,
            title: 'Existing conversation',
            messages: [],
            createdAt: 1,
            updatedAt: 1
          }
        ]}
        projects={[{ id: 'project-1', name: 'Existing project', createdAt: 1 }]}
        uiPreferences={defaultUiPreferences()}
        onArchiveAllProjectConversations={noop}
        onArchiveAllRootConversations={noop}
        onArchiveConversation={noop}
        onArchiveProjectConversations={noop}
        onMarkConversationUnread={noop}
        onNewConversation={noop}
        onNewProject={onNewProject}
        onOpenScheduled={onOpenScheduled}
        onOpenSettings={noop}
        onRemoveProject={async () => true}
        onRenameConversation={noop}
        onRenameProject={noop}
        onSelectConversation={noop}
        onShowProjectInFolder={noop}
        onTogglePinConversation={noop}
        onTogglePinProject={noop}
        onUiPreferencesChange={noop}
        scheduledAttentionCount={attentionCount}
        scheduledSelected={selected}
      />
    </div>
  )
}

describe('LeftSidebar scheduled navigation', () => {
  it('adds one selected navigation row without replacing projects or conversations', async () => {
    const onOpenScheduled = vi.fn()
    const screen = await renderSidebar({ attentionCount: 3, onOpenScheduled, selected: true })

    const scheduled = screen.getByRole('button', { name: 'Scheduled (3)' })
    await expect.element(scheduled).toHaveAttribute('aria-current', 'page')
    await expect.element(screen.getByText('3', { exact: true })).toBeVisible()
    await expect.element(screen.getByText('Existing project', { exact: true })).toBeVisible()
    await expect.element(screen.getByText('Existing conversation', { exact: true })).toBeVisible()
    expect(screen.container.querySelector('.left-sidebar__notification-action')).toBeNull()

    await scheduled.click()
    expect(onOpenScheduled).toHaveBeenCalledOnce()
  })

  it('hides a zero badge and caps large counts at 99+', async () => {
    const zeroScreen = await renderSidebar()
    expect(zeroScreen.container.querySelector('.left-sidebar__scheduled-badge')).toBeNull()
    await zeroScreen.unmount()

    const cappedScreen = await renderSidebar({ attentionCount: 120 })
    await expect.element(cappedScreen.getByText('99+', { exact: true })).toBeVisible()
  })

  it('opens the native project-folder flow from the rightmost project header action', async () => {
    const onNewProject = vi.fn().mockResolvedValue(null)
    const screen = await renderSidebar({ onNewProject })
    const projectActions = screen.container.querySelectorAll(
      '.left-sidebar__projects .left-sidebar__section-action'
    )

    expect(projectActions).toHaveLength(2)
    await screen.getByRole('button', { name: 'project.newProject', exact: true }).click()
    expect(onNewProject).toHaveBeenCalledOnce()
  })
})
