import {
  ArrowLeft,
  ChevronRight,
  GitBranch,
  Pencil,
  Plus,
  Redo2,
  RefreshCw,
  Trash2,
  Undo2
} from 'lucide-react'
import { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react'
import type { AgentTemplate, WorkflowRecord } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { ConfirmationDialog } from '../../components/dialog/ConfirmationDialog'
import { requestWorkflows } from './workflowClient'
import {
  createWorkflow,
  nameUnnamedWorkflowFlows,
  nameUnnamedWorkflowGates
} from './workflowAuthoring'
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

export function WorkflowSettingsSection({
  templates,
  onEditorModeChange,
  onDirtyChange,
  onSavingChange
}: {
  templates: readonly AgentTemplate[]
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
  const [revision, setRevision] = useState(0)
  const [baseline, setBaseline] = useState('')
  const [tab, setTab] = useState<'details' | 'structure'>('details')
  const createButtonRef = useRef<HTMLButtonElement>(null)
  const saveButtonRef = useRef<HTMLButtonElement>(null)
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState(false)
  const busyRef = useRef(false)
  const [error, setError] = useState(false)
  const [conflict, setConflict] = useState(false)
  const [saveIssues, setSaveIssues] = useState<WorkflowRecord | null>(null)
  const [editorHidden, setEditorHidden] = useState(false)
  const afterClose = useRef<(() => void) | undefined>(undefined)
  const [pendingDelete, setPendingDelete] = useState<WorkflowRecord | null>(null)
  const [confirmDiscard, setConfirmDiscard] = useState(false)
  const sequence = useRef(0)
  const tabId = useId()
  const editing = Boolean(draft && !editorHidden)
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
  useSettingsPageNavigation('workflows', (target) => {
    if (busyRef.current) return
    if (target.view === 'workflows') setEditorHidden(true)
    if (target.view === 'editor') {
      setEditorHidden(false)
      setTab(target.id === 'workflow-structure' ? 'structure' : 'details')
    }
  })
  const reload = useCallback(async () => {
    const request = ++sequence.current
    setLoading(true)
    setError(false)
    setConflict(false)
    try {
      const result = await requestWorkflows({ operation: 'list' })
      if (request === sequence.current) setRecords(result.records)
    } catch {
      if (request === sequence.current) setError(true)
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
  const open = (record?: WorkflowRecord) => {
    if (draft && ((!record && revision === 0) || record?.definition.id === draft.id)) {
      setEditorHidden(false)
      return
    }
    const perform = () => {
      const definition = record
        ? nameUnnamedWorkflowGates(nameUnnamedWorkflowFlows(structuredClone(record.definition)))
        : createWorkflow()
      setBaseline(workflowContentKey(definition))
      reset(definition)
      setRevision(record?.revision ?? 0)
      setError(false)
      setConflict(false)
      setEditorHidden(false)
      setSaveIssues(null)
      setTab('details')
    }
    if (dirty) {
      afterClose.current = perform
      setConfirmDiscard(true)
    } else perform()
  }
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
  const save = async () => {
    if (!draft || busyRef.current || conflict) return
    busyRef.current = true
    onSavingChange?.(true)
    setBusy(true)
    setError(false)
    const saving = draft
    try {
      const response = await requestWorkflows({
        operation: 'save',
        definition: saving,
        expectedRevision: revision
      })
      const saved = response.records.find((record) => record.definition.id === saving.id)
      if (!saved) throw new Error('Saved workflow is missing from response')
      setRecords(response.records)
      checkpoint()
      setRevision(saved.revision)
      setBaseline(workflowContentKey(saving))
      setSaveIssues(saved.issues.length ? saved : null)
    } catch (saveError) {
      setConflict(
        Boolean(
          saveError &&
          typeof saveError === 'object' &&
          'code' in saveError &&
          saveError.code === -32009
        )
      )
      setError(true)
    } finally {
      busyRef.current = false
      onSavingChange?.(false)
      setBusy(false)
    }
  }
  const setEnabled = async (record: WorkflowRecord) => {
    if (
      busyRef.current ||
      loading ||
      conflict ||
      record.issues.length > 0 ||
      typeof record.enabled !== 'boolean'
    )
      return
    busyRef.current = true
    onSavingChange?.(true)
    setBusy(true)
    setError(false)
    try {
      const response = await requestWorkflows({
        operation: 'setEnabled',
        id: record.definition.id,
        enabled: !record.enabled,
        expectedRevision: record.revision
      })
      const updated = response.records.find((item) => item.definition.id === record.definition.id)
      if (!updated) throw new Error('Updated workflow is missing from response')
      setRecords(response.records)
      // Enabling changes only saved metadata. Keep a hidden editing draft intact,
      // while advancing its revision only when it started from this saved record.
      if (draft?.id === record.definition.id)
        setRevision((current) => (current === record.revision ? updated.revision : current))
    } catch (enableError) {
      setConflict(
        Boolean(
          enableError &&
          typeof enableError === 'object' &&
          'code' in enableError &&
          enableError.code === -32009
        )
      )
      setError(true)
    } finally {
      busyRef.current = false
      onSavingChange?.(false)
      setBusy(false)
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
      modal: Boolean(saveIssues || pendingDelete || confirmDiscard)
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

  const errorMessage = error ? (
    <div className="workflow-error workflow-operation-error" role="alert">
      {text(conflict ? (editing ? 'conflict' : 'conflictReload') : 'error')}
      {!editing ? (
        <button className="workflow-button" onClick={() => void reload()} type="button">
          <RefreshCw aria-hidden="true" />
          {text('retry')}
        </button>
      ) : null}
    </div>
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
              <button
                ref={saveButtonRef}
                className="primary-settings-button"
                type="button"
                aria-label={text('save')}
                disabled={busy || conflict}
                onClick={() => void save()}
              >
                {text('save')}
              </button>
            </div>
          </header>
          {errorMessage}
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
                <WorkflowGraphEditor
                  definition={draft}
                  templates={templates}
                  text={text}
                  onChange={applyChange}
                />
              </div>
            </div>
          ))}
        </>
      ) : (
        <>
          {errorMessage}
          {renderSettingsNodes(workflowLibrarySettings, () => (
            <section className="settings-list-section workflows-settings__library">
              <div className="workflows-settings__heading">
                <h1>{text('title')}</h1>
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
              {loading ? <p role="status">{text('loading')}</p> : null}
              {!loading && !error && records.length === 0 ? (
                <div className="workflows-settings__empty">
                  <GitBranch aria-hidden="true" />
                  <strong>{text('empty')}</strong>
                </div>
              ) : null}
              {records.map((record) => (
                <article key={record.definition.id} className="workflow-record">
                  <div>
                    <div className="workflow-record__title">
                      <strong>{record.definition.name || text('create')}</strong>
                      {record.issues.length > 0 ? <span>{text('draft')}</span> : null}
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
                    <button
                      className="workflow-button"
                      type="button"
                      aria-label={`${text('delete')} ${record.definition.name}`}
                      disabled={loading || busy}
                      onClick={() => setPendingDelete(record)}
                    >
                      <Trash2 aria-hidden="true" />
                    </button>
                    <button
                      className="settings-switch workflow-record__enabled"
                      type="button"
                      role="switch"
                      aria-label={`${text('enable')} ${record.definition.name || text('create')}`}
                      aria-checked={record.issues.length === 0 && Boolean(record.enabled)}
                      data-state={record.issues.length === 0 && record.enabled ? 'on' : 'off'}
                      title={
                        typeof record.enabled !== 'boolean'
                          ? text('enableRequiresRestart')
                          : record.issues.length > 0
                            ? text('enableRequiresValid')
                            : undefined
                      }
                      disabled={
                        loading ||
                        busy ||
                        conflict ||
                        record.issues.length > 0 ||
                        typeof record.enabled !== 'boolean'
                      }
                      onClick={() => void setEnabled(record)}
                    >
                      <span className="settings-switch__thumb" aria-hidden="true" />
                    </button>
                  </div>
                </article>
              ))}
            </section>
          ))}
        </>
      )}
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
          title={text('deleteTitle')}
          description={text('deleteDescription')}
          cancelLabel={text('cancel')}
          confirmLabel={text('delete')}
          onCancel={() => setPendingDelete(null)}
          onConfirm={async () => {
            busyRef.current = true
            onSavingChange?.(true)
            setBusy(true)
            setError(false)
            setConflict(false)
            try {
              const response = await requestWorkflows({
                operation: 'delete',
                id: pendingDelete.definition.id,
                expectedRevision: pendingDelete.revision
              })
              setRecords(response.records)
              setPendingDelete(null)
              setSaveIssues(null)
              if (pendingDelete.definition.id === draft?.id) reset(null)
            } catch (deleteError) {
              setConflict(
                Boolean(
                  deleteError &&
                  typeof deleteError === 'object' &&
                  'code' in deleteError &&
                  deleteError.code === -32009
                )
              )
              setError(true)
              setPendingDelete(null)
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
