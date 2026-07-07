// Renderer UI.
import { X } from "lucide-react";
import { useEffect, useId } from "react";
import { createPortal } from "react-dom";
import "./ConfirmationDialog.css";

type ConfirmationDialogVariant = "danger" | "primary";

interface ConfirmationDialogProps {
  cancelLabel: string;
  confirmLabel: string;
  confirmVariant?: ConfirmationDialogVariant;
  description: string;
  onCancel: () => void;
  onConfirm: () => void;
  title: string;
}

export function ConfirmationDialog({
  cancelLabel,
  confirmLabel,
  confirmVariant = "danger",
  description,
  onCancel,
  onConfirm,
  title,
}: ConfirmationDialogProps) {
  const titleId = useId();
  const descriptionId = useId();

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onCancel();
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onCancel]);

  return createPortal(
    <div
      className="app-confirm-dialog__backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (event.currentTarget === event.target) onCancel();
      }}
    >
      <section
        className="app-confirm-dialog__card"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={descriptionId}
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
        <div className="app-confirm-dialog__actions">
          <button
            className="app-confirm-dialog__button app-confirm-dialog__button--cancel"
            type="button"
            onClick={onCancel}
          >
            {cancelLabel}
          </button>
          <button
            className={`app-confirm-dialog__button app-confirm-dialog__button--${confirmVariant}`}
            type="button"
            onClick={onConfirm}
          >
            {confirmLabel}
          </button>
        </div>
      </section>
    </div>,
    document.body,
  );
}
