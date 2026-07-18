// Renderer skills management UI: presents source selection and frozen installation previews.
import { useEffect, useId, useRef, useState, type KeyboardEvent, type ReactNode } from 'react'
import { AlertTriangle, FolderOpen, GitFork, LoaderCircle, X } from 'lucide-react'
import { createPortal } from 'react-dom'
import type { SkillGitHubReference, SkillInstallationPreview } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { SettingsSelect } from '../../settings/components/SettingsSelect'
import type { GitHubReferenceKind } from './skillInstallationSource'
import type { useSkillInstallationWorkflow } from './useSkillInstallationWorkflow'
import './SkillInstallationDialog.css'

interface SkillInstallationDialogProps {
  workflow: ReturnType<typeof useSkillInstallationWorkflow>
}

export function SkillInstallationDialog({ workflow }: SkillInstallationDialogProps) {
  const { t } = useFrontendConfig()
  const state = workflow.state
  const dialogRef = useRef<HTMLElement>(null)
  const titleId = useId()
  const descriptionId = useId()
  const [now, setNow] = useState(0)
  const isOpen = state.status !== 'idle'
  const isBusy = state.status === 'inspecting' || state.status === 'committing'
  const isCommitting = state.status === 'committing'

  useEffect(() => {
    if (state.status !== 'preview' && state.status !== 'committing') return undefined
    setNow(Date.now())
    const interval = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(interval)
  }, [state.status])

  useEffect(() => {
    if (!isOpen) return undefined
    const previousFocus =
      document.activeElement instanceof HTMLElement ? document.activeElement : null
    const frame = window.requestAnimationFrame(() => {
      getFocusableElements(dialogRef.current)[0]?.focus({ preventScroll: true })
    })
    return () => {
      window.cancelAnimationFrame(frame)
      if (previousFocus?.isConnected) previousFocus.focus({ preventScroll: true })
    }
  }, [isOpen])

  if (state.status === 'idle') return null

  const operationLabel =
    state.context.operation === 'install'
      ? t('skills.installOperation')
      : t('skills.updateOperation')
  const title =
    state.context.operation === 'install'
      ? t('skills.installDialogTitle')
      : replaceTokens(t('skills.updateDialogTitle'), {
          name: state.context.entry.name
        })

  const handleKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === 'Escape' && !isCommitting) {
      event.preventDefault()
      workflow.close()
      return
    }
    if (event.key !== 'Tab') return
    const focusable = getFocusableElements(dialogRef.current)
    if (focusable.length === 0) return
    const first = focusable[0]
    const last = focusable.at(-1)
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault()
      last?.focus()
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault()
      first?.focus()
    }
  }

  return createPortal(
    <div
      className="skill-install-dialog__backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (!isCommitting && event.currentTarget === event.target) workflow.close()
      }}
    >
      <section
        aria-busy={isBusy || undefined}
        aria-describedby={descriptionId}
        aria-labelledby={titleId}
        aria-modal="true"
        className="skill-install-dialog"
        onKeyDown={handleKeyDown}
        ref={dialogRef}
        role="dialog"
      >
        <header className="skill-install-dialog__header">
          <div>
            <h2 id={titleId}>{title}</h2>
            <p id={descriptionId}>{t('skills.installDialogDescription')}</p>
          </div>
          <button
            aria-label={t('skills.closeDialog')}
            className="skill-install-dialog__close"
            disabled={isCommitting}
            onClick={workflow.close}
            type="button"
          >
            <X aria-hidden="true" />
          </button>
        </header>

        <div className="skill-install-dialog__body">
          {state.status === 'choosingSource' && (
            <div className="skill-source-options">
              <button type="button" onClick={() => void workflow.chooseLocalDirectory()}>
                <FolderOpen aria-hidden="true" />
                <span>
                  <strong>{t('skills.installLocal')}</strong>
                  <small>{t('skills.installLocalDescription')}</small>
                </span>
              </button>
              <button type="button" onClick={workflow.chooseGitHub}>
                <GitFork aria-hidden="true" />
                <span>
                  <strong>{t('skills.installGitHub')}</strong>
                  <small>{t('skills.installGitHubDescription')}</small>
                </span>
              </button>
            </div>
          )}

          {state.status === 'githubSource' && (
            <form
              className="skill-github-form"
              onSubmit={(event) => {
                event.preventDefault()
                void workflow.inspectGitHub()
              }}
            >
              <label>
                <span>{t('skills.githubRepository')}</span>
                <input
                  aria-invalid={state.fieldError === 'repository' || undefined}
                  autoFocus
                  onChange={(event) =>
                    workflow.updateGitHubForm({ repository: event.target.value })
                  }
                  placeholder={t('skills.githubRepositoryPlaceholder')}
                  value={state.form.repository}
                />
                {state.fieldError === 'repository' && (
                  <small className="skill-form-error">{t('skills.invalidGitHubRepository')}</small>
                )}
              </label>
              <div className="skill-github-form__field">
                <span>{t('skills.githubReferenceType')}</span>
                <SettingsSelect<GitHubReferenceKind>
                  ariaLabel={t('skills.githubReferenceType')}
                  className="skill-github-reference-select"
                  onChange={(referenceKind) =>
                    workflow.updateGitHubForm({
                      referenceKind,
                      referenceValue: ''
                    })
                  }
                  options={[
                    { label: t('skills.githubDefaultBranch'), value: 'defaultBranch' },
                    { label: t('skills.githubNamedReference'), value: 'named' },
                    { label: t('skills.githubCommit'), value: 'commit' }
                  ]}
                  value={state.form.referenceKind}
                />
              </div>
              {state.form.referenceKind !== 'defaultBranch' && (
                <label>
                  <span>{t('skills.githubReferenceValue')}</span>
                  <input
                    aria-invalid={state.fieldError === 'reference' || undefined}
                    onChange={(event) =>
                      workflow.updateGitHubForm({ referenceValue: event.target.value })
                    }
                    placeholder={
                      state.form.referenceKind === 'commit'
                        ? t('skills.githubCommitPlaceholder')
                        : t('skills.githubNamedReferencePlaceholder')
                    }
                    value={state.form.referenceValue}
                  />
                  {state.fieldError === 'reference' && (
                    <small className="skill-form-error">{t('skills.invalidGitHubReference')}</small>
                  )}
                </label>
              )}
              <label>
                <span>{t('skills.githubSubdirectory')}</span>
                <input
                  onChange={(event) =>
                    workflow.updateGitHubForm({ subdirectory: event.target.value })
                  }
                  placeholder={t('skills.githubSubdirectoryPlaceholder')}
                  value={state.form.subdirectory}
                />
              </label>
              <DialogActions>
                <button
                  className="skill-dialog-button"
                  onClick={workflow.backToSource}
                  type="button"
                >
                  {t('skills.back')}
                </button>
                <button className="skill-dialog-button skill-dialog-button--primary" type="submit">
                  {t('skills.inspect')}
                </button>
              </DialogActions>
            </form>
          )}

          {state.status === 'inspecting' && (
            <div className="skill-dialog-progress" role="status">
              <LoaderCircle aria-hidden="true" />
              <span>{t('skills.inspecting')}</span>
            </div>
          )}

          {(state.status === 'preview' || state.status === 'committing') && (
            <SkillPreview
              acceptedIssueIds={state.acceptedIssueIds}
              committing={state.status === 'committing'}
              errorMessage={state.status === 'preview' ? state.errorMessage : null}
              now={now}
              operationLabel={operationLabel}
              onBack={workflow.backToSource}
              onCommit={() => void workflow.commit()}
              onToggleAcknowledgement={workflow.toggleAcknowledgement}
              preview={state.preview}
            />
          )}

          {state.status === 'error' && (
            <div className="skill-dialog-error" role="alert">
              <AlertTriangle aria-hidden="true" />
              <div>
                <strong>
                  {state.expired ? t('skills.previewExpiredTitle') : t('skills.operationFailed')}
                </strong>
                <p>{state.expired ? t('skills.previewExpiredDescription') : state.message}</p>
              </div>
              <DialogActions>
                <button className="skill-dialog-button" onClick={workflow.close} type="button">
                  {t('skills.cancel')}
                </button>
                <button
                  className="skill-dialog-button skill-dialog-button--primary"
                  onClick={
                    state.retry ? () => void workflow.retryInspection() : workflow.backToSource
                  }
                  type="button"
                >
                  {state.retry
                    ? state.expired
                      ? t('skills.inspectAgain')
                      : t('skills.retryInspection')
                    : t('skills.chooseSourceAgain')}
                </button>
              </DialogActions>
            </div>
          )}
        </div>
      </section>
    </div>,
    document.body
  )
}

