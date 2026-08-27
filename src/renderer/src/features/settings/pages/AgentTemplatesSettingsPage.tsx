import type { AgentTemplate } from '@mycopilot/protocol'
import { Bot, Pencil, Plus, RefreshCw, Trash2 } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { ConfirmationDialog } from '../../../components/dialog/ConfirmationDialog'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { getUserFacingErrorMessage } from '../../../errors/userFacingError'
import { useModelSettings } from '../../../config/ModelSettingsProvider'
import type { AppProject } from '../../../config/projectConfig'
import { ModelConfigPicker } from '../../modelSelection/ModelConfigPicker'
import {
  createAgentTemplate,
  deleteAgentTemplate,
  listAgentTemplates,
  setAgentTemplateEnabled,
  setAgentTemplateProjectAssignment,
  updateAgentTemplate
} from '../../agentCollaboration/collaborationClient'
import { SettingsBreadcrumbs } from '../components/SettingsBreadcrumbs'
import './AgentTemplatesSettingsPage.css'

interface AgentTemplatesSettingsPageProps {
  initialProjectId?: string | null
  onNavigateSettingsRoot?: () => void
  projects: readonly AppProject[]
}

interface TemplateFormState {
  description: string
  instructions: string
  kind: 'create' | 'edit'
  modelConfigId: string | null
  name: string
  projectIds: string[]
  template: AgentTemplate | null
}

