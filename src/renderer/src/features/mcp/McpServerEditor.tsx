import { FolderOpen, Plus, Trash2 } from 'lucide-react'
import { useEffect, useId, useRef, useState } from 'react'
import { MCP_MANAGEMENT_LIMITS, type McpServerDetailsView } from '@mycopilot/protocol'
import { ConfirmationDialog } from '../../components/dialog/ConfirmationDialog'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { validateMcpDraft } from './mcpDraftValidation'
import type { McpServerDraft } from './mcpManagementInputs'

interface McpServerEditorProps {
  busy: boolean
  initial?: McpServerDetailsView
  onCancel: () => void
  onDirtyChange?: (dirty: boolean) => void
  onSelectExecutable: () => Promise<string | null>
  onSelectWorkingDirectory: () => Promise<string | null>
  onSubmit: (draft: McpServerDraft) => Promise<void>
}

export function McpServerEditor({
  busy,
  initial,
  onCancel,
  onDirtyChange,
  onSelectExecutable,
  onSelectWorkingDirectory,
  onSubmit
}: McpServerEditorProps) {
  const { t } = useFrontendConfig()
  const id = useId()
  const initialIdentity = initial
    ? `${initial.serverId}:${initial.configEpoch}:${initial.configDigest}`
    : 'new'
  const [baseline, setBaseline] = useState(() => ({
    draft: draftFromServer(initial),
    identity: initialIdentity
  }))
  const initialDraft = baseline.draft
  const [draft, setDraft] = useState<McpServerDraft>(initialDraft)
  const [errors, setErrors] = useState<readonly string[]>([])
  const [showDiscard, setShowDiscard] = useState(false)
  const cancelButtonRef = useRef<HTMLButtonElement | null>(null)
  const dirty = !draftsEqual(initialDraft, draft)
  const launchChanged = Boolean(initial && launchFieldsChanged(initialDraft, draft))

  useEffect(() => {
    if (baseline.identity === initialIdentity) return
    const next = draftFromServer(initial)
    setBaseline({ draft: next, identity: initialIdentity })
    setDraft(next)
    setErrors([])
  }, [baseline.identity, initial, initialIdentity])

  useEffect(() => {
    onDirtyChange?.(dirty)
  }, [dirty, onDirtyChange])

  useEffect(() => () => onDirtyChange?.(false), [onDirtyChange])

  const submit = async () => {
    if (busy) return
    const validationErrors = validateMcpDraft(draft, t)
    setErrors(validationErrors)
    if (validationErrors.length > 0) return
    await onSubmit(draft)
  }

  const chooseExecutable = async () => {
    try {
      const selected = await onSelectExecutable()
      if (selected !== null) setDraft((current) => ({ ...current, executable: selected }))
    } catch {
      setErrors([t('mcp.form.pickerFailed')])
    }
  }

  const chooseWorkingDirectory = async () => {
    try {
      const selected = await onSelectWorkingDirectory()
      if (selected !== null) setDraft((current) => ({ ...current, cwd: selected }))
    } catch {
      setErrors([t('mcp.form.pickerFailed')])
    }
  }

  const requestCancel = () => {
    if (!dirty) {
      onCancel()
      return
    }
    setShowDiscard(true)
  }

  return (
    <section aria-busy={busy || undefined} className="mcp-editor">
      <div className="mcp-editor__field">
        <label htmlFor={`${id}-name`}>{t('mcp.form.name')}</label>
        <input
          autoComplete="off"
          id={`${id}-name`}
          maxLength={MCP_MANAGEMENT_LIMITS.displayNameBytes}
          onChange={(event) => {
            const displayName = event.currentTarget.value
            setDraft((current) => ({ ...current, displayName }))
          }}
          value={draft.displayName}
        />
      </div>

      <div className="mcp-editor__field">
        <span className="mcp-editor__label">{t('mcp.form.transport')}</span>
        <div className="mcp-readonly-value">STDIO · {t('mcp.form.localProcess')}</div>
      </div>

      <div className="mcp-editor__field">
        <label htmlFor={`${id}-executable`}>{t('mcp.form.executable')}</label>
        <div className="mcp-path-field">
          <input
            autoComplete="off"
            id={`${id}-executable`}
            onChange={(event) => {
              const executable = event.currentTarget.value
              setDraft((current) => ({ ...current, executable }))
            }}
            spellCheck={false}
            value={draft.executable}
          />
          <button
            aria-label={t('mcp.form.chooseExecutable')}
            disabled={busy}
            onClick={() => void chooseExecutable()}
            type="button"
          >
            <FolderOpen aria-hidden="true" />
          </button>
        </div>
      </div>

      <fieldset className="mcp-arguments-field">
        <legend>{t('mcp.form.arguments')}</legend>
        <p>{t('mcp.form.argumentsHelp')}</p>
        <div className="mcp-argument-list">
          {draft.arguments.map((argument, index) => (
            <div className="mcp-argument-row" key={index}>
              <label className="mcp-visually-hidden" htmlFor={`${id}-argument-${index}`}>
                {t('mcp.form.argument')} {index + 1}
              </label>
              <input
                id={`${id}-argument-${index}`}
                onChange={(event) => {
                  const value = event.currentTarget.value
                  setDraft((current) => ({
                    ...current,
                    arguments: current.arguments.map((item, itemIndex) =>
                      itemIndex === index ? value : item
                    )
                  }))
                }}
                spellCheck={false}
                value={argument}
              />
              <button
                aria-label={`${t('mcp.form.removeArgument')} ${index + 1}`}
                disabled={busy}
                onClick={() =>
                  setDraft((current) => ({
                    ...current,
                    arguments: current.arguments.filter((_, itemIndex) => itemIndex !== index)
                  }))
                }
                type="button"
              >
                <Trash2 aria-hidden="true" />
              </button>
            </div>
          ))}
        </div>
        <button
          className="mcp-add-argument"
          disabled={busy || draft.arguments.length >= MCP_MANAGEMENT_LIMITS.arguments}
          onClick={() =>
            setDraft((current) => ({ ...current, arguments: [...current.arguments, ''] }))
          }
          type="button"
        >
          <Plus aria-hidden="true" />
          {t('mcp.form.addArgument')}
        </button>
      </fieldset>

      <div className="mcp-editor__field">
        <label htmlFor={`${id}-cwd`}>{t('mcp.form.cwd')}</label>
        <div className="mcp-path-field">
          <input
            autoComplete="off"
            id={`${id}-cwd`}
            onChange={(event) => {
              const cwd = event.currentTarget.value
              setDraft((current) => ({ ...current, cwd }))
            }}
            spellCheck={false}
            value={draft.cwd}
          />
          <button
            aria-label={t('mcp.form.chooseCwd')}
            disabled={busy}
            onClick={() => void chooseWorkingDirectory()}
            type="button"
          >
            <FolderOpen aria-hidden="true" />
          </button>
        </div>
        <small>{t('mcp.form.cwdHelp')}</small>
      </div>

      <div className="mcp-editor__field">
        <label htmlFor={`${id}-approval`}>{t('mcp.form.approvalMode')}</label>
        <select
          id={`${id}-approval`}
          onChange={(event) => {
            const approvalMode = event.currentTarget.value === 'deny' ? 'deny' : 'prompt'
            setDraft((current) => ({ ...current, approvalMode }))
          }}
          value={draft.approvalMode}
        >
          <option value="prompt">{t('mcp.form.promptEveryCall')}</option>
          <option value="deny">{t('mcp.form.denyCalls')}</option>
        </select>
      </div>

      <div className="mcp-security-notice" role="status">
        <strong>{t('mcp.form.securityTitle')}</strong>
        <p>{t('mcp.form.persistenceWarning')}</p>
        <p>{t('mcp.form.noSecretsWarning')}</p>
      </div>

      {launchChanged && (
        <div className="mcp-reauthorization-warning" role="alert">
          {t('mcp.form.launchChangedWarning')}
        </div>
      )}

      {errors.length > 0 && (
        <div className="mcp-editor__errors" role="alert">
          <strong>{t('mcp.form.validationFailed')}</strong>
          {errors.map((error) => (
            <p key={error}>{error}</p>
          ))}
        </div>
      )}

      <div className="mcp-editor__actions">
        <button
          className="mcp-secondary-button"
          disabled={busy}
          onClick={requestCancel}
          ref={cancelButtonRef}
          type="button"
        >
          {t('mcp.actions.cancel')}
        </button>
        <button
          className="mcp-primary-button"
          disabled={busy}
          onClick={() => void submit()}
          type="button"
        >
          {initial ? t('mcp.actions.saveChanges') : t('mcp.actions.saveServer')}
        </button>
      </div>

      {showDiscard && (
        <ConfirmationDialog
          cancelLabel={t('mcp.actions.keepEditing')}
          confirmLabel={t('mcp.actions.discard')}
          description={t('mcp.form.discardDescription')}
          onCancel={() => {
            setShowDiscard(false)
            requestAnimationFrame(() => cancelButtonRef.current?.focus())
          }}
          onConfirm={() => {
            setShowDiscard(false)
            onCancel()
          }}
          title={t('mcp.form.discardTitle')}
        />
      )}
    </section>
  )
}

function draftFromServer(server?: McpServerDetailsView): McpServerDraft {
  return {
    approvalMode: server?.approvalMode ?? 'prompt',
    arguments: server ? [...server.arguments] : [],
    cwd: server?.cwd ?? '',
    displayName: server?.displayName ?? '',
    executable: server?.executable ?? ''
  }
}

function draftsEqual(left: McpServerDraft, right: McpServerDraft): boolean {
  return (
    left.approvalMode === right.approvalMode &&
    left.cwd === right.cwd &&
    left.displayName === right.displayName &&
    left.executable === right.executable &&
    left.arguments.length === right.arguments.length &&
    left.arguments.every((argument, index) => argument === right.arguments[index])
  )
}

function launchFieldsChanged(left: McpServerDraft, right: McpServerDraft): boolean {
  return (
    left.cwd !== right.cwd ||
    left.executable !== right.executable ||
    left.arguments.length !== right.arguments.length ||
    left.arguments.some((argument, index) => argument !== right.arguments[index])
  )
}
