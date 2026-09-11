import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { EnvironmentSettingsPage } from '../pages/EnvironmentSettingsPage'
import '../../../styles/global.css'

const projectSettings = vi.hoisted(() => ({
  openCreateProjectDialog: vi.fn(async () => null),
  openEditProjectDialog: vi.fn(async () => 'cancelled' as 'cancelled' | 'remove-requested')
}))

vi.mock('../../../config/FrontendConfigProvider', async () => {
  const { getTranslation } = await import('../../../config/frontendTranslations')
  return {
    useFrontendConfig: () => ({
      language: 'en-US',
      t: (key: Parameters<typeof getTranslation>[1]) => getTranslation('en-US', key)
    })
  }
})

vi.mock('../../../config/ProjectSettingsProvider', async () => {
  const { singleFolderProject } = await import('../../projects/__tests__/projectFixtures')
  return {
    useProjectSettings: () => ({
      projects: [
        singleFolderProject({ id: 'project-a', name: 'Project A', path: '/workspace/a' }),
        singleFolderProject({ id: 'project-b', name: 'Project B', path: '/workspace/b' })
      ],
      deleteProject: vi.fn(),
      openCreateProjectDialog: projectSettings.openCreateProjectDialog,
      openEditProjectDialog: projectSettings.openEditProjectDialog,
      showProjectInFolder: vi.fn(),
      togglePinProject: vi.fn()
    })
  }
})

describe('EnvironmentSettingsPage', () => {
  beforeEach(() => {
    projectSettings.openCreateProjectDialog.mockClear()
    projectSettings.openEditProjectDialog.mockReset()
    projectSettings.openEditProjectDialog.mockResolvedValue('cancelled')
  })

  it('lists projects by name only, with an edit button ahead of the delete button', async () => {
    const screen = await render(<EnvironmentSettingsPage onRemoveProject={async () => true} />)

    await expect.element(screen.getByText('Project A')).toBeVisible()
    expect(document.body.textContent).not.toContain('/workspace/a')

    const card = document.querySelector('.environment-project-card')!
    const actions = Array.from(card.querySelectorAll('button')).map((button) =>
      button.getAttribute('aria-label')
    )
    expect(actions).toEqual(['Edit project Project A', 'Delete project Project A'])
  })

  it('routes add and edit through the project dialogs', async () => {
    const screen = await render(<EnvironmentSettingsPage onRemoveProject={async () => true} />)

    await screen.getByRole('button', { name: 'Add project' }).click()
    expect(projectSettings.openCreateProjectDialog).toHaveBeenCalledOnce()

    await screen.getByRole('button', { name: 'Edit project Project B' }).click()
    expect(projectSettings.openEditProjectDialog).toHaveBeenCalledWith('project-b')
    expect(document.querySelector('[role="dialog"]')).toBeNull()
  })

  it('opens the removal confirmation when the editor requests removal', async () => {
    projectSettings.openEditProjectDialog.mockResolvedValue('remove-requested')
    const onRemoveProject = vi.fn(async () => true)
    const screen = await render(<EnvironmentSettingsPage onRemoveProject={onRemoveProject} />)

    await screen.getByRole('button', { name: 'Edit project Project A' }).click()
    const dialog = screen.getByRole('dialog')
    await expect.element(dialog).toBeVisible()
    await expect.element(screen.getByText('Remove Project A?')).toBeVisible()

    await screen.getByRole('button', { name: 'Remove', exact: true }).click()
    expect(onRemoveProject).toHaveBeenCalledWith('project-a')
    await expect.element(dialog).not.toBeInTheDocument()
  })
})
