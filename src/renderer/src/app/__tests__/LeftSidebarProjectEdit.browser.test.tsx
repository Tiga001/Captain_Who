import type { CSSProperties } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ProjectEditDialogResult } from '../../config/ProjectSettingsProvider'
import { singleFolderProject } from '../../features/projects/__tests__/projectFixtures'
import { defaultUiPreferences } from '../../features/storage/storageClient'
import { LeftSidebar } from '../shell/sidebar/LeftSidebar'
import '../shell/sidebar/LeftSidebar.css'

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ language: 'en-US', t: (key: string) => key })
}))

vi.mock('../../host/hostClient', () => ({ hostClient: {} }))

const noop = () => undefined
const project = singleFolderProject({ id: 'project-1', name: 'Wire workspace' })

async function renderSidebar({
  onEditProject,
  onRemoveProject = vi.fn(async () => true)
}: {
  onEditProject: (projectId: string) => Promise<ProjectEditDialogResult>
  onRemoveProject?: (projectId: string) => Promise<boolean>
}) {
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
        activeConversationId={null}
        conversations={[]}
        projects={[project]}
        uiPreferences={defaultUiPreferences()}
        onArchiveAllProjectConversations={noop}
        onArchiveAllRootConversations={noop}
        onArchiveConversation={noop}
        onArchiveProjectConversations={noop}
        onEditProject={onEditProject}
        onMarkConversationUnread={noop}
        onNewConversation={noop}
        onNewProject={vi.fn(async () => null)}
        onOpenScheduled={noop}
        onOpenSettings={noop}
        onRemoveProject={onRemoveProject}
        onRenameConversation={noop}
        onSelectConversation={noop}
        onShowProjectInFolder={noop}
        onTogglePinConversation={noop}
        onTogglePinProject={noop}
        onUiPreferencesChange={noop}
        scheduledAttentionCount={0}
        scheduledSelected={false}
      />
    </div>
  )
}

describe('LeftSidebar project editing', () => {
  it('replaces the rename entry with an editor entry that hands the project id upward', async () => {
    const onEditProject = vi.fn(async () => 'saved' as const)
    const screen = await renderSidebar({ onEditProject })

    await screen.getByRole('button', { name: 'project.moreActions' }).nth(1).click()
    const menu = screen.getByRole('menu')
    await expect.element(menu).toBeVisible()
    expect(
      Array.from(document.querySelectorAll('[role="menuitem"]')).map((item) => item.textContent)
    ).toEqual([
      'project.pinProject',
      'project.showInFolder',
      'project.editProject',
      'project.archiveConversations',
      'project.removeProject'
    ])

    await screen.getByRole('menuitem', { name: 'project.editProject' }).click()
    expect(onEditProject).toHaveBeenCalledWith('project-1')
    await expect.element(menu).not.toBeInTheDocument()
    expect(document.querySelector('[role="dialog"]')).toBeNull()
  })

  it('opens the removal confirmation when the editor asks to remove the project', async () => {
    const onRemoveProject = vi.fn(async () => true)
    const screen = await renderSidebar({
      onEditProject: vi.fn(async () => 'remove-requested' as const),
      onRemoveProject
    })

    await screen.getByRole('button', { name: 'project.moreActions' }).nth(1).click()
    await screen.getByRole('menuitem', { name: 'project.editProject' }).click()

    const dialog = screen.getByRole('dialog')
    await expect.element(dialog).toBeVisible()
    await expect.element(screen.getByRole('heading', { name: 'project.removeTitle' })).toBeVisible()
    await screen.getByRole('button', { name: 'project.confirmRemove' }).click()
    expect(onRemoveProject).toHaveBeenCalledWith('project-1')
    await expect.element(dialog).not.toBeInTheDocument()
  })
})