function SkillPreview({
  acceptedIssueIds,
  committing,
  errorMessage,
  now,
  onBack,
  onCommit,
  onToggleAcknowledgement,
  operationLabel,
  preview
}: {
  acceptedIssueIds: readonly string[]
  committing: boolean
  errorMessage: string | null
  now: number
  onBack: () => void
  onCommit: () => void
  onToggleAcknowledgement: (issueId: string) => void
  operationLabel: string
  preview: SkillInstallationPreview
}) {
  const { t } = useFrontendConfig()
  const expired = now >= preview.expiresAtUnixMs
  const requiredIssues = preview.compatibility.issues.filter(
    (issue) => issue.requiresAcknowledgement
  )
  const acknowledged = requiredIssues.every((issue) => acceptedIssueIds.includes(issue.id))
  const incompatible = preview.compatibility.status === 'incompatible'

  return (
    <div className="skill-install-preview">
      <div className="skill-install-preview__identity">
        <span className="skill-install-preview__operation">{operationLabel}</span>
        <h3>{preview.package.name}</h3>
        <p>{preview.package.description}</p>
      </div>

      <dl className="skill-install-preview__facts">
        <div>
          <dt>{t('skills.previewSource')}</dt>
          <dd>{formatPreviewSource(preview, t)}</dd>
        </div>
        <div>
          <dt>{t('skills.previewFiles')}</dt>
          <dd>
            {replaceTokens(t('skills.previewFilesValue'), { count: preview.package.fileCount })}
          </dd>
        </div>
        <div>
          <dt>{t('skills.previewSize')}</dt>
          <dd>{formatBytes(preview.package.totalBytes)}</dd>
        </div>
        <div>
          <dt>{t('skills.previewFormat')}</dt>
          <dd>v{preview.package.formatVersion}</dd>
        </div>
        <div>
          <dt>{t('skills.previewChanges')}</dt>
          <dd>
            {replaceTokens(t('skills.previewChangesValue'), {
              content: t(`skills.change.${preview.changes.content}`),
              source: t(`skills.change.${preview.changes.source}`)
            })}
          </dd>
        </div>
        <div>
          <dt>{t('skills.previewExpires')}</dt>
          <dd>{new Date(preview.expiresAtUnixMs).toLocaleTimeString()}</dd>
        </div>
      </dl>

      <div className="skill-compatibility" data-status={preview.compatibility.status}>
        <strong>{t(`skills.compatibility.${preview.compatibility.status}`)}</strong>
        {preview.compatibility.issues.length > 0 && (
          <ul>
            {preview.compatibility.issues.map((issue) => (
              <li key={issue.id} data-severity={issue.severity}>
                {issue.requiresAcknowledgement ? (
                  <label>
                    <input
                      checked={acceptedIssueIds.includes(issue.id)}
                      disabled={committing}
                      onChange={() => onToggleAcknowledgement(issue.id)}
                      type="checkbox"
                    />
                    <span>{issue.message}</span>
                  </label>
                ) : (
                  <span>{issue.message}</span>
                )}
              </li>
            ))}
          </ul>
        )}
      </div>

      {expired && (
        <p className="skill-preview-expired" role="alert">
          {t('skills.previewExpiredDescription')}
        </p>
      )}
      {errorMessage && (
        <p className="skill-form-error" role="alert">
          {errorMessage}
        </p>
      )}

      <DialogActions>
        <button
          className="skill-dialog-button"
          disabled={committing}
          onClick={onBack}
          type="button"
        >
          {t('skills.back')}
        </button>
        <button
          className="skill-dialog-button skill-dialog-button--primary"
          disabled={committing || expired || incompatible || !acknowledged}
          onClick={onCommit}
          type="button"
        >
          {committing ? t('skills.committing') : t('skills.confirmInstallation')}
        </button>
      </DialogActions>
    </div>
  )
}

