import type { AgentTemplate } from '@mycopilot/protocol'
import { page } from 'vitest/browser'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { SettingsSearchNavigationProvider } from '../../features/settings/settingsSearchNavigation'

const service = vi.hoisted(() => ({
  getSettings: vi.fn(),
  updateSettings: vi.fn(),
  settingsListener: null as
    null | ((settings: { enabled: boolean; revision: number; updatedAt: number }) => void),
  assign: vi.fn(),
  create: vi.fn(),
  delete: vi.fn(),
  list: vi.fn(),
  setEnabled: vi.fn(),
  translate: (key: string) => {
    if (key === 'agentTemplates.projectCount') return 'Assigned projects: {count}'
    if (key === 'agentTemplates.deleteDescription') {
      return 'Assigned to {count} projects. Existing Agents keep their snapshot.'
    }
    return key
  },
  update: vi.fn()
}))

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: service.translate })
}))

vi.mock('../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({
    enabledModels: [model('model-one', 'Model One', true), model('model-two', 'Model Two', true)],
    models: [
      model('model-one', 'Model One', true),
      model('model-two', 'Model Two', true),
      model('model-retired', 'Retired Model', false)
    ]
  })
}))

vi.mock('../../features/agentCollaboration/collaborationClient', () => ({
  getCollaborationSettings: service.getSettings,
  updateCollaborationSettings: service.updateSettings,
  onCollaborationSettingsChanged: (listener: typeof service.settingsListener) => {
    service.settingsListener = listener
    return () => {
      service.settingsListener = null
    }
  },
  createAgentTemplate: service.create,
  deleteAgentTemplate: service.delete,
  listAgentTemplates: service.list,
  setAgentTemplateEnabled: service.setEnabled,
  setAgentTemplateProjectAssignment: service.assign,
  updateAgentTemplate: service.update
}))

const { AgentTemplatesSettingsPage, createTemplateMachineKey } =
  await import('../../features/settings/pages/AgentTemplatesSettingsPage')

const PROJECTS = [
  { createdAt: 1, id: 'project-a', name: 'Project A', path: '/workspace/a' },
  { createdAt: 2, id: 'project-b', name: 'Project B', path: '/workspace/b' }
]

let inventory: AgentTemplate[]

beforeEach(() => {
  service.getSettings.mockReset().mockResolvedValue({ enabled: true, revision: 1, updatedAt: 0 })
  service.updateSettings
    .mockReset()
    .mockResolvedValue({ enabled: false, revision: 2, updatedAt: 1 })
  inventory = [
    template('template-a', 'Global Alpha', 'model-one', ['project-a']),
    template('template-b', 'Global Beta', 'model-two', ['project-b'])
  ]
  service.assign.mockReset()
  service.create.mockReset()
  service.delete.mockReset()
  service.list.mockReset()
  service.setEnabled.mockReset()
  service.update.mockReset()
  service.list.mockImplementation(async () => ({
    schemaVersion: 1,
    templates: inventory.map(cloneTemplate)
  }))
  service.create.mockImplementation(async (input) => {
    const created = {
      ...template(input.templateId, input.name, input.modelConfigId, []),
      description: input.description,
      enabled: input.enabled,
      instructions: input.instructions,
      machineKey: input.machineKey
    }
    inventory = [...inventory, created]
    return cloneTemplate(created)
  })
  service.update.mockImplementation(async (input) => {
    const current = inventory.find((candidate) => candidate.templateId === input.templateId)!
    const updated = {
      ...current,
      description: input.description,
      instructions: input.instructions,
      modelConfigId: input.modelConfigId,
      name: input.name,
      revision: input.expectedRevision + 1
    }
    inventory = inventory.map((candidate) =>
      candidate.templateId === updated.templateId ? updated : candidate
    )
    return cloneTemplate(updated)
  })
  service.assign.mockImplementation(async (input) => {
    const current = inventory.find((candidate) => candidate.templateId === input.templateId)!
    const projectIds = input.assigned
      ? [...new Set([...current.projectIds, input.projectId])]
      : current.projectIds.filter((projectId) => projectId !== input.projectId)
    const updated = { ...current, projectIds, revision: current.revision + 1 }
    inventory = inventory.map((candidate) =>
      candidate.templateId === updated.templateId ? updated : candidate
    )
    return cloneTemplate(updated)
  })
  service.setEnabled.mockImplementation(async (input) => {
    const current = inventory.find((candidate) => candidate.templateId === input.templateId)!
    const updated = {
      ...current,
      enabled: input.enabled,
      revision: input.expectedRevision + 1
    }
    inventory = inventory.map((candidate) =>
      candidate.templateId === updated.templateId ? updated : candidate
    )
    return cloneTemplate(updated)
  })
  service.delete.mockImplementation(async (input) => {
    const current = inventory.find((candidate) => candidate.templateId === input.templateId)!
    inventory = inventory.filter((candidate) => candidate.templateId !== input.templateId)
    return { ...cloneTemplate(current), enabled: false, revision: input.expectedRevision + 1 }
  })
})

