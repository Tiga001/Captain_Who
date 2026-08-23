import { Check, ChevronDown } from 'lucide-react'
import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react'
import type { KeyboardEvent } from 'react'
import { useDismissOnOutsidePointer } from '../../../hooks/useDismissOnOutsidePointer'

export interface AutomationOption<Value extends string = string> {
  disabled?: boolean
  label: string
  value: Value
}

interface AutomationSelectProps<Value extends string> {
  ariaLabel: string
  disabled?: boolean
  onChange: (value: Value) => void
  options: ReadonlyArray<AutomationOption<Value>>
  value: Value
}

export function AutomationSelect<Value extends string>({
  ariaLabel,
  disabled = false,
  onChange,
  options,
  value
}: AutomationSelectProps<Value>) {
  const [open, setOpen] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const optionRefs = useRef<Array<HTMLButtonElement | null>>([])
  const listboxId = useId()
  const selectedIndex = Math.max(
    0,
    options.findIndex((option) => option.value === value)
  )
  const selected = options[selectedIndex]
  const close = useCallback(() => setOpen(false), [])
  useDismissOnOutsidePointer(rootRef, open, close)

  useEffect(() => {
    if (disabled) close()
  }, [close, disabled])

  const focusOption = (start: number, direction: 1 | -1) => {
    for (let offset = 0; offset < options.length; offset += 1) {
      const index = (start + direction * offset + options.length) % options.length
      if (!options[index]?.disabled) {
        optionRefs.current[index]?.focus()
        return
      }
    }
  }

  const openAndFocus = (index = selectedIndex, direction: 1 | -1 = 1) => {
    setOpen(true)
    window.requestAnimationFrame(() => focusOption(index, direction))
  }

  const select = (option: AutomationOption<Value>) => {
    if (option.disabled) return
    onChange(option.value)
    setOpen(false)
    window.requestAnimationFrame(() => triggerRef.current?.focus())
  }

  const onOptionKeyDown = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault()
      const direction = event.key === 'ArrowDown' ? 1 : -1
      focusOption(index + direction, direction)
    } else if (event.key === 'Escape') {
      event.preventDefault()
      event.stopPropagation()
      setOpen(false)
      triggerRef.current?.focus()
    }
  }

  if (!selected) return null

  return (
    <div className="automation-select" ref={rootRef} data-open={open || undefined}>
      <button
        ref={triggerRef}
        type="button"
        className="automation-select__trigger"
        aria-controls={open ? listboxId : undefined}
        aria-expanded={open}
        aria-haspopup="listbox"
        aria-label={`${ariaLabel}: ${selected.label}`}
        disabled={disabled}
        onClick={() => (open ? close() : openAndFocus())}
        onKeyDown={(event) => {
          if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
            event.preventDefault()
            openAndFocus(
              selectedIndex + (event.key === 'ArrowDown' ? 1 : -1),
              event.key === 'ArrowDown' ? 1 : -1
            )
          } else if (event.key === 'Escape') {
            event.preventDefault()
            event.stopPropagation()
            close()
          }
        }}
      >
        <span>{selected.label}</span>
        <ChevronDown aria-hidden="true" />
      </button>
      {open && (
        <div
          className="automation-select__menu"
          id={listboxId}
          role="listbox"
          aria-label={ariaLabel}
        >
          {options.map((option, index) => {
            const isSelected = option.value === value
            return (
              <button
                key={option.value}
                ref={(node) => {
                  optionRefs.current[index] = node
                }}
                type="button"
                role="option"
                className="automation-select__option"
                aria-disabled={option.disabled || undefined}
                aria-selected={isSelected}
                disabled={option.disabled}
                data-selected={isSelected || undefined}
                tabIndex={isSelected ? 0 : -1}
                onClick={() => select(option)}
                onKeyDown={(event) => onOptionKeyDown(event, index)}
              >
                <span>{option.label}</span>
                {isSelected && <Check aria-hidden="true" />}
              </button>
            )
          })}
        </div>
      )}
    </div>
  )
}

interface AutomationMultiSelectProps<Value extends string | number> {
  ariaLabel: string
  disabled?: boolean
  formatValue: (value: Value) => string
  onChange: (values: Value[]) => void
  options: readonly Value[]
  values: readonly Value[]
}

export function AutomationMultiSelect<Value extends string | number>({
  ariaLabel,
  disabled = false,
  formatValue,
  onChange,
  options,
  values
}: AutomationMultiSelectProps<Value>) {
  const [open, setOpen] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const optionRefs = useRef<Array<HTMLButtonElement | null>>([])
  const id = useId()
  const selected = useMemo(() => new Set(values), [values])
  const summary = values.map(formatValue).join(', ')
  const close = useCallback(() => setOpen(false), [])
  useDismissOnOutsidePointer(rootRef, open, close)

  useEffect(() => {
    if (disabled) close()
  }, [close, disabled])

  const toggle = (value: Value) => {
    if (disabled) return
    if (selected.has(value)) {
      if (values.length === 1) return
      onChange(values.filter((candidate) => candidate !== value))
      return
    }
    onChange(options.filter((candidate) => selected.has(candidate) || candidate === value))
  }

  const focusOption = (index: number) => {
    if (optionRefs.current.length === 0) return
    const normalized = (index + optionRefs.current.length) % optionRefs.current.length
    optionRefs.current[normalized]?.focus()
  }

  const openAndFocus = () => {
    setOpen(true)
    const firstSelected = options.findIndex((option) => selected.has(option))
    window.requestAnimationFrame(() => focusOption(Math.max(0, firstSelected)))
  }

  return (
    <div className="automation-multiselect" ref={rootRef} data-open={open || undefined}>
      <button
        ref={triggerRef}
        type="button"
        className="automation-select__trigger"
        aria-controls={open ? id : undefined}
        aria-expanded={open}
        aria-haspopup="listbox"
        aria-label={`${ariaLabel}: ${summary}`}
        disabled={disabled}
        title={summary}
        onClick={() => (open ? close() : openAndFocus())}
        onKeyDown={(event) => {
          if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
            event.preventDefault()
            openAndFocus()
          } else if (event.key === 'Escape') {
            event.preventDefault()
            event.stopPropagation()
            close()
          }
        }}
      >
        <span>{summary}</span>
        <ChevronDown aria-hidden="true" />
      </button>
      {open && (
        <div
          className="automation-select__menu automation-select__menu--multi"
          id={id}
          role="listbox"
          aria-label={ariaLabel}
          aria-multiselectable="true"
        >
          {options.map((option, index) => {
            const checked = selected.has(option)
            return (
              <button
                key={String(option)}
                ref={(node) => {
                  optionRefs.current[index] = node
                }}
                type="button"
                role="option"
                className="automation-select__option"
                aria-selected={checked}
                disabled={disabled}
                data-selected={checked || undefined}
                onClick={() => toggle(option)}
                onKeyDown={(event) => {
                  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
                    event.preventDefault()
                    focusOption(index + (event.key === 'ArrowDown' ? 1 : -1))
                  } else if (event.key === 'Escape') {
                    event.preventDefault()
                    event.stopPropagation()
                    close()
                    triggerRef.current?.focus()
                  }
                }}
              >
                <span>{formatValue(option)}</span>
                {checked && <Check aria-hidden="true" />}
              </button>
            )
          })}
        </div>
      )}
    </div>
  )
}
