import { useState } from 'react'
import type { AutomationPermissionMode } from '@mycopilot/protocol'
import { ConfirmationDialog } from '../../../components/dialog/ConfirmationDialog'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
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
  const options: AutomationOption<AutomationPermissionMode>[] = [
    { value: 'default', label: t('automation.permissionDefault') },
    {
      value: 'full',
      label: t('automation.permissionFull'),
      disabled: !availability.full
    },
    {
      value: 'custom',
      label: t('automation.permissionCustom'),
      disabled: !availability.custom
    }
  ]
  const unavailable =
    (value === 'full' && !availability.full) || (value === 'custom' && !availability.custom)

  return (
    <>
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
      <p className="automation-permission-picker__description">
        {t(
          value === 'default'
            ? 'general.defaultPermissionDescription'
            : value === 'full'
              ? 'general.fullPermissionDescription'
              : 'general.customPermissionDescription'
        )}
      </p>
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
    </>
  )
}