describe('Agent template settings', () => {
  it('retains a selected template draft while search visits the library and another field', async () => {
    const screen = await render(
      <SettingsSearchNavigationProvider
        target={{
          page: 'agentTemplates',
          id: 'agent-template-instructions',
          view: 'editor',
          prerequisiteId: 'agent-templates-list',
          revision: 1
        }}
      >
        <AgentTemplatesSettingsPage projects={PROJECTS} />
      </SettingsSearchNavigationProvider>
    )
    await expect.element(screen.getByText('Global Alpha', { exact: true })).toBeVisible()
    expect(screen.container.querySelector('.agent-template-form')).toBeNull()
    await screen.getByRole('button', { name: 'agentTemplates.edit Global Alpha' }).click()
    await screen
      .getByRole('textbox', { name: 'agentTemplates.instructions' })
      .fill('Keep this unsaved draft.')

    await screen.rerender(
      <SettingsSearchNavigationProvider
        target={{
          page: 'agentTemplates',
          id: 'agent-templates-create',
          view: 'templates',
          revision: 2
        }}
      >
        <AgentTemplatesSettingsPage projects={PROJECTS} />
      </SettingsSearchNavigationProvider>
    )
    await expect
      .element(screen.getByRole('button', { name: 'agentTemplates.create', exact: true }))
      .toBeVisible()
    expect(screen.container.querySelector('.agent-template-form')).toBeNull()

    await screen.rerender(
      <SettingsSearchNavigationProvider
        target={{
          page: 'agentTemplates',
          id: 'agent-template-name',
          view: 'editor',
          prerequisiteId: 'agent-templates-list',
          revision: 3
        }}
      >
        <AgentTemplatesSettingsPage projects={PROJECTS} />
      </SettingsSearchNavigationProvider>
    )
    await expect
      .element(screen.getByRole('textbox', { name: 'agentTemplates.instructions' }))
      .toHaveValue('Keep this unsaved draft.')
    await expect
      .element(screen.getByRole('textbox', { name: 'agentTemplates.name' }))
      .toHaveValue('Global Alpha')
    expect(screen.container.querySelector('[data-setting-id="agent-template-name"]')?.tagName).toBe(
      'LABEL'
    )
    expect(service.create).not.toHaveBeenCalled()
    expect(service.update).not.toHaveBeenCalled()
    expect(service.setEnabled).not.toHaveBeenCalled()
    expect(service.delete).not.toHaveBeenCalled()
  })

  it('loads one global library instead of a project-scoped list', async () => {
    const screen = await render(
      <AgentTemplatesSettingsPage initialProjectId="project-a" projects={PROJECTS} />
    )

    await expect.element(screen.getByText('Global Alpha', { exact: true })).toBeVisible()
    await expect.element(screen.getByText('Global Beta', { exact: true })).toBeVisible()
    expect(service.list).toHaveBeenCalledWith({ includeDisabled: true })
    expect(service.list.mock.calls[0]?.[0]).not.toHaveProperty('projectId')
    expect(screen.getByText('Assigned projects: 1', { exact: true }).elements()).toHaveLength(2)
  })

  it('creates a global template with zero projects even when no projects exist', async () => {
    inventory = []
    expect(createTemplateMachineKey('Code Reviewer', 'abc12345', [])).toBe('code_reviewer')
    expect(createTemplateMachineKey('中文', 'ABC1-2345', ['agent'])).toBe('agent-abc12345')
    const navigateSettingsRoot = vi.fn()
    const screen = await render(
      <AgentTemplatesSettingsPage
        initialProjectId={null}
        onNavigateSettingsRoot={navigateSettingsRoot}
        projects={[]}
      />
    )
    await expect.element(screen.getByText('agentTemplates.empty', { exact: true })).toBeVisible()

    await screen.getByRole('button', { name: /agentTemplates.create/ }).click()
    await expect
      .element(screen.getByText('agentTemplates.noProjectsAvailable', { exact: true }))
      .toBeVisible()
    await expect
      .element(screen.getByRole('textbox', { name: 'agentTemplates.description' }))
      .toHaveAttribute('placeholder', 'agentTemplates.descriptionPlaceholder')
    await expect
      .element(screen.getByRole('textbox', { name: 'agentTemplates.instructions' }))
      .toHaveAttribute('placeholder', 'agentTemplates.instructionsPlaceholder')
    const breadcrumbs = screen.getByRole('navigation', { name: 'settings.breadcrumb.label' })
    await breadcrumbs.getByRole('button', { name: 'settings.breadcrumb.root' }).click()
    expect(navigateSettingsRoot).toHaveBeenCalledTimes(1)
    await screen.getByRole('textbox', { name: 'agentTemplates.name' }).fill('Code Reviewer')
    await screen
      .getByRole('textbox', { name: 'agentTemplates.instructions' })
      .fill('Review the requested change.')
    await screen.getByRole('button', { name: 'agentTemplates.selectModel' }).click()
    const modelTwoOption = screen.getByRole('option', {
      name: 'Model Two'
    })
    await expect.element(modelTwoOption).toBeVisible()
    await modelTwoOption.click()
    await screen.getByRole('button', { name: 'agentTemplates.save' }).click()

    await expect.poll(() => service.create).toHaveBeenCalledTimes(1)
    expect(service.create).toHaveBeenCalledWith(
      expect.objectContaining({
        machineKey: 'code_reviewer',
        modelConfigId: 'model-two',
        name: 'Code Reviewer'
      })
    )
    expect(service.create.mock.calls[0]?.[0]).not.toHaveProperty('projectId')
    expect(service.assign).not.toHaveBeenCalled()
    await expect.element(screen.getByText('Assigned projects: 0', { exact: true })).toBeVisible()
  })

  it('defaults only a new template to the initial project and supports multiple assignments', async () => {
    inventory = []
    const screen = await render(
      <AgentTemplatesSettingsPage initialProjectId="project-a" projects={PROJECTS} />
    )
    await screen.getByRole('button', { name: /agentTemplates.create/ }).click()

    const projectA = screen.getByRole('checkbox', { name: 'Project A' })
    const projectB = screen.getByRole('checkbox', { name: 'Project B' })
    await expect.element(projectA).toBeChecked()
    await expect.element(projectB).not.toBeChecked()
    await projectB.click()
    await screen.getByRole('textbox', { name: 'agentTemplates.name' }).fill('Researcher')
    await screen
      .getByRole('textbox', { name: 'agentTemplates.instructions' })
      .fill('Research the assigned topic.')
    await screen.getByRole('button', { name: 'agentTemplates.save' }).click()

    await expect.poll(() => service.assign).toHaveBeenCalledTimes(2)
    expect(service.assign.mock.calls.map(([input]) => input)).toEqual([
      expect.objectContaining({ assigned: true, projectId: 'project-a' }),
      expect.objectContaining({ assigned: true, projectId: 'project-b' })
    ])
    await expect.element(screen.getByText('Assigned projects: 2', { exact: true })).toBeVisible()
  })

  it('applies only the assignment difference after editing a global template', async () => {
    inventory = [template('template-edit', 'Researcher', 'model-two', ['project-a'])]
    const screen = await render(
      <AgentTemplatesSettingsPage initialProjectId="project-b" projects={PROJECTS} />
    )
    await expect.element(screen.getByText('Researcher', { exact: true })).toBeVisible()
    await screen.getByRole('button', { name: 'agentTemplates.edit Researcher' }).click()

    await expect.element(screen.getByRole('checkbox', { name: 'Project A' })).toBeChecked()
    await expect.element(screen.getByRole('checkbox', { name: 'Project B' })).not.toBeChecked()
    await screen.getByRole('checkbox', { name: 'Project A' }).click()
    await screen.getByRole('checkbox', { name: 'Project B' }).click()
    await screen.getByRole('button', { name: 'agentTemplates.save' }).click()

    await expect.poll(() => service.update).toHaveBeenCalledTimes(1)
    expect(service.update.mock.calls[0]?.[0]).not.toHaveProperty('projectId')
    await expect.poll(() => service.assign).toHaveBeenCalledTimes(2)
    expect(service.assign.mock.calls.map(([input]) => input)).toEqual([
      { assigned: false, projectId: 'project-a', templateId: 'template-edit' },
      { assigned: true, projectId: 'project-b', templateId: 'template-edit' }
    ])
    await expect.element(screen.getByText('Assigned projects: 1', { exact: true })).toBeVisible()
  })

  it('locks a create submission until its durable definition settles', async () => {
    inventory = []
    const pendingCreate = deferred<AgentTemplate>()
    service.create.mockReturnValueOnce(pendingCreate.promise)
    const screen = await render(<AgentTemplatesSettingsPage projects={[]} />)

    await screen.getByRole('button', { name: /agentTemplates.create/ }).click()
    await screen.getByRole('textbox', { name: 'agentTemplates.name' }).fill('Code Reviewer')
    await screen
      .getByRole('textbox', { name: 'agentTemplates.instructions' })
      .fill('Review the requested change.')
    const save = screen.getByRole('button', { name: 'agentTemplates.save' })
    await save.click()

    await expect.poll(() => service.create).toHaveBeenCalledTimes(1)
    await expect.element(save).toBeDisabled()
    pendingCreate.resolve(template('template-created', 'Code Reviewer', 'model-one', []))
    await expect.element(screen.getByText('Code Reviewer', { exact: true })).toBeVisible()
    expect(service.create).toHaveBeenCalledTimes(1)
  })

  it('requires an explicit replacement for an unavailable model before update', async () => {
    inventory = [template('template-retired', 'Retired template', 'model-retired', [])]
    const screen = await render(
      <AgentTemplatesSettingsPage initialProjectId="project-a" projects={PROJECTS} />
    )
    await screen.getByRole('button', { name: 'agentTemplates.edit Retired template' }).click()

    await expect
      .element(screen.getByRole('alert'))
      .toHaveTextContent('agentTemplates.reselectModel')
    await expect.element(screen.getByRole('button', { name: 'agentTemplates.save' })).toBeDisabled()
    await page.screenshot({
      element: screen.container.querySelector<HTMLElement>('.agent-templates-page')!,
      path: '__screenshots__/AgentTemplatesSettingsPage.browser.test.tsx/template-form-unavailable-model.png'
    })
    await screen.getByRole('button', { name: 'agentTemplates.selectModel' }).click()
    await expect
      .element(
        screen.getByRole('option', {
          name: /Retired Model/
        })
      )
      .toBeDisabled()
    await screen.getByRole('option', { name: 'Model Two' }).click()
    await screen.getByRole('button', { name: 'agentTemplates.save' }).click()

    await expect.poll(() => service.update).toHaveBeenCalledTimes(1)
    expect(service.update).toHaveBeenCalledWith(
      expect.objectContaining({
        expectedRevision: 1,
        modelConfigId: 'model-two',
        templateId: 'template-retired'
      })
    )
  })

  it('never exposes a missing model configuration ID when no visible snapshot label exists', async () => {
    const internalId = '0197f53a-24e8-7a61-b630-secret-template-model'
    inventory = [
      {
        ...template('template-missing', 'Missing model template', internalId, []),
        modelDisplayName: '   '
      }
    ]
    const screen = await render(
      <AgentTemplatesSettingsPage initialProjectId="project-a" projects={PROJECTS} />
    )

    await expect
      .element(screen.getByText('agentTemplates.modelUnavailable', { exact: true }))
      .toBeVisible()
    expect(screen.container.textContent).not.toContain(internalId)

    await screen.getByRole('button', { name: 'agentTemplates.edit Missing model template' }).click()
    await screen.getByRole('button', { name: 'agentTemplates.selectModel' }).click()
    await expect
      .element(screen.getByRole('option', { name: 'agentTemplates.modelUnavailable' }))
      .toBeDisabled()
    expect(screen.container.textContent).not.toContain(internalId)
  })

  it('reloads canonical state and shows an error when project assignment fails', async () => {
    inventory = [template('template-failure', 'Failure template', 'model-two', [])]
    service.assign.mockRejectedValueOnce(new Error('assignment failed'))
    const screen = await render(
      <AgentTemplatesSettingsPage initialProjectId="project-a" projects={PROJECTS} />
    )
    await screen.getByRole('button', { name: 'agentTemplates.edit Failure template' }).click()
    await screen.getByRole('checkbox', { name: 'Project A' }).click()
    await screen.getByRole('button', { name: 'agentTemplates.save' }).click()

    await expect.poll(() => service.assign).toHaveBeenCalledTimes(1)
    await expect.poll(() => service.list).toHaveBeenCalledTimes(2)
    await expect
      .element(screen.getByRole('alert'))
      .toHaveTextContent('agentTemplates.operationFailed')
    await expect.element(screen.getByRole('button', { name: 'agentTemplates.save' })).toBeEnabled()
  })

  it('retries a failed global library load', async () => {
    service.list.mockRejectedValueOnce(new Error('catalog unavailable'))
    const screen = await render(<AgentTemplatesSettingsPage projects={PROJECTS} />)
    await expect.element(screen.getByRole('alert')).toHaveTextContent('agentTemplates.loadFailed')

    await screen.getByRole('button', { name: 'agentTemplates.retry' }).click()
    await expect.element(screen.getByText('Global Alpha', { exact: true })).toBeVisible()
    await expect.element(screen.getByText('Global Beta', { exact: true })).toBeVisible()
  })

  it('deletes a global template and reports the number of affected projects', async () => {
    inventory = [template('template-delete', 'Delete me', 'model-two', ['project-a', 'project-b'])]
    const screen = await render(
      <AgentTemplatesSettingsPage initialProjectId="project-a" projects={PROJECTS} />
    )
    await screen.getByRole('button', { name: 'agentTemplates.delete Delete me' }).click()
    await expect.element(screen.getByText(/Assigned to 2 projects/)).toBeVisible()
    await screen.getByRole('button', { name: 'agentTemplates.delete', exact: true }).click()

    await expect.poll(() => service.delete).toHaveBeenCalledTimes(1)
    expect(service.delete).toHaveBeenCalledWith({
      expectedRevision: 1,
      templateId: 'template-delete'
    })
    await expect.poll(() => screen.container.textContent).not.toContain('Delete me')
  })
})

