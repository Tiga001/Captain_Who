import { describe, expect, it } from 'vitest'
import { testState } from './state'
import { renderSelectedConversation } from './setup'

describe('title project editor removal', () => {
  it('routes remove-requested through confirmation before removing the project', async () => {
    testState.openEditProjectDialog.mockResolvedValue('remove-requested')
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'project.openDetails' }).click()
    await screen.getByRole('button', { name: 'project.editProject' }).click()
    expect(testState.openEditProjectDialog).toHaveBeenCalledWith('project-a')
    await expect.element(screen.getByRole('heading', { name: 'project.removeTitle' })).toBeVisible()
    expect(testState.deleteProject).not.toHaveBeenCalled()
    await screen.getByRole('button', { name: 'project.cancel', exact: true }).last().click()
    expect(testState.deleteProject).not.toHaveBeenCalled()

    await screen.getByRole('button', { name: 'project.openDetails' }).click()
    await screen.getByRole('button', { name: 'project.editProject' }).click()
    await screen.getByRole('button', { name: 'project.confirmRemove' }).click()
    await expect.poll(() => testState.deleteProject.mock.calls).toEqual([['project-a']])
    await expect
      .element(screen.getByRole('heading', { name: 'project.removeTitle' }))
      .not.toBeInTheDocument()
  })
})
