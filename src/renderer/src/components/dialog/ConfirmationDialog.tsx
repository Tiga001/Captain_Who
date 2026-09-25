import { X } from 'lucide-react'
import { useEffect, useId, useRef, useState, type RefObject } from 'react'
import { createPortal } from 'react-dom'
import './ConfirmationDialog.css'

type ConfirmationDialogVariant = 'danger' | 'primary'

interface ConfirmationDialogProps {
  cancelLabel: string
  confirmLabel: string
  confirmVariant?: ConfirmationDialogVariant
  description?: string
  descriptionClassName?: string
  dialogRole?: 'alertdialog' | 'dialog'
  fallbackFocusRef?: RefObject<HTMLElement | null>
  onCancel: () => void
  onConfirm: () => void | Promise<void>
  restoreFocusRef?: RefObject<HTMLElement | null>
  showCancelButton?: boolean
  title: string
}

export function ConfirmationDialog({
  cancelLabel,
  confirmLabel,
  confirmVariant = 'danger',
  description,
  descriptionClassName,
  dialogRole = 'dialog',
  fallbackFocusRef,
  onCancel,
  onConfirm,
  restoreFocusRef,
  showCancelButton = true,
  title
}: ConfirmationDialogProps) {
  const titleId = useId()
  const descriptionId = useId()
  const [isConfirming, setIsConfirming] = useState(false)
  const confirmingRef = useRef(false)
  const cardRef = useRef<HTMLElement>(null)
  const cancelButtonRef = useRef<HTMLButtonElement>(null)
  const confirmButtonRef = useRef<HTMLButtonElement>(null)
  const previouslyFocusedRef = useRef<HTMLElement | null>(null)

  useEffect(() => {
    previouslyFocusedRef.current =
      document.activeElement instanceof HTMLElement ? document.activeElement : null
    const fallbackFocus = fallbackFocusRef?.current ?? null
    const explicitRestoreTarget = restoreFocusRef?.current ?? null
    const frameId = window.requestAnimationFrame(() => {
      if (showCancelButton) {
        cancelButtonRef.current?.focus()
      } else {
        confirmButtonRef.current?.focus()
      }
    })

    return () => {
      window.cancelAnimationFrame(frameId)
      const previous = previouslyFocusedRef.current
      if (explicitRestoreTarget?.isConnected) {
        explicitRestoreTarget.focus()
      } else if (previous?.isConnected) {
        previous.focus()
      } else {
        fallbackFocus?.focus()
      }
    }
  }, [fallbackFocusRef, restoreFocusRef, showCancelButton])

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault()
        event.stopImmediatePropagation()
        if (!isConfirming) onCancel()
        return
      }
      if (event.key !== 'Tab') return

      const focusable = Array.from(
        cardRef.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? []
      )
      if (focusable.length === 0) return
      const first = focusable[0]
      const last = focusable[focusable.length - 1]
      const active = document.activeElement
      if (event.shiftKey && active === first) {
        event.preventDefault()
        last.focus()
      } else if (!event.shiftKey && active === last) {
        event.preventDefault()
        first.focus()
      }
    }

    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [isConfirming, onCancel])

  const confirm = async () => {
    if (confirmingRef.current) return
    confirmingRef.current = true
    setIsConfirming(true)
    try {
      await onConfirm()
    } catch (error) {
      console.error('Confirmation action failed', error)
    } finally {
      confirmingRef.current = false
      setIsConfirming(false)
    }
  }

  return createPortal(
    <div
      className="app-confirm-dialog__backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (!isConfirming && event.currentTarget === event.target) onCancel()
      }}
    >
      <section
        ref={cardRef}
        className="app-confirm-dialog__card"
        role={dialogRole}
        aria-modal="true"
        aria-busy={isConfirming || undefined}
        aria-labelledby={titleId}
        aria-describedby={description ? descriptionId : undefined}
      >
        <button
          className="app-confirm-dialog__close"
          type="button"
          aria-label={cancelLabel}
          disabled={isConfirming}
          onClick={onCancel}
        >
          <X aria-hidden="true" />
        </button>
        <h2 id={titleId}>{title}</h2>
        {description && (
          <p id={descriptionId} className={descriptionClassName}>
            {description}
          </p>
        )}
        <div className="app-confirm-dialog__actions">
          {showCancelButton && (
            <button
              ref={cancelButtonRef}
              className="app-confirm-dialog__button app-confirm-dialog__button--cancel"
              type="button"
              disabled={isConfirming}
              onClick={onCancel}
            >
              {cancelLabel}
            </button>
          )}
          <button
            ref={confirmButtonRef}
            className={`app-confirm-dialog__button app-confirm-dialog__button--${confirmVariant}`}
            type="button"
            disabled={isConfirming}
            onClick={() => void confirm()}
          >
            {confirmLabel}
          </button>
        </div>
      </section>
    </div>,
    document.body
  )
}