function template(
  templateId: string,
  name: string,
  modelConfigId: string,
  projectIds: string[]
): AgentTemplate {
  return {
    createdAt: 1,
    description: `${name} description`,
    enabled: true,
    instructions: `${name} instructions`,
    machineKey: templateId.replace(/-/g, '_'),
    modelConfigId,
    modelDisplayName: modelConfigId === 'model-retired' ? 'Retired Model' : 'Model Two',
    name,
    projectIds,
    revision: 1,
    schemaVersion: 1,
    templateId,
    updatedAt: 2
  }
}

function cloneTemplate(template: AgentTemplate): AgentTemplate {
  return { ...template, projectIds: [...template.projectIds] }
}

function model(id: string, displayName: string, enabled: boolean) {
  return {
    displayName,
    enabled,
    id,
    providerModelId: `provider-${id}`,
    supportsImage: false
  }
}

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise
  })
  return { promise, resolve }
}

describe('global collaboration permission', () => {
  it('persists the global switch independently of template enablement and accepts newer revisions', async () => {
    const screen = await render(<AgentTemplatesSettingsPage projects={PROJECTS} />)
    const toggle = screen.getByRole('switch', {
      name: 'agentTemplates.collaborationEnabled',
      exact: true
    })
    await expect.element(toggle).toHaveAttribute('aria-checked', 'true')
    await toggle.click()
    await expect.element(toggle).toHaveAttribute('aria-checked', 'false')
    expect(service.updateSettings).toHaveBeenCalledWith({ enabled: false, expectedRevision: 1 })
    expect(service.setEnabled).not.toHaveBeenCalled()
    service.settingsListener?.({ enabled: true, revision: 3, updatedAt: 2 })
    await expect.element(toggle).toHaveAttribute('aria-checked', 'true')
    service.settingsListener?.({ enabled: false, revision: 2, updatedAt: 1 })
    await expect.element(toggle).toHaveAttribute('aria-checked', 'true')
  })

  it('keeps the persisted choice after a failed save and lets the user refresh', async () => {
    service.updateSettings.mockRejectedValueOnce(new Error('revision conflict'))
    const screen = await render(<AgentTemplatesSettingsPage projects={PROJECTS} />)
    const toggle = screen.getByRole('switch', {
      name: 'agentTemplates.collaborationEnabled',
      exact: true
    })
    await expect.element(toggle).toHaveAttribute('aria-checked', 'true')
    await toggle.click()
    await expect
      .element(screen.getByRole('alert'))
      .toHaveTextContent('humanInteraction.settings.saveFailed')
    await expect.element(toggle).toHaveAttribute('aria-checked', 'true')
    service.getSettings.mockResolvedValueOnce({ enabled: false, revision: 2, updatedAt: 1 })
    await screen.getByRole('button', { name: 'agentTemplates.retry', exact: true }).click()
    await expect.element(toggle).toHaveAttribute('aria-checked', 'false')
  })
})
