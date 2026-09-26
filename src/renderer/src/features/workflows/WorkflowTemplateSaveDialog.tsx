import { X } from 'lucide-react'
import { useEffect, useId, useRef } from 'react'
import { createPortal } from 'react-dom'
import type { WorkflowTemplateUsage } from '@mycopilot/protocol'
import type { WorkflowText } from './workflowText'
import '../../components/dialog/ConfirmationDialog.css'

export function WorkflowTemplateSaveDialog({
  usage,
  text,
  busy,
  onCancel,
  onPublish,
  onCopy,
  onStash
}: {
  usage: WorkflowTemplateUsage
  text: WorkflowText
  busy: boolean
  onCancel: () => void
  onPublish: () => void
  onCopy: () => void
  onStash: () => void
}) {
  const titleId = useId()
  const descriptionId = useId()
  const cardRef = useRef<HTMLElement>(null)
  const cancelRef = useRef<HTMLButtonElement>(null)
  const running = usage.instances.some((instance) => instance.running)
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null
    cancelRef.current?.focus()
    return () => {
      if (previous?.isConnected) previous.focus()
    }
  }, [])
  useEffect(() => {
    const keydown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault()
        event.stopImmediatePropagation()
        if (!busy) onCancel()
      }
      if (event.key !== 'Tab') return
      const buttons = Array.from(
        cardRef.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? []
      )
      const first = buttons[0],
        last = buttons.at(-1)
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault()
        last?.focus()
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault()
        first?.focus()
      }
    }
    window.addEventListener('keydown', keydown)
    return () => window.removeEventListener('keydown', keydown)
  }, [busy, onCancel])
  return createPortal(
    <div
      className="app-confirm-dialog__backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (!busy && event.currentTarget === event.target) onCancel()
      }}
    >
      <section
        ref={cardRef}
        className="app-confirm-dialog__card workflow-template-save-dialog"
        role="dialog"
        aria-modal="true"
        aria-busy={busy || undefined}
        aria-labelledby={titleId}
        aria-describedby={descriptionId}
      >
        <button
          className="app-confirm-dialog__close"
          type="button"
          aria-label={text('close')}
          disabled={busy}
          onClick={onCancel}
        >
          <X aria-hidden="true" />
        </button>
        <h2 id={titleId}>{text(running ? 'runningTitle' : 'usageTitle')}</h2>
        <p id={descriptionId}>{text(running ? 'runningDescription' : 'usageDescription')}</p>
        <ul className="workflow-template-usage-list">
          {usage.instances.map((instance) => (
            <li key={instance.id}>
              <strong>{instance.name}</strong>
              {instance.running ? <span>{text('runningInstance')}</span> : null}
              {instance.projectNames.length ? (
                <small>{instance.projectNames.join(' · ')}</small>
              ) : null}
            </li>
          ))}
        </ul>
        <div className="app-confirm-dialog__actions">
          <button
            ref={cancelRef}
            className="app-confirm-dialog__button app-confirm-dialog__button--cancel"
            type="button"
            disabled={busy}
            onClick={onCancel}
          >
            {text('keep')}
          </button>
          {running ? (
            <button
              className="app-confirm-dialog__button app-confirm-dialog__button--cancel"
              type="button"
              disabled={busy}
              onClick={onStash}
            >
              {text('stash')}
            </button>
          ) : null}
          <button
            className="app-confirm-dialog__button app-confirm-dialog__button--primary"
            type="button"
            disabled={busy}
            onClick={running ? onCopy : onPublish}
          >
            {text(running ? 'saveCopy' : 'confirmPublish')}
          </button>
        </div>
      </section>
    </div>,
    document.body
  )
}
