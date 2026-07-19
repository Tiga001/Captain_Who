// Renderer skills management UI: resolves public URLs and presents frozen installation previews.
import { useEffect, useId, useRef, useState, type KeyboardEvent, type ReactNode } from 'react'
import {
  AlertTriangle,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  FolderOpen,
  GitFork,
  LoaderCircle,
  PackageOpen,
  X
} from 'lucide-react'
import { createPortal } from 'react-dom'
import type {
  SkillGitHubReference,
  SkillInstallationPreview,
  SkillSourceResolutionCandidate
} from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import type {
  SkillInstallationWorkflowState,
  useSkillInstallationWorkflow
} from './useSkillInstallationWorkflow'
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
  const githubUrlInputId = useId()
  const githubUrlHintId = useId()
  const [now, setNow] = useState(0)
  const isOpen = state.status !== 'idle'
  const isBusy =
    state.status === 'resolving' || state.status === 'inspecting' || state.status === 'committing'
  const isCommitting = state.status === 'committing'

  useEffect(() => {
    if (
      state.status !== 'candidates' &&
      state.status !== 'preview' &&
      state.status !== 'committing'
    ) {
      return undefined
    }
    setNow(Date.now())
    const interval = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(interval)
  }, [state.status])

  useEffect(() => {
    if (!isOpen) return undefined
    const frame = window.requestAnimationFrame(() => {
      getFocusableElements(dialogRef.current)[0]?.focus({ preventScroll: true })
    })
    return () => {
      window.cancelAnimationFrame(frame)
    }
  }, [isOpen])

  if (state.status === 'idle') return null

  const context = getDialogContext(state)
  const operationLabel =
    context.operation === 'install' ? t('skills.installOperation') : t('skills.updateOperation')
  const title =
    context.operation === 'install'
      ? t('skills.installDialogTitle')
      : replaceTokens(t('skills.updateDialogTitle'), { name: context.entry.name })
  const description = getDialogDescription(state, t)

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
        aria-describedby={description ? descriptionId : undefined}
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
            {description && <p id={descriptionId}>{description}</p>}
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
            <SkillSourceChoice
              localError={state.localError}
              onChooseGitHub={workflow.chooseGitHubSource}
              onChooseLocal={() => void workflow.chooseLocalDirectory()}
            />
          )}

          {state.status === 'urlInput' && (
            <form
              className="skill-url-form"
              onSubmit={(event) => {
                event.preventDefault()
                void workflow.submitUrl()
              }}
            >
              <div className="skill-url-form__field">
                <label className="sr-only" htmlFor={githubUrlInputId}>
                  {t('skills.githubUrl')}
                </label>
                <small className="skill-url-form__hint" id={githubUrlHintId}>
                  {t('skills.githubUrlDescription')}
                </small>
                <input
                  aria-describedby={githubUrlHintId}
                  aria-invalid={state.fieldError || undefined}
                  autoFocus
                  id={githubUrlInputId}
                  onChange={(event) => workflow.updateUrl(event.target.value)}
                  placeholder={t('skills.githubUrlPlaceholder')}
                  type="url"
                  value={state.url}
                />
                {state.fieldError && (
                  <small className="skill-form-error">{t('skills.githubUrlRequired')}</small>
                )}
              </div>
              <DialogActions>
                <button
                  className="skill-dialog-button"
                  onClick={workflow.returnToSourceChoice}
                  type="button"
                >
                  {t('skills.back')}
                </button>
                <button className="skill-dialog-button skill-dialog-button--primary" type="submit">
                  {t('skills.continue')}
                </button>
              </DialogActions>
            </form>
          )}

          {state.status === 'resolving' && <Progress label={t('skills.resolvingSource')} />}

          {state.status === 'candidates' && (
            <SkillCandidateSelection
              candidates={state.output.candidates}
              expired={now >= state.output.expiresAtUnixMs}
              onBack={workflow.returnToPreviousStep}
              onChoose={(candidate) => void workflow.chooseCandidate(candidate)}
            />
          )}

          {state.status === 'inspecting' && <Progress label={t('skills.inspecting')} />}

          {(state.status === 'preview' || state.status === 'committing') && (
            <SkillPreview
              acceptedIssueIds={state.acceptedIssueIds}
              committing={state.status === 'committing'}
              errorMessage={state.status === 'preview' ? state.errorMessage : null}
              now={now}
              operationLabel={operationLabel}
              onBack={workflow.returnToPreviousStep}
              onCommit={() => void workflow.commit()}
              onToggleAcknowledgement={workflow.toggleAcknowledgement}
              preview={state.preview}
            />
          )}

          {state.status === 'error' && (
            <SkillWorkflowError
              message={state.message}
              onCancel={workflow.close}
              onRecover={() => void workflow.recover()}
              recovery={state.details.recovery}
            />
          )}
        </div>
      </section>
    </div>,
    document.body
  )
}

