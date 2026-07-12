import { Check, ChevronDown } from 'lucide-react'
import { useCallback, useId, useRef, useState } from 'react'
import type { FocusEvent, KeyboardEvent, ReactNode } from 'react'
import { useDismissOnOutsidePointer } from '../../../hooks/useDismissOnOutsidePointer'
import './SettingsSelect.css'

export interface SettingsSelectOption<Value extends string = string> {
  label: string
  value: Value
}

interface SettingsSelectProps<Value extends string> {
  ariaLabel: string
  className?: string
  leadingIcon?: ReactNode
  onChange: (value: Value) => void
  options: ReadonlyArray<SettingsSelectOption<Value>>
  value: Value
}

export function SettingsSelect<Value extends string>({
  ariaLabel,
  className,
  leadingIcon,
  onChange,
  options,
  value
}: SettingsSelectProps<Value>): React.JSX.Element | null {
  const [isOpen, setOpen] = useState(false)
  const rootRef = useRef<HTMLSpanElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const optionRefs = useRef<Array<HTMLButtonElement | null>>([])
  const listboxId = useId()
  const selectedIndex = Math.max(
    0,
    options.findIndex((option) => option.value === value)
  )
  const selectedOption = options[selectedIndex]
  const closeMenu = useCallback(() => setOpen(false), [])

  useDismissOnOutsidePointer(rootRef, isOpen, closeMenu)

  if (!selectedOption) return null

  const focusOption = (index: number) => {
    const normalizedIndex = (index + options.length) % options.length
    optionRefs.current[normalizedIndex]?.focus()
  }

  const openMenu = (focusIndex = selectedIndex) => {
    setOpen(true)
    window.requestAnimationFrame(() => focusOption(focusIndex))
  }

  const closeMenuAndRestoreFocus = () => {
    setOpen(false)
    window.requestAnimationFrame(() => triggerRef.current?.focus())
  }

  const selectOption = (option: SettingsSelectOption<Value>) => {
    onChange(option.value)
    closeMenuAndRestoreFocus()
  }

  const handleBlur = (event: FocusEvent<HTMLSpanElement>) => {
    if (!event.currentTarget.contains(event.relatedTarget)) closeMenu()
  }

  const handleTriggerKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault()
      openMenu(selectedIndex)
    } else if (event.key === 'Home') {
      event.preventDefault()
      openMenu(0)
    } else if (event.key === 'End') {
      event.preventDefault()
      openMenu(options.length - 1)
    } else if (event.key === 'Escape' && isOpen) {
      event.preventDefault()
      closeMenu()
    }
  }

  const handleOptionKeyDown = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    if (event.key === 'ArrowDown') {
      event.preventDefault()
      focusOption(index + 1)
    } else if (event.key === 'ArrowUp') {
      event.preventDefault()
      focusOption(index - 1)
    } else if (event.key === 'Home') {
      event.preventDefault()
      focusOption(0)
    } else if (event.key === 'End') {
      event.preventDefault()
      focusOption(options.length - 1)
    } else if (event.key === 'Escape') {
      event.preventDefault()
      closeMenuAndRestoreFocus()
    }
  }

  return (
    <span
      className={['settings-select', className].filter(Boolean).join(' ')}
      data-open={isOpen || undefined}
      onBlur={handleBlur}
      ref={rootRef}
    >
      <button
        aria-controls={isOpen ? listboxId : undefined}
        aria-expanded={isOpen}
        aria-haspopup="listbox"
        aria-label={`${ariaLabel}: ${selectedOption.label}`}
        className="settings-select__button"
        onClick={() => (isOpen ? closeMenu() : openMenu())}
        onKeyDown={handleTriggerKeyDown}
        ref={triggerRef}
        type="button"
      >
        {leadingIcon && (
          <span className="settings-select__leading" aria-hidden="true">
            {leadingIcon}
          </span>
        )}
        <span className="settings-select__value">{selectedOption.label}</span>
        <ChevronDown aria-hidden="true" className="settings-select__chevron" />
      </button>

      {isOpen && (
        <span
          aria-label={ariaLabel}
          className="settings-select__menu"
          id={listboxId}
          role="listbox"
        >
          {options.map((option, index) => {
            const isSelected = option.value === selectedOption.value
            return (
              <button
                aria-selected={isSelected}
                className="settings-select__option"
                data-selected={isSelected || undefined}
                key={option.value}
                onClick={() => selectOption(option)}
                onKeyDown={(event) => handleOptionKeyDown(event, index)}
                ref={(node) => {
                  optionRefs.current[index] = node
                }}
                role="option"
                tabIndex={isSelected ? 0 : -1}
                type="button"
              >
                <span className="settings-select__option-label">{option.label}</span>
                {isSelected && <Check aria-hidden="true" />}
              </button>
            )
          })}
        </span>
      )}
    </span>
  )
}
