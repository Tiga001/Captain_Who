import type { AgentTemplate } from '@mycopilot/protocol'
import { page } from 'vitest/browser'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'

const service = vi.hoisted(() => ({
  create: vi.fn(),
  delete: vi.fn(),
  list: vi.fn(),
  setEnabled: vi.fn(),
  translate: (key: string) => key,
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
  createAgentTemplate: service.create,
  deleteAgentTemplate: service.delete,
  listAgentTemplates: service.list,
  setAgentTemplateEnabled: service.setEnabled,
  updateAgentTemplate: service.update
}))

const { AgentTemplatesSettingsPage, createTemplateMachineKey } =
  await import('../../features/settings/pages/AgentTemplatesSettingsPage')

const PROJECTS = [
  { createdAt: 1, id: 'project-a', name: 'Project A', path: '/workspace/a' },
  { createdAt: 2, id: 'project-b', name: 'Project B', path: '/workspace/b' }
]

beforeEach(() => {
  service.create.mockReset()
  service.delete.mockReset()
  service.list.mockReset()
  service.setEnabled.mockReset()
  service.update.mockReset()
  service.list.mockImplementation(async ({ projectId }: { projectId: string }) => ({
    schemaVersion: 1,
    templates:
      projectId === 'project-a'
        ? [template('template-a', 'Project A template', 'model-retired')]
        : [template('template-b', 'Project B template', 'model-two')]
  }))
  service.create.mockImplementation(async (input) => ({
    ...template(input.templateId, input.name, input.modelConfigId),
    description: input.description,
    enabled: input.enabled,
    instructions: input.instructions,
    machineKey: input.machineKey,
    projectId: input.projectId
  }))
  service.update.mockImplementation(async (input) => ({
    ...template(input.templateId, input.name, input.modelConfigId),
    description: input.description,
    instructions: input.instructions,
    projectId: input.projectId,
    revision: input.expectedRevision + 1
  }))
  service.setEnabled.mockImplementation(async (input) => ({
    ...template(input.templateId, input.templateId, 'model-two'),
    enabled: input.enabled,
    projectId: input.projectId,
    revision: input.expectedRevision + 1
  }))
  service.delete.mockImplementation(async (input) => ({
    ...template(input.templateId, input.templateId, 'model-two'),
    enabled: false,
    projectId: input.projectId,
    revision: input.expectedRevision + 1
  }))
})

