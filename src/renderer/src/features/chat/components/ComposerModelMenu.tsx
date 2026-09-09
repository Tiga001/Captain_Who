import { ArrowLeft, Check } from 'lucide-react'
import { useEffect, useId, useRef, useState, type RefObject } from 'react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import type { ComposerModelMenuOption } from '../../modelSelection/composerModelPresentation'
import './ComposerModelMenu.css'

export interface ComposerModelMenuProps {
  options: readonly ComposerModelMenuOption[]
  value: string | null
  disabled: boolean
  onChange: (modelConfigId: string) => void
  onBack: () => void
  scrollContainerRef?: RefObject<HTMLDivElement | null>
}

/** Model selection is a local navigation action; the caller owns persistence and availability. */
export function ComposerModelMenu({
  options,
  value,
  disabled,
  onChange,
  onBack,
  scrollContainerRef
}: ComposerModelMenuProps) {
  const { t } = useFrontendConfig()
  const listRef = useRef<HTMLDivElement>(null)
  const optionRefs = useRef<Array<HTMLButtonElement | null>>([])
  const listId = useId()
  const [highlightedId, setHighlightedId] = useState<string | null>(value)
  const highlightedIndex = Math.max(
    0,
    options.findIndex((option) => option.id === highlightedId)
  )
  const composing = useRef(false)
  const compositionEndedAt = useRef(-Infinity)

  useEffect(() => {
    listRef.current?.focus({ preventScroll: true })
  }, [])

  useEffect(() => {
    const selected = optionRefs.current[highlightedIndex]
    const list = listRef.current
    if (!selected || !list) return
    const boundary = scrollContainerRef?.current
    const containers = boundary?.contains(list) ? [list, boundary] : [list]
    // Limit keyboard reveal to the list and its portal viewport, preserving chat/page scroll.
    for (const container of containers) {
      const selectedRect = selected.getBoundingClientRect()
      const containerRect = container.getBoundingClientRect()
      if (selectedRect.top < containerRect.top) {
        container.scrollTop += selectedRect.top - containerRect.top
      } else if (selectedRect.bottom > containerRect.bottom) {
        container.scrollTop += selectedRect.bottom - containerRect.bottom
      }
    }
  }, [highlightedIndex, options.length, scrollContainerRef])

  const selectHighlighted = () => {
    const option = options[highlightedIndex]
    if (!disabled && option && !option.disabled) onChange(option.id)
  }

  return (
    <section
      className="composer-model-command-menu"
      aria-label={t('chat.commands.model')}
      onCompositionStart={() => {
        composing.current = true
      }}
      onCompositionEnd={(event) => {
        composing.current = false
        compositionEndedAt.current = event.timeStamp
      }}
      onKeyDown={(event) => {
        event.stopPropagation()
        if (
          composing.current ||
          event.nativeEvent.isComposing ||
          event.keyCode === 229 ||
          event.timeStamp - compositionEndedAt.current < 120
        ) {
          if (event.key === 'Enter' || event.key === 'Escape') event.preventDefault()
          return
        }
        const isDeleteBack =
          (event.key === 'Backspace' || event.key === 'Delete') &&
          !event.ctrlKey &&
          !event.metaKey &&
          !event.altKey &&
          !event.shiftKey
        if (event.key === 'Escape' || isDeleteBack) {
          event.preventDefault()
          onBack()
        } else if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
          event.preventDefault()
          if (!options.length) return
          const index =
            event.key === 'Home'
              ? 0
              : event.key === 'End'
                ? options.length - 1
                : (highlightedIndex + (event.key === 'ArrowDown' ? 1 : -1) + options.length) %
                  options.length
          setHighlightedId(options[index].id)
          listRef.current?.focus({ preventScroll: true })
        } else if (
          (event.key === 'Enter' || event.key === ' ') &&
          (event.target === listRef.current ||
            (event.target instanceof HTMLElement &&
              event.target.closest('.composer-model-command-menu__option')))
        ) {
          event.preventDefault()
          if (!event.repeat) selectHighlighted()
        }
      }}
    >
      <header className="composer-model-command-menu__header">
        <button
          className="composer-model-command-menu__back"
          type="button"
          aria-label={t('capabilityCenter.back')}
          onClick={onBack}
        >
          <ArrowLeft aria-hidden="true" />
        </button>
        <span>{t('chat.commands.model')}</span>
      </header>
      <div
        ref={listRef}
        className="composer-model-command-menu__list"
        role="listbox"
        aria-label={t('chat.selectModel')}
        aria-activedescendant={
          options.length > 0 ? `${listId}-option-${highlightedIndex}` : undefined
        }
        tabIndex={0}
      >
        {options.map((option, index) => {
          const selected = option.id === value
          const unavailable = disabled || option.disabled
          return (
            <button
              key={option.id}
              ref={(node) => {
                optionRefs.current[index] = node
              }}
              id={`${listId}-option-${index}`}
              className="composer-model-command-menu__option"
              role="option"
              type="button"
              tabIndex={-1}
              disabled={unavailable}
              aria-selected={selected}
              aria-disabled={unavailable || undefined}
              data-highlighted={index === highlightedIndex || undefined}
              title={[
                option.label,
                option.modelId,
                option.providerLabel,
                option.contextWindowLabel,
                option.capabilityLabel
              ]
                .filter(Boolean)
                .join(' · ')}
              onMouseEnter={() => setHighlightedId(option.id)}
              onFocus={() => setHighlightedId(option.id)}
              onClick={() => {
                if (!unavailable) onChange(option.id)
              }}
            >
              <span className="composer-model-command-menu__name">{option.modelId}</span>
              <span className="composer-model-command-menu__metadata">
                <span>{option.providerLabel}</span>
                <span>{option.contextWindowLabel}</span>
                {option.capabilityLabel && (
                  <span
                    className="composer-model-command-menu__modality"
                    data-supported={option.capabilitySupported || undefined}
                  >
                    {option.capabilityLabel}
                  </span>
                )}
              </span>
              <span className="composer-model-command-menu__selection" aria-hidden="true">
                {selected && <Check />}
              </span>
            </button>
          )
        })}
        {options.length === 0 && (
          <div className="composer-model-command-menu__empty">{t('chat.noEnabledModels')}</div>
        )}
      </div>
    </section>
  )
}
