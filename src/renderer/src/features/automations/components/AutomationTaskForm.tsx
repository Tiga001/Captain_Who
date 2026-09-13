import { useEffect, useMemo, useState } from 'react'
import type {
  AutomationNotificationPolicy,
  AutomationScheduleInput,
  AutomationTask
} from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import type { ModelConfig } from '../../../config/modelConfig'
import type { AppProject } from '../../../config/projectConfig'
import type { ChatPermissionMode, ChatConversation } from '../../chat/chatTypes'
import { ModelConfigPicker } from '../../modelSelection/ModelConfigPicker'
import type { ModelConfigPickerOption } from '../../modelSelection/ModelConfigPicker'
import { formatModelConfigLabel } from '../../modelSelection/modelConfigPresentation'
import type { AutomationDraft } from '../automationTypes'
import { withSystemTimeZone } from '../automationSchedule'
import { validateAutomationDraft as validateProtocolAutomationDraft } from '../automationValidation'
import { AutomationChatPicker } from './AutomationChatPicker'
import { AutomationField, AutomationScheduleEditor } from './AutomationScheduleEditor'
import { AutomationPermissionPicker } from './AutomationPermissionPicker'
import { AutomationSelect, type AutomationOption } from './AutomationControls'

interface AutomationTaskFormProps {
  conversations: readonly ChatConversation[]
  disabled?: boolean
  initialDraft: AutomationDraft
  mode: 'create' | 'edit'
  models: readonly ModelConfig[]
  onCancel: () => void
  onDirtyChange: (dirty: boolean) => void
  onOpenPermissionSettings?: () => void
  onSubmittingChange: (submitting: boolean) => void
  onSubmit: (draft: AutomationDraft) => Promise<void>
  permissionModeAvailability: { custom: boolean; full: boolean }
  projects: readonly AppProject[]
  task?: AutomationTask | null
}

type FormErrors = Record<string, string>

function cloneDraft(draft: AutomationDraft): AutomationDraft {
  return JSON.parse(JSON.stringify(draft)) as AutomationDraft
}

function legacyReasoningProjection(model: ModelConfig | null) {
  const config = model?.providerProfileConfig
  if (!config) return undefined
  if (config.schemaVersion === 1) return config.reasoning
  if (
    config.settings.kind === 'deepseek_flash_chat' ||
    config.settings.kind === 'deepseek_pro_chat'
  ) {
    return config.settings.reasoning
  }
  if (config.settings.kind === 'moonshot_k3_chat') {
    return { mode: 'enabled' as const, effort: config.settings.reasoningEffort }
  }
  if (config.settings.kind === 'moonshot_k2_7_code_chat') {
    return { mode: 'enabled' as const, effort: 'provider_default' as const }
  }
  if (config.settings.kind === 'moonshot_k2_6_chat') {
    return {
      mode:
        config.settings.thinkingMode === 'disabled'
          ? ('disabled' as const)
          : config.settings.thinkingMode === 'provider_default'
            ? ('provider_default' as const)
            : ('enabled' as const),
      effort: 'provider_default' as const
    }
  }
  return { mode: 'provider_default' as const, effort: 'provider_default' as const }
}