export function AgentTemplatesSettingsPage({
  initialProjectId,
  onNavigateSettingsRoot,
  projects
}: AgentTemplatesSettingsPageProps) {
  const { t } = useFrontendConfig()
  const { enabledModels, models } = useModelSettings()
  const [templates, setTemplates] = useState<AgentTemplate[]>([])
  const [loading, setLoading] = useState(false)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [operationError, setOperationError] = useState<string | null>(null)
  const [busyTemplateId, setBusyTemplateId] = useState<string | null>(null)
  const [form, setForm] = useState<TemplateFormState | null>(null)
  const [pendingDelete, setPendingDelete] = useState<AgentTemplate | null>(null)
  const loadRequestRef = useRef(0)

  const reload = useCallback(async (): Promise<AgentTemplate[] | null> => {
    const requestId = ++loadRequestRef.current
    setLoading(true)
    setLoadError(null)
    try {
      const result = await listAgentTemplates({ includeDisabled: true })
      if (loadRequestRef.current !== requestId) return null
      const sorted = sortTemplates(result.templates)
      setTemplates(sorted)
      return sorted
    } catch (loadError) {
      if (loadRequestRef.current !== requestId) return null
      setLoadError(getUserFacingErrorMessage(loadError, t, 'agentTemplates.loadFailed'))
      return null
    } finally {
      if (loadRequestRef.current === requestId) setLoading(false)
    }
  }, [t])

  useEffect(() => {
    void reload()
    return () => {
      loadRequestRef.current += 1
    }
  }, [reload])

  const enabledModelIds = useMemo(
    () => new Set(enabledModels.map((model) => model.id)),
    [enabledModels]
  )

  const startCreate = () => {
    setOperationError(null)
    setForm({
      description: '',
      instructions: '',
      kind: 'create',
      modelConfigId: enabledModels[0]?.id ?? null,
      name: '',
      projectIds:
        initialProjectId && projects.some((project) => project.id === initialProjectId)
          ? [initialProjectId]
          : [],
      template: null
    })
  }

  const startEdit = (template: AgentTemplate) => {
    setOperationError(null)
    setForm({
      description: template.description,
      instructions: template.instructions,
      kind: 'edit',
      modelConfigId: template.modelConfigId,
      name: template.name,
      projectIds: [...template.projectIds],
      template
    })
  }

  const runTemplateMutation = async (
    templateId: string,
    mutation: () => Promise<AgentTemplate>
  ): Promise<AgentTemplate | null> => {
    setBusyTemplateId(templateId)
    setOperationError(null)
    try {
      const result = await mutation()
      setTemplates((current) =>
        sortTemplates([
          ...current.filter((template) => template.templateId !== result.templateId),
          result
        ])
      )
      return result
    } catch (mutationError) {
      const message = getUserFacingErrorMessage(mutationError, t, 'agentTemplates.operationFailed')
      await reload()
      setOperationError(message)
      return null
    } finally {
      setBusyTemplateId(null)
    }
  }

  if (form) {
    const unavailableModel =
      form.modelConfigId && !enabledModelIds.has(form.modelConfigId)
        ? {
            disabled: true,
            id: form.modelConfigId,
            label: `${
              form.template?.modelDisplayName ??
              models.find((model) => model.id === form.modelConfigId)?.displayName ??
              form.modelConfigId
            } · ${t('agentTemplates.modelUnavailable')}`
          }
        : null
    const modelOptions = [
      ...(unavailableModel ? [unavailableModel] : []),
      ...enabledModels.map((model) => ({ id: model.id, label: model.displayName }))
    ]
    const modelAvailable = Boolean(form.modelConfigId && enabledModelIds.has(form.modelConfigId))
    const canSave =
      form.name.trim().length > 0 && form.instructions.trim().length > 0 && modelAvailable

    return (
      <article className="settings-list-page agent-templates-page">
        <SettingsBreadcrumbs
          ariaLabel={t('settings.breadcrumb.label')}
          items={[
            {
              id: 'settings',
              label: t('settings.breadcrumb.root'),
              onSelect: onNavigateSettingsRoot
            },
            {
              id: 'agent-templates',
              label: t('settings.page.agentTemplates'),
              onSelect: () => setForm(null)
            },
            {
              id: form.kind,
              label:
                form.kind === 'create'
                  ? t('agentTemplates.createTitle')
                  : t('agentTemplates.editTitle')
            }
          ]}
        />
        <h1>
          {form.kind === 'create' ? t('agentTemplates.createTitle') : t('agentTemplates.editTitle')}
        </h1>
        <p className="agent-templates-page__intro">{t('agentTemplates.snapshotHint')}</p>

        <form
          className="agent-template-form"
          onSubmit={(event) => {
            event.preventDefault()
            if (!canSave || !form.modelConfigId || busyTemplateId !== null) return
            const templateId = form.template?.templateId ?? crypto.randomUUID()
            setBusyTemplateId(templateId)
            setOperationError(null)
            void (async () => {
              try {
                const savedDefinition =
                  form.kind === 'create'
                    ? await createAgentTemplate({
                        description: form.description,
                        enabled: true,
                        instructions: form.instructions,
                        machineKey: createTemplateMachineKey(
                          form.name,
                          templateId,
                          templates.map((template) => template.machineKey)
                        ),
                        modelConfigId: form.modelConfigId!,
                        name: form.name,
                        templateId
                      })
                    : await updateAgentTemplate({
                        description: form.description,
                        expectedRevision: form.template!.revision,
                        instructions: form.instructions,
                        modelConfigId: form.modelConfigId!,
                        name: form.name,
                        templateId
                      })
                const saved = await reconcileProjectAssignments(savedDefinition, form.projectIds)
                setTemplates((current) => upsertTemplate(current, saved))
                setForm(null)
              } catch (mutationError) {
                const message = getUserFacingErrorMessage(
                  mutationError,
                  t,
                  'agentTemplates.operationFailed'
                )
                const refreshed = await reload()
                const canonical = refreshed?.find((template) => template.templateId === templateId)
                if (canonical) {
                  setForm((current) =>
                    current
                      ? {
                          ...current,
                          kind: 'edit',
                          template: canonical
                        }
                      : current
                  )
                }
                setOperationError(message)
              } finally {
                setBusyTemplateId(null)
              }
            })()
          }}
        >
          <label>
            <span>{t('agentTemplates.name')}</span>
            <input
              autoFocus
              maxLength={256}
              onChange={(event) => setForm({ ...form, name: event.currentTarget.value })}
              required
              value={form.name}
            />
          </label>
          <fieldset className="agent-template-form__projects">
            <legend>{t('agentTemplates.projects')}</legend>
            <p>{t('agentTemplates.projectsHint')}</p>
            {projects.length > 0 ? (
              <div className="agent-template-form__project-list">
                {projects.map((project) => (
                  <label key={project.id}>
                    <input
                      checked={form.projectIds.includes(project.id)}
                      onChange={(event) => {
                        const checked = event.currentTarget.checked
                        setForm({
                          ...form,
                          projectIds: checked
                            ? [...form.projectIds, project.id]
                            : form.projectIds.filter((projectId) => projectId !== project.id)
                        })
                      }}
                      type="checkbox"
                    />
                    <span>{project.name}</span>
                  </label>
                ))}
              </div>
            ) : (
              <span className="agent-template-form__no-projects">
                {t('agentTemplates.noProjectsAvailable')}
              </span>
            )}
          </fieldset>
          <label>
            <span>{t('agentTemplates.description')}</span>
            <textarea
              maxLength={4096}
              onChange={(event) => setForm({ ...form, description: event.currentTarget.value })}
              placeholder={t('agentTemplates.descriptionPlaceholder')}
              rows={3}
              value={form.description}
            />
          </label>
          <label>
            <span>{t('agentTemplates.instructions')}</span>
            <textarea
              maxLength={65536}
              onChange={(event) => setForm({ ...form, instructions: event.currentTarget.value })}
              placeholder={t('agentTemplates.instructionsPlaceholder')}
              required
              rows={8}
              value={form.instructions}
            />
          </label>
          <label>
            <span>{t('agentTemplates.model')}</span>
            <ModelConfigPicker
              ariaLabel={t('agentTemplates.selectModel')}
              emptyLabel={t('chat.noEnabledModels')}
              onChange={(modelConfigId) => setForm({ ...form, modelConfigId })}
              options={modelOptions}
              value={form.modelConfigId}
              variant="settings"
            />
          </label>
          {!modelAvailable && form.modelConfigId ? (
            <p className="agent-template-form__warning" role="alert">
              {t('agentTemplates.reselectModel')}
            </p>
          ) : null}
          <div className="agent-template-form__actions">
            <button
              className="secondary-settings-button"
              onClick={() => setForm(null)}
              type="button"
            >
              {t('agentTemplates.cancel')}
            </button>
            <button
              className="primary-settings-button"
              disabled={!canSave || busyTemplateId !== null}
              type="submit"
            >
              {t('agentTemplates.save')}
            </button>
          </div>
        </form>
        {operationError ? (
          <p className="agent-templates-page__error" role="alert">
            {operationError}
          </p>
        ) : null}
      </article>
    )
  }

  return (
    <article className="settings-list-page agent-templates-page">
      <header className="agent-templates-page__heading">
        <div>
          <h1>{t('settings.page.agentTemplates')}</h1>
          <p className="settings-list-page__description">{t('agentTemplates.descriptionText')}</p>
        </div>
        <button
          className="agent-templates-create-button"
          disabled={enabledModels.length === 0}
          onClick={startCreate}
          type="button"
        >
          <Plus aria-hidden="true" />
          <span>{t('agentTemplates.create')}</span>
        </button>
      </header>

      {operationError ? (
        <p className="agent-templates-page__error" role="alert">
          {operationError}
        </p>
      ) : null}

      <section aria-label={t('agentTemplates.list')} className="agent-templates-page__list">
        {loading ? <p role="status">{t('agentTemplates.loading')}</p> : null}
        {!loading && loadError ? (
          <div className="agent-templates-page__load-error" role="alert">
            <span>{loadError}</span>
            <button onClick={() => void reload()} type="button">
              <RefreshCw aria-hidden="true" />
              {t('agentTemplates.retry')}
            </button>
          </div>
        ) : null}
        {!loading && !loadError && templates.length === 0 ? (
          <div className="agent-templates-page__empty">
            <Bot aria-hidden="true" />
            <strong>{t('agentTemplates.empty')}</strong>
            <span>{t('agentTemplates.emptyHint')}</span>
          </div>
        ) : null}
        {!loading && !loadError && templates.length > 0
          ? templates.map((template) => {
              const modelAvailable = enabledModelIds.has(template.modelConfigId)
              const busy = busyTemplateId === template.templateId
              return (
                <article className="agent-template-row" key={template.templateId}>
                  <div className="agent-template-row__copy">
                    <div className="agent-template-row__title">
                      <strong>{template.name}</strong>
                      <span data-status={template.enabled ? 'enabled' : 'disabled'}>
                        {template.enabled
                          ? t('agentTemplates.enabled')
                          : t('agentTemplates.disabled')}
                      </span>
                    </div>
                    {template.description ? <p>{template.description}</p> : null}
                    <small data-unavailable={!modelAvailable || undefined}>
                      {modelAvailable
                        ? (template.modelDisplayName ?? template.modelConfigId)
                        : `${template.modelDisplayName ?? template.modelConfigId} · ${t(
                            'agentTemplates.modelUnavailable'
                          )}`}
                    </small>
                    <small>
                      {replaceTokens(t('agentTemplates.projectCount'), {
                        count: String(template.projectIds.length)
                      })}
                    </small>
                    {!modelAvailable ? (
                      <span className="agent-template-row__warning">
                        {t('agentTemplates.reselectModel')}
                      </span>
                    ) : null}
                  </div>
                  <div className="agent-template-row__actions">
                    <button
                      aria-label={`${t('agentTemplates.edit')} ${template.name}`}
                      disabled={busy}
                      onClick={() => startEdit(template)}
                      type="button"
                    >
                      <Pencil aria-hidden="true" />
                    </button>
                    <button
                      disabled={busy || (!modelAvailable && !template.enabled)}
                      onClick={() => {
                        void runTemplateMutation(template.templateId, () =>
                          setAgentTemplateEnabled({
                            enabled: !template.enabled,
                            expectedRevision: template.revision,
                            templateId: template.templateId
                          })
                        )
                      }}
                      type="button"
                    >
                      {template.enabled ? t('agentTemplates.disable') : t('agentTemplates.enable')}
                    </button>
                    <button
                      aria-label={`${t('agentTemplates.delete')} ${template.name}`}
                      disabled={busy}
                      onClick={() => setPendingDelete(template)}
                      type="button"
                    >
                      <Trash2 aria-hidden="true" />
                    </button>
                  </div>
                </article>
              )
            })
          : null}
      </section>

      {pendingDelete ? (
        <ConfirmationDialog
          cancelLabel={t('agentTemplates.cancel')}
          confirmLabel={t('agentTemplates.delete')}
          description={replaceTokens(t('agentTemplates.deleteDescription'), {
            count: String(pendingDelete.projectIds.length)
          })}
          onCancel={() => setPendingDelete(null)}
          onConfirm={() => {
            const template = pendingDelete
            setPendingDelete(null)
            setBusyTemplateId(template.templateId)
            setOperationError(null)
            void deleteAgentTemplate({
              expectedRevision: template.revision,
              templateId: template.templateId
            })
              .then(() => {
                setTemplates((current) =>
                  current.filter((candidate) => candidate.templateId !== template.templateId)
                )
              })
              .catch(async (deleteError) => {
                const message = getUserFacingErrorMessage(
                  deleteError,
                  t,
                  'agentTemplates.operationFailed'
                )
                await reload()
                setOperationError(message)
              })
              .finally(() => {
                setBusyTemplateId(null)
              })
          }}
          title={t('agentTemplates.deleteTitle')}
        />
      ) : null}
    </article>
  )
}

