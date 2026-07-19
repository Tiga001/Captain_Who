import { useState } from 'react'
import type { ClipboardEvent } from 'react'
import { Eye, EyeOff } from 'lucide-react'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'

interface SecretInputProps {
  ariaLabel: string
  onChange: (value: string) => void
  placeholder?: string
  tabIndex?: number
  value: string
}

export function SecretInput({
  ariaLabel,
  onChange,
  placeholder,
  tabIndex,
  value
}: SecretInputProps) {
  const { t } = useFrontendConfig()
  const [isVisible, setVisible] = useState(false)
  const preventClipboard = (event: ClipboardEvent<HTMLInputElement>) => {
    event.preventDefault()
  }

  return (
    <span className="configuration-secret-input">
      <input
        aria-label={ariaLabel}
        className="settings-list-control"
        onChange={(event) => onChange(event.target.value)}
        onCopy={preventClipboard}
        onCut={preventClipboard}
        placeholder={placeholder}
        tabIndex={tabIndex}
        type={isVisible ? 'text' : 'password'}
        value={value}
      />
      <button
        aria-label={
          isVisible ? t('configuration.hideSecretValue') : t('configuration.showSecretValue')
        }
        aria-pressed={isVisible}
        className="configuration-secret-input__toggle"
        onClick={() => setVisible((visible) => !visible)}
        tabIndex={tabIndex}
        type="button"
      >
        {isVisible ? <EyeOff aria-hidden="true" /> : <Eye aria-hidden="true" />}
      </button>
    </span>
  )
}
