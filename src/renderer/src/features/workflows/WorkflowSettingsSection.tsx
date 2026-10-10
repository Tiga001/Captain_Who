import {
  AlertTriangle,
  ArrowLeft,
  ChevronRight,
  Copy,
  Download,
  Network,
  Pencil,
  Plus,
  Redo2,
  RefreshCw,
  Trash2,
  Undo2,
  Upload
} from 'lucide-react'
import { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react'
import type {
  WorkflowDefinition,
  WorkflowRecord,
  WorkflowResponse,
  WorkflowEditingDraft,
  WorkflowInvalidRecord,
  WorkflowInvalidDraft
} from '@mycopilot/protocol'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { Tooltip } from '../../components/overlay/Tooltip'
import { ConfirmationDialog } from '../../components/dialog/ConfirmationDialog'
import { exportWorkflowTemplate, importWorkflowTemplate, requestWorkflows } from './workflowClient'
import {
  workflowOperationError,
  type WorkflowErrorOperation,
  type WorkflowOperationError
} from './workflowErrors'
import { createWorkflow } from './workflowAuthoring'
import { workflowText } from './workflowText'
import { WorkflowGraphEditor } from './WorkflowGraphEditor'
import { WorkflowIssues } from './WorkflowIssues'
import { renderSettingsNodes } from '../settings/settingsDefinition'
import { useSettingsPageNavigation } from '../settings/settingsSearchNavigation'
import {
  workflowLibrarySettings,
  workflowEditorSettings,
  workflowFormSettings
} from '../settings/pages/managementSettings.definition'
import { useWorkflowHistory } from './useWorkflowHistory'
import {
  workflowContentKey,
  type WorkflowChangeOptions,
  type WorkflowUpdate
} from './workflowHistory'
import './workflows.css'

type WorkflowDeleteTarget = {
  kind: 'template' | 'draft'
  id: string
  name: string
  revision: number
}

export function WorkflowSettingsSection({
  onEditorModeChange,
  onDirtyChange,
  onSavingChange
}: {
  onNavigateSettingsRoot?: () => void
  onEditorModeChange?: (editing: boolean) => void
  onDirtyChange?: (dirty: boolean) => void
  onSavingChange?: (saving: boolean) => void
}) {
  const { language } = useFrontendConfig()
  const text = useMemo(() => workflowText(language), [language])
  const history = useWorkflowHistory()
  const { draft, reset, change, undo, redo, checkpoint, canUndo, canRedo } = history
  const [records, setRecords] = useState<WorkflowRecord[]>([])
  const [invalidRecords, setInvalidRecords] = useState<WorkflowInvalidRecord[]>([])
  const [invalidDrafts, setInvalidDrafts] = useState<WorkflowInvalidDraft[]>([])
  const [editingDrafts, setEditingDrafts] = useState<WorkflowEditingDraft[]>([])
  const [draftRevision, setDraftRevision] = useState(0)
  const [draftNotice, setDraftNotice] = useState<'stashed' | 'restoredDraft' | null>(null)
  const [revision, setRevision] = useState(0)
  const [baseline, setBaseline] = useState('')
  const [tab, setTab] = useState<'details' | 'structure'>('details')
  const createButtonRef = useRef<HTMLButtonElement>(null)
  const saveButtonRef = useRef<HTMLButtonElement>(null)
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState(false)
  const busyRef = useRef(false)
  const [error, setError] = useState<WorkflowOperationError | null>(null)
  const [loadFailure, setLoadFailure] = useState(false)
  const [conflict, setConflict] = useState(false)
  const [saveIssues, setSaveIssues] = useState<WorkflowRecord | null>(null)
  const [editorHidden, setEditorHidden] = useState(false)
  const afterClose = useRef<(() => void) | undefined>(undefined)
  const [pendingDelete, setPendingDelete] = useState<WorkflowDeleteTarget | null>(null)
  const [confirmDiscard, setConfirmDiscard] = useState(false)
  const sequence = useRef(0)
  const tabId = useId()
  const editing = Boolean(draft && !editorHidden)
  const invalidEditingDraft = invalidDrafts.find((item) => item.id === draft?.id)
  const dirty = Boolean(draft && workflowContentKey(draft) !== baseline)
  useEffect(() => {
    onEditorModeChange?.(editing)
  }, [editing, onEditorModeChange])
  useEffect(() => {
    onDirtyChange?.(dirty)
  }, [dirty, onDirtyChange])
  useEffect(
    () => () => {
      onEditorModeChange?.(false)
      onDirtyChange?.(false)
      onSavingChange?.(false)
    },
    [onEditorModeChange, onDirtyChange, onSavingChange]
  )
  const reload = useCallback(async () => {
    const request = ++sequence.current
    setLoading(true)
    setLoadFailure(false)
    setError(null)
    setConflict(false)
    try {
      const result = await requestWorkflows({ operation: 'list' })
      if (request === sequence.current) {
        setRecords(result.records)
        setEditingDrafts(result.drafts ?? [])
        setInvalidRecords(result.invalidRecords ?? [])
        setInvalidDrafts(result.invalidDrafts ?? [])
      }
    } catch (loadError) {
      if (request === sequence.current) {
        setLoadFailure(true)
        setError(workflowOperationError(loadError, 'load'))
      }
    } finally {
      if (request === sequence.current) setLoading(false)
    }
  }, [])
  useEffect(() => {
    void reload()
    return () => {
      sequence.current += 1
    }
  }, [reload])

  const applyChange = useCallback(
    (update: WorkflowUpdate, options?: WorkflowChangeOptions) => {
      if (!busyRef.current) change(update, options)
    },
    [change]
  )
  const open = (
    record?: WorkflowRecord,
    options?: { storedDrafts?: WorkflowEditingDraft[]; showIssues?: boolean }
  ) => {
    if (draft && ((!record && revision === 0) || record?.definition.id === draft.id)) {
      setEditorHidden(false)
      return
    }
    const perform = () => {
      const storedDraft = record
        ? (options?.storedDrafts ?? editingDrafts).find(
            (item) => item.definition.id === record.definition.id
          )
        : undefined
      const definition = record
        ? structuredClone(storedDraft?.definition ?? record.definition)
        : createWorkflow()
      setBaseline(workflowContentKey(definition))
      reset(definition)
      setRevision(storedDraft?.baseRevision ?? record?.revision ?? 0)
      setDraftRevision(storedDraft?.revision ?? 0)
      setDraftNotice(storedDraft ? 'restoredDraft' : null)
      if (!loadFailure) {
        setError(null)
      }
      setConflict(false)
      setEditorHidden(false)
      setSaveIssues(options?.showIssues && record?.issues.length ? record : null)
      setTab('details')
    }
    if (dirty) {
      afterClose.current = perform
      setConfirmDiscard(true)
    } else perform()
  }
  useSettingsPageNavigation('workflows', (target) => {
    if (busyRef.current) return
    if (target.view === 'create') {
      open()
      return
    }
    if (target.view === 'workflows') setEditorHidden(true)
    if (target.view === 'editor') {
      setEditorHidden(false)
      setTab(target.id === 'workflow-structure' ? 'structure' : 'details')
    }
  })
  const close = (after?: () => void) => {
    if (busyRef.current) return
    afterClose.current = after
    if (dirty) setConfirmDiscard(true)
    else {
      reset(null)
      setEditorHidden(false)
      if (conflict) void reload()
      after?.()
    }
  }
  const setMutationBusy = (value: boolean) => {
    busyRef.current = value
    onSavingChange?.(value)
    setBusy(value)
  }
  const acceptResponse = (response: WorkflowResponse) => {
    setRecords(response.records)
    setEditingDrafts(response.drafts ?? [])
    setInvalidRecords(response.invalidRecords ?? [])
    setInvalidDrafts(response.invalidDrafts ?? [])
  }
  const handleSaveError = async (
    saveError: unknown,
    operation: WorkflowErrorOperation = 'save'
  ) => {
    setConflict(
      Boolean(
        saveError &&
        typeof saveError === 'object' &&
        'code' in saveError &&
        saveError.code === -32009
      )
    )
    setError(workflowOperationError(saveError, operation))
  }
  const publish = async (saving: WorkflowDefinition, expectedRevision: number) => {
    if (busyRef.current) return
    setMutationBusy(true)
    setLoadFailure(false)
    setError(null)
    try {
      const response = await requestWorkflows({
        operation: 'save',
        definition: saving,
        expectedRevision,
        ...(expectedRevision > 0 ? { expectedDraftRevision: draftRevision } : {})
      })
      const saved = response.records.find((record) => record.definition.id === saving.id)
      if (!saved) throw new Error('Saved organization is missing from response')
      acceptResponse(response)
      if (saving.id !== draft?.id) reset(saving)
      else checkpoint()
      setRevision(saved.revision)
      setBaseline(workflowContentKey(saving))
      setDraftRevision(0)
      setDraftNotice(null)
      setConflict(false)
      setSaveIssues(saved.issues.length ? saved : null)
    } catch (saveError) {
      await handleSaveError(saveError)
    } finally {
      setMutationBusy(false)
    }
  }
  const save = async () => {
    if (!draft || busyRef.current || conflict || invalidEditingDraft) return
    await publish(draft, revision)
  }
  const copyName = (name: string) => {
    const base = name || text('create')
    const suffix = ` (${text('copySuffix')})`
    let nameCopy = `${base.slice(0, 128 - suffix.length)}${suffix}`
    let number = 2
    while (records.some((record) => record.definition.name === nameCopy)) {
      const nextSuffix = ` (${text('copySuffix')} ${number++})`
      nameCopy = `${base.slice(0, 128 - nextSuffix.length)}${nextSuffix}`
    }
    return nameCopy
  }
  const saveCopy = async () => {
    if (!draft || busyRef.current) return
    await publish(
      { ...structuredClone(draft), id: crypto.randomUUID(), name: copyName(draft.name) },
      0
    )
  }
  const duplicate = async (record: WorkflowRecord) => {
    if (busyRef.current) return
    setMutationBusy(true)
    setLoadFailure(false)
    setError(null)
    try {
      acceptResponse(
        await requestWorkflows({
          operation: 'duplicate',
          id: record.definition.id,
          expectedRevision: record.revision,
          newId: crypto.randomUUID(),
          name: copyName(record.definition.name)
        })
      )
    } catch (saveError) {
      await handleSaveError(saveError, 'duplicate')
    } finally {
      setMutationBusy(false)
    }
  }
  const importTemplate = async () => {
    if (busyRef.current) return
    setMutationBusy(true)
    setError(null)
    try {
      const response = await importWorkflowTemplate()
      if (!response) return
      acceptResponse(response)
      setLoadFailure(false)
      const imported = response.records.find(
        (record) => record.definition.id === response.importedTemplateId
      )
      if (!imported) throw new Error('Imported organization is missing from response')
      open(imported, { storedDrafts: response.drafts ?? [], showIssues: true })
    } catch (importError) {
      await handleSaveError(importError, 'import')
    } finally {
      setMutationBusy(false)
    }
  }
  const exportTemplate = async (record: WorkflowRecord) => {
    if (busyRef.current) return
    setMutationBusy(true)
    setError(null)
    try {
      await exportWorkflowTemplate({
        id: record.definition.id,
        expectedRevision: record.revision,
        language
      })
    } catch (exportError) {
      await handleSaveError(exportError, 'export')
    } finally {
      setMutationBusy(false)
    }
  }
  const shortcutActions = useRef({ save, undo, redo, editing, busy, modal: false })
  useLayoutEffect(() => {
    shortcutActions.current = {
      save,
      undo,
      redo,
      editing,
      busy,
      modal: Boolean(error || saveIssues || pendingDelete || confirmDiscard)
    }
  })
  useEffect(() => {
    const keydown = (event: KeyboardEvent) => {
      const actions = shortcutActions.current
      if (
        !actions.editing ||
        actions.busy ||
        actions.modal ||
        document.querySelector('[aria-modal="true"]') ||
        event.defaultPrevented ||
        event.isComposing ||
        (!event.metaKey && !event.ctrlKey)
      )
        return
      const target = event.target instanceof Element ? event.target : null
      const typing = target?.closest('input, textarea, select, [contenteditable="true"]')
      if (event.key.toLowerCase() === 's') {
        event.preventDefault()
        void actions.save()
      } else if (!typing && event.key.toLowerCase() === 'z') {
        event.preventDefault()
        if (event.shiftKey) actions.redo()
        else actions.undo()
      } else if (!typing && event.key.toLowerCase() === 'y') {
        event.preventDefault()
        actions.redo()
      }
    }
    window.addEventListener('keydown', keydown)
    return () => window.removeEventListener('keydown', keydown)
  }, [])

  const nameError =
    error?.kind === 'duplicate_member_name'
      ? 'duplicateMemberName'
      : error?.kind === 'duplicate_department_name'
        ? 'duplicateDepartmentName'
        : error?.kind === 'department_name_separator'
          ? 'departmentNameSeparator'
          : null
  const configurationError = error?.operation === 'save' && error.kind === 'configuration'
  const transferError =
    error?.operation === 'import'
      ? error.kind === 'format'
        ? 'importFormat'
        : error.kind === 'too_large'
          ? 'importSize'
          : error.kind === 'file'
            ? 'importFile'
            : null
      : error?.operation === 'export' && error.kind === 'file'
        ? 'exportFile'
        : null
  const errorMessage = error ? (
    <ConfirmationDialog
      title={text(
        transferError
          ? `${transferError}ErrorTitle`
          : nameError
            ? `${nameError}Title`
            : configurationError
              ? 'configurationErrorTitle'
              : `${error.operation}ErrorTitle`
      )}
      description={text(
        conflict
          ? editing
            ? 'conflict'
            : 'conflictReload'
          : transferError
            ? `${transferError}ErrorHint`
            : nameError
              ? `${nameError}Hint`
              : configurationError
                ? 'configurationErrorHint'
                : `${error.operation}ErrorHint`
      )}
      dialogRole="alertdialog"
      cancelLabel={text('close')}
      confirmLabel={text('acknowledge')}
      confirmVariant="primary"
      showCancelButton={false}
      restoreFocusRef={editing && !conflict ? saveButtonRef : createButtonRef}
      onCancel={() => setError(null)}
      onConfirm={() => setError(null)}
    />
  ) : null

  return (
    <article
      className={`settings-list-page workflows-settings${editing ? ' workflows-settings--editing' : ''}`}
      aria-busy={busy || undefined}
    >
      {draft && editing ? (
        <>
          <header className="workflow-page-header">
            <div className="workflow-page-header__identity">
              <button
                className="workflow-icon-button"
                type="button"
                aria-label={text('back')}
                disabled={busy}
                onClick={() => close()}
              >
                <ArrowLeft aria-hidden="true" />
              </button>
              <button
                className="workflow-breadcrumb"
                type="button"
                disabled={busy}
                onClick={() => close()}
              >
                {text('title')}
              </button>
              <ChevronRight aria-hidden="true" />
              <h1>{draft.name || text(revision === 0 ? 'create' : 'edit')}</h1>
              {dirty ? (
                <span className="workflow-unsaved-dot" role="status" aria-label={text('unsaved')} />
              ) : null}
            </div>
            <div className="workflow-page-tabs" role="tablist" aria-label={text('edit')}>
              <button
                id={`${tabId}-details`}
                aria-controls={`${tabId}-details-panel`}
                className="workflow-tab"
                type="button"
                role="tab"
                aria-selected={tab === 'details'}
                onClick={() => setTab('details')}
              >
                {text('basicInfo')}
              </button>
              <button
                id={`${tabId}-structure`}
                aria-controls={`${tabId}-structure-panel`}
                className="workflow-tab"
                type="button"
                role="tab"
                aria-selected={tab === 'structure'}
                onClick={() => setTab('structure')}
              >
                {text('structureTab')}
              </button>
            </div>
            <div className="workflow-page-header__actions">
              {loadFailure ? (
                <button
                  className="workflow-button"
                  type="button"
                  disabled={loading || busy}
                  onClick={() => void reload()}
                >
                  <RefreshCw aria-hidden="true" />
                  {text('retry')}
                </button>
              ) : null}
              <button
                className="workflow-icon-button"
                type="button"
                aria-label={text('undo')}
                title={`${text('undo')} (⌘/Ctrl Z)`}
                disabled={busy || !canUndo}
                onClick={undo}
              >
                <Undo2 aria-hidden="true" />
              </button>
              <button
                className="workflow-icon-button"
                type="button"
                aria-label={text('redo')}
                title={`${text('redo')} (⌘/Ctrl Shift Z)`}
                disabled={busy || !canRedo}
                onClick={redo}
              >
                <Redo2 aria-hidden="true" />
              </button>
              {conflict || invalidEditingDraft ? (
                <button
                  className="secondary-settings-button"
                  type="button"
                  disabled={busy}
                  onClick={() => void saveCopy()}
                >
                  {text('saveCopy')}
                </button>
              ) : null}
              <button
                ref={saveButtonRef}
                className="primary-settings-button"
                type="button"
                aria-label={text('save')}
                disabled={busy || conflict || Boolean(invalidEditingDraft)}
                onClick={() => void save()}
              >
                {text('save')}
              </button>
            </div>
          </header>
          {invalidEditingDraft ? (
            <div className="workflow-draft-notice workflow-draft-notice--unavailable" role="status">
              <span>{text('unavailableDraftEditing')}</span>
              <button
                className="workflow-button"
                type="button"
                disabled={busy}
                onClick={() => setPendingDelete({ ...invalidEditingDraft, kind: 'draft' })}
              >
                <Trash2 aria-hidden="true" />
                {text('deleteDraft')}
              </button>
            </div>
          ) : null}
          {draftNotice ? (
            <p className="workflow-draft-notice" role="status">
              {text(draftNotice)}
            </p>
          ) : null}
          {renderSettingsNodes(workflowEditorSettings, () => (
            <div className="workflow-editing-content">
              <div
                id={`${tabId}-details-panel`}
                role="tabpanel"
                aria-labelledby={`${tabId}-details`}
                hidden={tab !== 'details'}
                className="workflow-details-panel"
              >
                <form
                  className="workflow-definition-form"
                  onSubmit={(event) => {
                    event.preventDefault()
                    void save()
                  }}
                >
                  <fieldset disabled={busy}>
                    {renderSettingsNodes(workflowFormSettings, (node) => {
                      switch (node.id) {
                        case 'workflow-name':
                          return (
                            <label>
                              <span>{text('name')}</span>
                              <input
                                autoFocus
                                maxLength={128}
                                placeholder={text('namePlaceholder')}
                                value={draft.name}
                                onChange={(event) =>
                                  applyChange(
                                    { ...draft, name: event.currentTarget.value },
                                    { group: 'name' }
                                  )
                                }
                              />
                            </label>
                          )
                        case 'workflow-description':
                          return (
                            <label>
                              <span>{text('description')}</span>
                              <textarea
                                rows={3}
                                maxLength={2048}
                                placeholder={text('descriptionPlaceholder')}
                                value={draft.description}
                                onChange={(event) =>
                                  applyChange(
                                    { ...draft, description: event.currentTarget.value },
                                    { group: 'description' }
                                  )
                                }
                              />
                            </label>
                          )
                        case 'workflow-background':
                          return (
                            <label>
                              <span>{text('background')}</span>
                              <textarea
                                rows={7}
                                maxLength={32768}
                                placeholder={text('backgroundPlaceholder')}
                                value={draft.background}
                                onChange={(event) =>
                                  applyChange(
                                    { ...draft, background: event.currentTarget.value },
                                    { group: 'background' }
                                  )
                                }
                              />
                            </label>
                          )
                        case 'workflow-structure':
                          return null
                      }
                    })}
                  </fieldset>
                </form>
              </div>
              <div
                id={`${tabId}-structure-panel`}
                role="tabpanel"
                aria-labelledby={`${tabId}-structure`}
                hidden={tab !== 'structure'}
                className="workflow-structure-panel"
                data-setting-id="workflow-structure"
                inert={busy || undefined}
              >
                <WorkflowGraphEditor definition={draft} text={text} onChange={applyChange} />
              </div>
            </div>
          ))}
        </>
      ) : (
        <>
          {renderSettingsNodes(workflowLibrarySettings, () => (
            <section className="settings-list-section workflows-settings__library">
              <div className="workflows-settings__heading">
                <h1>{text('title')}</h1>
                <div className="workflow-actions">
                  {loadFailure || conflict ? (
                    <button
                      className="workflow-button"
                      type="button"
                      disabled={loading || busy}
                      onClick={() => void reload()}
                    >
                      <RefreshCw aria-hidden="true" />
                      {text('retry')}
                    </button>
                  ) : null}
                  <button
                    className="workflow-button"
                    type="button"
                    onClick={() => void importTemplate()}
                    disabled={loading || busy}
                  >
                    <Upload aria-hidden="true" />
                    {text('import')}
                  </button>
                  <button
                    ref={createButtonRef}
                    className="workflow-button workflows-settings__create"
                    type="button"
                    onClick={() => open()}
                    disabled={loading || busy}
                  >
                    <Plus aria-hidden="true" />
                    {text('create')}
                  </button>
                </div>
              </div>
              {loading ? <p role="status">{text('loading')}</p> : null}
              {!loading &&
              !loadFailure &&
              records.length === 0 &&
              invalidRecords.length === 0 &&
              invalidDrafts.length === 0 ? (
                <div className="workflows-settings__empty">
                  <Network aria-hidden="true" />
                  <strong>{text('empty')}</strong>
                </div>
              ) : null}
              {records.map((record) => (
                <article key={record.definition.id} className="workflow-record">
                  <div>
                    <div className="workflow-record__title">
                      <strong>{record.definition.name || text('create')}</strong>
                      {record.issues.length > 0 ? <span>{text('draft')}</span> : null}
                      {editingDrafts.some((item) => item.definition.id === record.definition.id) ? (
                        <span>{text('pendingEdits')}</span>
                      ) : null}
                    </div>
                    {record.definition.description ? <p>{record.definition.description}</p> : null}
                  </div>
                  <div className="workflow-actions">
                    <button
                      className="workflow-button"
                      type="button"
                      aria-label={`${text('edit')} ${record.definition.name}`}
                      disabled={loading || busy}
                      onClick={() => open(record)}
                    >
                      <Pencil aria-hidden="true" />
                    </button>
                    <Tooltip content={text('copy')} preferredPlacement="top">
                      <button
                        className="workflow-button"
                        type="button"
                        aria-label={`${text('copy')} ${record.definition.name}`}
                        disabled={loading || busy}
                        onClick={() => void duplicate(record)}
                      >
                        <Copy aria-hidden="true" />
                      </button>
                    </Tooltip>
                    <Tooltip content={text('export')} preferredPlacement="top">
                      <button
                        className="workflow-button"
                        type="button"
                        aria-label={`${text('export')} ${record.definition.name}`}
                        disabled={loading || busy}
                        onClick={() => void exportTemplate(record)}
                      >
                        <Download aria-hidden="true" />
                      </button>
                    </Tooltip>
                    <button
                      className="workflow-button"
                      type="button"
                      aria-label={`${text('delete')} ${record.definition.name}`}
                      disabled={loading || busy}
                      onClick={() =>
                        setPendingDelete({
                          kind: 'template',
                          id: record.definition.id,
                          name: record.definition.name,
                          revision: record.revision
                        })
                      }
                    >
                      <Trash2 aria-hidden="true" />
                    </button>
                  </div>
                </article>
              ))}
              {[
                ...invalidRecords.map((record) => ({ ...record, kind: 'template' as const })),
                ...invalidDrafts.map((record) => ({ ...record, kind: 'draft' as const }))
              ].map((record) => (
                <article
                  key={`${record.kind}:${record.id}`}
                  className="workflow-record workflow-record--unavailable"
                  aria-label={`${text(record.kind === 'draft' ? 'unavailableDraft' : 'unavailableTemplate')} ${record.name || record.id}`}
                >
                  <div>
                    <div className="workflow-record__title">
                      <AlertTriangle aria-hidden="true" />
                      <strong>{record.name || record.id}</strong>
                      <span>
                        {text(record.kind === 'draft' ? 'unavailableDraft' : 'unavailableTemplate')}
                      </span>
                    </div>
                    <p>
                      {text(
                        record.reason === 'incompatible_definition'
                          ? record.kind === 'draft'
                            ? 'incompatibleDraftDescription'
                            : 'incompatibleTemplateDescription'
                          : record.kind === 'draft'
                            ? 'invalidDraftDescription'
                            : 'invalidTemplateDescription'
                      )}
                    </p>
                  </div>
                  <div className="workflow-actions">
                    <Tooltip
                      content={text(record.kind === 'draft' ? 'deleteDraft' : 'delete')}
                      preferredPlacement="top"
                    >
                      <button
                        className="workflow-button"
                        type="button"
                        aria-label={`${text(record.kind === 'draft' ? 'deleteDraft' : 'delete')} ${record.name || record.id}`}
                        disabled={loading || busy}
                        onClick={() => setPendingDelete(record)}
                      >
                        <Trash2 aria-hidden="true" />
                      </button>
                    </Tooltip>
                  </div>
                </article>
              ))}
            </section>
          ))}
        </>
      )}
      {errorMessage}
      {saveIssues ? (
        <WorkflowIssues
          graph={saveIssues.definition}
          issues={saveIssues.issues}
          text={text}
          onClose={() => setSaveIssues(null)}
          restoreFocusRef={editing ? saveButtonRef : createButtonRef}
        />
      ) : null}
      {confirmDiscard ? (
        <ConfirmationDialog
          title={text('discardTitle')}
          description={text('discardDescription')}
          cancelLabel={text('keep')}
          confirmLabel={text('discard')}
          onCancel={() => setConfirmDiscard(false)}
          onConfirm={() => {
            setConfirmDiscard(false)
            reset(null)
            setEditorHidden(false)
            if (conflict) void reload()
            afterClose.current?.()
          }}
        />
      ) : null}
      {pendingDelete ? (
        <ConfirmationDialog
          title={text(pendingDelete.kind === 'draft' ? 'deleteDraftTitle' : 'deleteTitle')}
          description={`${pendingDelete.name || pendingDelete.id}：${text(pendingDelete.kind === 'draft' ? 'deleteDraftDescription' : 'deleteDescription')}`}
          cancelLabel={text('cancel')}
          confirmLabel={text(pendingDelete.kind === 'draft' ? 'deleteDraft' : 'delete')}
          onCancel={() => setPendingDelete(null)}
          onConfirm={async () => {
            busyRef.current = true
            onSavingChange?.(true)
            setBusy(true)
            setLoadFailure(false)
            setError(null)
            setConflict(false)
            try {
              const response = await requestWorkflows(
                pendingDelete.kind === 'draft'
                  ? {
                      operation: 'deleteDraft',
                      id: pendingDelete.id,
                      expectedDraftRevision: pendingDelete.revision
                    }
                  : {
                      operation: 'delete',
                      id: pendingDelete.id,
                      expectedRevision: pendingDelete.revision
                    }
              )
              acceptResponse(response)
              setPendingDelete(null)
              setSaveIssues(null)
              if (pendingDelete.id === draft?.id) {
                if (pendingDelete.kind === 'template') reset(null)
                else {
                  setDraftRevision(0)
                  setDraftNotice(null)
                }
              }
            } catch (deleteError) {
              setPendingDelete(null)
              setConflict(
                Boolean(
                  deleteError &&
                  typeof deleteError === 'object' &&
                  'code' in deleteError &&
                  deleteError.code === -32009
                )
              )
              setError(workflowOperationError(deleteError, 'delete'))
            } finally {
              busyRef.current = false
              onSavingChange?.(false)
              setBusy(false)
            }
          }}
        />
      ) : null}
    </article>
  )
}
