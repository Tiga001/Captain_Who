import { renderSettingsNodes, settingLabel, settingDescription } from '../settingsDefinition'
import { useSettingsPageNavigation } from '../settingsSearchNavigation'
import {
  agentCollaborationSettingsNodes,
  agentTemplateCreateSettings,
  agentTemplateEditorSettings,
  agentTemplateLibrarySettings,
  agentTemplateFormSettings,
  agentTemplateRowSettings
} from './managementSettings.definition'
import type { AgentTemplate } from '@mycopilot/protocol'
import { Bot, Pencil, Plus, RefreshCw, Trash2 } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { ConfirmationDialog } from '../../../components/dialog/ConfirmationDialog'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { getUserFacingErrorMessage } from '../../../errors/userFacingError'
import { useModelSettings } from '../../../config/ModelSettingsProvider'
import type { AppProject } from '../../../config/projectConfig'
import { ModelConfigPicker } from '../../modelSelection/ModelConfigPicker'
import { formatModelConfigLabel } from '../../modelSelection/modelConfigPresentation'
import {
  createAgentTemplate,
  deleteAgentTemplate,
  listAgentTemplates,
  setAgentTemplateEnabled,
  setAgentTemplateProjectAssignment,
  updateAgentTemplate
} from '../../agentCollaboration/collaborationClient'
import { SettingsBreadcrumbs } from '../components/SettingsBreadcrumbs'
import { useAgentCollaborationSettings } from './useAgentCollaborationSettings'
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
  const collaboration = useAgentCollaborationSettings()
  const { enabledModels, models } = useModelSettings()
  const [templates, setTemplates] = useState<AgentTemplate[]>([])
  const [loading, setLoading] = useState(false)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [operationError, setOperationError] = useState<string | null>(null)
  const [busyTemplateId, setBusyTemplateId] = useState<string | null>(null)
  const [form, setForm] = useState<TemplateFormState | null>(null)
  const [editorHidden, setEditorHidden] = useState(false)
  const [pendingDelete, setPendingDelete] = useState<AgentTemplate | null>(null)
  const loadRequestRef = useRef(0)

  useSettingsPageNavigation('agentTemplates', (target) => {
    // Search changes the presentation while retaining an existing unsaved draft.
    if (target.view === 'templates') setEditorHidden(true)
    if (target.view === 'editor') setEditorHidden(false)
  })

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
    setEditorHidden(false)
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
    setEditorHidden(false)
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

  if (form && !editorHidden) {
    const configuredModel = models.find((model) => model.id === form.modelConfigId)
    const storedModelLabel = form.template?.modelDisplayName?.trim() ?? ''
    const unavailableModelBaseLabel = configuredModel
      ? formatModelConfigLabel(configuredModel)
      : storedModelLabel
    const unavailableModel =
      form.modelConfigId && !enabledModelIds.has(form.modelConfigId)
        ? {
            disabled: true,
            id: form.modelConfigId,
            label: unavailableModelBaseLabel
              ? `${unavailableModelBaseLabel} · ${t('agentTemplates.modelUnavailable')}`
              : t('agentTemplates.modelUnavailable')
          }
        : null
    const modelOptions = [
      ...(unavailableModel ? [unavailableModel] : []),
      ...enabledModels.map((model) => ({ id: model.id, label: formatModelConfigLabel(model) }))
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

        {renderSettingsNodes(agentTemplateEditorSettings, () => (
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
                  const canonical = refreshed?.find(
                    (template) => template.templateId === templateId
                  )
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
            {renderSettingsNodes(agentTemplateFormSettings, (node) => {
              switch (node.id) {
                case 'agent-template-name':
                  return (
                    <label>
                      <span>{settingLabel(node, t)}</span>
                      <input
                        autoFocus
                        maxLength={256}
                        onChange={(event) => setForm({ ...form, name: event.currentTarget.value })}
                        required
                        value={form.name}
                      />
                    </label>
                  )
                case 'agent-template-projects':
                  return (
                    <fieldset className="agent-template-form__projects">
                      <legend>{settingLabel(node, t)}</legend>
                      <p>{settingDescription(node, t)}</p>
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
                                      : form.projectIds.filter(
                                          (projectId) => projectId !== project.id
                                        )
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
                  )
                case 'agent-template-description':
                  return (
                    <label>
                      <span>{settingLabel(node, t)}</span>
                      <textarea
                        maxLength={4096}
                        onChange={(event) =>
                          setForm({ ...form, description: event.currentTarget.value })
                        }
                        placeholder={settingDescription(node, t)}
                        rows={3}
                        value={form.description}
                      />
                    </label>
                  )
                case 'agent-template-instructions':
                  return (
                    <label>
                      <span>{settingLabel(node, t)}</span>
                      <textarea
                        maxLength={65536}
                        onChange={(event) =>
                          setForm({ ...form, instructions: event.currentTarget.value })
                        }
                        placeholder={settingDescription(node, t)}
                        required
                        rows={8}
                        value={form.instructions}
                      />
                    </label>
                  )
                case 'agent-template-model':
                  return (
                    <label>
                      <span>{settingLabel(node, t)}</span>
                      <ModelConfigPicker
                        ariaLabel={t('agentTemplates.selectModel')}
                        emptyLabel={t('chat.noEnabledModels')}
                        onChange={(modelConfigId) => setForm({ ...form, modelConfigId })}
                        options={modelOptions}
                        value={form.modelConfigId}
                        variant="settings"
                      />
                    </label>
                  )
              }
            })}
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
        ))}
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
      <h1>{t('settings.page.agentTemplates')}</h1>

      {renderSettingsNodes(agentCollaborationSettingsNodes, (node) => (
        <section
          className="agent-templates-page__collaboration settings-list-section"
          aria-labelledby="agent-collaboration-heading"
        >
          <h2 id="agent-collaboration-heading">{t('settings.page.agentTemplates')}</h2>
          <div className="settings-list">
            <div className="settings-list-row">
              <div className="settings-list-row__text">
                <span className="settings-list-row__title" id="agent-collaboration-settings-label">
                  {settingLabel(node, t)}
                </span>
                <p className="settings-list-row__description">{settingDescription(node, t)}</p>
              </div>
              {collaboration.settings ? (
                <button
                  className="settings-switch"
                  type="button"
                  role="switch"
                  aria-labelledby="agent-collaboration-settings-label"
                  aria-checked={collaboration.settings.enabled}
                  aria-busy={collaboration.saving || undefined}
                  data-state={collaboration.settings.enabled ? 'on' : 'off'}
                  disabled={collaboration.loading || collaboration.saving}
                  onClick={() => void collaboration.toggle()}
                >
                  <span className="settings-switch__thumb" aria-hidden="true" />
                </button>
              ) : collaboration.loading ? (
                <span role="status">{t('humanInteraction.settings.loading')}</span>
              ) : null}
            </div>
          </div>
          {collaboration.error ? (
            <div className="agent-templates-page__load-error">
              <span role="alert">
                {t(
                  collaboration.error === 'save'
                    ? 'humanInteraction.settings.saveFailed'
                    : 'humanInteraction.settings.loadFailed'
                )}
              </span>
              <button
                type="button"
                disabled={collaboration.loading || collaboration.saving}
                onClick={() => void collaboration.refresh()}
              >
                {t('agentTemplates.retry')}
              </button>
            </div>
          ) : null}
        </section>
      ))}

      {operationError ? (
        <p className="agent-templates-page__error" role="alert">
          {operationError}
        </p>
      ) : null}

      {renderSettingsNodes(agentTemplateLibrarySettings, (node) => (
        <section
          aria-labelledby="agent-templates-library-heading"
          className="settings-list-section agent-templates-page__list"
        >
          <div className="settings-list-section__header agent-templates-page__library-heading">
            <h2 id="agent-templates-library-heading">{settingLabel(node, t)}</h2>
            {renderSettingsNodes(agentTemplateCreateSettings, (createNode) => (
              <button
                className="agent-templates-create-button"
                disabled={enabledModels.length === 0}
                onClick={startCreate}
                type="button"
              >
                <Plus aria-hidden="true" />
                <span>{settingLabel(createNode, t)}</span>
              </button>
            ))}
          </div>
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
                const configuredModel = models.find((model) => model.id === template.modelConfigId)
                const storedModelLabel = template.modelDisplayName?.trim() ?? ''
                const visibleModelLabel = configuredModel
                  ? formatModelConfigLabel(configuredModel)
                  : storedModelLabel
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
                          ? visibleModelLabel || t('agentTemplates.modelUnavailable')
                          : visibleModelLabel
                            ? `${visibleModelLabel} · ${t('agentTemplates.modelUnavailable')}`
                            : t('agentTemplates.modelUnavailable')}
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
                      {renderSettingsNodes(agentTemplateRowSettings, (node) => {
                        switch (node.id) {
                          case 'agent-templates-enabled':
                            return (
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
                                {template.enabled
                                  ? t('agentTemplates.disable')
                                  : settingLabel(node, t)}
                              </button>
                            )
                          case 'agent-templates-delete':
                            return (
                              <button
                                aria-label={`${settingLabel(node, t)} ${template.name}`}
                                disabled={busy}
                                onClick={() => setPendingDelete(template)}
                                type="button"
                              >
                                <Trash2 aria-hidden="true" />
                              </button>
                            )
                        }
                      })}
                    </div>
                  </article>
                )
              })
            : null}
        </section>
      ))}

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
