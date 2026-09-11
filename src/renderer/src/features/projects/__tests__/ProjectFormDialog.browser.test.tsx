import { describe, expect, it, vi } from 'vitest'
import { userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import type { StorageProjectFolderPick } from '@mycopilot/protocol'
import { getTranslation, type TranslationKey } from '../../../config/frontendTranslations'
import { ProjectValidationError } from '../projectValidationError'
import { ProjectFormDialog, type ProjectFormValues } from '../ProjectFormDialog'
import '../../../styles/global.css'

const t = (key: TranslationKey) => getTranslation('en-US', key)

function pickQueue(picks: (StorageProjectFolderPick | null)[]) {
  const remaining = [...picks]
  return vi.fn(async () => remaining.shift() ?? null)
}

async function renderDialog(overrides: Partial<Parameters<typeof ProjectFormDialog>[0]> = {}) {
  const props = {
    initialFolders: [],
    initialName: '',
    mode: 'create' as const,
    onCancel: vi.fn(),
    onPickFolder: pickQueue([]),
    onSubmit: vi.fn<(values: ProjectFormValues) => Promise<void>>(async () => undefined),
    t,
    ...overrides
  }
  return { props, screen: await render(<ProjectFormDialog {...props} />) }
}

describe('ProjectFormDialog', () => {
  it('names the project after the first folder, marks it primary, and submits ordered folders', async () => {
    const onPickFolder = pickQueue([
      { path: '/workspace/app', name: 'app' },
      { path: '/workspace/docs', name: 'docs' }
    ])
    const { props, screen } = await renderDialog({ onPickFolder })

    const create = screen.getByRole('button', { name: 'Create', exact: true })
    await expect.element(create).toBeDisabled()

    await screen.getByRole('button', { name: 'Add folder' }).click()
    await expect.element(screen.getByRole('textbox', { name: 'Project name' })).toHaveValue('app')
    await expect.element(screen.getByText('Primary', { exact: true })).toBeVisible()

    await screen.getByRole('button', { name: 'Add folder' }).click()
    await expect.element(screen.getByRole('button', { name: 'Make primary docs' })).toBeVisible()
    await expect.element(create).toBeEnabled()

    await create.click()
    await expect.poll(() => vi.mocked(props.onSubmit).mock.calls.length).toBe(1)
    expect(props.onSubmit).toHaveBeenCalledWith({
      name: 'app',
      folders: [
        { path: '/workspace/app', role: 'primary' },
        { path: '/workspace/docs', role: 'auxiliary' }
      ]
    })
  })

  it('reassigns the primary role when a folder is promoted or the primary is removed', async () => {
    const { screen } = await renderDialog({
      mode: 'edit',
      initialName: 'Wire workspace',
      initialFolders: [
        { id: 'folder-1', path: '/workspace/app', role: 'primary' },
        { id: 'folder-2', path: '/workspace/docs', role: 'auxiliary' }
      ]
    })

    await screen.getByRole('button', { name: 'Make primary docs' }).click()
    await expect.element(screen.getByRole('button', { name: 'Make primary app' })).toBeVisible()

    await screen.getByRole('button', { name: 'Remove folder docs' }).click()
    await expect.element(screen.getByText('Primary', { exact: true })).toBeVisible()
    expect(document.querySelectorAll('.project-form-dialog__folder')).toHaveLength(1)
  })

  it('refuses duplicate folders locally and localizes Main validation codes', async () => {
    const onSubmit = vi.fn(async () => {
      throw new ProjectValidationError({
        kind: 'project_validation',
        code: 'folder_missing',
        path: '/workspace/app'
      })
    })
    const { screen } = await renderDialog({
      mode: 'edit',
      initialName: 'Wire workspace',
      initialFolders: [{ id: 'folder-1', path: '/workspace/app', role: 'primary' }],
      onPickFolder: pickQueue([{ path: '/workspace/app/', name: 'app' }]),
      onSubmit
    })

    await screen.getByRole('button', { name: 'Add folder' }).click()
    await expect
      .element(screen.getByRole('alert'))
      .toHaveTextContent('This folder is already part of the project.')
    expect(document.querySelectorAll('.project-form-dialog__folder')).toHaveLength(1)

    await screen.getByRole('button', { name: 'Save', exact: true }).click()
    await expect
      .element(screen.getByRole('alert'))
      .toHaveTextContent('This folder does not exist or is not a directory. (/workspace/app)')
    await expect.element(screen.getByRole('dialog')).toBeVisible()
  })

  it('offers project removal only while editing and cancels on Escape', async () => {
    const onRemoveProject = vi.fn()
    const onCancel = vi.fn()
    const { screen } = await renderDialog({
      mode: 'edit',
      initialName: 'Wire workspace',
      initialFolders: [{ id: 'folder-1', path: '/workspace/app', role: 'primary' }],
      onCancel,
      onRemoveProject
    })

    await screen.getByRole('button', { name: 'Remove local project' }).click()
    expect(onRemoveProject).toHaveBeenCalledOnce()

    await userEvent.keyboard('{Escape}')
    expect(onCancel).toHaveBeenCalledOnce()
  })

  it('hides project removal while creating', async () => {
    const { screen } = await renderDialog({ onRemoveProject: vi.fn() })
    await expect.element(screen.getByRole('heading', { name: 'New project' })).toBeVisible()
    expect(document.querySelector('.project-form-dialog__remove-project')).toBeNull()
  })
})
