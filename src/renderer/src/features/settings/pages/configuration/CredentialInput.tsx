import { useRef, useState } from 'react'
import type { ClipboardEvent, KeyboardEvent, MouseEvent } from 'react'
import {
  parseCredentialMutation,
  type CredentialMutation,
  type CredentialStatus
} from '@mycopilot/protocol'
import { Eye, EyeOff, Pencil, Trash2, X } from 'lucide-react'
import { ConfirmationDialog } from '../../../../components/dialog/ConfirmationDialog'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'

interface CredentialInputProps {
  ariaLabel: string
  disabled?: boolean
  mutation: CredentialMutation
  onCommit?: (mutation: CredentialMutation) => void | Promise<void>
  onMutationChange: (mutation: CredentialMutation) => void
  placeholder?: string
  status: CredentialStatus
  tabIndex?: number
}

/**
 * A write-only credential editor. Its API deliberately has no existing secret value: the Host
 * projects only a status, and Renderer can keep only a newly typed replacement draft.
 */
export function CredentialInput({
  ariaLabel,
  disabled = false,
  mutation,
  onCommit,
  onMutationChange,
  placeholder,
  status,
  tabIndex
}: CredentialInputProps) {
  const { t } = useFrontendConfig()
  const [isVisible, setVisible] = useState(false)
  const [isClearDialogOpen, setClearDialogOpen] = useState(false)
  const [isCommitPending, setCommitPending] = useState(false)
  const inputRef = useRef<HTMLInputElement>(null)
  const editButtonRef = useRef<HTMLButtonElement>(null)
  const clearButtonRef = useRef<HTMLButtonElement>(null)
  const commitInFlightRef = useRef(false)
  const enterCommittedValueRef = useRef<string | null>(null)
  const replacementValue = mutation.type === 'replace' ? mutation.value : ''
  const isReplacing = mutation.type === 'replace'
  const isConfigured = status === 'configured' && mutation.type === 'keep'
  const isUnavailable = status === 'unavailable'
  const effectiveDisabled = disabled || isCommitPending

  const preventClipboard = (event: ClipboardEvent<HTMLInputElement>) => event.preventDefault()
  const preventInputBlur = (event: MouseEvent<HTMLButtonElement>) => event.preventDefault()

  const beginReplacement = () => {
    setVisible(false)
    enterCommittedValueRef.current = null
    onMutationChange({ type: 'replace', value: '' })
    window.requestAnimationFrame(() => inputRef.current?.focus())
  }

  const cancelReplacement = () => {
    setVisible(false)
    enterCommittedValueRef.current = null
    onMutationChange({ type: 'keep' })
    window.requestAnimationFrame(() => editButtonRef.current?.focus())
  }

  const commitReplacement = () => {
    if (
      !onCommit ||
      mutation.type !== 'replace' ||
      mutation.value.length === 0 ||
      commitInFlightRef.current
    ) {
      return
    }
    let validatedMutation: CredentialMutation
    try {
      validatedMutation = parseCredentialMutation(mutation, 'credential replacement')
    } catch {
      return
    }
    commitInFlightRef.current = true
    setCommitPending(true)
    void Promise.resolve()
      .then(() => onCommit(validatedMutation))
      .catch(() => undefined)
      .finally(() => {
        commitInFlightRef.current = false
        setCommitPending(false)
      })
  }

  const handleInputKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'Escape' && isReplacing && status !== 'missing') {
      event.preventDefault()
      cancelReplacement()
    } else if (event.key === 'Enter' && onCommit) {
      event.preventDefault()
      enterCommittedValueRef.current = replacementValue
      commitReplacement()
    }
  }

  const handleInputBlur = () => {
    if (enterCommittedValueRef.current === replacementValue) {
      enterCommittedValueRef.current = null
      return
    }
    enterCommittedValueRef.current = null
    commitReplacement()
  }

  if (isConfigured) {
    return (
      <span className="configuration-secret-input configuration-credential-input">
        <span
          aria-label={`${ariaLabel}: ${t('configuration.credential.configured')}`}
          className="settings-list-control configuration-credential-input__configured"
          role="status"
        >
          <span aria-hidden="true" className="configuration-credential-input__mask">
            ••••••••
          </span>
          <span>{t('configuration.credential.configured')}</span>
        </span>
        <span className="configuration-credential-input__actions">
          <button
            ref={editButtonRef}
            aria-label={t('configuration.credential.replace')}
            className="configuration-secret-input__toggle"
            disabled={effectiveDisabled}
            onClick={beginReplacement}
            tabIndex={tabIndex}
            type="button"
          >
            <Pencil aria-hidden="true" />
          </button>
          <button
            ref={clearButtonRef}
            aria-label={t('configuration.credential.clear')}
            className="configuration-secret-input__toggle"
            disabled={effectiveDisabled}
            onClick={() => setClearDialogOpen(true)}
            tabIndex={tabIndex}
            type="button"
          >
            <Trash2 aria-hidden="true" />
          </button>
        </span>
        {isClearDialogOpen ? (
          <ConfirmationDialog
            cancelLabel={t('configuration.cancel')}
            confirmLabel={t('configuration.credential.clearConfirm')}
            description={t('configuration.credential.clearDescription')}
            dialogRole="alertdialog"
            fallbackFocusRef={inputRef}
            onCancel={() => setClearDialogOpen(false)}
            onConfirm={async () => {
              const clearMutation = { type: 'clear' } as const
              if (onCommit) {
                await onCommit(clearMutation)
              } else {
                onMutationChange(clearMutation)
              }
              setClearDialogOpen(false)
              window.requestAnimationFrame(() => inputRef.current?.focus())
            }}
            restoreFocusRef={clearButtonRef}
            title={t('configuration.credential.clearTitle')}
          />
        ) : null}
      </span>
    )
  }

  return (
    <span className="configuration-secret-input configuration-credential-input">
      <input
        ref={inputRef}
        aria-label={ariaLabel}
        autoComplete="new-password"
        className="settings-list-control"
        disabled={effectiveDisabled}
        onBlur={handleInputBlur}
        onChange={(event) => onMutationChange({ type: 'replace', value: event.target.value })}
        onCopy={preventClipboard}
        onCut={preventClipboard}
        onKeyDown={handleInputKeyDown}
        placeholder={isUnavailable ? t('configuration.credential.unavailable') : placeholder}
        spellCheck={false}
        tabIndex={tabIndex}
        type={isVisible ? 'text' : 'password'}
        value={replacementValue}
      />
      <span className="configuration-credential-input__actions">
        <button
          aria-label={
            isVisible ? t('configuration.hideSecretValue') : t('configuration.showSecretValue')
          }
          aria-pressed={isVisible}
          className="configuration-secret-input__toggle"
          disabled={effectiveDisabled}
          onClick={() => setVisible((visible) => !visible)}
          onMouseDown={preventInputBlur}
          tabIndex={tabIndex}
          type="button"
        >
          {isVisible ? <EyeOff aria-hidden="true" /> : <Eye aria-hidden="true" />}
        </button>
        {isReplacing && status !== 'missing' ? (
          <button
            aria-label={t('configuration.credential.cancelReplace')}
            className="configuration-secret-input__toggle"
            disabled={effectiveDisabled}
            onClick={cancelReplacement}
            onMouseDown={preventInputBlur}
            tabIndex={tabIndex}
            type="button"
          >
            <X aria-hidden="true" />
          </button>
        ) : null}
      </span>
    </span>
  )
}
