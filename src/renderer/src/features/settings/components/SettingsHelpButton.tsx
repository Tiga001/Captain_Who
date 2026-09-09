import { CircleHelp } from 'lucide-react'
import { useState } from 'react'
import { ConfirmationDialog } from '../../../components/dialog/ConfirmationDialog'
import './SettingsHelpButton.css'

interface SettingsHelpButtonProps {
  label: string
  title: string
  description: string
  closeLabel: string
  acknowledgeLabel: string
}

export function SettingsHelpButton({
  label,
  title,
  description,
  closeLabel,
  acknowledgeLabel
}: SettingsHelpButtonProps) {
  const [isOpen, setOpen] = useState(false)

  return (
    <>
      <button
        className="settings-help-button"
        type="button"
        aria-expanded={isOpen}
        aria-haspopup="dialog"
        aria-label={label}
        title={label}
        onClick={() => setOpen(true)}
      >
        <CircleHelp aria-hidden="true" />
      </button>
      {isOpen && (
        <ConfirmationDialog
          cancelLabel={closeLabel}
          confirmLabel={acknowledgeLabel}
          confirmVariant="primary"
          description={description}
          onCancel={() => setOpen(false)}
          onConfirm={() => setOpen(false)}
          showCancelButton={false}
          title={title}
        />
      )}
    </>
  )
}