describe('Agent template settings', () => {
  it('keeps machine keys stable and creates through the shared model-config picker', async () => {
    expect(createTemplateMachineKey('Code Reviewer', 'abc12345', [])).toBe('code_reviewer')
    expect(createTemplateMachineKey('中文', 'ABC1-2345', ['agent'])).toBe('agent-abc12345')

    service.list.mockResolvedValue({ schemaVersion: 1, templates: [] })
    const navigateSettingsRoot = vi.fn()
    const screen = await render(
      <AgentTemplatesSettingsPage
        initialProjectId="project-a"
        onNavigateSettingsRoot={navigateSettingsRoot}
        projects={PROJECTS}
      />
    )
    await expect.element(screen.getByText('agentTemplates.empty', { exact: true })).toBeVisible()

    await screen.getByRole('button', { name: /agentTemplates.create/ }).click()
    const breadcrumbs = screen.getByRole('navigation', { name: 'settings.breadcrumb.label' })
    await expect.element(breadcrumbs).toBeVisible()
    await breadcrumbs.getByRole('button', { name: 'settings.breadcrumb.root' }).click()
    expect(navigateSettingsRoot).toHaveBeenCalledTimes(1)
    await screen.getByRole('textbox', { name: 'agentTemplates.name' }).fill('Code Reviewer')
    await screen
      .getByRole('textbox', { name: 'agentTemplates.instructions' })
      .fill('Review the requested change.')
    await screen.getByRole('button', { name: 'agentTemplates.selectModel' }).click()
    await screen.getByRole('option', { name: 'Model Two' }).click()
    await screen.getByRole('button', { name: 'agentTemplates.save' }).click()

    await expect.poll(() => service.create).toHaveBeenCalledTimes(1)
    expect(service.create).toHaveBeenCalledWith(
      expect.objectContaining({
        machineKey: 'code_reviewer',
        modelConfigId: 'model-two',
        name: 'Code Reviewer',
        projectId: 'project-a'
      })
    )
  })

  it('locks a create submission until its durable mutation settles', async () => {
    service.list.mockResolvedValue({ schemaVersion: 1, templates: [] })
    const pendingCreate = deferred<AgentTemplate>()
    service.create.mockReturnValueOnce(pendingCreate.promise)
    const screen = await render(
      <AgentTemplatesSettingsPage initialProjectId="project-a" projects={PROJECTS} />
    )

    await screen.getByRole('button', { name: /agentTemplates.create/ }).click()
    await screen.getByRole('textbox', { name: 'agentTemplates.name' }).fill('Code Reviewer')
    await screen
      .getByRole('textbox', { name: 'agentTemplates.instructions' })
      .fill('Review the requested change.')
    const save = screen.getByRole('button', { name: 'agentTemplates.save' })
    await save.click()

    await expect.poll(() => service.create).toHaveBeenCalledTimes(1)
    await expect.element(save).toBeDisabled()
    pendingCreate.resolve(template('template-created', 'Code Reviewer', 'model-one'))
    await expect.element(screen.getByText('Code Reviewer', { exact: true })).toBeVisible()
    expect(service.create).toHaveBeenCalledTimes(1)
  })

  it('requires an explicit replacement for an unavailable model before update', async () => {
    const screen = await render(
      <AgentTemplatesSettingsPage initialProjectId="project-a" projects={PROJECTS} />
    )
    await expect.element(screen.getByText('Project A template', { exact: true })).toBeVisible()
    await screen.getByRole('button', { name: 'agentTemplates.edit Project A template' }).click()

    await expect
      .element(screen.getByRole('alert'))
      .toHaveTextContent('agentTemplates.reselectModel')
    await expect.element(screen.getByRole('button', { name: 'agentTemplates.save' })).toBeDisabled()
    await page.screenshot({
      element: screen.container.querySelector<HTMLElement>('.agent-templates-page')!,
      path: '__screenshots__/AgentTemplatesSettingsPage.browser.test.tsx/template-form-unavailable-model.png'
    })
    await screen.getByRole('button', { name: 'agentTemplates.selectModel' }).click()
    await expect.element(screen.getByRole('option', { name: /Retired Model/ })).toBeDisabled()
    await screen.getByRole('option', { name: 'Model Two' }).click()
    await expect.element(screen.getByRole('button', { name: 'agentTemplates.save' })).toBeEnabled()
    await screen.getByRole('button', { name: 'agentTemplates.save' }).click()

    await expect.poll(() => service.update).toHaveBeenCalledTimes(1)
    expect(service.update).toHaveBeenCalledWith(
      expect.objectContaining({
        expectedRevision: 1,
        modelConfigId: 'model-two',
        projectId: 'project-a',
        templateId: 'template-a'
      })
    )
  })

  it('does not let a deferred mutation from project A overwrite project B', async () => {
    const pending = deferred<AgentTemplate>()
    service.setEnabled.mockReturnValueOnce(pending.promise)
    const screen = await render(
      <AgentTemplatesSettingsPage initialProjectId="project-a" projects={PROJECTS} />
    )
    await expect.element(screen.getByText('Project A template', { exact: true })).toBeVisible()

    // The retired model can still be disabled; it cannot be re-enabled without replacement.
    await screen.getByRole('button', { name: 'agentTemplates.disable' }).click()
    await screen.getByRole('button', { name: /agentTemplates.project: Project A/ }).click()
    await screen.getByRole('option', { name: 'Project B' }).click()
    await expect.element(screen.getByText('Project B template', { exact: true })).toBeVisible()
    expect(screen.container.textContent).not.toContain('Project A template')

    pending.resolve({
      ...template('template-a', 'Late Project A result', 'model-retired'),
      enabled: false
    })
    await expect.poll(() => service.setEnabled).toHaveBeenCalledTimes(1)
    await new Promise((resolve) => window.setTimeout(resolve, 0))
    await expect.element(screen.getByText('Project B template', { exact: true })).toBeVisible()
    expect(screen.container.textContent).not.toContain('Late Project A result')
  })

  it('ignores a deferred project A retry after switching to project B', async () => {
    const pendingRetry = deferred<{ schemaVersion: 1; templates: AgentTemplate[] }>()
    service.list
      .mockRejectedValueOnce(new Error('project-a unavailable'))
      .mockReturnValueOnce(pendingRetry.promise)
      .mockResolvedValueOnce({
        schemaVersion: 1,
        templates: [template('template-b', 'Project B template', 'model-two')]
      })
    const screen = await render(
      <AgentTemplatesSettingsPage initialProjectId="project-a" projects={PROJECTS} />
    )
    await expect.element(screen.getByRole('alert')).toHaveTextContent('agentTemplates.loadFailed')
    expect(screen.container.textContent).not.toContain('project-a unavailable')

    await screen.getByRole('button', { name: 'agentTemplates.retry' }).click()
    await screen.getByRole('button', { name: /agentTemplates.project: Project A/ }).click()
    await screen.getByRole('option', { name: 'Project B' }).click()
    await expect.element(screen.getByText('Project B template', { exact: true })).toBeVisible()

    pendingRetry.resolve({
      schemaVersion: 1,
      templates: [template('template-a', 'Late retry from Project A', 'model-retired')]
    })
    await new Promise((resolve) => window.setTimeout(resolve, 0))
    await expect.element(screen.getByText('Project B template', { exact: true })).toBeVisible()
    expect(screen.container.textContent).not.toContain('Late retry from Project A')
  })

  it('deletes by stable template id and captured project revision', async () => {
    service.list.mockResolvedValue({
      schemaVersion: 1,
      templates: [template('template-delete', 'Delete me', 'model-two')]
    })
    const screen = await render(
      <AgentTemplatesSettingsPage initialProjectId="project-a" projects={PROJECTS} />
    )
    await expect.element(screen.getByText('Delete me', { exact: true })).toBeVisible()
    await screen.getByRole('button', { name: 'agentTemplates.delete Delete me' }).click()
    await screen.getByRole('button', { name: 'agentTemplates.delete', exact: true }).click()

    await expect.poll(() => service.delete).toHaveBeenCalledTimes(1)
    expect(service.delete).toHaveBeenCalledWith({
      expectedRevision: 1,
      projectId: 'project-a',
      templateId: 'template-delete'
    })
    await expect.poll(() => screen.container.textContent).not.toContain('Delete me')
  })
})

function template(templateId: string, name: string, modelConfigId: string): AgentTemplate {
  return {
    createdAt: 1,
    description: `${name} description`,
    enabled: true,
    instructions: `${name} instructions`,
    machineKey: templateId.replace(/-/g, '_'),
    modelConfigId,
    modelDisplayName: modelConfigId === 'model-retired' ? 'Retired Model' : 'Model Two',
    name,
    projectId: 'project-a',
    revision: 1,
    schemaVersion: 1,
    templateId,
    updatedAt: 2
  }
}

function model(id: string, displayName: string, enabled: boolean) {
  return { displayName, enabled, id, supportsImage: false }
}

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise
  })
  return { promise, resolve }
}
