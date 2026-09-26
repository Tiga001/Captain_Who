import type { CSSProperties } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { defaultUiPreferences } from '../../features/storage/storageClient'
import { LeftSidebar } from '../shell/sidebar/LeftSidebar'
import { singleFolderProject } from '../../features/projects/__tests__/projectFixtures'
import '../shell/sidebar/LeftSidebar.css'

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'en-US',
    t: (key: string) =>
      key === 'sidebar.scheduled' ? 'Scheduled' : key === 'sidebar.workflows' ? 'Workflows' : key
  })
}))

vi.mock('../../host/hostClient', () => ({ hostClient: {} }))

const noop = () => undefined

function renderSidebar({
  attentionCount = 0,
  onNewProject = vi.fn().mockResolvedValue(null),
  onOpenScheduled = vi.fn(),
  selected = false,
  onOpenWorkflows,
  workflowsSelected = false
}: {
  attentionCount?: number
  onNewProject?: () => Promise<null>
  onOpenScheduled?: () => void
  selected?: boolean
  onOpenWorkflows?: () => void
  workflowsSelected?: boolean
} = {}) {
  return render(
    <div
      style={
        {
          '--mc-layout-sidebar-action-height': '29px',
          '--titlebar-height': '38px',
          height: 640,
          width: 280
        } as CSSProperties
      }
    >
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
        projects={[singleFolderProject({ id: 'project-1', name: 'Existing project' })]}
        uiPreferences={defaultUiPreferences()}
        onArchiveAllProjectConversations={noop}
        onArchiveAllRootConversations={noop}
        onArchiveConversation={noop}
        onArchiveProjectConversations={noop}
        onEditProject={async () => 'cancelled' as const}
        onMarkConversationUnread={noop}
        onNewConversation={noop}
        onNewProject={onNewProject}
        onOpenScheduled={onOpenScheduled}
        onOpenSettings={noop}
        onRemoveProject={async () => true}
        onRenameConversation={noop}
        onSelectConversation={noop}
        onShowProjectInFolder={noop}
        onTogglePinConversation={noop}
        onTogglePinProject={noop}
        onUiPreferencesChange={noop}
        scheduledAttentionCount={attentionCount}
        scheduledSelected={selected}
        onOpenWorkflows={onOpenWorkflows}
        workflowsSelected={workflowsSelected}
        workflowMemberships={{
          'conversation-1': { id: 'workflow-1', name: 'Release review', color: '#2478d4' }
        }}
      />
    </div>
  )
}

describe('LeftSidebar scheduled navigation', () => {
  it('keeps the brand in the reserved header space above the existing navigation', async () => {
    const screen = await renderSidebar()
    const logo = screen.getByRole('img', { name: 'Captain Who' }).element()
    const newConversation = screen
      .getByRole('button', { name: 'sidebar.newConversation', exact: true })
      .element()
    const sidebar = screen.getByRole('complementary').element()
    const brand = logo.closest<HTMLElement>('.left-sidebar__brand')
    const scheduled = screen.getByRole('button', { name: 'Scheduled', exact: true }).element()
    const scroll = sidebar.querySelector<HTMLElement>('.left-sidebar__scroll')!
    const projects = sidebar.querySelector<HTMLElement>('.left-sidebar__projects')!
    const sidebarBounds = sidebar.getBoundingClientRect()
    const logoBounds = logo.getBoundingClientRect()
    const newConversationBounds = newConversation.getBoundingClientRect()
    const scheduledBounds = scheduled.getBoundingClientRect()
    const scrollBounds = scroll.getBoundingClientRect()
    const projectsBounds = projects.getBoundingClientRect()

    expect(brand).not.toBeNull()
    expect(getComputedStyle(brand!).pointerEvents).toBe('none')
    expect(brand!.getBoundingClientRect().top - sidebarBounds.top).toBeCloseTo(6, 0)
    expect(newConversationBounds.top - sidebarBounds.top).toBeCloseTo(44, 0)
    expect(newConversationBounds.top - logoBounds.bottom).toBeLessThanOrEqual(14)
    expect(logoBounds.bottom).toBeLessThanOrEqual(newConversationBounds.top)
    expect(scrollBounds.top - sidebarBounds.top).toBeCloseTo(149, 0)
    expect(projectsBounds.top - scrollBounds.top).toBeCloseTo(24, 0)
    expect(scheduledBounds.bottom).toBeLessThanOrEqual(scrollBounds.top)
    expect(sidebar.scrollWidth).toBeLessThanOrEqual(sidebar.clientWidth)
  })

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

  it('opens global workflows below Scheduled and makes conversations draggable for binding', async () => {
    const onOpenWorkflows = vi.fn()
    const screen = await renderSidebar({ onOpenWorkflows, workflowsSelected: true })
    const workflows = screen.getByRole('button', { name: 'Workflows', exact: true })
    const scheduled = screen.getByRole('button', { name: 'Scheduled', exact: true })
    await expect.element(workflows).toHaveAttribute('aria-current', 'page')
    expect(workflows.element().getBoundingClientRect().top).toBeGreaterThanOrEqual(
      scheduled.element().getBoundingClientRect().bottom
    )
    await workflows.click()
    expect(onOpenWorkflows).toHaveBeenCalledOnce()
    const conversation = screen.getByRole('button', {
      name: 'Release review Existing conversation'
    })
    await expect.element(conversation).toHaveAttribute('draggable', 'true')
    const transfer = new DataTransfer()
    conversation
      .element()
      .dispatchEvent(new DragEvent('dragstart', { bubbles: true, dataTransfer: transfer }))
    expect(transfer.getData('application/x-captain-workflow-conversation')).toBe('conversation-1')
    const marker = screen.container.querySelector<HTMLElement>('.left-sidebar__workflow-marker')!
    expect(marker.style.backgroundColor).toBe('rgb(36, 120, 212)')
    expect(screen.container.querySelector('.left-sidebar__projects')).not.toBeNull()
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