function SkillSourceChoice({
  localError,
  onChooseGitHub,
  onChooseLocal
}: {
  localError: boolean
  onChooseGitHub: () => void
  onChooseLocal: () => void
}) {
  const { t } = useFrontendConfig()
  return (
    <div className="skill-source-choice">
      <div className="skill-source-choice__list">
        <button className="skill-source-choice__option" onClick={onChooseGitHub} type="button">
          <span className="skill-source-choice__icon">
            <GitFork aria-hidden="true" />
          </span>
          <span className="skill-source-choice__copy">
            <strong>{t('skills.installFromGitHub')}</strong>
            <small>{t('skills.installFromGitHubDescription')}</small>
          </span>
          <ChevronRight aria-hidden="true" className="skill-source-choice__chevron" />
        </button>
        <button className="skill-source-choice__option" onClick={onChooseLocal} type="button">
          <span className="skill-source-choice__icon">
            <FolderOpen aria-hidden="true" />
          </span>
          <span className="skill-source-choice__copy">
            <strong>{t('skills.installLocal')}</strong>
            <small>{t('skills.installLocalDescription')}</small>
          </span>
          <ChevronRight aria-hidden="true" className="skill-source-choice__chevron" />
        </button>
      </div>
      {localError && (
        <p className="skill-form-error" role="alert">
          {t('skills.localSelectionFailed')}
        </p>
      )}
    </div>
  )
}

