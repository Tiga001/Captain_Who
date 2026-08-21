import type { AgentTemplate } from '@mycopilot/protocol'
import { Bot, ChevronLeft, Pencil, Plus, RefreshCw, Trash2 } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { ConfirmationDialog } from '../../../components/dialog/ConfirmationDialog'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { useModelSettings } from '../../../config/ModelSettingsProvider'
import type { AppProject } from '../../../config/projectConfig'
import { ModelConfigPicker } from '../../modelSelection/ModelConfigPicker'
import {
  createAgentTemplate,
  deleteAgentTemplate,
  listAgentTemplates,
  setAgentTemplateEnabled,
  updateAgentTemplate
} from '../../agentCollaboration/collaborationClient'
import { SettingsSelect } from '../components/SettingsSelect'
import './AgentTemplatesSettingsPage.css'

interface AgentTemplatesSettingsPageProps {
  initialProjectId?: string | null
  projects: readonly AppProject[]
}

interface TemplateFormState {
  description: string
  instructions: string
  kind: 'create' | 'edit'
  modelConfigId: string | null
  name: string
  template: AgentTemplate | null
}

export function AgentTemplatesSettingsPage({
  initialProjectId,
  projects
}: AgentTemplatesSettingsPageProps) {
  const { t } = useFrontendConfig()
  const { enabledModels, models } = useModelSettings()
  const [projectId, setProjectId] = useState<string | null>(() =>
    selectInitialProjectId(projects, initialProjectId)
  )
  const [templates, setTemplates] = useState<AgentTemplate[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [busyTemplateId, setBusyTemplateId] = useState<string | null>(null)
  const [form, setForm] = useState<TemplateFormState | null>(null)
  const [pendingDelete, setPendingDelete] = useState<AgentTemplate | null>(null)
  const projectIdRef = useRef(projectId)
  const selectProject = useCallback((nextProjectId: string | null) => {
    projectIdRef.current = nextProjectId
    setProjectId(nextProjectId)
    setTemplates([])
    setBusyTemplateId(null)
    setForm(null)
    setPendingDelete(null)
    setError(null)
  }, [])

  useEffect(() => {
    projectIdRef.current = projectId
  }, [projectId])

  useEffect(() => {
    if (projectId && projects.some((project) => project.id === projectId)) return
    selectProject(selectInitialProjectId(projects, initialProjectId))
  }, [initialProjectId, projectId, projects, selectProject])

  const reload = useCallback(async () => {
    const requestProjectId = projectId
    if (!requestProjectId) {
      setTemplates([])
      setLoading(false)
      setError(null)
      return
    }
    setLoading(true)
    setError(null)
    try {
      const result = await listAgentTemplates({
        includeDisabled: true,
        projectId: requestProjectId
      })
      if (projectIdRef.current !== requestProjectId) return
      setTemplates(sortTemplates(result.templates))
    } catch (loadError) {
      if (projectIdRef.current !== requestProjectId) return
      setError(loadError instanceof Error ? loadError.message : t('agentTemplates.loadFailed'))
    } finally {
      if (projectIdRef.current === requestProjectId) setLoading(false)
    }
  }, [projectId, t])

  useEffect(() => {
    let cancelled = false
    if (!projectId) {
      setTemplates([])
      setLoading(false)
      setError(null)
      return
    }
    setLoading(true)
    setError(null)
    const requestProjectId = projectId
    void listAgentTemplates({ includeDisabled: true, projectId: requestProjectId })
      .then((result) => {
        if (!cancelled && projectIdRef.current === requestProjectId) {
          setTemplates(sortTemplates(result.templates))
        }
      })
      .catch((loadError) => {
        if (!cancelled && projectIdRef.current === requestProjectId) {
          setError(loadError instanceof Error ? loadError.message : t('agentTemplates.loadFailed'))
        }
      })
      .finally(() => {
        if (!cancelled && projectIdRef.current === requestProjectId) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [projectId, t])

  const projectOptions = projects.map((project) => ({ label: project.name, value: project.id }))
  const enabledModelIds = useMemo(
    () => new Set(enabledModels.map((model) => model.id)),
    [enabledModels]
  )

  const startCreate = () => {
    setForm({
      description: '',
      instructions: '',
      kind: 'create',
      modelConfigId: enabledModels[0]?.id ?? null,
      name: '',
      template: null
    })
  }

  const startEdit = (template: AgentTemplate) => {
    setForm({
      description: template.description,
      instructions: template.instructions,
      kind: 'edit',
      modelConfigId: template.modelConfigId,
      name: template.name,
      template
    })
  }

  const runTemplateMutation = async (
    mutationProjectId: string,
    templateId: string,
    mutation: () => Promise<AgentTemplate>
  ): Promise<AgentTemplate | null> => {
    setBusyTemplateId(templateId)
    setError(null)
    try {
      const result = await mutation()
      if (projectIdRef.current !== mutationProjectId) return null
      setTemplates((current) =>
        sortTemplates([
          ...current.filter((template) => template.templateId !== result.templateId),
          result
        ])
      )
      return result
    } catch (mutationError) {
      if (projectIdRef.current !== mutationProjectId) return null
      setError(
        mutationError instanceof Error ? mutationError.message : t('agentTemplates.operationFailed')
      )
      void reload()
      return null
    } finally {
      if (projectIdRef.current === mutationProjectId) setBusyTemplateId(null)
    }
  }

  if (form && projectId) {
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
        <button className="agent-templates-page__back" onClick={() => setForm(null)} type="button">
          <ChevronLeft aria-hidden="true" />
          {t('agentTemplates.back')}
        </button>
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
            void runTemplateMutation(projectId, templateId, () =>
              form.kind === 'create'
                ? createAgentTemplate({
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
                    projectId,
                    templateId
                  })
                : updateAgentTemplate({
                    description: form.description,
                    expectedRevision: form.template!.revision,
                    instructions: form.instructions,
                    modelConfigId: form.modelConfigId!,
                    name: form.name,
                    projectId,
                    templateId
                  })
            ).then((saved) => {
              if (saved) setForm(null)
            })
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
          <label>
            <span>{t('agentTemplates.description')}</span>
            <textarea
              maxLength={4096}
              onChange={(event) => setForm({ ...form, description: event.currentTarget.value })}
              rows={3}
              value={form.description}
            />
          </label>
          <label>
            <span>{t('agentTemplates.instructions')}</span>
            <textarea
              maxLength={65536}
              onChange={(event) => setForm({ ...form, instructions: event.currentTarget.value })}
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
        {error ? <p className="agent-templates-page__error">{error}</p> : null}
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
          disabled={!projectId || enabledModels.length === 0}
          onClick={startCreate}
          type="button"
        >
          <Plus aria-hidden="true" />
          <span>{t('agentTemplates.create')}</span>
        </button>
      </header>

      {projects.length > 0 && projectId ? (
        <label className="agent-templates-page__project">
          <span>{t('agentTemplates.project')}</span>
          <SettingsSelect
            ariaLabel={t('agentTemplates.project')}
            onChange={(nextProjectId) => {
              selectProject(nextProjectId)
            }}
            options={projectOptions}
            value={projectId}
          />
        </label>
      ) : (
        <div className="agent-templates-page__empty">
          <Bot aria-hidden="true" />
          <strong>{t('agentTemplates.noProject')}</strong>
          <span>{t('agentTemplates.noProjectHint')}</span>
        </div>
      )}

      {projectId ? (
        <section aria-label={t('agentTemplates.list')} className="agent-templates-page__list">
          {loading ? <p role="status">{t('agentTemplates.loading')}</p> : null}
          {!loading && error ? (
            <div className="agent-templates-page__load-error" role="alert">
              <span>{error}</span>
              <button onClick={() => void reload()} type="button">
                <RefreshCw aria-hidden="true" />
                {t('agentTemplates.retry')}
              </button>
            </div>
          ) : null}
          {!loading && !error && templates.length === 0 ? (
            <div className="agent-templates-page__empty">
              <Bot aria-hidden="true" />
              <strong>{t('agentTemplates.empty')}</strong>
              <span>{t('agentTemplates.emptyHint')}</span>
            </div>
          ) : null}
          {!loading && templates.length > 0
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
                          void runTemplateMutation(projectId, template.templateId, () =>
                            setAgentTemplateEnabled({
                              enabled: !template.enabled,
                              expectedRevision: template.revision,
                              projectId,
                              templateId: template.templateId
                            })
                          )
                        }}
                        type="button"
                      >
                        {template.enabled
                          ? t('agentTemplates.disable')
                          : t('agentTemplates.enable')}
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
      ) : null}

      {pendingDelete && projectId ? (
        <ConfirmationDialog
          cancelLabel={t('agentTemplates.cancel')}
          confirmLabel={t('agentTemplates.delete')}
          description={t('agentTemplates.deleteDescription')}
          onCancel={() => setPendingDelete(null)}
          onConfirm={() => {
            const template = pendingDelete
            setPendingDelete(null)
            setBusyTemplateId(template.templateId)
            setError(null)
            void deleteAgentTemplate({
              expectedRevision: template.revision,
              projectId,
              templateId: template.templateId
            })
              .then(() => {
                if (projectIdRef.current !== projectId) return
                setTemplates((current) =>
                  current.filter((candidate) => candidate.templateId !== template.templateId)
                )
              })
              .catch((deleteError) => {
                if (projectIdRef.current !== projectId) return
                setError(
                  deleteError instanceof Error
                    ? deleteError.message
                    : t('agentTemplates.operationFailed')
                )
                void reload()
              })
              .finally(() => {
                if (projectIdRef.current === projectId) setBusyTemplateId(null)
              })
          }}
          title={t('agentTemplates.deleteTitle')}
        />
      ) : null}
    </article>
  )
}

function selectInitialProjectId(
  projects: readonly AppProject[],
  preferred: string | null | undefined
): string | null {
  if (preferred && projects.some((project) => project.id === preferred)) return preferred
  return projects[0]?.id ?? null
}

function sortTemplates(templates: readonly AgentTemplate[]): AgentTemplate[] {
  return [...templates].sort(
    (left, right) =>
      Number(right.enabled) - Number(left.enabled) ||
      left.name.localeCompare(right.name) ||
      left.templateId.localeCompare(right.templateId)
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
