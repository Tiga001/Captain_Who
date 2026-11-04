import { Check } from 'lucide-react'
import type { TranslationKey } from '../../../config/frontendTranslations'
import type { ChatPermissionPresentation } from '../chatPermissionPresentation'
import type { ChatPermissionMode } from '../chatTypes'

interface ComposerPermissionMenuProps {
  permissionOptions: readonly ChatPermissionPresentation[]
  permissionMode: ChatPermissionMode
  isModelSelectionDisabled: boolean
  selectPermissionMode: (mode: ChatPermissionMode) => void
  t: (key: TranslationKey) => string
}

export function ComposerPermissionMenu({
  permissionOptions,
  permissionMode,
  isModelSelectionDisabled,
  selectPermissionMode,
  t
}: ComposerPermissionMenuProps) {
  return (
    <div
      className="composer-permission-menu"
      role="listbox"
      aria-label={t('chat.selectPermission')}
    >
      {permissionOptions.map((option) => {
        const OptionIcon = option.icon
        const isSelected = option.id === permissionMode

        return (
          <button
            type="button"
            role="option"
            aria-selected={isSelected}
            className="composer-permission-option"
            disabled={isModelSelectionDisabled}
            data-permission={option.id}
            data-selected={isSelected || undefined}
            key={option.id}
            onClick={() => selectPermissionMode(option.id)}
          >
            <OptionIcon aria-hidden="true" />
            <span>{t(option.labelKey)}</span>
            {isSelected && (
              <Check className="composer-permission-option__check" aria-hidden="true" />
            )}
          </button>
        )
      })}
    </div>
  )
}
