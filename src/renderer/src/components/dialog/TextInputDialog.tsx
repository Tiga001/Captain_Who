import { X } from 'lucide-react'
import { useEffect, useId } from 'react'
import { createPortal } from 'react-dom'
import './ConfirmationDialog.css'

interface TextInputDialogProps {
  cancelLabel: string
  confirmDisabled?: boolean
  confirmLabel: string
  description: string
  inputAriaLabel?: string
  onCancel: () => void
  onConfirm: () => void
  onValueChange: (value: string) => void
  title: string
  value: string
}

export function TextInputDialog({
  cancelLabel,
  confirmDisabled = false,
  confirmLabel,
  description,
  inputAriaLabel,
  onCancel,
  onConfirm,
  onValueChange,
  title,
  value
}: TextInputDialogProps) {
  const titleId = useId()
  const descriptionId = useId()

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onCancel()
    }

    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [onCancel])

  return createPortal(
    <div
      className="app-confirm-dialog__backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (event.currentTarget === event.target) onCancel()
      }}
    >
      <form
        className="app-confirm-dialog__card"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={descriptionId}
        onSubmit={(event) => {
          event.preventDefault()
          if (!confirmDisabled) onConfirm()
        }}
      >
        <button
          className="app-confirm-dialog__close"
          type="button"
          aria-label={cancelLabel}
          onClick={onCancel}
        >
          <X aria-hidden="true" />
        </button>
        <h2 id={titleId}>{title}</h2>
        <p id={descriptionId}>{description}</p>
        <input
          className="app-confirm-dialog__input"
          autoFocus
          aria-label={inputAriaLabel ?? title}
          value={value}
          onChange={(event) => onValueChange(event.target.value)}
        />
        <div className="app-confirm-dialog__actions">
          <button
            className="app-confirm-dialog__button app-confirm-dialog__button--cancel"
            type="button"
            onClick={onCancel}
          >
            {cancelLabel}
          </button>
          <button
            className="app-confirm-dialog__button app-confirm-dialog__button--primary"
            type="submit"
            disabled={confirmDisabled}
          >
            {confirmLabel}
          </button>
        </div>
      </form>
    </div>,
    document.body
  )
}