function DialogActions({ children }: { children: ReactNode }) {
  return <div className="skill-install-dialog__actions">{children}</div>
}

function formatPreviewSource(
  preview: SkillInstallationPreview,
  t: ReturnType<typeof useFrontendConfig>['t']
): string {
  const source = preview.source
  if (source.kind === 'localDirectory' || source.kind === 'installedSource') {
    return source.displayName
  }
  return `${source.owner}/${source.repository} · ${formatGitHubReference(source.reference, t)}${
    source.subdirectory ? ` · ${source.subdirectory}` : ''
  }`
}

function formatGitHubReference(
  reference: SkillGitHubReference,
  t: ReturnType<typeof useFrontendConfig>['t']
): string {
  if (reference.kind === 'defaultBranch') return t('skills.githubDefaultBranch')
  if (reference.kind === 'commit') return reference.sha
  return reference.value
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

function replaceTokens(template: string, values: Record<string, string | number>): string {
  return Object.entries(values).reduce(
    (current, [key, value]) => current.replaceAll(`{${key}}`, String(value)),
    template
  )
}

function getFocusableElements(container: HTMLElement | null): HTMLElement[] {
  if (!container) return []
  return Array.from(
    container.querySelectorAll<HTMLElement>(
      'button:not(:disabled), input:not(:disabled), select:not(:disabled), [tabindex]:not([tabindex="-1"])'
    )
  )
}
