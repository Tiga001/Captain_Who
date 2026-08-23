import { useState } from 'react'
import type { AutomationPermissionMode } from '@mycopilot/protocol'
import { ConfirmationDialog } from '../../../components/dialog/ConfirmationDialog'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { CHAT_PERMISSION_PRESENTATIONS } from '../../chat/chatPermissionPresentation'
import { AutomationSelect, type AutomationOption } from './AutomationControls'

interface AutomationPermissionPickerProps {
  availability: { custom: boolean; full: boolean }
  disabled?: boolean
  onChange: (mode: AutomationPermissionMode) => void
  onOpenSettings?: () => void
  value: AutomationPermissionMode
}

export function AutomationPermissionPicker({
  availability,
  disabled = false,
  onChange,
  onOpenSettings,
  value
}: AutomationPermissionPickerProps) {
  const { t } = useFrontendConfig()
  const [confirmingFull, setConfirmingFull] = useState(false)
  const options: AutomationOption<AutomationPermissionMode>[] = CHAT_PERMISSION_PRESENTATIONS.map(
    (presentation) => ({
      value: presentation.id,
      label: t(
        presentation.id === 'default'
          ? 'automation.permissionDefault'
          : presentation.id === 'full'
            ? 'automation.permissionFull'
            : 'automation.permissionCustom'
      ),
      icon: presentation.icon,
      disabled:
        (presentation.id === 'full' && !availability.full) ||
        (presentation.id === 'custom' && !availability.custom)
    })
  )
  const unavailable =
    (value === 'full' && !availability.full) || (value === 'custom' && !availability.custom)

  return (
    <div className="automation-permission-picker">
      <AutomationSelect
        ariaLabel={t('automation.permission')}
        disabled={disabled}
        onChange={(nextMode) => {
          if (nextMode === 'full' && value !== 'full') {
            setConfirmingFull(true)
            return
          }
          onChange(nextMode)
        }}
        options={options}
        value={value}
      />
      {unavailable && (
        <div className="automation-permission-picker__warning" role="alert">
          <p>{t('automation.permissionDisabled')}</p>
          {onOpenSettings && (
            <button type="button" onClick={onOpenSettings}>
              {t('automation.openPermissionSettings')}
            </button>
          )}
        </div>
      )}
      {confirmingFull && (
        <ConfirmationDialog
          cancelLabel={t('chat.fullPermissionConfirmCancel')}
          confirmLabel={t('chat.fullPermissionConfirmAction')}
          confirmVariant="primary"
          description={t('chat.fullPermissionConfirmDescription')}
          onCancel={() => setConfirmingFull(false)}
          onConfirm={() => {
            if (availability.full) onChange('full')
            setConfirmingFull(false)
          }}
          title={t('chat.fullPermissionConfirmTitle')}
        />
      )}
    </div>
  )
}