function SkillCandidateSelection({
  candidates,
  expired,
  onBack,
  onChoose
}: {
  candidates: readonly SkillSourceResolutionCandidate[]
  expired: boolean
  onBack: () => void
  onChoose: (candidate: SkillSourceResolutionCandidate) => void
}) {
  const { t } = useFrontendConfig()
  return (
    <div className="skill-candidate-selection">
      <div>
        <h3>{t('skills.chooseCandidateTitle')}</h3>
        <p>{t('skills.chooseCandidateDescription')}</p>
      </div>
      <div className="skill-candidate-list">
        {candidates.map((candidate) => (
          <button
            aria-label={replaceTokens(t('skills.chooseCandidateNamed'), {
              name: candidate.package.name
            })}
            className="skill-candidate-card"
            disabled={expired}
            key={candidate.candidateId}
            onClick={() => onChoose(candidate)}
            type="button"
          >
            <span className="skill-candidate-card__identity">
              <strong>{candidate.package.name}</strong>
              <small>{candidate.package.description}</small>
            </span>
            <span className="skill-candidate-card__source">
              {formatResolvedSource(candidate, t)}
            </span>
            <span className="skill-candidate-card__facts">
              {replaceTokens(t('skills.candidateFacts'), {
                count: candidate.package.fileCount,
                size: formatBytes(candidate.package.totalBytes)
              })}
            </span>
          </button>
        ))}
      </div>
      {expired && (
        <p className="skill-preview-expired" role="alert">
          {t('skills.candidateExpired')}
        </p>
      )}
      <DialogActions>
        <button className="skill-dialog-button" onClick={onBack} type="button">
          {expired ? t('skills.resolveAgain') : t('skills.back')}
        </button>
      </DialogActions>
    </div>
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
  const [technicalDetailsOpen, setTechnicalDetailsOpen] = useState(false)
  const expired = now >= preview.expiresAtUnixMs
  const requiredIssues = preview.compatibility.issues.filter(
    (issue) => issue.requiresAcknowledgement
  )
  const acknowledged = requiredIssues.every((issue) => acceptedIssueIds.includes(issue.id))
  const incompatible = preview.compatibility.status === 'incompatible'
  const alreadyCurrent =
    preview.operation === 'update' &&
    preview.changes.content === 'unchanged' &&
    preview.changes.source === 'unchanged'
  const hasCompatibilityIssues = preview.compatibility.issues.length > 0
  const friendlySource = formatFriendlyPreviewSource(preview, t)

  return (
    <div className="skill-install-preview">
      <div className="skill-install-preview__identity">
        <h3>{preview.package.name}</h3>
        <p>{preview.package.description}</p>
      </div>

      <div className="skill-install-preview__source-summary">
        <span className="skill-install-preview__source-icon">
          {preview.source.kind === 'githubRepository' ? (
            <GitFork aria-hidden="true" />
          ) : preview.source.kind === 'localDirectory' ? (
            <FolderOpen aria-hidden="true" />
          ) : (
            <PackageOpen aria-hidden="true" />
          )}
        </span>
        <div>
          <span>{t('skills.previewSource')}</span>
          <strong>{friendlySource}</strong>
          <small>
            {replaceTokens(t('skills.previewContents'), {
              count: preview.package.fileCount,
              size: formatBytes(preview.package.totalBytes)
            })}
          </small>
        </div>
      </div>

      <div
        className="skill-install-preview__result"
        data-state={alreadyCurrent ? 'current' : 'ready'}
      >
        <CheckCircle2 aria-hidden="true" />
        <div>
          <strong>
            {alreadyCurrent
              ? t('skills.previewAlreadyCurrent')
              : preview.operation === 'update'
                ? t('skills.previewUpdateAvailable')
                : t('skills.previewReadyToInstall')}
          </strong>
          <p>
            {alreadyCurrent
              ? t('skills.previewAlreadyCurrentDescription')
              : preview.operation === 'update'
                ? t('skills.previewUpdateAvailableDescription')
                : t('skills.previewReadyToInstallDescription')}
          </p>
        </div>
      </div>

      <div className="skill-compatibility" data-status={preview.compatibility.status}>
        {hasCompatibilityIssues || incompatible ? (
          <AlertTriangle aria-hidden="true" />
        ) : (
          <CheckCircle2 aria-hidden="true" />
        )}
        <div>
          <strong>
            {!hasCompatibilityIssues && preview.compatibility.status === 'compatible'
              ? t('skills.previewSafetyPassed')
              : t(`skills.compatibility.${preview.compatibility.status}`)}
          </strong>
          {!hasCompatibilityIssues && preview.compatibility.status === 'compatible' ? (
            <p>{t('skills.previewSafetyPassedDescription')}</p>
          ) : (
            preview.compatibility.issues.length > 0 && (
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
            )
          )}
        </div>
      </div>

      <div className="skill-install-preview__technical">
        <button
          aria-expanded={technicalDetailsOpen}
          className="skill-install-preview__technical-toggle"
          onClick={() => setTechnicalDetailsOpen((open) => !open)}
          type="button"
        >
          <span>
            {technicalDetailsOpen
              ? t('skills.hideTechnicalDetails')
              : t('skills.showTechnicalDetails')}
          </span>
          <ChevronDown aria-hidden="true" />
        </button>
        {technicalDetailsOpen && (
          <dl className="skill-install-preview__technical-facts">
            <div>
              <dt>{t('skills.previewExactSource')}</dt>
              <dd>{formatPreviewSource(preview, t)}</dd>
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

      {alreadyCurrent ? (
        <DialogActions>
          <button
            className="skill-dialog-button skill-dialog-button--primary"
            disabled={committing}
            onClick={onBack}
            type="button"
          >
            {t('skills.done')}
          </button>
        </DialogActions>
      ) : (
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
            {committing ? t('skills.committing') : operationLabel}
          </button>
        </DialogActions>
      )}
    </div>
  )
}

function SkillWorkflowError({
  message,
  onCancel,
  onRecover,
  recovery
}: {
  message: string | null
  onCancel: () => void
  onRecover: () => void
  recovery?: string
}) {
  const { t } = useFrontendConfig()
  const isCapacity = recovery === 'freeCapacity'
  const isSupport = recovery === 'contactSupport'
  const canRecover = Boolean(recovery && !isCapacity && !isSupport)
  const description = isCapacity
    ? t('skills.freeCapacityDescription')
    : isSupport
      ? t('skills.contactSupportDescription')
      : (message ?? t('skills.localOperationFailed'))

  return (
    <div className="skill-dialog-error" role="alert">
      <AlertTriangle aria-hidden="true" />
      <div>
        <strong>
          {isCapacity
            ? t('skills.capacityTitle')
            : isSupport
              ? t('skills.contactSupportTitle')
              : t('skills.operationFailed')}
        </strong>
        <p>{description}</p>
      </div>
      <DialogActions>
        <button className="skill-dialog-button" onClick={onCancel} type="button">
          {t('skills.cancel')}
        </button>
        {canRecover && (
          <button
            className="skill-dialog-button skill-dialog-button--primary"
            onClick={onRecover}
            type="button"
          >
            {recoveryLabel(recovery ?? '', t)}
          </button>
        )}
      </DialogActions>
    </div>
  )
}

function Progress({ label }: { label: string }) {
  return (
    <div className="skill-dialog-progress" role="status">
      <LoaderCircle aria-hidden="true" />
      <span>{label}</span>
    </div>
  )
}

function DialogActions({ children, className = '' }: { children: ReactNode; className?: string }) {
  return <div className={`skill-install-dialog__actions ${className}`.trim()}>{children}</div>
}

function getDialogContext(state: Exclude<SkillInstallationWorkflowState, { status: 'idle' }>) {
  if (state.status === 'inspecting') return state.inspection.context
  if (state.status === 'preview' || state.status === 'committing') return state.inspection.context
  return state.context
}

function getDialogDescription(
  state: Exclude<SkillInstallationWorkflowState, { status: 'idle' }>,
  t: ReturnType<typeof useFrontendConfig>['t']
): string | null {
  if (state.status === 'choosingSource') return t('skills.installSourceDescription')
  if (
    state.status === 'urlInput' ||
    state.status === 'resolving' ||
    state.status === 'candidates'
  ) {
    return null
  }
  return getDialogContext(state).operation === 'update'
    ? t('skills.updateDialogDescription')
    : t('skills.installReviewDescription')
}

function formatPreviewSource(
  preview: SkillInstallationPreview,
  t: ReturnType<typeof useFrontendConfig>['t']
): string {
  const source = preview.source
  if (source.kind === 'localDirectory') return t('skills.previewLocalSource')
  if (source.kind === 'installedSource') return source.displayName
  return `${source.owner}/${source.repository} · ${formatGitHubReference(source.reference, t)}${
    source.subdirectory ? ` · ${source.subdirectory}` : ''
  }`
}

function formatFriendlyPreviewSource(
  preview: SkillInstallationPreview,
  t: ReturnType<typeof useFrontendConfig>['t']
): string {
  const source = preview.source
  if (source.kind === 'localDirectory') return t('skills.previewLocalSource')
  if (source.kind === 'installedSource') return source.displayName
  return `${source.owner}/${source.repository}`
}

function formatResolvedSource(
  candidate: SkillSourceResolutionCandidate,
  t: ReturnType<typeof useFrontendConfig>['t']
): string {
  const source = candidate.source
  return `${source.owner}/${source.repository} · ${formatGitHubReference(source.reference, t)}${
    source.subdirectory ? ` · ${source.subdirectory}` : ''
  }`
}

function formatGitHubReference(
  reference: SkillGitHubReference,
  t: ReturnType<typeof useFrontendConfig>['t']
): string {
  if (reference.kind === 'defaultBranch') return t('skills.githubDefaultBranch')
  if (reference.kind === 'commit') return reference.sha.slice(0, 12)
  return reference.value
}

function recoveryLabel(recovery: string, t: ReturnType<typeof useFrontendConfig>['t']): string {
  if (recovery === 'refreshManagement') return t('skills.refresh')
  if (
    recovery === 'startNewResolution' ||
    recovery === 'resolveAgain' ||
    recovery === 'inspectAgain' ||
    recovery === 'newInstallationIdentity'
  ) {
    return t('skills.inspectAgain')
  }
  if (
    recovery === 'fixLocator' ||
    recovery === 'narrowLocator' ||
    recovery === 'chooseDifferentSource' ||
    recovery === 'fixSource'
  ) {
    return t('skills.editUrl')
  }
  return t('skills.retry')
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
      'button:not(:disabled), input:not(:disabled), [tabindex]:not([tabindex="-1"])'
    )
  )
}