export function validateAutomationFormDraft(
  draft: AutomationDraft,
  modelIds: ReadonlySet<string>,
  conversationIds: ReadonlySet<string>,
  projectIds: ReadonlySet<string>,
  availability: { custom: boolean; full: boolean },
  messages: {
    chat: string
    minute: string
    model: string
    positive: string
    prompt: string
    selection: string
    timezone: string
    title: string
  }
): FormErrors {
  const errors: FormErrors = {}
  const protocolErrors = validateProtocolAutomationDraft(draft).errors
  if (protocolErrors.title) errors.title = messages.title
  if (protocolErrors.prompt) errors.prompt = messages.prompt
  if (protocolErrors.destination) errors.destination = messages.selection
  if (protocolErrors.projectId) errors.projectId = messages.selection
  if (protocolErrors.modelId) errors.modelId = messages.model
  if (protocolErrors.conversationId) errors.conversationId = messages.chat
  if (protocolErrors.permissionMode) errors.permissionMode = messages.selection
  if (protocolErrors.schedule) errors.kind = messages.selection
  if (protocolErrors.scheduleAmount) errors.amount = messages.positive
  if (protocolErrors.scheduleInterval) errors.interval = messages.positive
  if (protocolErrors.scheduleMinute) errors.minuteOfHour = messages.minute
  if (protocolErrors.scheduleTime) errors.timeMinutes = messages.selection
  if (protocolErrors.scheduleWeekdays) errors.weekdays = messages.selection
  if (protocolErrors.scheduleMonthDays) errors.monthDays = messages.selection
  if (protocolErrors.scheduleMonths) errors.months = messages.selection
  if (protocolErrors.timezone) errors.timezone = messages.timezone
  if (protocolErrors.notificationPolicy) errors.notificationPolicy = messages.selection

  if (draft.destination.kind === 'new_chat') {
    if (!draft.destination.modelId || !modelIds.has(draft.destination.modelId)) {
      errors.modelId = messages.model
    }
    if (
      draft.destination.projectBinding === 'project' &&
      (!draft.destination.projectId || !projectIds.has(draft.destination.projectId))
    ) {
      errors.projectId = messages.selection
    }
  } else if (!conversationIds.has(draft.destination.conversationId)) {
    errors.conversationId = messages.chat
  }

  if (
    (draft.permissionMode === 'full' && !availability.full) ||
    (draft.permissionMode === 'custom' && !availability.custom)
  ) {
    errors.permissionMode = messages.selection
  }

  return errors
}

function taskErrorPresentation(
  error: unknown,
  fallback: string,
  localizedCodes: Readonly<Record<string, string>>,
  localizedFields: Readonly<Record<string, string>>
): { field?: string; message: string } {
  if (!error || typeof error !== 'object') return { message: fallback }
  const candidate = error as { code?: unknown; field?: unknown; message?: unknown }
  const code = typeof candidate.code === 'string' ? candidate.code : ''
  const field = typeof candidate.field === 'string' ? candidate.field.split('.').pop() : undefined
  const message =
    localizedCodes[code] ??
    (code === 'validation' && field ? localizedFields[field] : undefined) ??
    (typeof candidate.message === 'string' && candidate.message.trim()
      ? candidate.message
      : fallback)
  return {
    field,
    message
  }
}