function sortTemplates(templates: readonly AgentTemplate[]): AgentTemplate[] {
  return [...templates].sort(
    (left, right) =>
      Number(right.enabled) - Number(left.enabled) ||
      left.name.localeCompare(right.name) ||
      left.templateId.localeCompare(right.templateId)
  )
}

function upsertTemplate(
  templates: readonly AgentTemplate[],
  template: AgentTemplate
): AgentTemplate[] {
  return sortTemplates([
    ...templates.filter((candidate) => candidate.templateId !== template.templateId),
    template
  ])
}

async function reconcileProjectAssignments(
  template: AgentTemplate,
  desiredProjectIds: readonly string[]
): Promise<AgentTemplate> {
  const currentProjectIds = new Set(template.projectIds)
  const desired = new Set(desiredProjectIds)
  const projectIds = [...new Set([...currentProjectIds, ...desired])].sort()
  let canonical = template
  for (const projectId of projectIds) {
    const assigned = desired.has(projectId)
    if (currentProjectIds.has(projectId) === assigned) continue
    canonical = await setAgentTemplateProjectAssignment({
      assigned,
      projectId,
      templateId: template.templateId
    })
  }
  return canonical
}

function replaceTokens(template: string, values: Record<string, string>): string {
  return Object.entries(values).reduce(
    (current, [key, value]) => current.replaceAll(`{${key}}`, value),
    template
  )
}

export function createTemplateMachineKey(
  name: string,
  templateId: string,
  existingMachineKeys: readonly string[]
): string {
  const normalized = name
    .normalize('NFKD')
    .replace(/[\u0300-\u036f]/g, '')
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, '_')
    .replace(/^[_\d-]+/, '')
    .replace(/_+/g, '_')
    .replace(/^_+|_+$/g, '')
  const base = (normalized || 'agent').slice(0, 64)
  const existing = new Set(existingMachineKeys)
  if (!existing.has(base)) return base
  const suffix =
    templateId
      .toLowerCase()
      .replace(/[^a-z0-9]/g, '')
      .slice(0, 8) || 'new'
  return `${base.slice(0, Math.max(1, 63 - suffix.length))}-${suffix}`
}
