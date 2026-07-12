import { X } from 'lucide-react'
import { useEffect, useId, useState } from 'react'
import { createPortal } from 'react-dom'
import './ConfirmationDialog.css'

type ConfirmationDialogVariant = 'danger' | 'primary'

interface ConfirmationDialogProps {
  cancelLabel: string
  confirmLabel: string
  confirmVariant?: ConfirmationDialogVariant
  description?: string
  onCancel: () => void
  onConfirm: () => void | Promise<void>
  title: string
}

export function ConfirmationDialog({
  cancelLabel,
  confirmLabel,
  confirmVariant = 'danger',
  description,
  onCancel,
  onConfirm,
  title
}: ConfirmationDialogProps) {
  const titleId = useId()
  const descriptionId = useId()
  const [isConfirming, setIsConfirming] = useState(false)

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !isConfirming) onCancel()
    }

    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [isConfirming, onCancel])

  const confirm = async () => {
    if (isConfirming) return
    setIsConfirming(true)
    try {
      await onConfirm()
    } catch (error) {
      console.error('Confirmation action failed', error)
    } finally {
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
        className="app-confirm-dialog__card"
        role="dialog"
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
        {description && <p id={descriptionId}>{description}</p>}
        <div className="app-confirm-dialog__actions">
          <button
            className="app-confirm-dialog__button app-confirm-dialog__button--cancel"
            type="button"
            disabled={isConfirming}
            onClick={onCancel}
          >
            {cancelLabel}
          </button>
          <button
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