export function AutomationTaskForm({
  conversations,
  disabled = false,
  initialDraft,
  mode,
  models,
  onCancel,
  onDirtyChange,
  onOpenPermissionSettings,
  onSubmittingChange,
  onSubmit,
  permissionModeAvailability,
  projects,
  task
}: AutomationTaskFormProps) {
  const { t } = useFrontendConfig()
  const [draft, setDraft] = useState<AutomationDraft>(() => cloneDraft(initialDraft))
  const [errors, setErrors] = useState<FormErrors>({})
  const [submitError, setSubmitError] = useState<string | null>(null)
  const [submitting, setSubmitting] = useState(false)
  const incomingFingerprint = useMemo(() => JSON.stringify(initialDraft), [initialDraft])
  const [baselineFingerprint, setBaselineFingerprint] = useState(incomingFingerprint)
  const currentFingerprint = JSON.stringify(draft)
  const dirty = currentFingerprint !== baselineFingerprint

  useEffect(() => onDirtyChange(dirty), [dirty, onDirtyChange])
  useEffect(() => onSubmittingChange(submitting), [onSubmittingChange, submitting])
  useEffect(
    () => () => {
      onDirtyChange(false)
      onSubmittingChange(false)
    },
    [onDirtyChange, onSubmittingChange]
  )

  useEffect(() => {
    if (incomingFingerprint === baselineFingerprint || dirty) return
    setDraft(cloneDraft(initialDraft))
    setBaselineFingerprint(incomingFingerprint)
    setErrors({})
    setSubmitError(null)
  }, [baselineFingerprint, dirty, incomingFingerprint, initialDraft])

  const modelIds = useMemo(
    () => new Set(models.filter((model) => model.enabled).map((model) => model.id)),
    [models]
  )
  const projectIds = useMemo(() => new Set(projects.map((project) => project.id)), [projects])
  const conversationIds = useMemo(
    () =>
      new Set(
        conversations
          .filter(
            (conversation) =>
              !conversation.archivedAt && conversation.pendingArchivedAt === undefined
          )
          .map((conversation) => conversation.id)
      ),
    [conversations]
  )

  const validationMessages = {
    chat: t('automation.validationChatRequired'),
    minute: t('automation.validationMinute'),
    model: t('automation.validationModelRequired'),
    positive: t('automation.validationPositiveInteger'),
    prompt: t('automation.validationPromptRequired'),
    selection: t('automation.validationSelectionRequired'),
    timezone: t('automation.validationTimezone'),
    title: t('automation.validationTitleRequired')
  }
  const normalizedDraft = {
    ...draft,
    schedule: withSystemTimeZone(draft.schedule)
  }
  const validation = validateAutomationFormDraft(
    normalizedDraft,
    modelIds,
    conversationIds,
    projectIds,
    permissionModeAvailability,
    validationMessages
  )
  const canSubmit =
    !disabled && !submitting && Object.keys(validation).length === 0 && (mode === 'create' || dirty)
  const newChatDestination = draft.destination.kind === 'new_chat' ? draft.destination : null
  const selectedModel = newChatDestination
    ? (models.find((model) => model.id === newChatDestination.modelId) ?? null)
    : null
  const taskNewChatDestination = task?.destination.kind === 'new_chat' ? task.destination : null
  const reasoningProjection =
    taskNewChatDestination && taskNewChatDestination.modelId === newChatDestination?.modelId
      ? taskNewChatDestination.reasoning
      : legacyReasoningProjection(selectedModel)

  const update = (patch: Partial<AutomationDraft>) => {
    setSubmitError(null)
    setErrors({})
    setDraft((current) => ({ ...current, ...patch }))
  }

  const updateSchedule = (schedule: AutomationScheduleInput) =>
    update({ schedule: withSystemTimeZone(schedule) })

  const projectOptions: AutomationOption<string>[] = [
    { value: '__none__', label: t('automation.noProject') },
    ...projects.map((project) => ({ value: project.id, label: project.name }))
  ]
  if (
    newChatDestination?.projectId &&
    !projectOptions.some((option) => option.value === newChatDestination.projectId)
  ) {
    projectOptions.push({
      value: newChatDestination.projectId,
      label: task?.targetSnapshot.projectName ?? newChatDestination.projectId,
      disabled: true
    })
  }
  const enabledModels = models.filter((model) => model.enabled)
  const modelOptions: ModelConfigPickerOption[] = enabledModels.map((model) => ({
    capabilityLabel: model.supportsImage ? t('configuration.image') : t('configuration.text'),
    capabilitySupported: model.supportsImage,
    id: model.id,
    label: formatModelConfigLabel(model)
  }))
  const selectedModelId = newChatDestination?.modelId
  if (selectedModelId && !modelOptions.some((option) => option.id === selectedModelId)) {
    const unavailableModel = models.find((model) => model.id === selectedModelId)
    const unavailableModelLabel = unavailableModel ? formatModelConfigLabel(unavailableModel) : ''
    const storedModelLabel = task?.targetSnapshot.modelDisplayName?.trim() ?? ''
    modelOptions.push({
      capabilityLabel: unavailableModel
        ? unavailableModel.supportsImage
          ? t('configuration.image')
          : t('configuration.text')
        : undefined,
      capabilitySupported: unavailableModel?.supportsImage,
      id: selectedModelId,
      label: unavailableModelLabel || storedModelLabel || t('automation.modelMissing'),
      disabled: true
    })
  }

  const notificationOptions: AutomationOption<AutomationNotificationPolicy>[] =
    draft.destination.kind === 'new_chat'
      ? [
          { value: 'all_runs', label: t('automation.notificationAllRuns') },
          {
            value: 'unsuccessful_only',
            label: t('automation.notificationUnsuccessfulOnly')
          }
        ]
      : [
          {
            value: 'important_updates',
            label: t('automation.notificationImportantUpdates')
          },
          {
            value: 'unsuccessful_only',
            label: t('automation.notificationUnsuccessfulOnly')
          }
        ]

  const notificationHelp =
    draft.notificationPolicy === 'all_runs'
      ? t('automation.notificationAllRunsHelp')
      : draft.notificationPolicy === 'important_updates'
        ? t('automation.notificationImportantHelp')
        : t('automation.notificationUnsuccessfulHelp')

  const setDestinationKind = (kind: 'new_chat' | 'existing_chat') => {
    if (kind === draft.destination.kind) return
    if (kind === 'existing_chat') {
      update({
        destination: { kind, conversationId: '' },
        notificationPolicy:
          draft.notificationPolicy === 'all_runs' ? 'important_updates' : draft.notificationPolicy
      })
    } else {
      const firstModel = models.find((model) => model.enabled) ?? models[0]
      update({
        destination: {
          kind,
          projectBinding: 'none',
          projectId: null,
          modelId: firstModel?.id ?? ''
        },
        notificationPolicy:
          draft.notificationPolicy === 'important_updates' ? 'all_runs' : draft.notificationPolicy
      })
    }
  }

  const submit = async () => {
    const submissionDraft = {
      ...draft,
      // Resolve this at click time so a system timezone change while the drawer
      // is open cannot leak the previous zone into the request.
      schedule: withSystemTimeZone(draft.schedule)
    }
    const nextErrors = validateAutomationFormDraft(
      submissionDraft,
      modelIds,
      conversationIds,
      projectIds,
      permissionModeAvailability,
      validationMessages
    )
    setErrors(nextErrors)
    if (Object.keys(nextErrors).length > 0) return
    setSubmitting(true)
    setSubmitError(null)
    try {
      await onSubmit(submissionDraft)
      setDraft(submissionDraft)
      setBaselineFingerprint(JSON.stringify(submissionDraft))
    } catch (error) {
      const presentation = taskErrorPresentation(
        error,
        t('automation.saveFailed'),
        {
          not_found: t('automation.updatedElsewhere'),
          permission_disabled: t('automation.permissionRepair'),
          revision_conflict: t('automation.revisionConflict'),
          schedule_invalid: t('automation.scheduleInvalid'),
          target_invalid: t('automation.targetMissing')
        },
        {
          conversationId: validationMessages.chat,
          destination: validationMessages.selection,
          modelId: validationMessages.model,
          notificationPolicy: validationMessages.selection,
          permissionMode: validationMessages.selection,
          projectId: validationMessages.selection,
          prompt: validationMessages.prompt,
          schedule: t('automation.validationSchedule'),
          timezone: validationMessages.timezone,
          title: validationMessages.title
        }
      )
      if (presentation.field) setErrors({ [presentation.field]: presentation.message })
      setSubmitError(presentation.message)
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <form
      className="automation-form"
      aria-busy={submitting || undefined}
      onSubmit={(event) => {
        event.preventDefault()
        void submit()
      }}
    >
      <div className="automation-form__body">
        <label className="automation-form__text-field">
          <span>{t('automation.taskName')}</span>
          <input
            autoFocus={mode === 'create'}
            disabled={disabled || submitting}
            maxLength={512}
            value={draft.title}
            placeholder={t('automation.taskNamePlaceholder')}
            aria-invalid={Boolean(errors.title) || undefined}
            onChange={(event) => update({ title: event.currentTarget.value })}
          />
          {(errors.title || submitError) && errors.title && (
            <small role="alert">{errors.title}</small>
          )}
        </label>

        <label className="automation-form__text-field">
          <span>{t('automation.prompt')}</span>
          <textarea
            disabled={disabled || submitting}
            maxLength={65_536}
            rows={5}
            value={draft.prompt}
            placeholder={t('automation.promptPlaceholder')}
            aria-invalid={Boolean(errors.prompt) || undefined}
            onChange={(event) => update({ prompt: event.currentTarget.value })}
          />
          {errors.prompt && <small role="alert">{errors.prompt}</small>}
        </label>

        <section className="automation-form__section" aria-labelledby="automation-details-heading">
          <h3 id="automation-details-heading">{t('automation.details')}</h3>
          <div className="automation-form__card">
            <AutomationField label={t('automation.destination')}>
              <AutomationSelect
                ariaLabel={t('automation.destination')}
                disabled={disabled || submitting}
                options={[
                  { value: 'new_chat', label: t('automation.destinationNewChat') },
                  { value: 'existing_chat', label: t('automation.destinationExistingChat') }
                ]}
                value={draft.destination.kind}
                onChange={setDestinationKind}
              />
            </AutomationField>

            {newChatDestination ? (
              <>
                <AutomationField label={t('automation.project')} error={errors.projectId}>
                  <AutomationSelect
                    ariaLabel={t('automation.project')}
                    disabled={disabled || submitting}
                    options={projectOptions}
                    value={newChatDestination.projectId ?? '__none__'}
                    onChange={(projectId) =>
                      update({
                        destination: {
                          ...newChatDestination,
                          projectBinding: projectId === '__none__' ? 'none' : 'project',
                          projectId: projectId === '__none__' ? null : projectId
                        }
                      })
                    }
                  />
                </AutomationField>
                <AutomationField label={t('automation.model')} error={errors.modelId}>
                  <ModelConfigPicker
                    ariaLabel={`${t('automation.model')}: ${
                      modelOptions.find((option) => option.id === newChatDestination.modelId)
                        ?.label ?? t('chat.noEnabledModels')
                    }`}
                    className="automation-model-picker"
                    disabled={disabled || submitting}
                    emptyLabel={t('chat.noEnabledModels')}
                    onChange={(modelId) =>
                      update({ destination: { ...newChatDestination, modelId } })
                    }
                    options={modelOptions}
                    showSelectedCapability
                    value={newChatDestination.modelId}
                    variant="settings"
                  />
                </AutomationField>
                {mode === 'edit' && (
                  <AutomationField label={t('automation.reasoning')}>
                    <span
                      className="automation-form__readonly"
                      title={t('automation.reasoningFromModel')}
                    >
                      {reasoningProjection?.mode === 'disabled'
                        ? t('automation.reasoningNone')
                        : reasoningProjection?.effort === 'low'
                          ? t('automation.reasoningLow')
                          : reasoningProjection?.effort === 'max'
                            ? t('automation.reasoningMax')
                            : reasoningProjection?.effort === 'high'
                              ? t('automation.reasoningHigh')
                              : t('automation.reasoningFromModel')}
                    </span>
                  </AutomationField>
                )}
              </>
            ) : (
              <AutomationField label={t('automation.chat')} error={errors.conversationId}>
                <AutomationChatPicker
                  conversations={conversations}
                  disabled={disabled || submitting}
                  fallbackTitle={task?.targetSnapshot.conversationTitle}
                  onChange={(conversationId) => {
                    if (conversationId === null) {
                      setDestinationKind('new_chat')
                    } else {
                      update({ destination: { kind: 'existing_chat', conversationId } })
                    }
                  }}
                  projects={projects}
                  value={
                    draft.destination.kind === 'existing_chat'
                      ? draft.destination.conversationId || null
                      : null
                  }
                />
              </AutomationField>
            )}

            <AutomationField label={t('automation.permission')} error={errors.permissionMode}>
              <AutomationPermissionPicker
                availability={permissionModeAvailability}
                disabled={disabled || submitting}
                onChange={(permissionMode: ChatPermissionMode) => update({ permissionMode })}
                onOpenSettings={onOpenPermissionSettings}
                value={draft.permissionMode}
              />
            </AutomationField>
          </div>
        </section>

        <AutomationScheduleEditor
          disabled={disabled || submitting}
          errors={errors}
          onChange={updateSchedule}
          schedule={draft.schedule}
        />

        <section
          className="automation-form__section"
          aria-labelledby="automation-notification-heading"
        >
          <h3 id="automation-notification-heading">{t('automation.notification')}</h3>
          <div className="automation-form__card">
            <AutomationField label={t('automation.notification')}>
              <AutomationSelect
                ariaLabel={t('automation.notification')}
                disabled={disabled || submitting}
                options={notificationOptions}
                value={draft.notificationPolicy}
                onChange={(notificationPolicy) => update({ notificationPolicy })}
              />
            </AutomationField>
            <p className="automation-form__help">{notificationHelp}</p>
          </div>
        </section>

        {submitError && (
          <p className="automation-form__submit-error" role="alert">
            {submitError}
          </p>
        )}
      </div>

      <footer className="automation-form__footer">
        <button
          type="button"
          className="automation-button automation-button--secondary"
          disabled={disabled || submitting}
          onClick={onCancel}
        >
          {t('automation.cancel')}
        </button>
        <button
          type="submit"
          className="automation-button automation-button--primary"
          disabled={!canSubmit}
        >
          {submitting
            ? mode === 'create'
              ? t('automation.creating')
              : t('automation.saving')
            : mode === 'create'
              ? t('automation.createTask')
              : t('automation.save')}
        </button>
      </footer>
    </form>
  )
}
