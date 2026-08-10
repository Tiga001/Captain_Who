import { Check, ChevronDown } from 'lucide-react'
import { useCallback, useEffect, useId, useRef, useState } from 'react'
import type { FocusEvent, KeyboardEvent, ReactNode } from 'react'
import { useDismissOnOutsidePointer } from '../../../hooks/useDismissOnOutsidePointer'
import './SettingsSelect.css'

export interface SettingsSelectOption<Value extends string = string> {
  disabled?: boolean
  label: string
  value: Value
}

interface SettingsSelectProps<Value extends string> {
  ariaLabel: string
  className?: string
  disabled?: boolean
  leadingIcon?: ReactNode
  onChange: (value: Value) => void
  options: ReadonlyArray<SettingsSelectOption<Value>>
  tabIndex?: number
  value: Value
}

export function SettingsSelect<Value extends string>({
  ariaLabel,
  className,
  disabled = false,
  leadingIcon,
  onChange,
  options,
  tabIndex,
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

  useEffect(() => {
    if (disabled) closeMenu()
  }, [closeMenu, disabled])

  if (!selectedOption) return null

  const focusOption = (index: number, direction: 1 | -1 = 1) => {
    for (let offset = 0; offset < options.length; offset += 1) {
      const normalizedIndex = (index + direction * offset + options.length) % options.length
      if (!options[normalizedIndex]?.disabled) {
        optionRefs.current[normalizedIndex]?.focus()
        return
      }
    }
  }

  const openMenu = (focusIndex = selectedIndex, direction: 1 | -1 = 1) => {
    setOpen(true)
    window.requestAnimationFrame(() => focusOption(focusIndex, direction))
  }

  const closeMenuAndRestoreFocus = () => {
    setOpen(false)
    window.requestAnimationFrame(() => triggerRef.current?.focus())
  }

  const selectOption = (option: SettingsSelectOption<Value>) => {
    if (option.disabled) return
    onChange(option.value)
    closeMenuAndRestoreFocus()
  }

  const handleBlur = (event: FocusEvent<HTMLSpanElement>) => {
    if (!event.currentTarget.contains(event.relatedTarget)) closeMenu()
  }

  const handleTriggerKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault()
      openMenu(
        selectedIndex + (event.key === 'ArrowDown' ? 1 : -1),
        event.key === 'ArrowDown' ? 1 : -1
      )
    } else if (event.key === 'Home') {
      event.preventDefault()
      openMenu(0)
    } else if (event.key === 'End') {
      event.preventDefault()
      setOpen(true)
      window.requestAnimationFrame(() => focusOption(options.length - 1, -1))
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
      focusOption(index - 1, -1)
    } else if (event.key === 'Home') {
      event.preventDefault()
      focusOption(0)
    } else if (event.key === 'End') {
      event.preventDefault()
      focusOption(options.length - 1, -1)
    } else if (event.key === 'Escape') {
      event.preventDefault()
      closeMenuAndRestoreFocus()
    }
  }

  return (
    <span
      className={['settings-select', className].filter(Boolean).join(' ')}
      data-disabled={disabled || undefined}
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
        disabled={disabled}
        onClick={() => (isOpen ? closeMenu() : openMenu())}
        onKeyDown={handleTriggerKeyDown}
        ref={triggerRef}
        tabIndex={tabIndex}
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
                aria-disabled={option.disabled || undefined}
                aria-selected={isSelected}
                className="settings-select__option"
                data-selected={isSelected || undefined}
                disabled={option.disabled}
                key={option.value}
                onClick={() => selectOption(option)}
                onKeyDown={(event) => handleOptionKeyDown(event, index)}
                ref={(node) => {
                  optionRefs.current[index] = node
                }}
                role="option"
                tabIndex={!option.disabled && isSelected ? 0 : -1}
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
