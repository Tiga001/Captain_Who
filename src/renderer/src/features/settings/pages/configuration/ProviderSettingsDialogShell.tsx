import { X } from 'lucide-react'
import { useEffect, useId, useRef, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'

interface ProviderSettingsDialogShellProps {
  settingId?: string
  title: string
  description: string
  children: ReactNode
  confirmDisabled?: boolean
  onCancel: () => void
  onConfirm: () => void
}

export function ProviderSettingsDialogShell({
  settingId,
  title,
  description,
  children,
  confirmDisabled = false,
  onCancel,
  onConfirm
}: ProviderSettingsDialogShellProps) {
  const { t } = useFrontendConfig()
  const titleId = useId()
  const descriptionId = useId()
  const cardRef = useRef<HTMLElement>(null)
  const cancelButtonRef = useRef<HTMLButtonElement>(null)
  const previouslyFocusedRef = useRef<HTMLElement | null>(null)
  const onCancelRef = useRef(onCancel)

  useEffect(() => {
    onCancelRef.current = onCancel
  }, [onCancel])

  useEffect(() => {
    previouslyFocusedRef.current =
      document.activeElement instanceof HTMLElement ? document.activeElement : null
    const frameId = window.requestAnimationFrame(() => cancelButtonRef.current?.focus())
    return () => {
      window.cancelAnimationFrame(frameId)
      if (previouslyFocusedRef.current?.isConnected) previouslyFocusedRef.current.focus()
    }
  }, [])

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return
      if (event.key === 'Escape') {
        if (cardRef.current?.querySelector('.settings-select[data-open="true"]')) return
        event.preventDefault()
        onCancelRef.current()
        return
      }
      if (event.key !== 'Tab') return
      const focusable = Array.from(
        cardRef.current?.querySelectorAll<HTMLElement>('button:not(:disabled)') ?? []
      ).filter((element) => element.tabIndex >= 0)
      if (focusable.length === 0) return
      const first = focusable[0]
      const last = focusable[focusable.length - 1]
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault()
        last.focus()
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault()
        first.focus()
      }
    }
    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [])

  return createPortal(
    <div
      className="provider-settings-dialog__backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (event.currentTarget === event.target) onCancel()
      }}
    >
      <section
        data-setting-id={settingId}
        ref={cardRef}
        className="provider-settings-dialog__card"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={descriptionId}
      >
        <button
          className="provider-settings-dialog__close"
          type="button"
          aria-label={t('configuration.providerSettings.cancel')}
          onClick={onCancel}
        >
          <X aria-hidden="true" />
        </button>
        <h2 id={titleId}>{title}</h2>
        <p id={descriptionId} className="provider-settings-dialog__intro">
          {description}
        </p>
        <div className="provider-settings-dialog__fields">{children}</div>
        <div className="provider-settings-dialog__actions">
          <button
            ref={cancelButtonRef}
            className="secondary-settings-button"
            type="button"
            onClick={onCancel}
          >
            {t('configuration.providerSettings.cancel')}
          </button>
          <button
            className="primary-settings-button"
            type="button"
            disabled={confirmDisabled}
            onClick={onConfirm}
          >
            {t('configuration.providerSettings.confirm')}
          </button>
        </div>
      </section>
    </div>,
    document.body
  )
}
