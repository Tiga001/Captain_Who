import { Check, ChevronDown } from 'lucide-react'
import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react'
import type { FocusEvent, KeyboardEvent } from 'react'
import { useDismissOnOutsidePointer } from '../../hooks/useDismissOnOutsidePointer'
import { AnchoredPopover } from '../../components/overlay/AnchoredPopover'
import './ModelConfigPicker.css'

export interface ModelConfigPickerOption {
  capabilityLabel?: string
  capabilitySupported?: boolean
  disabled?: boolean
  id: string
  label: string
}

interface ModelConfigPickerProps {
  ariaLabel: string
  className?: string
  disabled?: boolean
  emptyLabel: string
  onChange: (modelConfigId: string) => void
  options: readonly ModelConfigPickerOption[]
  portalMenu?: boolean
  showSelectedCapability?: boolean
  title?: string
  value: string | null
  variant: 'composer' | 'settings'
}

/**
 * Shared selector for safe model-config identities. Callers own model availability and persistence;
 * this component never guesses a replacement for an unknown or disabled model.
 */
export function ModelConfigPicker({
  ariaLabel,
  className,
  disabled = false,
  emptyLabel,
  onChange,
  options,
  portalMenu = false,
  showSelectedCapability = false,
  title,
  value,
  variant
}: ModelConfigPickerProps) {
  const [isOpen, setOpen] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const popoverRef = useRef<HTMLDivElement>(null)
  const optionRefs = useRef<Array<HTMLButtonElement | null>>([])
  const listboxId = useId()
  const selectedIndex = useMemo(
    () => options.findIndex((option) => option.id === value),
    [options, value]
  )
  const selectedOption = selectedIndex >= 0 ? options[selectedIndex] : undefined
  const enabledOptions = options.filter((option) => !option.disabled)
  const effectivelyDisabled = disabled || enabledOptions.length === 0
  const closeMenu = useCallback(() => setOpen(false), [])

  useDismissOnOutsidePointer(rootRef, isOpen, closeMenu, (target) =>
    Boolean(popoverRef.current?.contains(target))
  )

  useEffect(() => {
    if (effectivelyDisabled) closeMenu()
  }, [closeMenu, effectivelyDisabled])

  const focusOption = useCallback(
    (start: number, direction: 1 | -1 = 1) => {
      if (options.length === 0) return
      for (let offset = 0; offset < options.length; offset += 1) {
        const index =
          (((start + direction * offset) % options.length) + options.length) % options.length
        if (!options[index]?.disabled) {
          optionRefs.current[index]?.focus()
          return
        }
      }
    },
    [options]
  )

  const openMenu = (focusIndex?: number, direction: 1 | -1 = 1) => {
    if (effectivelyDisabled) return
    setOpen(true)
    const fallbackIndex = options.findIndex((option) => !option.disabled)
    const target = focusIndex ?? (selectedIndex >= 0 ? selectedIndex : fallbackIndex)
    window.requestAnimationFrame(() => focusOption(target, direction))
  }

  const closeMenuAndRestoreFocus = () => {
    setOpen(false)
    window.requestAnimationFrame(() => triggerRef.current?.focus())
  }

  const handleTriggerKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault()
      openMenu(
        Math.max(0, selectedIndex) + (event.key === 'ArrowDown' ? 1 : -1),
        event.key === 'ArrowDown' ? 1 : -1
      )
    } else if (event.key === 'Home') {
      event.preventDefault()
      openMenu(0)
    } else if (event.key === 'End') {
      event.preventDefault()
      openMenu(options.length - 1, -1)
    } else if (event.key === 'Escape' && isOpen) {
      event.preventDefault()
      closeMenu()
    }
  }

  const handleOptionKeyDown = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault()
      focusOption(index + (event.key === 'ArrowDown' ? 1 : -1), event.key === 'ArrowDown' ? 1 : -1)
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

  const handleBlur = (event: FocusEvent<HTMLDivElement>) => {
    if (
      !event.currentTarget.contains(event.relatedTarget) &&
      !popoverRef.current?.contains(event.relatedTarget)
    ) {
      closeMenu()
    }
  }

  const rootClassName = [
    'model-config-picker',
    `model-config-picker--${variant}`,
    variant === 'composer' ? 'composer-model-picker' : null,
    className
  ]
    .filter(Boolean)
    .join(' ')
  const triggerClassName =
    variant === 'composer' ? 'composer-model-button' : 'model-config-picker__button'
  const menuClassName = variant === 'composer' ? 'composer-model-menu' : 'model-config-picker__menu'
  const optionClassName =
    variant === 'composer' ? 'composer-model-option' : 'model-config-picker__option'

  return (
    <div
      className={rootClassName}
      data-open={isOpen || undefined}
      onBlur={handleBlur}
      ref={rootRef}
    >
      <button
        aria-controls={isOpen ? listboxId : undefined}
        aria-expanded={isOpen}
        aria-haspopup="listbox"
        aria-label={ariaLabel}
        className={triggerClassName}
        disabled={effectivelyDisabled}
        onClick={() => (isOpen ? closeMenu() : openMenu())}
        onKeyDown={handleTriggerKeyDown}
        ref={triggerRef}
        title={title}
        type="button"
      >
        <span className="model-config-picker__selected-name">
          {selectedOption?.label ?? emptyLabel}
        </span>
        {showSelectedCapability && selectedOption?.capabilityLabel ? (
          <span
            className="model-config-picker__capability model-config-picker__selected-capability"
            data-supported={selectedOption.capabilitySupported || undefined}
          >
            {selectedOption.capabilityLabel}
          </span>
        ) : null}
        <ChevronDown aria-hidden="true" />
      </button>

      {isOpen && (
        <AnchoredPopover
          align="end"
          anchorRef={triggerRef}
          className={variant === 'composer' ? 'chat-composer-menu-popover' : undefined}
          enabled={portalMenu}
          matchAnchorWidth={variant === 'settings'}
          onClose={closeMenu}
          popoverRef={popoverRef}
        >
          <div aria-label={ariaLabel} className={menuClassName} id={listboxId} role="listbox">
            {options.map((option, index) => {
              const isSelected = option.id === selectedOption?.id
              return (
                <button
                  aria-disabled={option.disabled || undefined}
                  aria-selected={isSelected}
                  className={optionClassName}
                  data-selected={isSelected || undefined}
                  disabled={option.disabled}
                  key={option.id}
                  onClick={() => {
                    if (option.disabled) return
                    onChange(option.id)
                    closeMenuAndRestoreFocus()
                  }}
                  onKeyDown={(event) => handleOptionKeyDown(event, index)}
                  ref={(node) => {
                    optionRefs.current[index] = node
                  }}
                  role="option"
                  tabIndex={!option.disabled && isSelected ? 0 : -1}
                  type="button"
                >
                  <span
                    className={
                      variant === 'composer'
                        ? 'composer-model-option__name'
                        : 'model-config-picker__option-name'
                    }
                  >
                    {option.label}
                  </span>
                  {option.capabilityLabel ? (
                    <span
                      className={`model-config-picker__capability ${
                        variant === 'composer'
                          ? 'composer-model-option__capability'
                          : 'model-config-picker__option-capability'
                      }`}
                      data-supported={option.capabilitySupported || undefined}
                    >
                      {option.capabilityLabel}
                    </span>
                  ) : isSelected ? (
                    <Check aria-hidden="true" />
                  ) : null}
                </button>
              )
            })}
          </div>
        </AnchoredPopover>
      )}
    </div>
  )
}
