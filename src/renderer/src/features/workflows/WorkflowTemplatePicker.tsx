import { Check, ChevronDown } from 'lucide-react'
import { useCallback, useId, useRef, useState, type KeyboardEvent } from 'react'
import type { AgentTemplate } from '@mycopilot/protocol'
import { AnchoredPopover } from '../../components/overlay/AnchoredPopover'
import { useDismissOnOutsidePointer } from '../../hooks/useDismissOnOutsidePointer'
import { workflowTemplateModelLabel, type WorkflowModelDisplay } from './workflowModelPresentation'
import type { WorkflowText } from './workflowText'

interface WorkflowTemplatePickerProps {
  value: string | null
  templates: readonly AgentTemplate[]
  models: readonly WorkflowModelDisplay[]
  text: WorkflowText
  onChange: (templateId: string | null) => void
}

interface TemplateOption {
  id: string | null
  name: string
  model?: string
  disabled?: boolean
}

/** Template identities stay separate from model selection and retain missing references. */
export function WorkflowTemplatePicker({
  value,
  templates,
  models,
  text,
  onChange
}: WorkflowTemplatePickerProps) {
  const [open, setOpen] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const popoverRef = useRef<HTMLDivElement>(null)
  const optionRefs = useRef<Array<HTMLButtonElement | null>>([])
  const listboxId = useId()
  const options: TemplateOption[] = [
    { id: null, name: text('noTemplate') },
    ...(value && !templates.some((template) => template.templateId === value)
      ? [{ id: value, name: text('missingTemplate'), disabled: true }]
      : []),
    ...templates.map((template) => ({
      id: template.templateId,
      name: template.name,
      model: workflowTemplateModelLabel(template, models, text)
    }))
  ]
  const selectedIndex = options.findIndex((option) => option.id === value)
  const selected = options[selectedIndex] ?? options[0]!
  const close = useCallback(() => setOpen(false), [])

  useDismissOnOutsidePointer(rootRef, open, close, (target) =>
    Boolean(popoverRef.current?.contains(target))
  )

  const focusOption = (start: number, direction: 1 | -1 = 1) => {
    for (let offset = 0; offset < options.length; offset++) {
      const index = (start + direction * offset + options.length) % options.length
      if (!options[index]?.disabled) {
        optionRefs.current[index]?.focus()
        return
      }
    }
  }
  const openMenu = (index = Math.max(0, selectedIndex), direction: 1 | -1 = 1) => {
    setOpen(true)
    window.requestAnimationFrame(() => focusOption(index, direction))
  }
  const closeAndFocus = () => {
    close()
    window.requestAnimationFrame(() => triggerRef.current?.focus({ preventScroll: true }))
  }
  const handleKeyDown = (event: KeyboardEvent<HTMLButtonElement>, index?: number) => {
    if (event.key === 'Escape' && open) {
      event.preventDefault()
      closeAndFocus()
      return
    }
    let next: number
    let direction: 1 | -1 = 1
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      direction = event.key === 'ArrowDown' ? 1 : -1
      next = (index ?? Math.max(0, selectedIndex)) + direction
    } else if (event.key === 'Home') next = 0
    else if (event.key === 'End') {
      next = options.length - 1
      direction = -1
    } else return
    event.preventDefault()
    if (open) focusOption(next, direction)
    else openMenu(next, direction)
  }

  return (
    <div
      className="workflow-template-picker"
      ref={rootRef}
      onBlur={(event) => {
        if (
          !event.currentTarget.contains(event.relatedTarget) &&
          !popoverRef.current?.contains(event.relatedTarget)
        )
          close()
      }}
    >
      <button
        ref={triggerRef}
        className="workflow-template-picker__trigger"
        type="button"
        aria-label={text('template')}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listboxId : undefined}
        data-unavailable={selected.disabled || undefined}
        onClick={() => (open ? close() : openMenu())}
        onKeyDown={(event) => handleKeyDown(event)}
      >
        <span className="workflow-template-picker__identity">
          <span>{selected.name}</span>
          {selected.model ? <small>{selected.model}</small> : null}
        </span>
        <ChevronDown aria-hidden="true" />
      </button>
      {open ? (
        <AnchoredPopover
          anchorRef={triggerRef}
          className="workflow-template-popover"
          enabled
          matchAnchorWidth
          onClose={close}
          popoverRef={popoverRef}
        >
          <div
            className="workflow-template-picker__menu"
            role="listbox"
            aria-label={text('template')}
            id={listboxId}
          >
            {options.map((option, index) => (
              <button
                key={option.id ? `template:${option.id}` : 'none'}
                ref={(element) => {
                  optionRefs.current[index] = element
                }}
                className="workflow-template-picker__option"
                type="button"
                role="option"
                aria-selected={option.id === value}
                aria-disabled={option.disabled || undefined}
                disabled={option.disabled}
                tabIndex={index === selectedIndex && !option.disabled ? 0 : -1}
                onKeyDown={(event) => handleKeyDown(event, index)}
                onClick={() => {
                  onChange(option.id)
                  closeAndFocus()
                }}
              >
                <span className="workflow-template-picker__identity">
                  <span>{option.name}</span>
                  {option.model ? <small>{option.model}</small> : null}
                </span>
                {option.id === value ? <Check aria-hidden="true" /> : null}
              </button>
            ))}
          </div>
        </AnchoredPopover>
      ) : null}
    </div>
  )
}
